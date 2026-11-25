"""Verification gate for the frozen desktop sidecar (ADR 0010 Phase 1b).

Drives ``src-tauri/binaries/promptcad-sidecar/promptcad-sidecar.exe`` (built by
``scripts/build_sidecar.py``) the way the Tauri shell does:

a. server mode - spawn with a one-time token and a temporary data dir, wait for the READY
   line, check ``/api/v1/capabilities`` reports ``cadquery_available: true``, then POST
   ``/api/v1/generate-from-spec`` for every base type x every hole type (plus the checked-in
   ``examples/generated/*`` specs) requesting step/stl/dxf/svg, and one prompt through
   ``/api/v1/generate``. Every job must finish ``completed`` with renderer ``cadquery``,
   every requested file must be non-empty and the STEP file must start with
   ``ISO-10303-21``;
b. render-worker mode - time direct ``--render-worker`` invocations on a sample model.py.

``--measure`` additionally repeats spawn->READY and worker runs (first + median of warm
runs) and writes ``build/sidecar/measurements.json`` and ``measurements.md``.

``--isolated-env`` approximates a clean machine (Windows only): the sidecar and the direct
worker runs get ``PATH=%SystemRoot%\\System32;%SystemRoot%`` and no ``PYTHON*``,
``VIRTUAL_ENV*`` or ``CONDA*`` variables (only the two ``PYTHON*`` values the Tauri shell
itself sets), and a watcher thread polls the sidecar's process tree during the server
checks. Any descendant whose image lies outside the sidecar directory (other than the
console host) or is named ``python*.exe`` / ``py.exe`` fails the run, so a bundle that
silently relies on a developer interpreter cannot pass. The watcher must also see at least
one render worker, otherwise the check would be vacuous.

Usage::

    python scripts/frozen_smoke.py [--exe PATH] [--measure] [--installer PATH] [--isolated-env]

Exits non-zero on any failure. Only the standard library is used, so any Python 3.11+
interpreter can run it.
"""

from __future__ import annotations

import argparse
import ctypes
import json
import os
import platform
import queue
import secrets
import shutil
import statistics
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
from collections.abc import Callable, Iterable, Mapping
from dataclasses import dataclass, field
from datetime import UTC, datetime
from pathlib import Path, PureWindowsPath
from typing import Any, NamedTuple

ROOT = Path(__file__).resolve().parents[1]
EXE_NAME = "promptcad-sidecar" + (".exe" if sys.platform == "win32" else "")
DEFAULT_EXE = ROOT / "src-tauri" / "binaries" / "promptcad-sidecar" / EXE_NAME
MEASURE_DIR = ROOT / "build" / "sidecar"
EXAMPLES_DIR = ROOT / "examples" / "generated"
READY_PREFIX = "PROMPTCAD_SIDECAR_READY"
CAD_FORMATS = ["step", "stl", "dxf", "svg"]
FORMAT_FILES = {"step": "model.step", "stl": "model.stl", "dxf": "model.dxf", "svg": "model.svg"}
STEP_MAGIC = b"ISO-10303-21"
HOLE_TYPES = ("through", "blind", "clearance", "tapped", "counterbore", "countersink")
SAMPLE_PROMPT = (
    "畫一個鋁合金固定板，長120mm、寬60mm、厚10mm，四角各一個 M6 通孔，孔中心離邊緣10mm，四角 R5。"
)


class SmokeFailure(Exception):
    pass


# --------------------------------------------------------------------------- specs


def _hole(hole_type: str, x: float, y: float, material: float) -> dict[str, Any]:
    """A z-axis hole of ``hole_type`` sized so it passes validation for ``material`` depth."""
    hole: dict[str, Any] = {"kind": "hole", "x": x, "y": y, "z": 0.0, "axis": "z"}
    if hole_type == "through":
        hole.update(diameter=5.0, hole_type="through")
    elif hole_type == "blind":
        hole.update(diameter=5.0, hole_type="blind", depth=round(material * 0.5, 3))
    elif hole_type == "clearance":
        hole.update(diameter=6.6, hole_type="clearance", thread="M6")
    elif hole_type == "tapped":
        hole.update(diameter=5.0, hole_type="tapped", thread="M6")
    elif hole_type == "counterbore":
        hole.update(
            diameter=6.6,
            hole_type="counterbore",
            counterbore_diameter=11.0,
            counterbore_depth=round(material * 0.4, 3),
        )
    elif hole_type == "countersink":
        # 90 deg, 12 -> 6.6 mm: 2.7 mm deep, so every base's material depth must exceed that.
        hole.update(
            diameter=6.6, hole_type="countersink", countersink_diameter=12.0, countersink_angle=90.0
        )
    else:
        raise ValueError(hole_type)
    return hole


def _ring_positions(radius: float, count: int) -> list[tuple[float, float]]:
    import math

    return [
        (
            round(radius * math.cos(2 * math.pi * i / count), 4),
            round(radius * math.sin(2 * math.pi * i / count), 4),
        )
        for i in range(count)
    ]


