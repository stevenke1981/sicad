"""Pure tests for the cross-platform parts of scripts/frozen_smoke.py (ADR 0011).

The ps / /proc parsers run on hand-written captures in the tools' real output format, so
the Linux and macOS code paths are covered on any OS.
"""

from __future__ import annotations

import importlib.util
import inspect
import os
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]


def _load(name: str):
    spec = importlib.util.spec_from_file_location(f"_{name}", ROOT / "scripts" / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module  # dataclasses resolve annotations via sys.modules
    spec.loader.exec_module(module)
    return module


fs = _load("frozen_smoke")

# `ps -A -o pid=,ppid=,comm=` on Linux (procps-ng): bare task names truncated to 15 bytes.
PS_LINUX = """\
      1       0 systemd
      2       0 kthreadd
    812       1 sshd
   4242     812 bash
   5001    4242 promptcad-sidec
   5002    5001 promptcad-sidec
   5003    5001 python3.12
   5004       2 kworker/0:1-events
"""

# `ps -A -o pid=,ppid=,comm=` on macOS: full executable paths, possibly with spaces.
PS_MAC = """\
    1     0 /sbin/launchd
  501     1 /usr/libexec/logd
  733     1 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome
 9001   733 /Users/runner/work/PromptCAD.app/Contents/Resources/sidecar/promptcad-sidecar
 9002  9001 /Users/runner/work/PromptCAD.app/Contents/Resources/sidecar/promptcad-sidecar
 9003  9001 (python3.12)
garbage line
"""

# /proc/<pid>/stat whose comm contains spaces and a ')'.
PROC_STAT = (
    "5002 (promptcad (x) y) S 5001 5001 4242 34816 5001 4194560 1830 0 0 0 12 3 0 0 20 0 "
    "4 0 987654 1234567890 12345 18446744073709551615 1 1 0 0 0 0 0 4096 0 0 0 0 17 3 0 0"
)


def test_contract_signatures():
    params = inspect.signature(fs.spawn_sidecar).parameters
    for name in ("isolated", "on_spawn", "extra_args", "stdin"):
        assert params[name].kind is inspect.Parameter.KEYWORD_ONLY
    assert params["extra_args"].default == ()
    assert params["stdin"].default is None
    for func in (fs.process_snapshot, fs.describe_process):
        assert callable(func)
    suffix = ".exe" if sys.platform == "win32" else ""
    assert (fs.EXE_NAME, fs.DEFAULT_EXE.name) == ("promptcad-sidecar" + suffix,) * 2


def test_sidecar_command_appends_extra_args():
    exe = Path("/opt/sidecar/promptcad-sidecar")
    assert fs.sidecar_command(exe) == [str(exe), "--host", "127.0.0.1", "--port", "0"]
    assert fs.sidecar_command(exe, ["--exit-on-stdin-eof"])[-1] == "--exit-on-stdin-eof"


def test_parse_ps_linux():
    rows = fs.parse_ps_output(PS_LINUX)
    assert rows[0] == fs.ProcEntry(1, 0, "systemd")
    assert fs.ProcEntry(5002, 5001, "promptcad-sidec") in rows
    assert fs.ProcEntry(5004, 2, "kworker/0:1-events") in rows  # not an absolute path
    assert len(rows) == 8


def test_parse_ps_mac_paths_with_spaces_and_garbage():
    rows = {r.pid: r for r in fs.parse_ps_output(PS_MAC)}
    assert rows[733].name == "Google Chrome"
    assert rows[9002] == fs.ProcEntry(9002, 9001, "promptcad-sidecar")
    assert rows[9003].name == "(python3.12)"
    assert len(rows) == 6


def test_linux_full_name_undoes_truncation():
    exe = "/opt/promptcad/sidecar/promptcad-sidecar"
    assert fs.linux_full_name("promptcad-sidec", exe) == "promptcad-sidecar"
    assert fs.linux_full_name("promptcad-sidec", None) == "promptcad-sidec"
    assert fs.linux_full_name("promptcad-sidec", "/usr/bin/other-binary-name") == "promptcad-sidec"
    assert fs.linux_full_name("bash", "/usr/bin/bash-wrapper") == "bash"  # not truncated
    assert fs.strip_deleted("/tmp/x/promptcad-sidecar (deleted)") == "/tmp/x/promptcad-sidecar"
    assert fs.strip_deleted("/usr/bin/bash") == "/usr/bin/bash"


def test_linux_stat_starttime():
    assert fs.linux_stat_starttime(PROC_STAT) == 987654
    assert fs.linux_stat_starttime("garbage") is None
    assert fs.linux_stat_starttime("1 (x) S 0 1") is None


def test_parse_lstart_orders_processes():
    earlier = fs.parse_lstart("Wed Nov 26 10:00:00 2026")
    later = fs.parse_lstart("  Wed Nov 26 10:00:05 2026\n")
    assert earlier is not None and later - earlier == 5
    assert fs.parse_lstart("") is None
    assert fs.parse_lstart("yesterday") is None


@pytest.mark.parametrize(
    ("name", "expected"),
    [
        ("python3", True),
        ("python3.12", True),
        ("python", True),
        ("Python", True),
        ("pythonic-tool", False),
        ("python3-config", False),
        ("promptcad-sidecar", False),
    ],
)
def test_is_python_image_unix_names(name, expected):
    assert fs.is_python_image(name) is expected


def test_is_within_posix_and_windows():
    assert fs.is_windows_path(r"C:\App") and fs.is_windows_path(r"\\srv\share")
    assert not fs.is_windows_path("/opt/app")
    assert fs.is_within("/opt/app/sidecar/_internal/x", "/opt/app/sidecar")
    assert not fs.is_within("/opt/app/sidecar2/x", "/opt/app/sidecar")
    assert not fs.is_within("/opt/App/sidecar/x", "/opt/app/sidecar")  # POSIX is case-sensitive
    assert fs.is_within(r"d:/app/SIDECAR/x.exe", r"D:\App\sidecar")


def test_unix_tree_foreign_processes():
    sidecar_dir = "/opt/PromptCAD/sidecar"
    table = fs.parse_ps_output(
        """\
  100     1 promptcad-sidecar
  101   100 promptcad-sidecar
  102   100 python3
  103   100 sh
  104   100 python3.12
"""
    )
    described = {
        100: (1000, "/opt/PromptCAD/sidecar/promptcad-sidecar"),
        101: (1001, "/opt/PromptCAD/sidecar/promptcad-sidecar"),
        102: (1002, "/usr/bin/python3.12"),
        103: (1003, "/usr/bin/dash"),
        104: (None, None),  # exited before it could be described: foreign by name
    }
    tree = fs.walk_tree(table, {100: 1000}, lambda pid: described[pid])
    foreign = fs.foreign_processes(tree, sidecar_dir, r"C:\Windows")
    assert {f["pid"]: f["reason"] for f in foreign} == {
        102: "python interpreter",
        103: "image outside the sidecar directory",
        104: "python interpreter",
    }
    assert fs.foreign_processes([p for p in tree if p.pid == 101], sidecar_dir, "") == []


def test_isolated_env_unix():
    base = {
        "PATH": "/home/u/.venv/bin:/usr/local/bin:/usr/bin",
        "PYTHONPATH": "/repo",
        "VIRTUAL_ENV": "/home/u/.venv",
        "CONDA_PREFIX": "/opt/conda",
        "LD_LIBRARY_PATH": "/opt/vtk/lib",
        "LD_PRELOAD": "/x.so",
        "DYLD_LIBRARY_PATH": "/opt/homebrew/lib",
        "DYLD_FALLBACK_LIBRARY_PATH": "/opt/homebrew/lib",
        "HOME": "/home/u",
        "DISPLAY": ":99",
        "TMPDIR": "/tmp",
    }
    env = fs.isolated_env(base, platform="linux")
    assert env["PATH"] == "/usr/bin:/bin"
    assert {k for k in env if k.startswith("PYTHON")} == {"PYTHONUNBUFFERED", "PYTHONIOENCODING"}
    for gone in ("VIRTUAL_ENV", "CONDA_PREFIX", "LD_LIBRARY_PATH", "LD_PRELOAD",
                 "DYLD_LIBRARY_PATH", "DYLD_FALLBACK_LIBRARY_PATH"):
        assert gone not in env
    assert env["HOME"] == "/home/u" and env["DISPLAY"] == ":99" and env["TMPDIR"] == "/tmp"
    assert fs.isolated_env(base, platform="darwin")["PATH"] == "/usr/bin:/bin"
    assert base["LD_LIBRARY_PATH"] == "/opt/vtk/lib"  # not mutated
    # Windows rules are still selectable from any host.
    assert fs.isolated_env({"SystemRoot": r"C:\Windows"}, platform="win32")["PATH"] == (
        r"C:\Windows\System32;C:\Windows"
    )


def test_tree_stats_counts_and_dereferences_symlinks(tmp_path):
    onedir = tmp_path / "onedir"
    (onedir / "_internal" / "pkg").mkdir(parents=True)
    (onedir / "exe").write_bytes(b"x" * 10)
    (onedir / "_internal" / "pkg" / "lib.so.1").write_bytes(b"y" * 100)
    assert fs.tree_stats(onedir) == (110, 2, 0)
    try:
        os.symlink("lib.so.1", onedir / "_internal" / "pkg" / "lib.so")
        os.symlink(onedir / "_internal" / "pkg", onedir / "_internal" / "alias",
                   target_is_directory=True)
        os.symlink("missing", onedir / "_internal" / "dangling")
    except (OSError, NotImplementedError):
        pytest.skip("symlinks not permitted here")
    # lib.so -> +100; alias/ -> whole pkg tree again (+200 incl. its own link); dangling +0
    total, files, links = fs.tree_stats(onedir)
    assert links == 3
    assert files == 3  # exe, lib.so.1 and the file symlink lib.so
    assert total == 10 + 100 + 100 + 200
    assert fs.dir_stats(onedir) == (total, files)


def test_artifact_stats_file_and_directory(tmp_path):
    deb = tmp_path / "promptcad_0.1.0_amd64.deb"
    deb.write_bytes(b"d" * 7)
    assert fs.artifact_stats(deb) == {"path": str(deb), "kind": "deb", "bytes": 7}
    app = tmp_path / "PromptCAD.app"
    (app / "Contents" / "MacOS").mkdir(parents=True)
    (app / "Contents" / "MacOS" / "promptcad").write_bytes(b"m" * 5)
    record = fs.artifact_stats(app)
    assert record["kind"] == "directory" and record["bytes"] == 5 and record["files"] == 1
    assert fs.installer_label(record) == "Installer size (directory)"
    assert fs.installer_label(None) == "Installer size"


def test_sidecar_stop_closes_held_stdin(tmp_path):
    proc = subprocess.Popen(
        [sys.executable, "-c", "import sys; sys.stdin.read()"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        text=True,
    )
    sidecar = fs.Sidecar(proc, 0, 0.0, tmp_path / "stderr.log")
    sidecar.stop()
    assert proc.poll() is not None
    assert proc.stdin.closed


def test_truncated_linux_comm_without_path_is_not_foreign():
    # A render worker that exits before /proc/<pid>/exe is read shows only its 15-byte comm.
    tree = [
        fs.ProcInfo(pid=10, ppid=1, name=fs.EXE_NAME[:15], path=None, created=None),
        fs.ProcInfo(pid=11, ppid=1, name="curl", path=None, created=None),
    ]
    foreign = fs.foreign_processes(tree, "/opt/app/sidecar", "/")
    assert [f["pid"] for f in foreign] == [11]


def test_check_core_requires_core_available_and_version_ok(monkeypatch):
    replies = {
        ("GET", "/api/v1/capabilities"): {"core_available": True, "core_crate_version": "0.1.0"},
        ("POST", "/api/v1/core"): {"status": "ok", "result": {"crate_version": "0.1.0"}},
    }
    monkeypatch.setattr(fs, "_json", lambda sc, method, path, token, body=None: replies[(method, path)])
    fs.check_core(object(), "t")

    replies[("GET", "/api/v1/capabilities")] = {"core_available": False}
    with pytest.raises(fs.SmokeFailure, match="core_available"):
        fs.check_core(object(), "t")

    replies[("GET", "/api/v1/capabilities")] = {"core_available": True}
    replies[("POST", "/api/v1/core")] = {"status": "error"}
    with pytest.raises(fs.SmokeFailure, match="version op"):
        fs.check_core(object(), "t")


def _artifacts(*names: str, size: int = 10) -> list[dict[str, object]]:
    return [{"filename": name, "size": size} for name in names]


def _completed(artifacts: list[dict[str, object]], **extra: object) -> dict[str, object]:
    manifest: dict[str, object] = {
        "status": "completed",
        "renderer_used": "cadquery",
        "artifacts": artifacts,
        "validation": {"issues": []},
        "warnings": [],
    }
    manifest.update(extra)
    return manifest


def test_cad2_smoke_cases_covers_both_examples_without_test_only_key():
    cases = fs.cad2_smoke_cases()
    assert [label for label, _ in cases] == [
        "cad2:enclosure-with-lid",
        "cad2:plate-four-holes",
    ]
    for _, document in cases:
        assert "expected_resolved" not in document
        assert document["schema_version"] == "2.0"
    # The checked-in files keep their key (only the in-memory copy is stripped).
    on_disk = (fs.CAD2_EXAMPLES_DIR / "plate-four-holes.cad2.json").read_text(encoding="utf-8")
    assert '"expected_resolved"' in on_disk


def test_cad2_smoke_cases_reads_only_cad2_files(tmp_path):
    (tmp_path / "one.cad2.json").write_text(
        '{"schema_version": "2.0", "name": "one", "expected_resolved": {}}', encoding="utf-8"
    )
    (tmp_path / "other.json").write_text('{"schema_version": "2.0"}', encoding="utf-8")
    cases = fs.cad2_smoke_cases(tmp_path)
    assert cases == [("cad2:one", {"schema_version": "2.0", "name": "one"})]
    assert fs.cad2_smoke_cases(tmp_path / "missing") == []


def test_smoke_cases_unchanged_by_cad2_examples(tmp_path):
    # smoke_cases() reads examples/generated only; the cad2 directory never leaks into it.
    labels = [label for label, _ in fs.smoke_cases(tmp_path)]
    assert labels == list(fs.BASES)
    assert not any(label.startswith("cad2") for label, _ in fs.smoke_cases())


def test_cad_files_are_the_one_per_format_artifacts():
    assert fs.CAD_FILES == ("model.step", "model.stl", "model.dxf", "model.svg")
    assert fs.CAD2_EXPECTED_FILES == fs.CAD_FILES
    assert not {"model.scad", "preview.svg"} & set(fs.CAD2_EXPECTED_FILES)


def test_manifest_problems_10_default_requires_every_cad_file():
    full = _completed(_artifacts(*fs.CAD_FILES, "model.scad", "preview.svg"))
    assert fs.manifest_problems(full) == []
    missing_svg = _completed(_artifacts("model.step", "model.stl", "model.dxf"))
    assert fs.manifest_problems(missing_svg) == ["model.svg missing or empty"]
    empty = _completed([*_artifacts("model.step", "model.stl", "model.dxf"), {"filename": "model.svg", "size": 0}])
    assert fs.manifest_problems(empty) == ["model.svg missing or empty"]


def test_manifest_problems_20_needs_no_scad_or_preview():
    # A 2.0 manifest: the CAD files only, no model.scad / preview.svg.
    manifest = _completed(_artifacts("spec.json", "validation.json", "resolved.json", *fs.CAD_FILES))
    assert fs.manifest_problems(manifest, fs.CAD2_EXPECTED_FILES) == []
    # The 1.0 default would still demand the same CAD files, so a 2.0 check that
    # passes for a 2.0 manifest is not a weaker check: a missing STEP still fails.
    no_step = _completed(_artifacts("spec.json", "model.stl", "model.dxf", "model.svg"))
    assert fs.manifest_problems(no_step, fs.CAD2_EXPECTED_FILES) == ["model.step missing or empty"]


def test_manifest_problems_reports_status_renderer_and_core_fields():
    failed = _completed(_artifacts(*fs.CAD_FILES), status="failed", error="boom")
    problems = fs.manifest_problems(failed)
    assert problems == ["status='failed' error='boom'"]
    source_only = _completed(_artifacts(*fs.CAD_FILES), renderer_used="source_only")
    assert fs.manifest_problems(source_only) == ["renderer_used='source_only'"]
    bad_issue = _completed(_artifacts(*fs.CAD_FILES), validation={"issues": [{"layer": "L"}]})
    assert fs.manifest_problems(bad_issue) == ["issue without core fields layer/target: {'layer': 'L'}"]


def test_check_manifest_20_accepts_completed_manifest_with_step(monkeypatch):
    manifest = _completed(_artifacts("spec.json", *fs.CAD_FILES), job_id="job1")
    calls = []

    def fake_request(sidecar, method, path, token, body=None, timeout=600):
        calls.append(path)
        return 200, fs.STEP_MAGIC + b"\n"

    monkeypatch.setattr(fs, "_request", fake_request)
    fs.check_manifest(object(), "t", "cad2:plate", manifest, fs.CAD2_EXPECTED_FILES)
    assert calls == ["/api/v1/jobs/job1/files/model.step"]


def test_check_manifest_2_0_failure_names_the_case(monkeypatch):
    manifest = _completed(_artifacts("spec.json"), job_id="job2")
    monkeypatch.setattr(fs, "_request", lambda *a, **k: (200, fs.STEP_MAGIC))
    with pytest.raises(fs.SmokeFailure, match="cad2:plate-four-holes: model.step missing"):
        fs.check_manifest(object(), "t", "cad2:plate-four-holes", manifest, fs.CAD2_EXPECTED_FILES)


def test_check_manifest_1_0_default_unchanged(monkeypatch):
    manifest = _completed(_artifacts(*fs.CAD_FILES), job_id="job3")
    monkeypatch.setattr(fs, "_request", lambda *a, **k: (200, b"ISO-10303-21;"))
    fs.check_manifest(object(), "t", "example:x", manifest)
    bad_step = _completed(_artifacts(*fs.CAD_FILES), job_id="job4")
    monkeypatch.setattr(fs, "_request", lambda *a, **k: (200, b"not step"))
    with pytest.raises(fs.SmokeFailure, match="model.step download HTTP 200"):
        fs.check_manifest(object(), "t", "example:x", bad_step)
