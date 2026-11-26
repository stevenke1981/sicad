"""Proves the kill-on-close Job Object takes the whole frozen sidecar tree down (ADR 0010).

The Tauri shell (``src-tauri/src/job.rs``) puts the sidecar in a Job Object with
``JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`` so that neither the sidecar nor the ``--render-worker``
child it spawns for every CadQuery render can outlive the shell. This script checks that
against a real frozen sidecar, without the shell:

1. create a kill-on-close job, spawn the sidecar (isolated clean-machine env, see
   ``frozen_smoke.py --isolated-env``) and assign it to the job right after creation (the
   onedir sidecar spawns nothing before its READY line);
2. POST ``/api/v1/generate-from-spec`` on a thread and poll a Toolhelp snapshot every 20 ms
   until a render worker (a child of the sidecar PID with the sidecar's own image) exists;
3. open handles to the sidecar and the worker, check both are still running and in the job,
   then ``CloseHandle(job)``;
4. assert every tracked process exits within 5 s and no tracked process left a child behind.

If the render finished before a worker was observed, the attempt is retried with a heavier
spec (``--attempts``); "worker not observed" is never a pass.

``--shell-exe`` checks the shell itself: launch the installed ``promptcad-studio.exe``, wait
for its sidecar child, force-kill the shell (``TerminateProcess``, i.e. a crash) and assert
the sidecar is gone within 5 s.

Every check is scoped to processes this script started (tracked PIDs, held by handle, plus
their descendants by image path) - never a global match by process name, so a PromptCAD
Studio the user has open is left alone.

Usage::

    python scripts/frozen_kill_check.py --exe PATH [--shell-exe PATH] [--attempts 3]

Prints ``KILL_CHECK_OK`` and exits 0 on success, exits 1 on failure; off Windows it prints
``KILL_CHECK_SKIPPED`` and exits 0. Standard library only (ctypes).
"""

from __future__ import annotations

import argparse
import ctypes
import json
import secrets
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any

_SCRIPTS = str(Path(__file__).resolve().parent)
if _SCRIPTS not in sys.path:
    sys.path.insert(0, _SCRIPTS)

import frozen_smoke as fs  # noqa: E402

JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE = 0x00002000
JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS = 9
WAIT_OBJECT_0 = 0x00000000
WAIT_TIMEOUT = 0x00000102
STILL_ACTIVE = 259

POLL_SECONDS = 0.020
EXIT_DEADLINE_SECONDS = 5.0
SHELL_EXE_NAME = "promptcad-studio.exe"


class KillCheckFailure(Exception):
    pass


# --------------------------------------------------------------------------- structs


class JOBOBJECT_BASIC_LIMIT_INFORMATION(ctypes.Structure):
    _fields_ = [
        ("PerProcessUserTimeLimit", ctypes.c_int64),
        ("PerJobUserTimeLimit", ctypes.c_int64),
        ("LimitFlags", ctypes.c_uint32),
        ("MinimumWorkingSetSize", ctypes.c_size_t),
        ("MaximumWorkingSetSize", ctypes.c_size_t),
        ("ActiveProcessLimit", ctypes.c_uint32),
        ("Affinity", ctypes.c_size_t),
        ("PriorityClass", ctypes.c_uint32),
        ("SchedulingClass", ctypes.c_uint32),
    ]


class IO_COUNTERS(ctypes.Structure):
    _fields_ = [
        (name, ctypes.c_uint64)
        for name in (
            "ReadOperationCount",
            "WriteOperationCount",
            "OtherOperationCount",
            "ReadTransferCount",
            "WriteTransferCount",
            "OtherTransferCount",
        )
    ]


class JOBOBJECT_EXTENDED_LIMIT_INFORMATION(ctypes.Structure):
    _fields_ = [
        ("BasicLimitInformation", JOBOBJECT_BASIC_LIMIT_INFORMATION),
        ("IoInfo", IO_COUNTERS),
        ("ProcessMemoryLimit", ctypes.c_size_t),
        ("JobMemoryLimit", ctypes.c_size_t),
        ("PeakProcessMemoryUsed", ctypes.c_size_t),
        ("PeakJobMemoryUsed", ctypes.c_size_t),
    ]