# base kind -> (base dict, material depth for z holes, 6 hole centres)
BASES: dict[str, tuple[dict[str, Any], float, list[tuple[float, float]]]] = {
    "plate": (
        {"kind": "plate", "length": 160.0, "width": 100.0, "thickness": 10.0},
        10.0,
        [(x, y) for y in (-25.0, 25.0) for x in (-50.0, 0.0, 50.0)],
    ),
    "cylinder": ({"kind": "cylinder", "diameter": 120.0, "height": 20.0}, 20.0, _ring_positions(35, 6)),
    "ring": (
        {"kind": "ring", "outer_diameter": 140.0, "inner_diameter": 60.0, "height": 15.0},
        15.0,
        _ring_positions(50, 6),
    ),
    "l_bracket": (
        {"kind": "l_bracket", "width": 160.0, "depth": 80.0, "vertical_height": 60.0, "thickness": 6.0},
        6.0,
        [(x, y) for y in (0.0, 25.0) for x in (-50.0, 0.0, 50.0)],
    ),
    "enclosure": (
        {"kind": "enclosure", "length": 120.0, "width": 80.0, "height": 40.0, "wall_thickness": 4.0},
        4.0,
        [(x, y) for y in (-18.0, 18.0) for x in (-35.0, 0.0, 35.0)],
    ),
}


def base_spec(kind: str) -> dict[str, Any]:
    """A valid CadDocument (JSON) for ``kind`` carrying one hole of every hole type."""
    base, material, positions = BASES[kind]
    return {
        "schema_version": "1.0",
        "name": f"smoke-{kind}".replace("_", "-"),
        "source_prompt": f"frozen smoke: {kind} with every hole type",
        "unit": "mm",
        "material": "aluminum",
        "base": dict(base),
        "holes": [
            _hole(hole_type, x, y, material)
            for hole_type, (x, y) in zip(HOLE_TYPES, positions, strict=True)
        ],
        "fillets": [],
        "chamfers": [],
        "assumptions": [],
        "notes": [],
        "planner": {"planner": "frozen-smoke", "confidence": 1.0, "review_required": False},
    }


def smoke_cases(examples_dir: Path = EXAMPLES_DIR) -> list[tuple[str, dict[str, Any]]]:
    cases = [(kind, base_spec(kind)) for kind in BASES]
    if examples_dir.is_dir():
        for spec_path in sorted(examples_dir.glob("*/spec.json")):
            spec = json.loads(spec_path.read_text(encoding="utf-8"))
            cases.append((f"example:{spec_path.parent.name}", spec))
    return cases


# --------------------------------------------------------------------------- environment

# Prefixes (upper-cased) of variables a clean machine would not have. ``PATH`` is replaced.
_STRIPPED_ENV_PREFIXES = ("PYTHON", "VIRTUAL_ENV", "CONDA", "_CONDA")
# What the Tauri shell itself sets for the sidecar (src-tauri/src/sidecar.rs `start`).
SHELL_PYTHON_ENV = {"PYTHONUNBUFFERED": "1", "PYTHONIOENCODING": "utf-8"}


def _lookup(env: Mapping[str, str], name: str) -> str | None:
    """Case-insensitive lookup (Windows environment names are case-insensitive)."""
    for key, value in env.items():
        if key.upper() == name:
            return value
    return None


def isolated_env(base: Mapping[str, str], system_root: str | None = None) -> dict[str, str]:
    """``base`` as a clean Windows machine would see it: system-only PATH, no Python/venv/conda.

    Pure. Every ``PATH`` spelling is dropped and replaced by
    ``%SystemRoot%\\System32;%SystemRoot%``; ``PYTHON*``, ``VIRTUAL_ENV*`` and ``CONDA*``
    variables are dropped, then the shell's own ``PYTHONUNBUFFERED``/``PYTHONIOENCODING``
    are set so the environment matches what the installed app hands the sidecar.
    """
    root = (system_root or _lookup(base, "SYSTEMROOT") or r"C:\Windows").rstrip("\\/")
    env = {
        key: value
        for key, value in base.items()
        if key.upper() != "PATH" and not key.upper().startswith(_STRIPPED_ENV_PREFIXES)
    }
    env["PATH"] = f"{root}\\System32;{root}"
    env.update(SHELL_PYTHON_ENV)
    return env


def _base_env(isolated: bool) -> dict[str, str]:
    env = {k: v for k, v in os.environ.items() if not k.upper().startswith("PROMPTCAD_")}
    return isolated_env(env) if isolated else env


# --------------------------------------------------------------------------- process tree


class ProcEntry(NamedTuple):
    """One row of a process snapshot (Toolhelp ``PROCESSENTRY32W``)."""

    pid: int
    ppid: int
    name: str


class ProcInfo(NamedTuple):
    """A process in a tree walk: ``created`` is a FILETIME tick count, ``path`` the image."""

    pid: int
    ppid: int
    name: str
    path: str | None
    created: int | None


Describe = Callable[[int], tuple[int | None, str | None]]

