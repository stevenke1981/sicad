"""Pure tests for the clean-machine smoke helpers and the kill-on-close check (ADR 0010/0011).

The real checks need a frozen sidecar (Job Object on Windows, stdin EOF + process group on
macOS/Linux); they run in desktop-bundle.yml. These cover the parts that decide pass/fail
and run on any OS.
"""

from __future__ import annotations

import ctypes
import importlib.util
import json
import os
import sys
from pathlib import Path, PurePosixPath

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
    env = frozen_smoke.isolated_env(base, platform="win32")

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
    env = frozen_smoke.isolated_env({"SYSTEMROOT": "D:\\Win\\"}, platform="win32")
    assert env["PATH"] == r"D:\Win\System32;D:\Win"
    assert frozen_smoke.isolated_env({}, platform="win32")["PATH"] == r"C:\Windows\System32;C:\Windows"


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


@pytest.mark.parametrize("attempt", [1, 2, 3])
def test_kill_specs_are_valid_and_get_heavier(attempt):
    spec = kill_check.kill_spec(attempt)
    report = DesignValidator().validate(CadDocument.model_validate(spec))
    assert report.valid, report
    if attempt > 1:
        assert len(spec["holes"]) > len(kill_check.kill_spec(1)["holes"])


# --------------------------------------------------------------------------- unix mode


def test_stdin_eof_flag_matches_sidecar_and_shell():
    from app import parent_watch

    assert kill_check.STDIN_EOF_FLAG == parent_watch.FLAG == "--exit-on-stdin-eof"
    shell_src = (ROOT / "src-tauri" / "src" / "sidecar.rs").read_text(encoding="utf-8")
    assert 'EXIT_ON_STDIN_EOF_FLAG: &str = "--exit-on-stdin-eof"' in shell_src


def test_parse_group_rows_and_live_members():
    text = (
        "    1     1 Ss\n"
        "  500   500 S\n"
        "  501   500 R+\n"
        "  502   500 Z\n"  # exited worker waiting to be reaped: not alive
        "  503   400 S\n"
        "garbage line\n"
        "  504   500\n"  # malformed (no state): skipped
        "\n"
    )
    rows = kill_check.parse_group_rows(text)
    assert [r.pid for r in rows] == [1, 500, 501, 502, 503]
    assert rows[2] == kill_check.GroupRow(501, 500, "R+")
    assert kill_check.live_group_members(rows, 500) == [500, 501]
    assert kill_check.live_group_members(rows, 999) == []


@pytest.mark.parametrize(
    ("stat", "expected"),
    [("Z", True), ("Z+", True), (" z", True), ("S", False), ("R+", False), ("", False), (None, False)],
)
def test_is_zombie(stat, expected):
    assert kill_check.is_zombie(stat) is expected


def _info(pid, ppid, name, path):
    return frozen_smoke.ProcInfo(pid, ppid, name, path, 1)


def test_unix_tree_is_exactly_sidecar_and_workers(monkeypatch):
    monkeypatch.setattr(kill_check.fs, "EXE_NAME", "promptcad-sidecar")
    image_dir = PurePosixPath("/opt/app/sidecar")
    worker = _info(11, 10, "promptcad-sidecar", "/opt/app/sidecar/promptcad-sidecar")
    assert kill_check.unix_tree_problems([worker], 10, image_dir, {11: 10}) == []
    # Unknown pgid (exited between snapshots) is not a failure by itself.
    assert kill_check.unix_tree_problems([worker], 10, image_dir, {}) == []

    problems = kill_check.unix_tree_problems(
        [
            worker,
            _info(12, 10, "python3.12", "/usr/bin/python3.12"),  # foreign interpreter
            _info(13, 11, "promptcad-sidecar", "/opt/app/sidecar/promptcad-sidecar"),  # forked
            _info(14, 10, "promptcad-sidecar", "/elsewhere/promptcad-sidecar"),  # other bundle
        ],
        10,
        image_dir,
        {11: 77, 12: 10, 13: 10, 14: 10},
    )
    assert any("pid 11" in p and "process group 77" in p for p in problems)
    assert any("unexpected" in p and "pid 12" in p for p in problems)
    assert any("not a direct child" in p and "pid 13" in p for p in problems)
    assert any("unexpected" in p and "pid 14" in p for p in problems)
    assert len(problems) == 4


def test_is_sidecar_image_without_dir_needs_a_sidecar_folder(monkeypatch):
    monkeypatch.setattr(kill_check.fs, "EXE_NAME", "promptcad-sidecar")
    deb = _info(5, 4, "promptcad-sidecar", "/usr/lib/PromptCAD Studio/sidecar/promptcad-sidecar")
    assert kill_check.is_sidecar_image(deb, None)
    assert not kill_check.is_sidecar_image(
        _info(5, 4, "promptcad-sidecar", "/tmp/promptcad-sidecar"), None
    )
    assert not kill_check.is_sidecar_image(_info(5, 4, "promptcad-sidecar", None), None)
    assert not kill_check.is_sidecar_image(
        _info(5, 4, "WebKitWebProcess", "/usr/lib/sidecar/WebKitWebProcess"), None
    )


def test_sidecar_dir_for_shell():
    win = Path("C:/Users/u/AppData/Local/PromptCAD Studio/promptcad-studio.exe")
    assert kill_check.sidecar_dir_for_shell(win, "win32") == win.parent / "sidecar"
    mac = Path("/Applications/PromptCAD Studio.app/Contents/MacOS/promptcad-studio")
    assert kill_check.sidecar_dir_for_shell(mac, "darwin") == Path(
        "/Applications/PromptCAD Studio.app/Contents/Resources/sidecar"
    )
    assert kill_check.sidecar_dir_for_shell(Path("/usr/bin/promptcad-studio"), "linux") is None