def kill_on_close_limits() -> JOBOBJECT_EXTENDED_LIMIT_INFORMATION:
    """The limit block job.rs sets: only ``JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE``."""
    info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION()
    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
    return info


# --------------------------------------------------------------------------- win32


class KillOnCloseJob:
    """A Job Object that kills its members when the (only) handle is closed."""

    def __init__(self) -> None:
        k = fs.kernel32()
        handle = k.CreateJobObjectW(None, None)
        if not handle:
            raise fs.win_error("CreateJobObjectW")
        self.handle: int | None = handle
        info = kill_on_close_limits()
        if not k.SetInformationJobObject(
            handle,
            JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS,
            ctypes.byref(info),
            ctypes.sizeof(info),
        ):
            error = fs.win_error("SetInformationJobObject")
            self.close()
            raise error

    def assign(self, pid: int) -> None:
        k = fs.kernel32()
        proc = k.OpenProcess(
            fs.PROCESS_SET_QUOTA | fs.PROCESS_TERMINATE | fs.PROCESS_QUERY_LIMITED_INFORMATION,
            False,
            pid,
        )
        if not proc:
            raise fs.win_error(f"OpenProcess({pid})")
        try:
            if not k.AssignProcessToJobObject(self.handle, proc):
                raise fs.win_error(f"AssignProcessToJobObject({pid})")
        finally:
            k.CloseHandle(proc)

    def contains(self, process_handle: int) -> bool:
        result = ctypes.c_int(0)
        if not fs.kernel32().IsProcessInJob(process_handle, self.handle, ctypes.byref(result)):
            raise fs.win_error("IsProcessInJob")
        return bool(result.value)

    def close(self) -> None:
        if self.handle:
            fs.kernel32().CloseHandle(self.handle)
            self.handle = None


@dataclass
class Tracked:
    """A process this script is responsible for, held open by handle (no PID reuse)."""

    pid: int
    name: str
    role: str
    handle: int

    @classmethod
    def open(cls, pid: int, name: str, role: str) -> Tracked | None:
        access = fs.SYNCHRONIZE | fs.PROCESS_QUERY_LIMITED_INFORMATION | fs.PROCESS_TERMINATE
        handle = fs.kernel32().OpenProcess(access, False, pid)
        return cls(pid, name, role, handle) if handle else None

    def alive(self) -> bool:
        return fs.kernel32().WaitForSingleObject(self.handle, 0) == WAIT_TIMEOUT

    def wait(self, seconds: float) -> bool:
        ms = max(0, int(seconds * 1000))
        return fs.kernel32().WaitForSingleObject(self.handle, ms) == WAIT_OBJECT_0

    def exit_code(self) -> int | None:
        code = ctypes.c_uint32(0)
        if not fs.kernel32().GetExitCodeProcess(self.handle, ctypes.byref(code)):
            return None
        return None if code.value == STILL_ACTIVE else code.value

    def terminate(self) -> None:
        if self.alive():
            fs.kernel32().TerminateProcess(self.handle, 1)
            self.wait(10)

    def close(self) -> None:
        if self.handle:
            fs.kernel32().CloseHandle(self.handle)
            self.handle = 0

    def describe(self) -> str:
        return f"{self.role} pid {self.pid} ({self.name})"


def _close_all(tracked: list[Tracked], *, terminate: bool) -> None:
    for proc in tracked:
        if terminate:
            proc.terminate()
        proc.close()


def wait_all_exit(tracked: list[Tracked], deadline_seconds: float) -> list[Tracked]:
    """Returns the processes still running ``deadline_seconds`` after the call."""
    deadline = time.monotonic() + deadline_seconds
    return [p for p in tracked if not p.wait(deadline - time.monotonic())]