PYTHON_IMAGE_NAMES = ("py.exe", "pyw.exe")
# Console host Windows attaches to console processes started with CREATE_NO_WINDOW.
SYSTEM_HELPER_IMAGES = ("conhost.exe",)


def is_python_image(name: str) -> bool:
    lowered = name.lower()
    return lowered in PYTHON_IMAGE_NAMES or (
        lowered.startswith("python") and lowered.endswith(".exe")
    )


def is_within(path: str, directory: str | os.PathLike[str]) -> bool:
    """Windows semantics (case-insensitive, either slash), independent of the host OS."""
    return PureWindowsPath(str(directory)) in PureWindowsPath(path).parents


def walk_tree(
    entries: Iterable[ProcEntry], roots: Mapping[int, int | None], describe: Describe
) -> list[ProcInfo]:
    """Descendants of ``roots`` (pid -> creation time) in a snapshot; roots excluded.

    A child counts only if it was created no earlier than its parent, which rules out a
    reused PID whose stale parent id happens to equal one of ours. A child that cannot be
    described (exited between snapshot and query, or access denied) is kept only when its
    name alone already makes it suspicious (``python*.exe``), so nothing slips through.
    """
    children: dict[int, list[ProcEntry]] = {}
    for entry in entries:
        if entry.pid != entry.ppid:  # the idle process is its own parent
            children.setdefault(entry.ppid, []).append(entry)
    found: list[ProcInfo] = []
    seen = set(roots)
    stack = list(roots.items())
    while stack:
        parent_pid, parent_created = stack.pop()
        for child in children.get(parent_pid, ()):
            if child.pid in seen:
                continue
            created, path = describe(child.pid)
            if created is None:
                if is_python_image(child.name):
                    seen.add(child.pid)
                    found.append(ProcInfo(child.pid, child.ppid, child.name, path, None))
                continue
            if parent_created is not None and created < parent_created:
                continue
            seen.add(child.pid)
            found.append(ProcInfo(child.pid, child.ppid, child.name, path, created))
            stack.append((child.pid, created))
    return found


def foreign_processes(
    tree: Iterable[ProcInfo], sidecar_dir: str | os.PathLike[str], system_root: str
) -> list[dict[str, Any]]:
    """Descendants that betray a dependency outside the bundle. Pure.

    Foreign: any ``python*.exe`` / ``py.exe``; any image outside ``sidecar_dir`` except the
    console host in ``%SystemRoot%\\System32``; any process whose image path is unknown and
    whose name is not the sidecar's own.
    """
    system32 = PureWindowsPath(system_root) / "System32"
    foreign = []
    for proc in tree:
        reason = None
        if is_python_image(proc.name):
            reason = "python interpreter"
        elif proc.path is None:
            if proc.name.lower() not in (EXE_NAME.lower(), *SYSTEM_HELPER_IMAGES):
                reason = "image path unavailable"
        elif not is_within(proc.path, sidecar_dir):
            helper = proc.name.lower() in SYSTEM_HELPER_IMAGES and is_within(proc.path, system32)
            if not helper:
                reason = "image outside the sidecar directory"
        if reason:
            foreign.append({"pid": proc.pid, "name": proc.name, "path": proc.path, "reason": reason})
    return foreign


# Win32 plumbing (ctypes, resolved lazily so this module imports on any OS).
TH32CS_SNAPPROCESS = 0x00000002
PROCESS_TERMINATE = 0x0001
PROCESS_SET_QUOTA = 0x0100
PROCESS_QUERY_LIMITED_INFORMATION = 0x1000
SYNCHRONIZE = 0x00100000
_INVALID_HANDLE = ctypes.c_void_p(-1).value


class PROCESSENTRY32W(ctypes.Structure):
    _fields_ = [
        ("dwSize", ctypes.c_uint32),
        ("cntUsage", ctypes.c_uint32),
        ("th32ProcessID", ctypes.c_uint32),
        ("th32DefaultHeapID", ctypes.c_size_t),
        ("th32ModuleID", ctypes.c_uint32),
        ("cntThreads", ctypes.c_uint32),
        ("th32ParentProcessID", ctypes.c_uint32),
        ("pcPriClassBase", ctypes.c_int32),
        ("dwFlags", ctypes.c_uint32),
        ("szExeFile", ctypes.c_wchar * 260),
    ]


class FILETIME(ctypes.Structure):
    _fields_ = [("dwLowDateTime", ctypes.c_uint32), ("dwHighDateTime", ctypes.c_uint32)]


_KERNEL32: Any = None


