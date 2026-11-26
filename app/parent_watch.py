"""Unix kill-on-close for the desktop sidecar: stop when the shell's stdin pipe closes.

On Windows the Tauri shell puts the sidecar in a kill-on-close Job Object
(``src-tauri/src/job.rs``). macOS and Linux have no equivalent, so there (ADR 0011) the
shell starts the sidecar with ``--exit-on-stdin-eof``, in a new process group
(``process_group(0)``), with a stdin pipe whose only write end the shell holds. When the
shell exits or is killed, the kernel closes that pipe and the sidecar reads EOF; this
module then takes the whole group down: the sidecar itself plus the ``--render-worker``
children it spawns (they inherit the group; ``app/services/renderer.py`` starts them with
``stdin=DEVNULL`` and no new session).

On EOF: ``SIGTERM`` the group (uvicorn shuts down, workers die), then ``SIGKILL`` it after
``GRACE_SECONDS``, or as soon as the main thread is about to return (:meth:`StdinEofWatch.finish`),
so no worker that ignored ``SIGTERM`` can outlive the sidecar. The group is signalled only
when this process leads it; otherwise only this process is, so a launcher's own group (a
terminal, a test runner) is never hit.

Everything OS-facing (the stream, ``killpg``/``kill``, ``setpgid``, pids) is injectable so
the policy is unit-tested on any OS (``tests/test_parent_watch.py``).
"""

from __future__ import annotations

import contextlib
import os
import signal
import sys
import threading
from collections.abc import Callable
from typing import Any, BinaryIO

FLAG = "--exit-on-stdin-eof"
GRACE_SECONDS = 2.0
CHUNK = 65536

SIGTERM = int(signal.SIGTERM)
# signal.SIGKILL does not exist on Windows; the value is the same on every unix.
SIGKILL = int(getattr(signal, "SIGKILL", 9))

Kill = Callable[[int, int], None]
Log = Callable[[str], None]

# Unix-only os functions (None on Windows), as module-level defaults for install().
_SETPGID: Callable[[int, int], None] | None = getattr(os, "setpgid", None)
_GETPGRP: Callable[[], int] | None = getattr(os, "getpgrp", None)
_KILLPG: Kill | None = getattr(os, "killpg", None)


def _stderr_log(message: str) -> None:
    # stderr is a pipe read by the shell. Once the shell is gone that write raises
    # BrokenPipeError (Python ignores SIGPIPE), and must never stop the watch from signalling.
    with contextlib.suppress(OSError, ValueError):
        print(f"promptcad-sidecar: {message}", file=sys.stderr, flush=True)


def wait_for_eof(stream: Any) -> None:
    """Blocks until ``stream.read`` returns an empty chunk. A read error counts as EOF.

    Nothing is ever written to the pipe; any data that does arrive is discarded.
    """
    while True:
        try:
            chunk = stream.read(CHUNK)
        except (OSError, ValueError):  # closed or broken stream: the shell is gone too
            return
        if not chunk:
            return


def signal_target(pid: int, pgid: int | None) -> tuple[str, int]:
    """``("group", pgid)`` when ``pid`` leads its group, else ``("process", pid)``. Pure.

    Only a group leader may signal its whole group: otherwise the group belongs to whoever
    launched us and killing it would take them down too.
    """
    if pgid is not None and pgid == pid and pid > 1:
        return "group", pgid
    return "process", pid


def become_group_leader(setpgid: Callable[[int, int], None] | None) -> bool:
    """``setpgid(0, 0)``, ignoring errors (already a leader, session leader, Windows)."""
    if setpgid is None:
        return False
    try:
        setpgid(0, 0)
    except OSError:
        return False
    return True