def leftover_children(
    tracked: list[Tracked], created: dict[int, int | None], image_dir: Path
) -> list[fs.ProcInfo]:
    """Live descendants of tracked processes whose image lies in ``image_dir``."""
    roots = {p.pid: created.get(p.pid) for p in tracked}
    tree = fs.walk_tree(fs.process_snapshot(), roots, fs.describe_process)
    return [p for p in tree if p.path and fs.is_within(p.path, image_dir)]


# --------------------------------------------------------------------------- specs


def kill_spec(attempt: int) -> dict[str, Any]:
    """Attempt 1 is the 6-hole smoke plate; later attempts add a grid of hole features."""
    if attempt <= 1:
        return fs.base_spec("plate")
    spec = fs.base_spec("plate")
    spec["name"] = f"kill-check-heavy-{attempt}"
    hole_type = "countersink" if attempt == 2 else "counterbore"
    xs = [-63.0 + 14.0 * i for i in range(10)]
    ys = [-35.0 + 14.0 * j for j in range(6)]
    spec["holes"] = [fs._hole(hole_type, x, y, 10.0) for y in ys for x in xs]
    return spec


# --------------------------------------------------------------------------- sidecar mode


def _post_in_background(sidecar: fs.Sidecar, token: str, spec: dict[str, Any]) -> threading.Event:
    done = threading.Event()

    def run() -> None:
        try:
            fs._request(
                sidecar,
                "POST",
                "/api/v1/generate-from-spec",
                token,
                {"spec": spec, "formats": fs.CAD_FORMATS, "render": True},
                timeout=600,
            )
        except Exception:  # the connection dies when the job is closed: expected
            pass
        finally:
            done.set()

    threading.Thread(target=run, name="kill-check-post", daemon=True).start()
    return done


def _find_workers(sidecar_pid: int, created: int | None, image_dir: Path) -> list[fs.ProcInfo]:
    tree = fs.walk_tree(fs.process_snapshot(), {sidecar_pid: created}, fs.describe_process)
    return [
        p
        for p in tree
        if p.ppid == sidecar_pid
        and p.name.lower() == fs.EXE_NAME.lower()
        and p.path
        and fs.is_within(p.path, image_dir)
    ]