def kernel32() -> Any:
    """kernel32 with argtypes/restype set (Windows only)."""
    global _KERNEL32
    if _KERNEL32 is None:
        k = ctypes.WinDLL("kernel32", use_last_error=True)  # type: ignore[attr-defined]
        handle, dword, boolean = ctypes.c_void_p, ctypes.c_uint32, ctypes.c_int
        sigs: dict[str, tuple[Any, list[Any]]] = {
            "CreateToolhelp32Snapshot": (handle, [dword, dword]),
            "Process32FirstW": (boolean, [handle, ctypes.POINTER(PROCESSENTRY32W)]),
            "Process32NextW": (boolean, [handle, ctypes.POINTER(PROCESSENTRY32W)]),
            "OpenProcess": (handle, [dword, boolean, dword]),
            "CloseHandle": (boolean, [handle]),
            "GetProcessTimes": (boolean, [handle] + [ctypes.POINTER(FILETIME)] * 4),
            "QueryFullProcessImageNameW": (
                boolean,
                [handle, dword, ctypes.c_wchar_p, ctypes.POINTER(dword)],
            ),
            "WaitForSingleObject": (dword, [handle, dword]),
            "TerminateProcess": (boolean, [handle, ctypes.c_uint]),
            "GetExitCodeProcess": (boolean, [handle, ctypes.POINTER(dword)]),
            "CreateJobObjectW": (handle, [ctypes.c_void_p, ctypes.c_wchar_p]),
            "SetInformationJobObject": (boolean, [handle, ctypes.c_int, ctypes.c_void_p, dword]),
            "AssignProcessToJobObject": (boolean, [handle, handle]),
            "IsProcessInJob": (boolean, [handle, handle, ctypes.POINTER(boolean)]),
        }
        for name, (restype, argtypes) in sigs.items():
            func = getattr(k, name)
            func.restype, func.argtypes = restype, argtypes
        _KERNEL32 = k
    return _KERNEL32


def win_error(what: str) -> OSError:
    code = ctypes.get_last_error()  # type: ignore[attr-defined]
    return OSError(f"{what} failed: {ctypes.FormatError(code).strip()} ({code})")  # type: ignore[attr-defined]


def process_snapshot() -> list[ProcEntry]:
    k = kernel32()
    snap = k.CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
    if snap is None or snap == _INVALID_HANDLE:
        raise win_error("CreateToolhelp32Snapshot")
    try:
        entry = PROCESSENTRY32W()
        entry.dwSize = ctypes.sizeof(PROCESSENTRY32W)
        rows = []
        more = k.Process32FirstW(snap, ctypes.byref(entry))
        while more:
            rows.append(ProcEntry(entry.th32ProcessID, entry.th32ParentProcessID, entry.szExeFile))
            more = k.Process32NextW(snap, ctypes.byref(entry))
        return rows
    finally:
        k.CloseHandle(snap)


def describe_process(pid: int) -> tuple[int | None, str | None]:
    """(creation FILETIME ticks, full image path) of a live process, or (None, None)."""
    k = kernel32()
    handle = k.OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, False, pid)
    if not handle:
        return None, None
    try:
        times = [FILETIME() for _ in range(4)]
        created = None
        if k.GetProcessTimes(handle, *(ctypes.byref(t) for t in times)):
            created = (times[0].dwHighDateTime << 32) | times[0].dwLowDateTime
        buf = ctypes.create_unicode_buffer(32768)
        size = ctypes.c_uint32(len(buf))
        path = buf.value if k.QueryFullProcessImageNameW(handle, 0, buf, ctypes.byref(size)) else None
        return created, path
    finally:
        k.CloseHandle(handle)


def system_root() -> str:
    return _lookup(os.environ, "SYSTEMROOT") or r"C:\Windows"


class ProcessTreeWatcher:
    """Polls a process tree on a thread; records foreign and sidecar-image descendants."""

    def __init__(self, root_pid: int, sidecar_dir: Path, interval: float = 0.05) -> None:
        self.root_pid = root_pid
        self.root_created = describe_process(root_pid)[0]
        self.sidecar_dir = sidecar_dir
        self.interval = interval
        self.foreign: list[dict[str, Any]] = []
        self.workers_seen: set[tuple[int, int | None]] = set()
        self.errors: list[str] = []
        self._stop = threading.Event()
        self._seen_foreign: set[tuple[int, int | None]] = set()
        self._thread = threading.Thread(target=self._run, name="tree-watcher", daemon=True)

    def __enter__(self) -> ProcessTreeWatcher:
        self._thread.start()
        return self

    def __exit__(self, *exc: object) -> None:
        self._stop.set()
        self._thread.join(timeout=10)

    def poll_once(self) -> None:
        tree = walk_tree(process_snapshot(), {self.root_pid: self.root_created}, describe_process)
        for proc in tree:
            if proc.name.lower() == EXE_NAME.lower() and proc.path and is_within(
                proc.path, self.sidecar_dir
            ):
                self.workers_seen.add((proc.pid, proc.created))
        by_pid = {p.pid: p for p in tree}
        for item in foreign_processes(tree, self.sidecar_dir, system_root()):
            key = (item["pid"], by_pid[item["pid"]].created)
            if key not in self._seen_foreign:
                self._seen_foreign.add(key)
                self.foreign.append(item)

    def _run(self) -> None:
        while not self._stop.is_set():
            try:
                self.poll_once()
            except OSError as exc:
                if len(self.errors) < 5:
                    self.errors.append(str(exc))
            self._stop.wait(self.interval)


# --------------------------------------------------------------------------- process


