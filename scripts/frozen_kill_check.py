"""Proves the shell's kill-on-close takes the whole frozen sidecar tree down (ADR 0010/0011).

Windows (Job Object)
--------------------

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

macOS / Linux (stdin EOF + process group)
-----------------------------------------
The shell starts the sidecar in its own process group with ``--exit-on-stdin-eof`` and holds
the only write end of its stdin pipe (``src-tauri/src/sidecar.rs``, ``app/parent_watch.py``).
``--exe`` mimics that: spawn the sidecar with the flag and a pipe, check it leads its own
process group and runs Python in-process (no child before any work: the onedir bootloader
must not fork), start a long render and wait for its worker, check the tree is exactly
sidecar + render worker(s), all in the sidecar's group, then end the pipe:

* ``pipe``: this script holds the write end and closes it (the shell's normal shutdown);
* ``holder``: an intermediate holder process owns the only write end and is ``SIGKILL``ed
  (the shell crashing: the kernel closes the pipe).

Both must leave nothing alive in the sidecar's process group within 5 s.

``--shell-exe`` launches the installed shell (Linux: run the whole script under
``xvfb-run -a``; macOS: ``<App>.app/Contents/MacOS/<bin>`` or the ``.app`` itself), waits for
its sidecar child, ``SIGKILL``s the shell and asserts the sidecar and its group are gone in 5 s.

Every check is scoped to processes this script started (tracked PIDs, held by handle on
Windows, plus their descendants by image path; on unix also the sidecar's own process
group) - never a global match by process name, so a PromptCAD Studio the user has open is
left alone.

Usage::

    python scripts/frozen_kill_check.py --exe PATH [--shell-exe PATH] [--attempts 3]
        [--variant pipe|holder|all] [--repeat N] [--sidecar-dir DIR] [--json PATH]

Prints ``KILL_CHECK_OK`` and exits 0 on success, exits 1 on failure, 2 on bad input.
Standard library only (ctypes on Windows, ``ps`` on unix).
"""

from __future__ import annotations

import argparse
import ctypes
import json
import os
import plistlib
import secrets
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any, NamedTuple

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
# Unix (src-tauri/src/sidecar.rs EXIT_ON_STDIN_EOF_FLAG, app/parent_watch.py FLAG).
STDIN_EOF_FLAG = "--exit-on-stdin-eof"
UNIX_VARIANTS = ("pipe", "holder")
UNIX_POLL_SECONDS = 0.050
SIGKILL = int(getattr(signal, "SIGKILL", 9))
# Keeps the inherited write end of the sidecar's stdin pipe open until it is SIGKILLed.
HOLDER_CODE = "import time\nwhile True:\n    time.sleep(3600)\n"
PS_GROUP_CMD = ("ps", "-A", "-o", "pid=,pgid=,stat=")
PS_STATE_CMD = ("ps", "-o", "stat=", "-p")
_PS_ENV = {"LC_ALL": "C", "PATH": "/usr/bin:/bin"}


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


# --------------------------------------------------------------------------- unix


class GroupRow(NamedTuple):
    """One row of ``ps -A -o pid=,pgid=,stat=``."""

    pid: int
    pgid: int
    stat: str


def parse_group_rows(text: str) -> list[GroupRow]:
    """Rows of ``ps -A -o pid=,pgid=,stat=``; malformed lines are skipped. Pure."""
    rows = []
    for line in text.splitlines():
        parts = line.split()
        if len(parts) >= 3 and parts[0].isdigit() and parts[1].isdigit():
            rows.append(GroupRow(int(parts[0]), int(parts[1]), parts[2]))
    return rows


def is_zombie(stat: str | None) -> bool:
    """``ps`` state ``Z``: exited, waiting to be reaped - not running any more. Pure."""
    return bool(stat) and stat.lstrip().upper().startswith("Z")


def live_group_members(rows: list[GroupRow], pgid: int) -> list[int]:
    """Pids still running (not zombies) in process group ``pgid``. Pure."""
    return sorted(r.pid for r in rows if r.pgid == pgid and not is_zombie(r.stat))