def sidecar_attempt(
    exe: Path, attempt: int, ready_timeout: float, observe_timeout: float
) -> dict[str, Any] | None:
    """One kill attempt. Returns a result dict, or ``None`` if no worker was observed."""
    image_dir = exe.parent
    workdir = Path(tempfile.mkdtemp(prefix="promptcad-kill-check-"))
    job = KillOnCloseJob()
    tracked: list[Tracked] = []
    sidecar: fs.Sidecar | None = None
    killed = False
    try:
        token = secrets.token_urlsafe(32)
        sidecar = fs.spawn_sidecar(
            exe,
            workdir,
            token,
            ready_timeout,
            isolated=True,
            on_spawn=lambda proc: job.assign(proc.pid),
        )
        pid = sidecar.proc.pid
        root = Tracked.open(pid, fs.EXE_NAME, "sidecar")
        if root is None:
            raise KillCheckFailure(f"cannot open the sidecar process {pid}")
        tracked.append(root)
        if not job.contains(root.handle):
            raise KillCheckFailure("the sidecar is not in the job after AssignProcessToJobObject")
        created: dict[int, int | None] = {pid: fs.describe_process(pid)[0]}
        early = _find_workers(pid, created[pid], image_dir)
        print(f"attempt {attempt}: sidecar pid {pid} READY, children before work: {len(early)}")

        done = _post_in_background(sidecar, token, kill_spec(attempt))
        deadline = time.monotonic() + observe_timeout
        workers: list[fs.ProcInfo] = []
        while time.monotonic() < deadline:
            workers = _find_workers(pid, created[pid], image_dir)
            if workers or done.is_set():
                break
            time.sleep(POLL_SECONDS)
        held = [w for w in (Tracked.open(p.pid, p.name, "render worker") for p in workers) if w]
        tracked.extend(held)
        for proc in workers:
            created[proc.pid] = proc.created
        live_workers = [w for w in held if w.alive()]
        if not live_workers:
            print(f"attempt {attempt}: no live render worker observed before the render finished")
            return None
        for worker in live_workers:
            if not job.contains(worker.handle):
                raise KillCheckFailure(f"{worker.describe()} is not in the job (breakaway?)")

        started = time.perf_counter()
        job.close()
        killed = True
        survivors = wait_all_exit(tracked, EXIT_DEADLINE_SECONDS)
        elapsed = time.perf_counter() - started
        if survivors:
            names = ", ".join(p.describe() for p in survivors)
            raise KillCheckFailure(
                f"still running {EXIT_DEADLINE_SECONDS:.0f}s after closing the job: {names}"
            )
        orphans = leftover_children(tracked, created, image_dir)
        if orphans:
            names = ", ".join(f"pid {p.pid} {p.path}" for p in orphans)
            raise KillCheckFailure(f"processes left behind after closing the job: {names}")
        result = {
            "attempt": attempt,
            "killed": [
                {"role": p.role, "pid": p.pid, "exit_code": p.exit_code()} for p in tracked
            ],
            "exit_seconds": round(elapsed, 3),
        }
        print(
            f"attempt {attempt}: closed the job with {len(live_workers)} live render worker(s); "
            f"{len(tracked)} tracked processes exited in {elapsed:.3f}s"
        )
        return result
    finally:
        # Only our own processes: anything still alive here is a failure or an aborted attempt.
        _close_all(tracked, terminate=True)
        if not killed:
            job.close()
        if sidecar is not None:
            sidecar.stop()
        for _ in range(20):
            shutil.rmtree(workdir, ignore_errors=True)
            if not workdir.exists():
                break
            time.sleep(0.25)


def run_sidecar_check(
    exe: Path, attempts: int, ready_timeout: float, observe_timeout: float
) -> dict[str, Any]:
    for attempt in range(1, attempts + 1):
        result = sidecar_attempt(exe, attempt, ready_timeout, observe_timeout)
        if result is not None:
            return result
    raise KillCheckFailure(
        f"no render worker was observed in {attempts} attempt(s); the kill check proves nothing"
    )


# --------------------------------------------------------------------------- shell mode