def _env(token: str, data_dir: Path, isolated: bool = False) -> dict[str, str]:
    env = _base_env(isolated)
    env.update(
        PROMPTCAD_API_TOKEN=token,
        PROMPTCAD_DATA_DIR=str(data_dir),
        PROMPTCAD_PLANNER_MODE="rule",
        PROMPTCAD_RENDER_BACKEND="cadquery",
        # A render failure must surface as status=failed, not a silent source-only fallback.
        PROMPTCAD_ALLOW_SOURCE_FALLBACK="false",
        PROMPTCAD_CORS_ORIGINS="http://tauri.localhost,tauri://localhost",
    )
    return env


@dataclass
class Sidecar:
    proc: subprocess.Popen[str]
    port: int
    ready_seconds: float
    stderr_path: Path
    _stderr_file: Any = field(repr=False, default=None)

    @property
    def base_url(self) -> str:
        return f"http://127.0.0.1:{self.port}"

    def stop(self) -> None:
        if self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=30)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait(timeout=30)
        if self.proc.stdout:
            self.proc.stdout.close()
        if self._stderr_file:
            self._stderr_file.close()

    def stderr_tail(self, limit: int = 4000) -> str:
        try:
            return self.stderr_path.read_text(encoding="utf-8", errors="replace")[-limit:]
        except OSError:
            return ""


def spawn_sidecar(
    exe: Path,
    workdir: Path,
    token: str,
    timeout: float,
    *,
    isolated: bool = False,
    on_spawn: Callable[[subprocess.Popen[str]], None] | None = None,
) -> Sidecar:
    """Start the sidecar and wait for READY.

    ``on_spawn`` runs right after the process is created, before the READY line is read
    (frozen_kill_check.py assigns the process to its job there).
    """
    data_dir = workdir / "data"
    data_dir.mkdir(parents=True, exist_ok=True)
    stderr_path = workdir / f"sidecar-stderr-{time.monotonic_ns()}.log"
    stderr_file = stderr_path.open("w", encoding="utf-8")
    started = time.perf_counter()
    proc = subprocess.Popen(
        [str(exe), "--host", "127.0.0.1", "--port", "0"],
        cwd=workdir,  # no repo .env
        env=_env(token, data_dir, isolated),
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=stderr_file,
        text=True,
        encoding="utf-8",
    )
    if on_spawn is not None:
        try:
            on_spawn(proc)
        except BaseException:
            proc.kill()
            proc.wait(timeout=30)
            stderr_file.close()
            raise
    lines: queue.Queue[str] = queue.Queue()
    threading.Thread(target=lambda: lines.put(proc.stdout.readline()), daemon=True).start()
    sidecar = Sidecar(proc, 0, 0.0, stderr_path, stderr_file)
    try:
        line = lines.get(timeout=timeout)
    except queue.Empty:
        sidecar.stop()
        raise SmokeFailure(f"no READY line within {timeout:.0f}s\n{sidecar.stderr_tail()}") from None
    sidecar.ready_seconds = time.perf_counter() - started
    if not line.startswith(READY_PREFIX + " "):
        sidecar.stop()
        raise SmokeFailure(f"unexpected first stdout line {line!r}\n{sidecar.stderr_tail()}")
    sidecar.port = int(json.loads(line[len(READY_PREFIX) + 1 :])["port"])
    return sidecar


def _request(
    sidecar: Sidecar, method: str, path: str, token: str, body: Any = None, timeout: float = 600
) -> tuple[int, bytes]:
    data = None if body is None else json.dumps(body).encode("utf-8")
    req = urllib.request.Request(sidecar.base_url + path, data=data, method=method)
    req.add_header("Authorization", f"Bearer {token}")
    if data is not None:
        req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return resp.status, resp.read()
    except urllib.error.HTTPError as exc:
        return exc.code, exc.read()


def _json(sidecar: Sidecar, method: str, path: str, token: str, body: Any = None) -> Any:
    status, payload = _request(sidecar, method, path, token, body)
    if status != 200:
        raise SmokeFailure(f"{method} {path} -> HTTP {status}: {payload[:2000]!r}")
    return json.loads(payload)


def check_manifest(sidecar: Sidecar, token: str, label: str, manifest: dict[str, Any]) -> None:
    problems = []
    if manifest.get("status") != "completed":
        problems.append(f"status={manifest.get('status')!r} error={manifest.get('error')!r}")
    if manifest.get("renderer_used") != "cadquery":
        problems.append(f"renderer_used={manifest.get('renderer_used')!r}")
    sizes = {a["filename"]: a["size"] for a in manifest.get("artifacts", [])}
    for fmt in CAD_FORMATS:
        name = FORMAT_FILES[fmt]
        if sizes.get(name, 0) <= 0:
            problems.append(f"{name} missing or empty")
    if not problems:
        status, step = _request(
            sidecar, "GET", f"/api/v1/jobs/{manifest['job_id']}/files/model.step", token
        )
        if status != 200 or not step.startswith(STEP_MAGIC):
            problems.append(f"model.step download HTTP {status}, header {step[:20]!r}")
    if problems:
        warnings = manifest.get("warnings") or []
        raise SmokeFailure(f"{label}: " + "; ".join(problems) + f"\nwarnings: {warnings}")


