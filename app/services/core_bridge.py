"""Transport to the Rust core (ADR 0005, 0008, 0012): one ``promptcad-core rpc`` per call.

The server is a transparent, authenticated transport: it never interprets, validates or
re-serializes a 2.0 request or response (ADR 0008 rule 4). The request bytes go to the
child's stdin, the single response line comes back from its stdout.

Calls are bounded: at most ``core_max_concurrency`` children run at once, on a dedicated
thread pool (never the default pool used by render jobs); a request that cannot get a slot
within ``core_queue_wait_seconds`` (or finds the wait queue full) is rejected with
``CoreBusy`` before anything is spawned.
"""

from __future__ import annotations

import asyncio
import contextlib
import json
import os
import subprocess
import sys
import threading
import time
import weakref
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from pathlib import Path

from app.core.config import Settings

BINARY_NAME = "promptcad-core"
# The core refuses to emit a response larger than this (it answers with an `error` envelope
# instead). Keep in sync with crates/promptcad-core.
CORE_MAX_RESPONSE_BYTES = 24 * 1024 * 1024
# The bridge's own cap is a second line of defence and MUST stay above the core's cap, so a
# legitimate maximum-size response is never rejected here before the core's own limit applies.
MAX_RESPONSE_BYTES = CORE_MAX_RESPONSE_BYTES + 4 * 1024 * 1024
# At most this many times the concurrency limit may wait for a slot; the rest get 503 at once.
QUEUE_FACTOR = 4
PROBE_TIMEOUT_SECONDS = 5.0
PROBE_WAIT_SECONDS = 1.0
PROBE_FAILURE_TTL_SECONDS = 30.0
_STDERR_TAIL = 2000
_ENV_KEEP = frozenset(
    {"PATH", "SYSTEMROOT", "WINDIR", "TEMP", "TMP", "TMPDIR", "HOME", "LANG", "LC_ALL"}
)


class CoreError(Exception):
    """Base class of core transport failures."""


class CoreUnavailable(CoreError):
    """No usable promptcad-core binary (not found, or cannot be started)."""


class CoreTimeout(CoreError):
    """The core did not answer within the timeout; the child was killed."""


class CoreFailed(CoreError):
    """Non-zero exit, oversized or unparseable output."""


class CoreBusy(CoreError):
    """No concurrency slot within the wait deadline (or the wait queue is full)."""

    def __init__(self, message: str, retry_after: int = 1):
        super().__init__(message)
        self.retry_after = retry_after


@dataclass(frozen=True, slots=True)
class CoreVersion:
    crate_version: str
    api_version: int
    schema_versions: tuple[str, ...]


def _binary_names() -> list[str]:
    # Windows: only a real .exe. Never .cmd/.bat/scripts (those would run through cmd.exe).
    return [BINARY_NAME + ".exe"] if sys.platform == "win32" else [BINARY_NAME]


def _first_file(directory: Path, *, need_exec: bool = False) -> Path | None:
    for name in _binary_names():
        candidate = directory / name
        if candidate.is_file() and (not need_exec or os.access(candidate, os.X_OK)):
            return candidate
    return None


def _search_path() -> Path | None:
    """Explicit PATH search: absolute directories only, never the CWD, result resolved."""
    try:
        cwd = Path.cwd().resolve()
    except OSError:
        cwd = None
    for entry in os.environ.get("PATH", "").split(os.pathsep):
        entry = entry.strip().strip('"')
        if not entry:
            continue
        directory = Path(entry)
        if not directory.is_absolute():
            continue
        try:
            resolved_dir = directory.resolve()
            if cwd is not None and resolved_dir == cwd:
                continue
            found = _first_file(resolved_dir, need_exec=True)
            if found is not None:
                found = found.resolve()
                if found.is_file():
                    return found
        except OSError:
            continue
    return None


def _dev_fallback_allowed(settings: Settings) -> bool:
    """The crates/ build output is only for a source checkout, never production or a bundle."""
    return settings.env != "production" and not getattr(sys, "frozen", False)


def locate_core_binary(settings: Settings) -> tuple[Path | None, str]:
    """Locate promptcad-core; returns ``(path, reason-when-missing)``.

    Order: ``PROMPTCAD_CORE_BIN`` (exclusive: no fallback), next to the executable / frozen
    bundle, PATH, then (source checkout only) ``crates/promptcad-core/target``. The frozen
    desktop sidecar normally has none (the desktop UI calls the core through Tauri).
    """
    if settings.core_bin is not None:
        configured = Path(settings.core_bin)
        if configured.is_file():
            return configured.absolute(), ""
        return None, f"PROMPTCAD_CORE_BIN does not point at an existing file: {configured}"

    directories = [Path(sys.executable).resolve().parent]
    bundle = getattr(sys, "_MEIPASS", None)
    if bundle:
        directories.insert(0, Path(bundle))
    for directory in directories:
        found = _first_file(directory)
        if found:
            return found.resolve(), ""

    on_path = _search_path()
    if on_path:
        return on_path, ""

    if _dev_fallback_allowed(settings):
        repo_root = Path(__file__).resolve().parents[2]
        for profile in ("release", "debug"):
            found = _first_file(repo_root / "crates" / "promptcad-core" / "target" / profile)
            if found:
                return found.resolve(), ""
    return None, "promptcad-core binary not found (set PROMPTCAD_CORE_BIN)"