def test_resolve_shell_exe_reads_the_app_bundle(tmp_path):
    import plistlib

    app = tmp_path / "PromptCAD Studio.app"
    (app / "Contents" / "MacOS").mkdir(parents=True)
    with (app / "Contents" / "Info.plist").open("wb") as handle:
        plistlib.dump({"CFBundleExecutable": "promptcad-studio"}, handle)
    expected = app.resolve() / "Contents" / "MacOS" / "promptcad-studio"
    assert kill_check.resolve_shell_exe(app) == expected
    # No Info.plist: the only binary in Contents/MacOS.
    (app / "Contents" / "Info.plist").unlink()
    (app / "Contents" / "MacOS" / "only-bin").write_bytes(b"x")
    assert kill_check.resolve_shell_exe(app).name == "only-bin"
    # A plain executable path is returned as is.
    exe = tmp_path / "promptcad-studio"
    exe.write_bytes(b"x")
    assert kill_check.resolve_shell_exe(exe) == exe.resolve()


def test_unix_main_dispatches_variants(monkeypatch, tmp_path, capsys):
    exe = tmp_path / "promptcad-sidecar"
    exe.write_bytes(b"\x7fELF")
    calls = []

    def fake_check(exe_arg, variants, repeat, attempts, ready, observe):
        calls.append((exe_arg, variants, repeat, attempts))
        return {v: [{"variant": v}] for v in variants}

    def boom(*args, **kwargs):  # the Windows Job Object path must not run off Windows
        raise AssertionError("Windows path used off Windows")

    monkeypatch.setattr(sys, "platform", "linux")
    monkeypatch.setattr(kill_check, "run_unix_sidecar_check", fake_check)
    monkeypatch.setattr(kill_check, "run_sidecar_check", boom)
    out_json = tmp_path / "kill.json"
    assert kill_check.main(["--exe", str(exe), "--json", str(out_json)]) == 0
    assert calls == [(exe.resolve(), ("pipe", "holder"), 1, 3)]
    assert "KILL_CHECK_OK" in capsys.readouterr().out
    assert set(json.loads(out_json.read_text(encoding="utf-8"))["sidecar"]) == {"pipe", "holder"}

    calls.clear()
    args = ["--exe", str(exe), "--variant", "holder", "--repeat", "3", "--attempts", "2"]
    assert kill_check.main(args) == 0
    assert calls == [(exe.resolve(), ("holder",), 3, 2)]


def test_unix_main_rejects_missing_or_placeholder_exe(monkeypatch, tmp_path):
    monkeypatch.setattr(sys, "platform", "linux")

    def boom(*args, **kwargs):
        raise AssertionError("spawned a check for a missing sidecar")

    monkeypatch.setattr(kill_check, "run_unix_sidecar_check", boom)
    assert kill_check.main(["--exe", str(tmp_path / "missing")]) == 2
    placeholder = tmp_path / "promptcad-sidecar"
    placeholder.write_bytes(b"")
    assert kill_check.main(["--exe", str(placeholder)]) == 2


def test_unix_failure_is_reported(monkeypatch, tmp_path, capsys):
    exe = tmp_path / "promptcad-sidecar"
    exe.write_bytes(b"\x7fELF")

    def fail(*args, **kwargs):
        raise kill_check.KillCheckFailure("still running 5s after the stdin pipe closed")

    monkeypatch.setattr(sys, "platform", "darwin")
    monkeypatch.setattr(kill_check, "run_unix_sidecar_check", fail)
    assert kill_check.main(["--exe", str(exe)]) == 1
    captured = capsys.readouterr()
    assert "KILL_CHECK_FAILED" in captured.err
    assert "KILL_CHECK_OK" not in captured.out


def test_unix_shell_mode_on_linux_needs_a_display(monkeypatch, tmp_path):
    monkeypatch.setattr(sys, "platform", "linux")
    monkeypatch.delenv("DISPLAY", raising=False)
    monkeypatch.delenv("WAYLAND_DISPLAY", raising=False)

    def boom(*args, **kwargs):
        raise AssertionError("launched the shell without a display")

    monkeypatch.setattr(kill_check.subprocess, "Popen", boom)
    with pytest.raises(kill_check.KillCheckFailure, match="xvfb-run"):
        kill_check.run_unix_shell_check(tmp_path / "promptcad-studio", None, 1.0, 0.0)


def test_unix_shell_mode_dispatch(monkeypatch, tmp_path):
    shell = tmp_path / "promptcad-studio"
    shell.write_bytes(b"\x7fELF")
    seen = []
    monkeypatch.setattr(sys, "platform", "linux")
    monkeypatch.setattr(
        kill_check,
        "run_unix_shell_check",
        lambda exe, sidecar_dir, timeout, settle: seen.append((exe, sidecar_dir)) or {},
    )
    monkeypatch.setattr(kill_check, "run_shell_check", lambda *a: pytest.fail("Windows path"))
    side = tmp_path / "sidecar"
    assert kill_check.main(["--shell-exe", str(shell), "--sidecar-dir", str(side)]) == 0
    assert seen == [(shell.resolve(), side)]


def test_holder_code_blocks_forever():
    # The holder must keep the pipe's write end open until it is SIGKILLed.
    compile(kill_check.HOLDER_CODE, "<holder>", "exec")
    assert "while True" in kill_check.HOLDER_CODE