def run_server_checks(
    exe: Path, workdir: Path, timeout: float, results: dict[str, Any], isolated: bool = False
) -> None:
    token = secrets.token_urlsafe(32)
    sidecar = spawn_sidecar(exe, workdir, token, timeout, isolated=isolated)
    results["server_ready_seconds"] = round(sidecar.ready_seconds, 3)
    print(f"READY on port {sidecar.port} after {sidecar.ready_seconds:.2f}s")
    try:
        if isolated:
            with ProcessTreeWatcher(sidecar.proc.pid, exe.parent) as watcher:
                _server_requests(sidecar, token, results)
            check_watcher(watcher, results)
        else:
            _server_requests(sidecar, token, results)
    except (SmokeFailure, OSError) as exc:
        raise SmokeFailure(f"{exc}\n--- sidecar stderr ---\n{sidecar.stderr_tail()}") from exc
    finally:
        sidecar.stop()


def check_watcher(watcher: ProcessTreeWatcher, results: dict[str, Any]) -> None:
    """Fail on foreign descendants, or when no render worker was seen (vacuous check)."""
    results["foreign_processes"] = watcher.foreign
    results["render_workers_observed"] = len(watcher.workers_seen)
    if watcher.foreign:
        rows = "\n".join(
            f"  pid {f['pid']} {f['name']} {f['path']} ({f['reason']})" for f in watcher.foreign
        )
        raise SmokeFailure(f"the sidecar spawned processes from outside its bundle:\n{rows}")
    if not watcher.workers_seen:
        detail = f" (watcher errors: {watcher.errors})" if watcher.errors else ""
        raise SmokeFailure(f"the isolation watcher never observed a render worker{detail}")
    print(
        f"isolated env: {len(watcher.workers_seen)} render workers observed, "
        "foreign_processes=[]"
    )


def _server_requests(sidecar: Sidecar, token: str, results: dict[str, Any]) -> None:
    status, _ = _request(sidecar, "GET", "/api/v1/capabilities", "wrong-token")
    if status != 401:
        raise SmokeFailure(f"capabilities with a wrong token -> HTTP {status}, expected 401")
    caps = _json(sidecar, "GET", "/api/v1/capabilities", token)
    if caps.get("cadquery_available") is not True:
        raise SmokeFailure(f"cadquery_available is not true: {caps}")
    print("capabilities: cadquery_available=true")

    timings: list[dict[str, Any]] = []
    for label, spec in smoke_cases():
        started = time.perf_counter()
        manifest = _json(
            sidecar,
            "POST",
            "/api/v1/generate-from-spec",
            token,
            {"spec": spec, "formats": CAD_FORMATS, "render": True},
        )
        elapsed = time.perf_counter() - started
        check_manifest(sidecar, token, label, manifest)
        timings.append({"case": label, "seconds": round(elapsed, 3)})
        extra = f" ({len(manifest['warnings'])} warnings)" if manifest.get("warnings") else ""
        print(f"  generate-from-spec {label:<28} completed in {elapsed:6.2f}s{extra}")
    results["generate_from_spec"] = timings

    started = time.perf_counter()
    manifest = _json(
        sidecar,
        "POST",
        "/api/v1/generate",
        token,
        {"prompt": SAMPLE_PROMPT, "planner": "rule", "formats": CAD_FORMATS, "render": True},
    )
    elapsed = time.perf_counter() - started
    check_manifest(sidecar, token, "generate (prompt)", manifest)
    results["generate_prompt_seconds"] = round(elapsed, 3)
    print(f"  generate (rule planner prompt)       completed in {elapsed:6.2f}s")


def measure_ready(
    exe: Path, workdir: Path, timeout: float, runs: int, isolated: bool = False
) -> list[float]:
    samples = []
    for i in range(runs):
        sidecar = spawn_sidecar(
            exe, workdir / f"ready-{i}", secrets.token_urlsafe(16), timeout, isolated=isolated
        )
        samples.append(round(sidecar.ready_seconds, 3))
        sidecar.stop()
    return samples


def _worker_env(root: Path, isolated: bool) -> dict[str, str]:
    env = _base_env(isolated)
    env["PROMPTCAD_RENDER_ROOT"] = str(root)
    return env