def find_core_binary(settings: Settings) -> Path | None:
    return locate_core_binary(settings)[0]


def _child_env() -> dict[str, str]:
    """Minimal environment: the core needs nothing, so no tokens or API keys are inherited."""
    return {key: value for key, value in os.environ.items() if key.upper() in _ENV_KEEP}


class _Run:
    """Handle on one in-flight call so another thread can kill its child."""

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._proc: subprocess.Popen | None = None
        self.cancelled = False

    def attach(self, proc: subprocess.Popen) -> None:
        with self._lock:
            self._proc = proc
            if self.cancelled:
                with contextlib.suppress(OSError):
                    proc.kill()

    def cancel(self) -> None:
        with self._lock:
            self.cancelled = True
            if self._proc is not None:
                with contextlib.suppress(OSError):
                    self._proc.kill()


class _Gate:
    """Per-event-loop concurrency gate (an asyncio.Semaphore is bound to one loop)."""

    def __init__(self, concurrency: int):
        self.sem = asyncio.Semaphore(concurrency)
        self.waiting = 0


class CoreBridge:
    def __init__(self, settings: Settings):
        self.settings = settings
        self._binary: Path | None = None
        self._reason = ""
        self._resolved = False
        self._version: CoreVersion | None = None
        self._probe_failed_until = 0.0
        self._executor: ThreadPoolExecutor | None = None
        self._gates: weakref.WeakKeyDictionary = weakref.WeakKeyDictionary()
        self._runs: set[_Run] = set()
        self._lock = threading.Lock()
        self._closed = False

    # -- binary -------------------------------------------------------------------------
    def binary(self) -> Path | None:
        if not self._resolved:
            self._binary, self._reason = locate_core_binary(self.settings)
            self._resolved = True
        return self._binary

    def unavailable_reason(self) -> str:
        self.binary()
        return self._reason or "promptcad-core binary not found (set PROMPTCAD_CORE_BIN)"

    def override_binary(self, path: Path | None) -> None:
        """Test hook: pin the binary and drop the cached probe."""
        self._binary = path
        self._reason = ""
        self._resolved = True
        self._version = None
        self._probe_failed_until = 0.0

    # -- lifecycle ----------------------------------------------------------------------
    def shutdown(self) -> None:
        """Kill running children (do not await them) and stop the executor."""
        with self._lock:
            self._closed = True
            runs = list(self._runs)
            executor = self._executor
        for run in runs:
            run.cancel()
        if executor is not None:
            executor.shutdown(wait=False, cancel_futures=True)

    def _get_executor(self) -> ThreadPoolExecutor:
        with self._lock:
            if self._closed:
                raise CoreUnavailable("promptcad-core bridge is shut down")
            if self._executor is None:
                self._executor = ThreadPoolExecutor(
                    max_workers=self.settings.core_max_concurrency,
                    thread_name_prefix="promptcad-core",
                )
            return self._executor

    def _gate(self) -> _Gate:
        loop = asyncio.get_running_loop()
        gate = self._gates.get(loop)
        if gate is None:
            gate = self._gates[loop] = _Gate(self.settings.core_max_concurrency)
        return gate

    # -- one call -----------------------------------------------------------------------
    def call_bytes(
        self, request: bytes, *, timeout: float | None = None, run: _Run | None = None
    ) -> bytes:
        """Blocking: run ``promptcad-core rpc`` and return its single response line.

        Not concurrency-limited by itself; the async entry points are (see ``call_raw``).
        """
        binary = self.binary()
        if binary is None:
            raise CoreUnavailable(self.unavailable_reason())
        timeout = float(timeout if timeout is not None else self.settings.core_timeout_seconds)
        if run is not None and run.cancelled:
            raise CoreFailed("promptcad-core call cancelled")
        creationflags = subprocess.CREATE_NO_WINDOW if sys.platform == "win32" else 0
        try:
            proc = subprocess.Popen(
                [str(binary), "rpc"],
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                env=_child_env(),
                shell=False,
                creationflags=creationflags,
            )
        except OSError as exc:
            raise CoreUnavailable(f"cannot start promptcad-core: {exc.strerror or exc}") from exc
        if run is not None:
            run.attach(proc)

        timed_out = threading.Event()

        def kill() -> None:
            timed_out.set()
            proc.kill()

        timer = threading.Timer(timeout, kill)
        stderr_tail = bytearray()

        def feed() -> None:
            try:
                assert proc.stdin is not None
                proc.stdin.write(request)
                proc.stdin.close()
            except OSError:
                pass  # the child exited early; its exit code / output tells the story

        def drain_stderr() -> None:
            assert proc.stderr is not None
            while chunk := proc.stderr.read(4096):
                stderr_tail.extend(chunk)
                del stderr_tail[:-_STDERR_TAIL]

        threads = [
            threading.Thread(target=feed, daemon=True),
            threading.Thread(target=drain_stderr, daemon=True),
        ]
        oversized = False
        output = b""
        timer.start()
        try:
            for thread in threads:
                thread.start()
            assert proc.stdout is not None
            output = proc.stdout.read(MAX_RESPONSE_BYTES + 1)
            oversized = len(output) > MAX_RESPONSE_BYTES
            if oversized:
                proc.kill()
            proc.wait()
        finally:
            timer.cancel()
            if proc.poll() is None:
                proc.kill()
                proc.wait()
            for thread in threads:
                thread.join(timeout=2)
            for stream in (proc.stdin, proc.stdout, proc.stderr):
                if stream:
                    with contextlib.suppress(OSError):
                        stream.close()

        if run is not None and run.cancelled:
            raise CoreFailed("promptcad-core call cancelled")
        if timed_out.is_set():
            raise CoreTimeout(f"promptcad-core did not answer within {timeout:g}s")
        if oversized:
            raise CoreFailed("promptcad-core response exceeds the size limit")
        if proc.returncode != 0:
            tail = bytes(stderr_tail).decode("utf-8", errors="replace").strip()
            raise CoreFailed(f"promptcad-core exited with {proc.returncode}: {tail}"[:_STDERR_TAIL])
        try:
            parsed = json.loads(output)
        except ValueError as exc:
            raise CoreFailed("promptcad-core wrote an unparseable response") from exc
        if not isinstance(parsed, dict) or "status" not in parsed:
            raise CoreFailed("promptcad-core wrote a response without a status")
        return output.rstrip(b"\r\n")

    async def call_raw(
        self, request: bytes, *, timeout: float | None = None, wait: float | None = None
    ) -> bytes:
        """Run one call under the concurrency gate on the dedicated executor.

        Raises ``CoreBusy`` (nothing spawned) when no slot frees up within ``wait`` seconds
        or too many requests already wait. Cancelling the awaiting task kills the child and
        frees the slot once the worker thread has finished. The overall deadline of a call
        is ``wait`` plus ``timeout`` seconds.
        """
        if self.binary() is None:
            raise CoreUnavailable(self.unavailable_reason())
        wait = float(wait if wait is not None else self.settings.core_queue_wait_seconds)
        gate = self._gate()
        retry = max(1, round(wait))
        if gate.sem.locked() and gate.waiting >= QUEUE_FACTOR * self.settings.core_max_concurrency:
            raise CoreBusy("promptcad-core is busy (queue full)", retry)
        gate.waiting += 1
        try:
            async with asyncio.timeout(wait):
                await gate.sem.acquire()
        except TimeoutError as exc:
            raise CoreBusy("promptcad-core is busy (no free slot)", retry) from exc
        finally:
            gate.waiting -= 1

        loop = asyncio.get_running_loop()
        run = _Run()
        try:
            future = self._get_executor().submit(self.call_bytes, request, timeout=timeout, run=run)
        except RuntimeError as exc:  # executor shut down
            gate.sem.release()
            raise CoreUnavailable("promptcad-core bridge is shut down") from exc
        except BaseException:
            gate.sem.release()
            raise
        with self._lock:
            self._runs.add(run)

        def done(_future: object) -> None:
            with self._lock:
                self._runs.discard(run)
            with contextlib.suppress(RuntimeError):  # loop already closed
                loop.call_soon_threadsafe(gate.sem.release)

        future.add_done_callback(done)
        try:
            return await asyncio.wrap_future(future)
        except asyncio.CancelledError:
            run.cancel()
            raise

    async def call(self, request: dict | bytes) -> dict:
        """Send a request (dict or already-encoded JSON bytes); return the decoded response."""
        body = request if isinstance(request, bytes) else json.dumps(request).encode("utf-8")
        return json.loads(await self.call_raw(body))

    # -- probe --------------------------------------------------------------------------
    async def version(self) -> CoreVersion | None:
        """Cached ``{"op":"version"}`` probe; ``None`` when the core is unavailable.

        Failures are cached for ``PROBE_FAILURE_TTL_SECONDS`` so ``/capabilities`` does not
        spawn a process per call; the probe is short, gated and never raises.
        """
        if self._version is not None:
            return self._version
        if self.binary() is None:
            return None
        if time.monotonic() < self._probe_failed_until:
            return None
        try:
            timeout = min(float(self.settings.core_timeout_seconds), PROBE_TIMEOUT_SECONDS)
            raw = await self.call_raw(b'{"op":"version"}', timeout=timeout, wait=PROBE_WAIT_SECONDS)
            result = json.loads(raw)["result"]
            self._version = CoreVersion(
                crate_version=str(result["crate_version"]),
                api_version=int(result["api_version"]),
                schema_versions=tuple(str(v) for v in result["schema_versions"]),
            )
        except CoreBusy:
            return None  # transient: do not cache
        except (CoreError, KeyError, TypeError, ValueError):
            self._probe_failed_until = time.monotonic() + PROBE_FAILURE_TTL_SECONDS
            return None
        return self._version

    async def available(self) -> bool:
        return await self.version() is not None