def run_shell_check(shell_exe: Path, sidecar_timeout: float, settle: float) -> dict[str, Any]:
    """Launch the installed shell, force-kill it, assert its sidecar child is gone."""
    install_dir = shell_exe.parent
    sidecar_dir = install_dir / "sidecar"
    env = fs._base_env(isolated=True)
    flags = getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0)
    proc = subprocess.Popen(
        [str(shell_exe)],
        cwd=install_dir,
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        creationflags=flags,
    )
    shell = Tracked.open(proc.pid, shell_exe.name, "shell")
    if shell is None:
        proc.kill()
        raise KillCheckFailure(f"cannot open the shell process {proc.pid}")
    shell_created = fs.describe_process(proc.pid)[0]
    tracked: list[Tracked] = [shell]
    others: list[Tracked] = []
    try:
        deadline = time.monotonic() + sidecar_timeout
        found: list[fs.ProcInfo] = []
        while time.monotonic() < deadline and not found:
            if not shell.alive():
                raise KillCheckFailure(f"the shell exited early (code {shell.exit_code()})")
            found = _find_workers(proc.pid, shell_created, sidecar_dir)
            if not found:
                time.sleep(0.1)
        if not found:
            raise KillCheckFailure(
                f"no sidecar child of the shell appeared within {sidecar_timeout:.0f}s"
            )
        sidecar_info = found[0]
        sidecar = Tracked.open(sidecar_info.pid, sidecar_info.name, "sidecar")
        if sidecar is None:
            raise KillCheckFailure(f"cannot open the shell's sidecar {sidecar_info.pid}")
        tracked.append(sidecar)
        # The shell assigns the (suspended) sidecar to its job immediately after spawning it;
        # give it a moment so the kill does not race that window.
        time.sleep(settle)
        if not sidecar.alive():
            raise KillCheckFailure(f"the shell's sidecar exited by itself (code {sidecar.exit_code()})")
        # Everything else the shell started (WebView2) - cleaned up later, never asserted.
        for info in fs.walk_tree(
            fs.process_snapshot(), {proc.pid: shell_created}, fs.describe_process
        ):
            if info.pid != sidecar.pid:
                other = Tracked.open(info.pid, info.name, "shell descendant")
                if other:
                    others.append(other)
        print(f"shell pid {proc.pid} running sidecar pid {sidecar.pid}; force-killing the shell")
        fs.kernel32().TerminateProcess(shell.handle, 1)
        if not shell.wait(10):
            raise KillCheckFailure("the shell did not die after TerminateProcess")
        started = time.perf_counter()
        survivors = wait_all_exit([sidecar], EXIT_DEADLINE_SECONDS)
        elapsed = time.perf_counter() - started
        if survivors:
            raise KillCheckFailure(
                f"the sidecar (pid {sidecar.pid}) outlived the killed shell by "
                f"{EXIT_DEADLINE_SECONDS:.0f}s: job.rs kill-on-close did not fire"
            )
        print(f"shell killed: sidecar pid {sidecar.pid} exited {elapsed:.3f}s later")
        return {"shell_pid": proc.pid, "sidecar_pid": sidecar.pid, "exit_seconds": round(elapsed, 3)}
    finally:
        _close_all(tracked, terminate=True)
        time.sleep(1)  # let WebView2 notice its host is gone before we step in
        _close_all(others, terminate=True)
        proc.wait(timeout=10)


# --------------------------------------------------------------------------- main


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Kill-on-close job check for the frozen sidecar")
    parser.add_argument("--exe", type=Path, help=f"frozen sidecar (default {fs.DEFAULT_EXE})")
    parser.add_argument("--shell-exe", type=Path, help=f"installed {SHELL_EXE_NAME} to crash")
    parser.add_argument("--attempts", type=int, default=3)
    parser.add_argument("--ready-timeout", type=float, default=180.0)
    parser.add_argument("--observe-timeout", type=float, default=300.0)
    parser.add_argument("--shell-timeout", type=float, default=120.0)
    parser.add_argument("--settle", type=float, default=3.0, help="seconds before killing the shell")
    parser.add_argument("--json", type=Path, help="also write the results here")
    args = parser.parse_args(argv)

    if sys.platform != "win32":
        print("KILL_CHECK_SKIPPED: Job Objects are Windows-only")
        return 0

    exe = args.exe if args.exe or args.shell_exe else fs.DEFAULT_EXE
    results: dict[str, Any] = {}
    try:
        if exe is not None:
            exe = exe.resolve()
            if not exe.is_file() or exe.stat().st_size == 0:
                print(f"frozen sidecar not found (or a placeholder): {exe}", file=sys.stderr)
                return 2
            results["sidecar"] = run_sidecar_check(
                exe, max(1, args.attempts), args.ready_timeout, args.observe_timeout
            )
        if args.shell_exe is not None:
            shell_exe = args.shell_exe.resolve()
            if not shell_exe.is_file():
                print(f"shell executable not found: {shell_exe}", file=sys.stderr)
                return 2
            results["shell"] = run_shell_check(shell_exe, args.shell_timeout, args.settle)
    except (KillCheckFailure, fs.SmokeFailure, OSError, subprocess.TimeoutExpired) as exc:
        print(f"KILL_CHECK_FAILED: {exc}", file=sys.stderr)
        return 1
    if args.json:
        args.json.parent.mkdir(parents=True, exist_ok=True)
        args.json.write_text(json.dumps(results, indent=2) + "\n", encoding="utf-8")
    print("KILL_CHECK_OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