def is_sidecar_image(info: fs.ProcInfo, image_dir: Path | None) -> bool:
    """The sidecar's own image (a render worker, or the shell's sidecar). Pure.

    With ``image_dir`` the image must lie inside it; without (Linux shell mode, where the
    install location is the packager's choice) its directory must be named ``sidecar``.
    """
    if info.name != fs.EXE_NAME or not info.path:
        return False
    if image_dir is not None:
        return fs.is_within(info.path, image_dir)
    return Path(info.path).parent.name == "sidecar"


def unix_tree_problems(
    tree: list[fs.ProcInfo], sidecar_pid: int, image_dir: Path, pgids: dict[int, int]
) -> list[str]:
    """Why a sidecar tree is not exactly sidecar + render workers in the sidecar's group. Pure."""
    problems = []
    for proc in tree:
        label = f"pid {proc.pid} {proc.path or proc.name}"
        if not is_sidecar_image(proc, image_dir):
            problems.append(f"unexpected process in the sidecar tree: {label}")
        elif proc.ppid != sidecar_pid:
            problems.append(f"render worker is not a direct child of the sidecar: {label}")
        pgid = pgids.get(proc.pid)
        if pgid is not None and pgid != sidecar_pid:
            problems.append(f"{label} is in process group {pgid}, not the sidecar's {sidecar_pid}")
    return problems


def sidecar_dir_for_shell(shell_exe: Path, platform: str) -> Path | None:
    """Where the installed shell's ``bundle.resources`` sidecar lives (``None``: unknown). Pure."""
    if platform == "win32":
        return shell_exe.parent / "sidecar"
    if platform == "darwin":  # Foo.app/Contents/MacOS/<bin> -> Foo.app/Contents/Resources
        return shell_exe.parent.parent / "Resources" / "sidecar"
    return None  # Linux: /usr/lib/<product>/sidecar for the .deb, under $APPDIR for AppImage


def resolve_shell_exe(path: Path) -> Path:
    """A macOS ``.app`` bundle resolves to ``Contents/MacOS/<CFBundleExecutable>``."""
    path = path.resolve()
    if path.suffix == ".app" and path.is_dir():
        try:
            with (path / "Contents" / "Info.plist").open("rb") as handle:
                name = plistlib.load(handle).get("CFBundleExecutable")
        except (OSError, plistlib.InvalidFileException):
            name = None
        if not name:
            binaries = sorted((path / "Contents" / "MacOS").glob("*"))
            name = binaries[0].name if len(binaries) == 1 else path.stem
        return path / "Contents" / "MacOS" / str(name)
    return path