class StdinEofWatch:
    """Watches ``stream`` on a daemon thread; on EOF stops this process (and its group)."""

    def __init__(
        self,
        stream: Any,
        *,
        pid: int,
        pgid: int | None,
        killpg: Kill | None,
        kill: Kill,
        grace: float = GRACE_SECONDS,
        log: Log = _stderr_log,
    ) -> None:
        self.stream = stream
        self.kind, self.target = signal_target(pid, pgid)
        if self.kind == "group" and killpg is None:
            self.kind, self.target = "process", pid
        self._killpg = killpg
        self._kill = kill
        self.grace = grace
        self._log = log
        self._lock = threading.Lock()
        self._finished = threading.Event()
        self.triggered = False
        self.sent: list[int] = []
        self._thread: threading.Thread | None = None

    def start(self) -> threading.Thread:
        self._thread = threading.Thread(target=self.run, name="stdin-eof-watch", daemon=True)
        self._thread.start()
        return self._thread

    def run(self) -> None:
        wait_for_eof(self.stream)
        with self._lock:
            self.triggered = True
            finishing = self._finished.is_set()
        self._safe_log(f"stdin closed (the shell is gone); stopping {self.kind} {self.target}")
        if finishing:  # the main thread is already returning: no grace left
            self._signal(SIGKILL)
            return
        self._signal(SIGTERM)
        if not self._finished.wait(self.grace):
            self._signal(SIGKILL)

    def finish(self) -> None:
        """Called by the sidecar's main thread on its way out.

        After an EOF this ``SIGKILL``s the group at once (normally this process too), so a
        render worker cannot linger after uvicorn returns. Without an EOF it does nothing.
        """
        with self._lock:
            self._finished.set()
            triggered = self.triggered
        if triggered:
            self._signal(SIGKILL)

    def _safe_log(self, message: str) -> None:
        # The shell reads our stderr; after it dies a log write can raise. Logging is best
        # effort: the signals below are the point of this thread.
        with contextlib.suppress(Exception):
            self._log(message)

    def _signal(self, sig: int) -> None:
        self.sent.append(sig)
        try:
            if self.kind == "group" and self._killpg is not None:
                self._killpg(self.target, sig)
            else:
                self._kill(self.target, sig)
        except ProcessLookupError:
            pass
        except OSError as exc:
            self._safe_log(f"cannot send signal {sig} to {self.kind} {self.target}: {exc}")


def install(
    stream: BinaryIO | None = None,
    *,
    platform: str = sys.platform,
    log: Log = _stderr_log,
    setpgid: Callable[[int, int], None] | None = _SETPGID,
    getpgrp: Callable[[], int] | None = _GETPGRP,
    getpid: Callable[[], int] = os.getpid,
    killpg: Kill | None = _KILLPG,
    kill: Kill = os.kill,
    grace: float = GRACE_SECONDS,
    start: bool = True,
) -> StdinEofWatch | None:
    """Starts the watch for ``--exit-on-stdin-eof``; returns ``None`` when it does not apply.

    * Windows: a no-op (the shell's Job Object already does this), logged.
    * stdin missing or a terminal: not watched, logged. A terminal is never what the shell
      passes, and reading it from a background process group would stop us with ``SIGTTIN``.
    """
    if platform == "win32":
        log(f"{FLAG} is ignored on Windows (the shell's Job Object stops the sidecar)")
        return None
    if stream is None:
        stdin = sys.stdin
        stream = getattr(stdin, "buffer", None) if stdin is not None else None
        # Read the unbuffered FileIO: a daemon thread blocked in BufferedReader.read holds its
        # lock, and CPython aborts at shutdown ("could not acquire lock ... at interpreter
        # shutdown", exit 139) when the sidecar exits while the shell still holds the pipe.
        stream = getattr(stream, "raw", stream)
    if stream is None:
        log(f"{FLAG}: no stdin to watch; the sidecar will not stop with the shell")
        return None
    try:
        is_tty = bool(stream.isatty())
    except (AttributeError, OSError, ValueError):
        is_tty = False
    if is_tty:
        log(f"{FLAG}: stdin is a terminal, not the shell's pipe; not watching it")
        return None
    become_group_leader(setpgid)
    pgid = None
    if getpgrp is not None:
        try:
            pgid = getpgrp()
        except OSError:
            pgid = None
    watch = StdinEofWatch(
        stream, pid=getpid(), pgid=pgid, killpg=killpg, kill=kill, grace=grace, log=log
    )
    if watch.kind != "group":
        log(f"{FLAG}: not a process group leader; only this process will be stopped")
    if start:
        watch.start()
    return watch