def run_worker(exe: Path, root: Path, index: int, timeout: float, isolated: bool = False) -> float:
    """One direct ``--render-worker`` run of the plate example; returns wall seconds."""
    job = root / f"job-{index}"
    job.mkdir(parents=True)
    source = EXAMPLES_DIR / "plate-four-holes" / "model.py"
    shutil.copy2(source, job / "model.py")
    env = _worker_env(root, isolated)
    cmd = [
        str(exe),
        "--render-worker",
        str(job / "model.py"),
        "--output-dir",
        str(job),
        "--formats",
        *CAD_FORMATS,
    ]
    flags = subprocess.CREATE_NO_WINDOW if sys.platform == "win32" else 0
    started = time.perf_counter()
    completed = subprocess.run(
        cmd,
        cwd=job,
        env=env,
        stdin=subprocess.DEVNULL,
        capture_output=True,
        text=True,
        timeout=timeout,
        creationflags=flags,
    )
    elapsed = time.perf_counter() - started
    if completed.returncode != 0:
        raise SmokeFailure(
            f"--render-worker exited {completed.returncode}\n{completed.stderr[-4000:]}"
        )
    for fmt in CAD_FORMATS:
        out = job / FORMAT_FILES[fmt]
        if not out.is_file() or out.stat().st_size == 0:
            raise SmokeFailure(f"--render-worker did not write {out.name}")
    if not (job / "model.step").read_bytes().startswith(STEP_MAGIC):
        raise SmokeFailure("--render-worker model.step lacks the ISO-10303-21 header")
    return round(elapsed, 3)


def run_worker_rejects_outside_root(
    exe: Path, root: Path, outside: Path, isolated: bool = False
) -> None:
    """The worker must refuse a job directory outside PROMPTCAD_RENDER_ROOT (exit 2)."""
    outside.mkdir(parents=True)
    shutil.copy2(EXAMPLES_DIR / "plate-four-holes" / "model.py", outside / "model.py")
    env = _worker_env(root, isolated)
    completed = subprocess.run(
        [str(exe), "--render-worker", str(outside / "model.py"), "--output-dir", str(outside)],
        env=env,
        stdin=subprocess.DEVNULL,
        capture_output=True,
        text=True,
        timeout=120,
    )
    if completed.returncode != 2 or (outside / "model.step").exists():
        raise SmokeFailure(
            f"--render-worker outside the root exited {completed.returncode} (expected 2)"
        )


# --------------------------------------------------------------------------- measure


def dir_stats(path: Path) -> tuple[int, int]:
    total = count = 0
    for item in path.rglob("*"):
        if item.is_file():
            total += item.stat().st_size
            count += 1
    return total, count


def top_dirs(onedir: Path, top: int = 10) -> list[dict[str, Any]]:
    internal = onedir / "_internal"
    entries = []
    for child in internal.iterdir() if internal.is_dir() else ():
        size = dir_stats(child)[0] if child.is_dir() else child.stat().st_size
        entries.append({"path": f"_internal/{child.name}", "bytes": size})
    return sorted(entries, key=lambda e: e["bytes"], reverse=True)[:top]


def _version(dist: str) -> str:
    try:
        from importlib.metadata import version

        return version(dist)
    except Exception:
        return "unknown"


def _first_warm(samples: list[float]) -> dict[str, Any]:
    warm = samples[1:]
    return {
        "first": samples[0] if samples else None,
        "warm_median": round(statistics.median(warm), 3) if warm else None,
        "samples": samples,
    }


def _mib(value: int | None) -> str:
    return "n/a" if value is None else f"{value / 1_048_576:.1f} MiB"


def _secs(value: float | None) -> str:
    return "n/a" if value is None else f"{value:.2f} s"


def render_markdown(m: dict[str, Any]) -> str:
    size = m["onedir"]
    rows = [
        ("Onedir size", _mib(size["bytes"])),
        ("Onedir file count", str(size["files"])),
        ("NSIS installer size", _mib(m.get("installer", {}).get("bytes"))),
        ("Installed size", _mib(m.get("installed", {}).get("bytes"))),
        ("Spawn -> READY (first)", _secs(m["ready"]["first"])),
        (f"Spawn -> READY (warm median of {len(m['ready']['samples']) - 1})",
         _secs(m["ready"]["warm_median"])),
        ("Render worker, plate-four-holes (first)", _secs(m["render_worker"]["first"])),
        (f"Render worker (warm median of {len(m['render_worker']['samples']) - 1})",
         _secs(m["render_worker"]["warm_median"])),
        ("POST /generate end-to-end (rule planner, 4 formats)",
         _secs(m["server"].get("generate_prompt_seconds"))),
        ("POST /generate-from-spec (first in server)", _secs(m["generate_from_spec"]["first"])),
        ("POST /generate-from-spec (median of the rest)",
         _secs(m["generate_from_spec"]["warm_median"])),
    ]
    env = m["environment"]
    lines = [
        "# Frozen sidecar measurements",
        "",
        f"Measured {m['measured_at']} on {env['os']} ({env['machine']}, {env['cpu_count']} CPUs), "
        f"cadquery {env['cadquery']}, PyInstaller {env['pyinstaller']}, Python {env['python']}.",
        "",
        "| Metric | Value |",
        "| --- | --- |",
        *(f"| {k} | {v} |" for k, v in rows),
        "",
        "## Largest entries in `_internal/`",
        "",
        "| Path | Size |",
        "| --- | --- |",
        *(f"| `{e['path']}` | {_mib(e['bytes'])} |" for e in m["onedir"]["top"]),
        "",
        "## generate-from-spec cases",
        "",
        "| Case | Seconds |",
        "| --- | --- |",
        *(f"| {t['case']} | {t['seconds']:.2f} |" for t in m["server"]["generate_from_spec"]),
        "",
    ]
    return "\n".join(lines)