def _ps(cmd: tuple[str, ...]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(cmd, capture_output=True, text=True, env=_PS_ENV, timeout=30, check=False)


def group_rows() -> list[GroupRow]:
    completed = _ps(PS_GROUP_CMD)
    if completed.returncode != 0:
        raise OSError(f"ps failed ({completed.returncode}): {completed.stderr.strip()}")
    return parse_group_rows(completed.stdout)


def _process_state(pid: int) -> str | None:
    completed = _ps((*PS_STATE_CMD, str(pid)))
    if completed.returncode != 0:
        return None
    return completed.stdout.strip() or None


@dataclass
class UnixTracked:
    """A unix process this script is responsible for; ``proc`` when it is our own child."""

    pid: int
    name: str
    role: str
    created: int | None
    proc: subprocess.Popen[Any] | None = None

    def alive(self) -> bool:
        if self.proc is not None:
            return self.proc.poll() is None
        created, _path = fs.describe_process(self.pid)
        if created is None or (self.created is not None and created != self.created):
            return False  # gone, or the pid now belongs to another process
        state = _process_state(self.pid)
        return state is not None and not is_zombie(state)

    def exit_code(self) -> int | None:
        return self.proc.returncode if self.proc is not None else None

    def kill(self) -> None:
        if not self.alive():
            return
        try:
            if self.proc is not None:
                self.proc.kill()
                self.proc.wait(timeout=10)
            else:
                os.kill(self.pid, SIGKILL)
        except (OSError, subprocess.TimeoutExpired):
            pass

    def describe(self) -> str:
        return f"{self.role} pid {self.pid} ({self.name})"


def wait_unix_gone(tracked: list[UnixTracked], deadline_seconds: float) -> list[UnixTracked]:
    """Returns the processes still running ``deadline_seconds`` after the call."""
    deadline = time.monotonic() + deadline_seconds
    pending = list(tracked)
    while pending:
        pending = [p for p in pending if p.alive()]
        if not pending or time.monotonic() >= deadline:
            break
        time.sleep(UNIX_POLL_SECONDS)
    return pending


def _kill_group_if_leader(pid: int) -> None:
    """Cleanup: ``SIGKILL`` the group led by ``pid`` (an unreaped child of ours)."""
    try:
        if os.getpgid(pid) == pid:
            os.killpg(pid, SIGKILL)
        else:
            os.kill(pid, SIGKILL)
    except OSError:
        pass


def _spawn_holder(write_fd: int) -> subprocess.Popen[bytes]:
    return subprocess.Popen(
        [sys.executable, "-c", HOLDER_CODE],
        pass_fds=(write_fd,),
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def unix_sidecar_attempt(
    exe: Path, variant: str, attempt: int, ready_timeout: float, observe_timeout: float
) -> dict[str, Any] | None:
    """One unix kill attempt. Returns a result dict, or ``None`` if no worker was observed."""
    image_dir = exe.parent
    workdir = Path(tempfile.mkdtemp(prefix="promptcad-kill-check-"))
    holder: subprocess.Popen[bytes] | None = None
    sidecar: fs.Sidecar | None = None
    tracked: list[UnixTracked] = []
    label = f"{variant} attempt {attempt}"
    try:
        token = secrets.token_urlsafe(32)
        spawn: dict[str, Any] = {"isolated": True, "extra_args": (STDIN_EOF_FLAG,)}
        if variant == "holder":
            read_fd, write_fd = os.pipe()  # both non-inheritable
            try:
                holder = _spawn_holder(write_fd)
            finally:
                os.close(write_fd)  # the holder now owns the only write end
            try:
                sidecar = fs.spawn_sidecar(exe, workdir, token, ready_timeout, stdin=read_fd, **spawn)
            finally:
                os.close(read_fd)
        else:
            sidecar = fs.spawn_sidecar(
                exe, workdir, token, ready_timeout, stdin=subprocess.PIPE, **spawn
            )
        pid = sidecar.proc.pid
        created = fs.describe_process(pid)[0]
        tracked.append(UnixTracked(pid, fs.EXE_NAME, "sidecar", created, sidecar.proc))
        pgid = os.getpgid(pid)
        if pgid != pid:
            raise KillCheckFailure(
                f"the sidecar (pid {pid}) is in process group {pgid}, not its own: "
                f"{STDIN_EOF_FLAG} did not make it a group leader"
            )
        early = fs.walk_tree(fs.process_snapshot(), {pid: created}, fs.describe_process)
        if early:
            names = ", ".join(f"pid {p.pid} {p.path or p.name}" for p in early)
            raise KillCheckFailure(
                f"the sidecar has child processes before any work ({names}); the onedir "
                "bootloader must run Python in-process"
            )
        print(f"{label}: sidecar pid {pid} READY, own process group, single process")

        done = _post_in_background(sidecar, token, kill_spec(attempt))
        deadline = time.monotonic() + observe_timeout
        tree: list[fs.ProcInfo] = []
        while time.monotonic() < deadline:
            tree = fs.walk_tree(fs.process_snapshot(), {pid: created}, fs.describe_process)
            if tree or done.is_set():
                break
            time.sleep(POLL_SECONDS)
        if tree:
            pgids = {row.pid: row.pgid for row in group_rows()}
            problems = unix_tree_problems(tree, pid, image_dir, pgids)
            if problems:
                raise KillCheckFailure("; ".join(problems))
        workers = [p for p in tree if is_sidecar_image(p, image_dir) and p.ppid == pid]
        if not workers:
            print(f"{label}: no live render worker observed before the render finished")
            return None
        held = [UnixTracked(w.pid, w.name, "render worker", w.created) for w in workers]
        tracked.extend(held)
        live_workers = [w for w in held if w.alive()]
        if not live_workers:
            print(f"{label}: the render worker exited before the pipe was closed")
            return None

        started = time.perf_counter()
        if variant == "holder":
            assert holder is not None
            holder.send_signal(SIGKILL)
            holder.wait(timeout=10)
        else:
            assert sidecar.proc.stdin is not None
            sidecar.proc.stdin.close()
        survivors = wait_unix_gone(tracked, EXIT_DEADLINE_SECONDS)
        elapsed = time.perf_counter() - started
        if survivors:
            names = ", ".join(p.describe() for p in survivors)
            raise KillCheckFailure(
                f"still running {EXIT_DEADLINE_SECONDS:.0f}s after the stdin pipe closed "
                f"({variant}): {names}"
            )
        left = live_group_members(group_rows(), pid)
        if left:
            raise KillCheckFailure(f"processes left in the sidecar's process group {pid}: {left}")
        print(
            f"{label}: ended the pipe with {len(live_workers)} live render worker(s); "
            f"{len(tracked)} tracked processes exited in {elapsed:.3f}s"
        )
        return {
            "variant": variant,
            "attempt": attempt,
            "tree": [{"pid": p.pid, "ppid": p.ppid, "path": p.path} for p in tree],
            "killed": [
                {"role": p.role, "pid": p.pid, "exit_code": p.exit_code()} for p in tracked
            ],
            "exit_seconds": round(elapsed, 3),
        }
    finally:
        # Only our own processes: anything still alive here is a failure or an aborted attempt.
        if sidecar is not None and sidecar.proc.poll() is None:
            _kill_group_if_leader(sidecar.proc.pid)
        for proc in tracked:
            proc.kill()
        if holder is not None and holder.poll() is None:
            holder.kill()
            holder.wait(timeout=10)
        if sidecar is not None:
            sidecar.stop()
        shutil.rmtree(workdir, ignore_errors=True)


def run_unix_sidecar_check(
    exe: Path,
    variants: tuple[str, ...],
    repeat: int,
    attempts: int,
    ready_timeout: float,
    observe_timeout: float,
) -> dict[str, list[dict[str, Any]]]:
    results: dict[str, list[dict[str, Any]]] = {}
    for variant in variants:
        for run in range(1, repeat + 1):
            for attempt in range(1, attempts + 1):
                result = unix_sidecar_attempt(exe, variant, attempt, ready_timeout, observe_timeout)
                if result is not None:
                    result["run"] = run
                    results.setdefault(variant, []).append(result)
                    break
            else:
                raise KillCheckFailure(
                    f"{variant} run {run}: no render worker was observed in {attempts} "
                    "attempt(s); the kill check proves nothing"
                )
    return results


def _run_text(cmd: list[str]) -> str:
    try:
        out = subprocess.run(cmd, capture_output=True, text=True, timeout=30, check=False)
    except (OSError, subprocess.TimeoutExpired) as exc:
        return f"{' '.join(cmd)}: {exc}"
    return (out.stdout + out.stderr).strip()


def linux_pipe_holders(pid: int, proc_root: Path = Path("/proc")) -> list[str]:
    """Processes holding the pipe on ``pid``'s fd 0, with their fd and access mode."""
    try:
        target = os.readlink(proc_root / str(pid) / "fd" / "0")
    except OSError as exc:
        return [f"cannot read fd 0 of {pid}: {exc}"]
    rows = [f"pid {pid} fd 0 -> {target}"]
    for entry in sorted(proc_root.iterdir(), key=lambda e: e.name):
        if not entry.name.isdigit():
            continue
        try:
            fds = list((entry / "fd").iterdir())
        except OSError:
            continue
        for fd in fds:
            try:
                if os.readlink(fd) != target:
                    continue
                flags = (entry / "fdinfo" / fd.name).read_text().split("flags:", 1)[1].split()[0]
                mode = {0: "r", 1: "w", 2: "rw"}.get(int(flags, 8) & 3, "?")
                comm = (entry / "comm").read_text().strip()
            except (OSError, IndexError, ValueError):
                continue
            rows.append(f"  holder pid {entry.name} ({comm}) fd {fd.name} mode {mode}")
    return rows


def unix_shell_diagnostics(sidecar_pid: int, shell_log: Path) -> str:
    """Evidence for a sidecar that outlived its shell: who holds its stdin pipe, the group, and
    the shell's log tail (which includes the sidecar's forwarded stderr)."""
    parts = ["--- diagnostics ---"]
    if sys.platform.startswith("linux"):
        parts += linux_pipe_holders(sidecar_pid)
    else:
        parts.append(_run_text(["lsof", "-nP", "-a", "-p", str(sidecar_pid), "-d", "0"]))
        parts.append(_run_text(["sh", "-c", "lsof -nP 2>/dev/null | grep -E 'PIPE|pipe' | head -80"]))
    parts.append(_run_text(["ps", "-o", "pid,ppid,pgid,stat,command", "-g", str(sidecar_pid)]))
    try:
        tail = shell_log.read_text(encoding="utf-8", errors="replace").splitlines()[-80:]
    except OSError as exc:
        tail = [f"cannot read shell log: {exc}"]
    parts.append("--- shell log (tail) ---")
    parts += tail
    return "\n".join(parts)


def run_unix_shell_check(
    shell_exe: Path, sidecar_dir: Path | None, sidecar_timeout: float, settle: float
) -> dict[str, Any]:
    """Launch the installed shell, ``SIGKILL`` it, assert its sidecar's group is gone."""
    if sys.platform.startswith("linux") and not (
        os.environ.get("DISPLAY") or os.environ.get("WAYLAND_DISPLAY")
    ):
        raise KillCheckFailure("no display for the shell: run this script under `xvfb-run -a`")
    if sidecar_dir is None:
        sidecar_dir = sidecar_dir_for_shell(shell_exe, sys.platform)
    # The shell forwards the sidecar's stderr ("[sidecar] ...") to its own: keep it for the
    # failure report instead of discarding it.
    shell_log = tempfile.NamedTemporaryFile(  # noqa: SIM115 - Popen writes to it, closed below
        prefix="promptcad-shell-", suffix=".log", delete=False
    )
    proc = subprocess.Popen(
        [str(shell_exe)],
        cwd=shell_exe.parent,
        env=fs._base_env(isolated=True),
        stdin=subprocess.DEVNULL,
        stdout=shell_log,
        stderr=subprocess.STDOUT,
        start_new_session=True,  # keep our terminal's signals away from the GUI
    )
    shell_log.close()
    shell_created = fs.describe_process(proc.pid)[0]
    shell = UnixTracked(proc.pid, shell_exe.name, "shell", shell_created, proc)
    sidecar: UnixTracked | None = None
    others: list[UnixTracked] = []
    try:
        deadline = time.monotonic() + sidecar_timeout
        found: list[fs.ProcInfo] = []
        while time.monotonic() < deadline and not found:
            if not shell.alive():
                raise KillCheckFailure(f"the shell exited early (code {proc.returncode})")
            tree = fs.walk_tree(
                fs.process_snapshot(), {proc.pid: shell_created}, fs.describe_process
            )
            found = [p for p in tree if p.ppid == proc.pid and is_sidecar_image(p, sidecar_dir)]
            if not found:
                time.sleep(0.1)
        if not found:
            raise KillCheckFailure(
                f"no sidecar child of the shell appeared within {sidecar_timeout:.0f}s"
            )
        info = found[0]
        sidecar = UnixTracked(info.pid, info.name, "sidecar", info.created)
        time.sleep(settle)
        if not sidecar.alive():
            raise KillCheckFailure("the shell's sidecar exited by itself")
        pgid = os.getpgid(sidecar.pid)
        if pgid != sidecar.pid:
            raise KillCheckFailure(
                f"the shell's sidecar (pid {sidecar.pid}) is in process group {pgid}, not its own"
            )
        # Everything else the shell started (WebKit helpers) - cleaned up later, never asserted.
        for other in fs.walk_tree(
            fs.process_snapshot(), {proc.pid: shell_created}, fs.describe_process
        ):
            if other.pid != sidecar.pid:
                others.append(UnixTracked(other.pid, other.name, "shell descendant", other.created))
        print(f"shell pid {proc.pid} running sidecar pid {sidecar.pid}; SIGKILL the shell")
        proc.send_signal(SIGKILL)
        proc.wait(timeout=10)
        started = time.perf_counter()
        survivors = wait_unix_gone([sidecar], EXIT_DEADLINE_SECONDS)
        elapsed = time.perf_counter() - started
        if survivors:
            print(unix_shell_diagnostics(sidecar.pid, Path(shell_log.name)), flush=True)
            raise KillCheckFailure(
                f"the sidecar (pid {sidecar.pid}) outlived the killed shell by "
                f"{EXIT_DEADLINE_SECONDS:.0f}s: the stdin-EOF watch did not fire"
            )
        left = live_group_members(group_rows(), sidecar.pid)
        if left:
            raise KillCheckFailure(f"processes left in the sidecar's process group: {left}")
        print(f"shell killed: sidecar pid {sidecar.pid} exited {elapsed:.3f}s later")
        return {"shell_pid": proc.pid, "sidecar_pid": sidecar.pid, "exit_seconds": round(elapsed, 3)}
    finally:
        shell.kill()
        if sidecar is not None:
            sidecar.kill()
        time.sleep(1)  # let WebKit helpers notice their host is gone before we step in
        for other in others:
            other.kill()
        proc.wait(timeout=10)


# --------------------------------------------------------------------------- main


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Kill-on-close check for the frozen sidecar")
    parser.add_argument("--exe", type=Path, help=f"frozen sidecar (default {fs.DEFAULT_EXE})")
    parser.add_argument(
        "--shell-exe",
        type=Path,
        help=f"installed shell to crash ({SHELL_EXE_NAME}; macOS: the .app or its "
        "Contents/MacOS binary; Linux: /usr/bin/promptcad-studio under xvfb-run -a)",
    )
    parser.add_argument("--attempts", type=int, default=3)
    parser.add_argument("--ready-timeout", type=float, default=180.0)
    parser.add_argument("--observe-timeout", type=float, default=300.0)
    parser.add_argument("--shell-timeout", type=float, default=120.0)
    parser.add_argument("--settle", type=float, default=3.0, help="seconds before killing the shell")
    parser.add_argument(
        "--variant",
        choices=(*UNIX_VARIANTS, "all"),
        default="all",
        help="unix sidecar mode: close the pipe, SIGKILL a holder process, or both",
    )
    parser.add_argument(
        "--repeat", type=int, default=1, help="unix sidecar mode: run each variant N times"
    )
    parser.add_argument(
        "--sidecar-dir",
        type=Path,
        help="unix shell mode: installed sidecar directory (default: derived from the shell)",
    )
    parser.add_argument("--json", type=Path, help="also write the results here")
    args = parser.parse_args(argv)

    windows = sys.platform == "win32"
    exe = args.exe if args.exe or args.shell_exe else fs.DEFAULT_EXE
    results: dict[str, Any] = {}
    try:
        if exe is not None:
            exe = exe.resolve()
            if not exe.is_file() or exe.stat().st_size == 0:
                print(f"frozen sidecar not found (or a placeholder): {exe}", file=sys.stderr)
                return 2
            if windows:
                results["sidecar"] = run_sidecar_check(
                    exe, max(1, args.attempts), args.ready_timeout, args.observe_timeout
                )
            else:
                variants = UNIX_VARIANTS if args.variant == "all" else (args.variant,)
                results["sidecar"] = run_unix_sidecar_check(
                    exe,
                    variants,
                    max(1, args.repeat),
                    max(1, args.attempts),
                    args.ready_timeout,
                    args.observe_timeout,
                )
        if args.shell_exe is not None:
            shell_exe = args.shell_exe.resolve() if windows else resolve_shell_exe(args.shell_exe)
            if not shell_exe.is_file():
                print(f"shell executable not found: {shell_exe}", file=sys.stderr)
                return 2
            if windows:
                results["shell"] = run_shell_check(shell_exe, args.shell_timeout, args.settle)
            else:
                results["shell"] = run_unix_shell_check(
                    shell_exe, args.sidecar_dir, args.shell_timeout, args.settle
                )
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
