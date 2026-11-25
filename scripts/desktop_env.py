"""Create (or refresh) ``.venv-desktop``: the build environment for the frozen sidecar.

ADR 0010 Phase 1b bundles CadQuery into the desktop sidecar, so the venv PyInstaller
runs from must contain ``cadquery==2.8.0`` (the ``cad`` extra) and PyInstaller (the
``desktop`` extra). The project ``.venv`` stays CadQuery-free for fast tests.

Usage:

    python scripts/desktop_env.py            # create if missing, then sync deps
    python scripts/desktop_env.py --recreate # wipe and rebuild

Uses ``uv`` when it is on PATH (``uv venv --python 3.12`` + ``uv pip install``), otherwise
falls back to ``python -m venv`` + pip. Running it twice is cheap: an existing venv is
reused and the editable install only re-resolves.
"""

from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
VENV_DIR = ROOT / ".venv-desktop"
PYTHON_VERSION = "3.12"
EXTRAS = ".[cad,desktop]"


def venv_python(venv: Path = VENV_DIR) -> Path:
    if sys.platform == "win32":
        return venv / "Scripts" / "python.exe"
    return venv / "bin" / "python"


def _run(cmd: list[str]) -> None:
    print("+ " + " ".join(cmd), flush=True)
    subprocess.run(cmd, cwd=ROOT, check=True)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Create the desktop sidecar build venv")
    parser.add_argument("--recreate", action="store_true", help="delete and rebuild the venv")
    parser.add_argument("--no-uv", action="store_true", help="use venv + pip even if uv exists")
    args = parser.parse_args(argv)

    if args.recreate and VENV_DIR.exists():
        shutil.rmtree(VENV_DIR)

    uv = None if args.no_uv else shutil.which("uv")
    python = venv_python()

    if not python.is_file():
        if uv:
            _run([uv, "venv", "--python", PYTHON_VERSION, str(VENV_DIR)])
        else:
            _run([sys.executable, "-m", "venv", str(VENV_DIR)])
    else:
        print(f"Reusing {VENV_DIR}")

    if uv:
        _run([uv, "pip", "install", "--python", str(python), "-e", EXTRAS])
    else:
        _run([str(python), "-m", "pip", "install", "--upgrade", "pip"])
        _run([str(python), "-m", "pip", "install", "-e", EXTRAS])

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
