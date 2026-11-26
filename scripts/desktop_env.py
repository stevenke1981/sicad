"""Create (or refresh) ``.venv-desktop``: the build environment for the frozen sidecar.

ADR 0010 Phase 1b bundles CadQuery into the desktop sidecar, so the venv PyInstaller
runs from must contain ``cadquery==2.8.0`` (the ``cad`` extra) and PyInstaller (the
``desktop`` extra). The project ``.venv`` stays CadQuery-free for fast tests.

The venv is installed **from the hashed lock file of the host platform** (ADR 0011,
``scripts/desktop_lock.py``), so every machine and CI runner freezes the same versions::

    requirements/desktop-windows-x86_64.txt | desktop-macos-arm64.txt | desktop-linux-x86_64.txt

Usage:

    python scripts/desktop_env.py              # create if missing, then sync to the lock
    python scripts/desktop_env.py --recreate   # wipe and rebuild
    python scripts/desktop_env.py --venv DIR   # another location (or PROMPTCAD_DESKTOP_VENV)
    python scripts/desktop_env.py --python 3.12.10  # exact interpreter (CI: notices record it)

With ``uv`` on PATH: ``uv venv --python 3.12`` + ``uv pip sync --require-hashes <lock>`` (removes
anything not in the lock) + ``uv pip install --no-deps -e .``. Otherwise ``python -m venv`` +
``pip install --require-hashes -r <lock>`` + ``pip install --no-deps -e .`` (pip does not
remove extras; use ``--recreate``). Hosts without a lock file (e.g. Intel macOS, Linux arm64)
are rejected.
"""

from __future__ import annotations

import argparse
import os
import platform
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
VENV_DIR = ROOT / ".venv-desktop"
VENV_ENV_VAR = "PROMPTCAD_DESKTOP_VENV"
PYTHON_VERSION = "3.12"

sys.path.insert(0, str(Path(__file__).resolve().parent))
import desktop_lock  # noqa: E402  (sibling script, not a package import)


def venv_python(venv: Path = VENV_DIR) -> Path:
    if sys.platform == "win32":
        return venv / "Scripts" / "python.exe"
    return venv / "bin" / "python"


def default_venv() -> Path:
    override = os.environ.get(VENV_ENV_VAR)
    return Path(override).resolve() if override else VENV_DIR


def host_lock_file(
    sys_platform: str | None = None,
    machine: str | None = None,
    requirements_dir: Path = desktop_lock.REQUIREMENTS_DIR,
) -> Path:
    """The lock file for this host; SystemExit when the platform has none."""
    sys_platform = sys.platform if sys_platform is None else sys_platform
    machine = platform.machine() if machine is None else machine
    target = desktop_lock.target_for(sys_platform, machine)
    if target is None:
        supported = ", ".join(t.key for t in desktop_lock.TARGETS)
        raise SystemExit(
            f"unsupported desktop build platform {sys_platform}/{machine} (supported: {supported})"
        )
    return desktop_lock.lock_path(target, requirements_dir)


def install_commands(uv: str | None, python: Path, lock: Path) -> list[list[str]]:
    """Commands that make the venv at ``python`` match ``lock`` exactly, then add the project."""
    if uv:
        return [
            [uv, "pip", "sync", "--python", str(python), "--require-hashes", str(lock)],
            [uv, "pip", "install", "--python", str(python), "--no-deps", "-e", "."],
        ]
    return [
        [str(python), "-m", "pip", "install", "--upgrade", "pip"],
        [str(python), "-m", "pip", "install", "--require-hashes", "-r", str(lock)],
        [str(python), "-m", "pip", "install", "--no-deps", "-e", "."],
    ]


def _run(cmd: list[str]) -> None:
    print("+ " + " ".join(cmd), flush=True)
    subprocess.run(cmd, cwd=ROOT, check=True)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Create the desktop sidecar build venv")
    parser.add_argument("--recreate", action="store_true", help="delete and rebuild the venv")
    parser.add_argument("--no-uv", action="store_true", help="use venv + pip even if uv exists")
    parser.add_argument(
        "--venv",
        type=Path,
        default=None,
        help=f"venv directory (default: ${VENV_ENV_VAR} or .venv-desktop)",
    )
    parser.add_argument(
        "--python",
        default=PYTHON_VERSION,
        help="interpreter request for `uv venv` (default %(default)s; CI pins the patch "
        "release, which THIRD_PARTY_NOTICES.md records). Ignored with --no-uv.",
    )
    args = parser.parse_args(argv)

    venv = args.venv.resolve() if args.venv else default_venv()
    lock = host_lock_file()
    if not lock.is_file():
        print(f"missing {lock}; run python scripts/desktop_lock.py", file=sys.stderr)
        return 1

    if args.recreate and venv.exists():
        shutil.rmtree(venv)

    uv = None if args.no_uv else shutil.which("uv")
    python = venv_python(venv)

    if not python.is_file():
        if uv:
            _run([uv, "venv", "--python", args.python, str(venv)])
        else:
            _run([sys.executable, "-m", "venv", str(venv)])
    else:
        print(f"Reusing {venv}")

    print(f"Lock file: {lock.relative_to(ROOT)}")
    for cmd in install_commands(uv, python, lock):
        _run(cmd)

    check = subprocess.run(
        [
            str(python),
            "-c",
            "import cadquery, PyInstaller; print(cadquery.__version__, PyInstaller.__version__)",
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    if check.returncode != 0:
        print(f"cadquery/PyInstaller import check failed:\n{check.stderr}", file=sys.stderr)
        return 1
    cq_version, pyi_version = check.stdout.split()
    print(f"OK {python}: cadquery {cq_version}, PyInstaller {pyi_version}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
