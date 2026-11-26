"""Pure tests for scripts/desktop_lock.py and scripts/desktop_env.py (no uv, no network)."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]


def _load(name: str):
    scripts = str(ROOT / "scripts")
    if scripts not in sys.path:  # desktop_env imports desktop_lock by bare name
        sys.path.insert(0, scripts)
    spec = importlib.util.spec_from_file_location(f"_{name}", ROOT / "scripts" / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


lock = _load("desktop_lock")
env = _load("desktop_env")

SHARED_PINS = {
    "cadquery": "2.8.0",
    "cadquery-ocp": "7.9.3.1.1",
    "vtk": "9.6.2",
    "casadi": "3.8.2",
    "nlopt": "2.11.0",
    "numpy": "2.5.5",
    "pyinstaller": "6.22.3",
}


def test_targets_and_file_names():
    assert [t.filename for t in lock.TARGETS] == [
        "desktop-windows-x86_64.txt",
        "desktop-macos-arm64.txt",
        "desktop-linux-x86_64.txt",
    ]
    by_key = {t.key: t for t in lock.TARGETS}
    assert by_key["linux-x86_64"].python_platform == "x86_64-manylinux_2_31"  # cadquery-ocp
    assert by_key["macos-arm64"].python_platform == "aarch64-apple-darwin"
    assert by_key["windows-x86_64"].python_platform == "x86_64-pc-windows-msvc"

    assert lock.target_for("win32", "AMD64").key == "windows-x86_64"
    assert lock.target_for("darwin", "arm64").key == "macos-arm64"
    assert lock.target_for("linux", "x86_64").key == "linux-x86_64"
    assert lock.target_for("darwin", "x86_64") is None  # Intel mac: no lock
    assert lock.target_for("linux", "aarch64") is None
    assert lock.target_for("win32", "ARM64") is None


def test_compile_command():
    target = lock.TARGETS[2]
    cmd = lock.compile_command("uv", target, Path("out.txt"))
    assert cmd[:4] == ["uv", "pip", "compile", "pyproject.toml"]
    joined = " ".join(cmd)
    for part in (
        "--extra cad",
        "--extra desktop",
        "--python-version 3.12",
        "--python-platform x86_64-manylinux_2_31",
        "--generate-hashes",
        "-o out.txt",
    ):
        assert part in joined
    assert cmd[cmd.index("--custom-compile-command") + 1] == "python scripts/desktop_lock.py"
    assert "--upgrade" not in cmd
    assert lock.compile_command("uv", target, Path("o"), upgrade=True)[-1] == "--upgrade"


def test_pins_parser():
    text = (
        "# header\n"
        "Foo_Bar==1.2 \\\n"
        "    --hash=sha256:aa\n"
        "    # via x\n"
        "baz==3.0 ; sys_platform == 'linux' \\\n"
        "    --hash=sha256:bb\n"
        "-e file:///x\n"
    )
    assert lock.pins(text) == {"foo_bar": "1.2", "baz": "3.0"}


@pytest.mark.parametrize("target", lock.TARGETS, ids=lambda t: t.key)
def test_committed_lock_files(target):
    path = lock.lock_path(target)
    text = path.read_text(encoding="utf-8")
    assert "\r" not in text
    assert "#    python scripts/desktop_lock.py" in text.splitlines()[1]
    pins = lock.pins(text)
    for name, version in SHARED_PINS.items():
        assert pins.get(name) == version, (target.key, name)
    # every requirement carries hashes (uv pip sync --require-hashes)
    reqs = [ln for ln in text.splitlines() if ln and ln[0] not in "# "]
    assert len(reqs) == len(pins) > 50
    assert all(ln.endswith("\\") for ln in reqs)
    assert "promptcad-studio" not in pins  # the project is installed editable, separately
    windows_only = {"pefile", "pywin32-ctypes"}
    if target.key == "windows-x86_64":
        assert windows_only <= set(pins)
    else:
        assert not (windows_only & set(pins))


def test_lock_files_agree_on_shared_versions():
    all_pins = [lock.pins(lock.lock_path(t).read_text(encoding="utf-8")) for t in lock.TARGETS]
    common = set.intersection(*(set(p) for p in all_pins))
    assert {"cadquery", "fastapi", "uvicorn"} <= common
    for name in common:
        assert len({p[name] for p in all_pins}) == 1, name


def test_check_detects_drift(tmp_path, monkeypatch):
    reqs = tmp_path / "requirements"
    reqs.mkdir()
    for target in lock.TARGETS:
        (reqs / target.filename).write_text("# h\n#    x\nfoo==1.0 \\\n    --hash=sha256:aa\n")

    monkeypatch.setattr(lock, "_compile", lambda uv, target, out, upgrade: None)  # no change
    assert lock.check("uv", reqs) == []

    def bump(uv, target, out, upgrade):
        out.write_text(out.read_text().replace("foo==1.0", "foo==2.0"))

    monkeypatch.setattr(lock, "_compile", bump)
    problems = lock.check("uv", reqs)
    assert len(problems) == 3 and "+foo==2.0" in problems[0]

    (reqs / lock.TARGETS[0].filename).unlink()
    assert "missing" in lock.check("uv", reqs)[0]


def test_desktop_env_host_lock_and_commands(monkeypatch, tmp_path):
    assert env.venv_python(Path("v")) in (
        Path("v") / "Scripts" / "python.exe",
        Path("v") / "bin" / "python",
    )
    assert env.host_lock_file("linux", "x86_64").name == "desktop-linux-x86_64.txt"
    assert env.host_lock_file("darwin", "arm64").name == "desktop-macos-arm64.txt"
    assert env.host_lock_file("win32", "AMD64").name == "desktop-windows-x86_64.txt"
    with pytest.raises(SystemExit, match="unsupported"):
        env.host_lock_file("darwin", "x86_64")

    uv_cmds = env.install_commands("uv", Path("py"), Path("lock.txt"))
    assert uv_cmds[0] == ["uv", "pip", "sync", "--python", "py", "--require-hashes", "lock.txt"]
    assert uv_cmds[1][-3:] == ["--no-deps", "-e", "."]
    pip_cmds = env.install_commands(None, Path("py"), Path("lock.txt"))
    assert pip_cmds[1][-3:] == ["--require-hashes", "-r", "lock.txt"]
    assert pip_cmds[2][-3:] == ["--no-deps", "-e", "."]

    monkeypatch.delenv(env.VENV_ENV_VAR, raising=False)
    assert env.default_venv() == env.VENV_DIR
    monkeypatch.setenv(env.VENV_ENV_VAR, str(tmp_path / "alt"))
    assert env.default_venv() == (tmp_path / "alt").resolve()