# --------------------------------------------------------------------------- main


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Smoke-test the frozen PromptCAD sidecar")
    parser.add_argument("--exe", type=Path, default=DEFAULT_EXE, help="frozen sidecar executable")
    parser.add_argument("--measure", action="store_true", help="write build/sidecar/measurements.*")
    parser.add_argument("--installer", type=Path, help="NSIS installer to record the size of")
    parser.add_argument("--installed-dir", type=Path, help="installed app dir to record the size of")
    parser.add_argument("--warm-runs", type=int, default=5, help="warm samples for --measure")
    parser.add_argument("--ready-timeout", type=float, default=180.0)
    parser.add_argument("--render-timeout", type=float, default=600.0)
    parser.add_argument("--out-dir", type=Path, default=MEASURE_DIR)
    parser.add_argument(
        "--isolated-env",
        action="store_true",
        help="clean-machine env (system-only PATH, no PYTHON*/VIRTUAL_ENV/CONDA*) and fail if "
        "the sidecar spawns anything from outside its bundle (Windows only)",
    )
    args = parser.parse_args(argv)
    isolated = args.isolated_env
    if isolated and sys.platform != "win32":
        print("--isolated-env is only supported on Windows", file=sys.stderr)
        return 2

    exe = args.exe.resolve()
    if not exe.is_file() or exe.stat().st_size == 0:
        print(f"frozen sidecar not found (or a placeholder): {exe}", file=sys.stderr)
        return 2
    if args.installer and not args.installer.is_file():
        print(f"installer not found: {args.installer}", file=sys.stderr)
        return 2

    server: dict[str, Any] = {}
    workdir = Path(tempfile.mkdtemp(prefix="promptcad-frozen-smoke-"))
    try:
        # b first: the very first worker run is the closest to a cold start we can get here.
        render_root = workdir / "render-root"
        worker_runs = 1 + (args.warm_runs if args.measure else 0)
        if isolated:
            print("isolated env: PATH=" + isolated_env(os.environ)["PATH"])
        worker = [
            run_worker(exe, render_root, i, args.render_timeout, isolated)
            for i in range(worker_runs)
        ]
        print(f"render worker: {', '.join(f'{s:.2f}s' for s in worker)}")
        run_worker_rejects_outside_root(exe, render_root, workdir / "outside", isolated)
        print("render worker rejects a job outside PROMPTCAD_RENDER_ROOT")

        run_server_checks(exe, workdir / "server", args.ready_timeout, server, isolated)
        ready = [server["server_ready_seconds"]]
        if args.measure:
            ready += measure_ready(exe, workdir, args.ready_timeout, args.warm_runs, isolated)
            print(f"spawn->READY: {', '.join(f'{s:.2f}s' for s in ready)}")
    except (SmokeFailure, subprocess.TimeoutExpired, OSError) as exc:
        print(f"FROZEN SMOKE FAILED: {exc}", file=sys.stderr)
        return 1
    finally:
        shutil.rmtree(workdir, ignore_errors=True)

    print("FROZEN SMOKE PASSED")
    if not args.measure:
        return 0

    onedir = exe.parent
    total, files = dir_stats(onedir)
    spec_seconds = [t["seconds"] for t in server["generate_from_spec"]]
    measurements: dict[str, Any] = {
        "measured_at": datetime.now(UTC).isoformat(timespec="seconds"),
        "exe": str(exe),
        "onedir": {"bytes": total, "files": files, "top": top_dirs(onedir)},
        "ready": _first_warm(ready),
        "render_worker": _first_warm(worker),
        "generate_from_spec": _first_warm(spec_seconds),
        "server": server,
        "isolated_env": isolated,
        "foreign_processes": server.get("foreign_processes", []) if isolated else None,
        "render_workers_observed": server.get("render_workers_observed") if isolated else None,
        "environment": {
            "os": platform.platform(),
            "machine": platform.machine(),
            "processor": platform.processor(),
            "cpu_count": os.cpu_count(),
            "python": platform.python_version(),
            "cadquery": _version("cadquery"),
            "pyinstaller": _version("pyinstaller"),
        },
    }
    if args.installer:
        measurements["installer"] = {
            "path": str(args.installer),
            "bytes": args.installer.stat().st_size,
        }
    if args.installed_dir and args.installed_dir.is_dir():
        size, count = dir_stats(args.installed_dir)
        measurements["installed"] = {"path": str(args.installed_dir), "bytes": size, "files": count}

    args.out_dir.mkdir(parents=True, exist_ok=True)
    json_path = args.out_dir / "measurements.json"
    md_path = args.out_dir / "measurements.md"
    json_path.write_text(json.dumps(measurements, indent=2) + "\n", encoding="utf-8")
    md_path.write_text(render_markdown(measurements), encoding="utf-8")
    print(f"Wrote {json_path} and {md_path}")
    print(render_markdown(measurements))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
