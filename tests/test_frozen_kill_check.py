"""Pure tests for the clean-machine smoke helpers and the Job Object kill check (ADR 0010).

The real checks need a frozen sidecar and Windows; they run in desktop-bundle.yml. These
cover the parts that decide pass/fail and run on any OS.
"""

from __future__ import annotations

import ctypes
import importlib.util
import os
import sys
from pathlib import Path

import pytest

from app.models.cad import CadDocument
from app.services.validator import DesignValidator

ROOT = Path(__file__).resolve().parents[1]


def _load(name: str):
    spec = importlib.util.spec_from_file_location(f"_{name}", ROOT / "scripts" / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module  # dataclasses resolve annotations via sys.modules
    spec.loader.exec_module(module)
    return module


frozen_smoke = _load("frozen_smoke")
kill_check = _load("frozen_kill_check")


def test_isolated_env_strips_python_and_path():
    base = {
        "Path": r"C:\Python312;C:\Python312\Scripts;C:\Windows\System32",
        "PATH": r"D:\repo\.venv\Scripts",
        "PYTHONPATH": r"D:\repo",
        "PYTHONHOME": r"C:\Python312",
        "pythonstartup": "x.py",
        "VIRTUAL_ENV": r"D:\repo\.venv",
        "VIRTUAL_ENV_PROMPT": "(venv)",
        "CONDA_PREFIX": r"C:\conda",
        "CONDA_DEFAULT_ENV": "base",
        "_CONDA_ROOT": r"C:\conda",
        "SystemRoot": r"C:\Windows",
        "TEMP": r"C:\Temp",
        "USERPROFILE": r"C:\Users\u",
    }
    env = frozen_smoke.isolated_env(base)

    assert env["PATH"] == r"C:\Windows\System32;C:\Windows"
    assert [k for k in env if k.upper() == "PATH"] == ["PATH"]
    # Only what the Tauri shell itself sets survives among PYTHON* variables.
    python_vars = {k: v for k, v in env.items() if k.upper().startswith("PYTHON")}
    assert python_vars == {"PYTHONUNBUFFERED": "1", "PYTHONIOENCODING": "utf-8"}
    assert not [k for k in env if k.upper().startswith(("VIRTUAL_ENV", "CONDA", "_CONDA"))]
    # Everything a Windows process needs to start is kept.
    assert env["SystemRoot"] == r"C:\Windows"
    assert env["TEMP"] == r"C:\Temp"
    assert env["USERPROFILE"] == r"C:\Users\u"
    # The input is not mutated.
    assert base["PYTHONPATH"] == r"D:\repo"


def test_isolated_env_system_root_fallbacks():
    assert frozen_smoke.isolated_env({"SYSTEMROOT": "D:\\Win\\"})["PATH"] == r"D:\Win\System32;D:\Win"
    assert frozen_smoke.isolated_env({})["PATH"] == r"C:\Windows\System32;C:\Windows"


def test_smoke_env_isolated_keeps_promptcad_settings(tmp_path):
    env = frozen_smoke._env("tok", tmp_path, isolated=True)
    assert env["PATH"] == frozen_smoke.isolated_env(os.environ)["PATH"]
    assert env["PROMPTCAD_API_TOKEN"] == "tok"
    assert env["PROMPTCAD_RENDER_BACKEND"] == "cadquery"
    assert "VIRTUAL_ENV" not in env


def test_foreign_process_detection():
    E = frozen_smoke.ProcEntry
    sidecar_dir = r"D:\App\sidecar"
    table = [
        E(100, 4, "promptcad-sidecar.exe"),  # root
        E(101, 100, "promptcad-sidecar.exe"),  # render worker: fine
        E(102, 101, "conhost.exe"),  # console host in System32: fine
        E(103, 100, "python.exe"),  # a developer interpreter: foreign
        E(104, 103, "cmd.exe"),  # outside the bundle: foreign
        E(105, 100, "python.exe"),  # older than the root: stale reused parent PID, ignored
        E(106, 1, "python.exe"),  # unrelated process, ignored
        E(107, 100, "py.exe"),  # exited before it could be described: still foreign by name
        E(108, 100, "helper.exe"),  # exited before it could be described: ignored
        E(109, 101, "conhost.exe"),  # a "console host" outside System32: foreign
        E(110, 101, "Worker.EXE"),  # inside the bundle, odd casing and slashes: fine
    ]
    described = {
        100: (1000, r"D:\App\sidecar\promptcad-sidecar.exe"),
        101: (1100, r"D:\App\sidecar\promptcad-sidecar.exe"),
        102: (1101, r"C:\Windows\System32\conhost.exe"),
        103: (1200, r"C:\Python312\python.exe"),
        104: (1300, r"C:\Windows\System32\cmd.exe"),
        105: (500, r"C:\Python312\python.exe"),
        106: (1400, r"C:\Python312\python.exe"),
        107: (None, None),
        108: (None, None),
        109: (1500, r"D:\evil\conhost.exe"),
        110: (1600, "d:/app/SIDECAR/_internal/Worker.EXE"),
    }

    tree = frozen_smoke.walk_tree(table, {100: 1000}, lambda pid: described[pid])
    assert sorted(p.pid for p in tree) == [101, 102, 103, 104, 107, 109, 110]

    foreign = frozen_smoke.foreign_processes(tree, sidecar_dir, r"C:\Windows")
    assert {f["pid"]: f["reason"] for f in foreign} == {
        103: "python interpreter",
        104: "image outside the sidecar directory",
        107: "python interpreter",
        109: "image outside the sidecar directory",
    }
    # A clean tree (worker + console host) has no foreign processes.
    clean = [p for p in tree if p.pid in (101, 102, 110)]
    assert frozen_smoke.foreign_processes(clean, sidecar_dir, r"C:\Windows") == []


@pytest.mark.parametrize(
    ("name", "expected"),
    [
        ("python.exe", True),
        ("PYTHONW.EXE", True),
        ("python3.12.exe", True),
        ("py.exe", True),
        ("pyw.exe", True),
        ("promptcad-sidecar.exe", False),
        ("pythonic-tool", False),
        ("conhost.exe", False),
    ],
)
def test_is_python_image(name, expected):
    assert frozen_smoke.is_python_image(name) is expected


def test_job_limit_struct_size_and_flags():
    # Win64 layout is 144 bytes (basic 64 + IO_COUNTERS 48 + 4 SIZE_T); Win32 is 112.
    expected = 144 if ctypes.sizeof(ctypes.c_void_p) == 8 else 112
    assert ctypes.sizeof(kill_check.JOBOBJECT_EXTENDED_LIMIT_INFORMATION) == expected
    assert ctypes.sizeof(kill_check.IO_COUNTERS) == 48
    assert kill_check.JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE == 0x2000
    assert kill_check.JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS == 9

    info = kill_check.kill_on_close_limits()
    # Exactly kill-on-close, like src-tauri/src/job.rs: no breakaway, no other limits.
    assert info.BasicLimitInformation.LimitFlags == 0x2000
    assert info.ProcessMemoryLimit == info.JobMemoryLimit == 0
    assert info.BasicLimitInformation.ActiveProcessLimit == 0


def test_kill_check_skips_off_windows(monkeypatch, capsys):
    monkeypatch.setattr(sys, "platform", "linux")

    def boom(*args, **kwargs):  # nothing may be spawned off Windows
        raise AssertionError("spawned a process off Windows")

    monkeypatch.setattr(kill_check.fs, "spawn_sidecar", boom)
    assert kill_check.main(["--exe", "missing.exe"]) == 0
    out = capsys.readouterr().out
    assert "KILL_CHECK_SKIPPED" in out
    assert "KILL_CHECK_OK" not in out


@pytest.mark.parametrize("attempt", [1, 2, 3])
def test_kill_specs_are_valid_and_get_heavier(attempt):
    spec = kill_check.kill_spec(attempt)
    report = DesignValidator().validate(CadDocument.model_validate(spec))
    assert report.valid, report
    if attempt > 1:
        assert len(spec["holes"]) > len(kill_check.kill_spec(1)["holes"])
