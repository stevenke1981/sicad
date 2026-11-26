"""Hashed, per-platform lock files for the desktop sidecar build env (ADR 0011).

``.venv-desktop`` (see ``scripts/desktop_env.py``) is what PyInstaller freezes, so its exact
contents decide what ships in the bundle and what ``THIRD_PARTY_NOTICES.md`` must list. This
script pins it with ``uv pip compile`` (``pyproject.toml`` extras ``cad`` + ``desktop``,
CPython 3.12) once per bundle platform, all generated from one machine::

    requirements/desktop-windows-x86_64.txt   x86_64-pc-windows-msvc
    requirements/desktop-macos-arm64.txt      aarch64-apple-darwin
    requirements/desktop-linux-x86_64.txt     x86_64-manylinux_2_31  (cadquery-ocp floor)

Usage::

    python scripts/desktop_lock.py            # (re)compile, existing pins are preferences
    python scripts/desktop_lock.py --check    # recompile to a temp dir, fail on any diff
    python scripts/desktop_lock.py --upgrade  # refresh every pin on all three platforms

Only ``uv`` is needed (no venv, no CadQuery): resolution is cross-platform. Without
``--upgrade`` uv keeps every pin already in the output file, so ``--check`` only fails when
``pyproject.toml`` changed in a way the committed pins no longer satisfy (or a file was edited
by hand). The header names this script instead of the raw uv command so the files do not
depend on the output path.
"""

from __future__ import annotations

import argparse
import difflib
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
REQUIREMENTS_DIR = ROOT / "requirements"
PYTHON_VERSION = "3.12"
EXTRAS = ("cad", "desktop")
CUSTOM_COMPILE_COMMAND = "python scripts/desktop_lock.py"


@dataclass(frozen=True)
class LockTarget:
    key: str  # file suffix: requirements/desktop-<key>.txt
    python_platform: str  # uv --python-platform
    sys_platform: str  # sys.platform of a host that installs it
    machines: tuple[str, ...]  # platform.machine() values (lower case) of such a host

    @property
    def filename(self) -> str:
        return f"desktop-{self.key}.txt"


TARGETS: tuple[LockTarget, ...] = (
    LockTarget("windows-x86_64", "x86_64-pc-windows-msvc", "win32", ("amd64", "x86_64")),
    LockTarget("macos-arm64", "aarch64-apple-darwin", "darwin", ("arm64", "aarch64")),
    # cadquery-ocp ships manylinux_2_31 wheels: that is the glibc floor (ubuntu-22.04 runner).
    LockTarget("linux-x86_64", "x86_64-manylinux_2_31", "linux", ("x86_64", "amd64")),
)


def target_for(sys_platform: str, machine: str) -> LockTarget | None:
    """The lock target a host installs from, or None when the platform is not supported."""
    machine = machine.lower()
    for target in TARGETS:
        if sys_platform.startswith(target.sys_platform) and machine in target.machines:
            return target
    return None


def lock_path(target: LockTarget, requirements_dir: Path = REQUIREMENTS_DIR) -> Path:
    return requirements_dir / target.filename


def compile_command(uv: str, target: LockTarget, out: Path, *, upgrade: bool = False) -> list[str]:
    cmd = [uv, "pip", "compile", "pyproject.toml"]
    for extra in EXTRAS:
        cmd += ["--extra", extra]
    cmd += [
        "--python-version",
        PYTHON_VERSION,
        "--python-platform",
        target.python_platform,
        "--generate-hashes",
        "--custom-compile-command",
        CUSTOM_COMPILE_COMMAND,
        "--quiet",
        "-o",
        str(out),
    ]
    if upgrade:
        cmd.append("--upgrade")
    return cmd


def pins(text: str) -> dict[str, str]:
    """``name -> version`` of every ``name==version`` requirement line in a lock file."""
    out = {}
    for line in text.splitlines():
        if not line or line[0] in "# -":
            continue
        req = line.split(";", 1)[0].split("\\", 1)[0].strip()
        if "==" in req:
            name, version = req.split("==", 1)
            out[name.strip().lower()] = version.strip()
    return out


def _normalized(text: str) -> str:
    return text.replace("\r\n", "\n")


def _compile(uv: str, target: LockTarget, out: Path, *, upgrade: bool) -> None:
    cmd = compile_command(uv, target, out, upgrade=upgrade)
    print("+ " + " ".join(cmd), flush=True)
    subprocess.run(cmd, cwd=ROOT, check=True)


def check(uv: str, requirements_dir: Path = REQUIREMENTS_DIR) -> list[str]:
    """Recompile every target into a temp dir (seeded with the committed files) and diff."""
    problems = []
    with tempfile.TemporaryDirectory(prefix="desktop-lock-") as tmp:
        for target in TARGETS:
            committed = lock_path(target, requirements_dir)
            if not committed.is_file():
                problems.append(f"{committed} is missing; run {CUSTOM_COMPILE_COMMAND}")
                continue
            fresh = Path(tmp) / target.filename
            shutil.copyfile(committed, fresh)  # existing pins act as preferences
            _compile(uv, target, fresh, upgrade=False)
            old = _normalized(committed.read_text(encoding="utf-8"))
            new = _normalized(fresh.read_text(encoding="utf-8"))
            if old != new:
                diff = difflib.unified_diff(
                    old.splitlines(),
                    new.splitlines(),
                    f"committed/{target.filename}",
                    f"recompiled/{target.filename}",
                    lineterm="",
                    n=1,
                )
                problems.append(f"{committed} is out of date:\n" + "\n".join(list(diff)[:80]))
    return problems


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Compile the desktop build-env lock files")
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--check", action="store_true", help="fail if a recompile changes a file")
    mode.add_argument("--upgrade", action="store_true", help="refresh all pins (all platforms)")
    args = parser.parse_args(argv)

    uv = shutil.which("uv")
    if not uv:
        print("uv is required (https://docs.astral.sh/uv/)", file=sys.stderr)
        return 2

    if args.check:
        problems = check(uv)
        for problem in problems:
            print(problem, file=sys.stderr)
        if problems:
            print(f"lock drift: rerun `{CUSTOM_COMPILE_COMMAND}` and commit", file=sys.stderr)
            return 1
        print(f"{len(TARGETS)} desktop lock files are up to date")
        return 0

    REQUIREMENTS_DIR.mkdir(exist_ok=True)
    for target in TARGETS:
        out = lock_path(target)
        _compile(uv, target, out, upgrade=args.upgrade)
        print(f"{out.relative_to(ROOT)}: {len(pins(out.read_text(encoding='utf-8')))} pins")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
