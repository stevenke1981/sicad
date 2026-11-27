from __future__ import annotations

import sys
from pathlib import Path

if __package__ in {None, ""}:
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import importlib.util
import shutil

from app.core.config import get_settings
from app.services.core_bridge import locate_core_binary


def main() -> int:
    settings = get_settings()
    cadquery = importlib.util.find_spec("cadquery") is not None
    openscad = shutil.which("openscad")
    print("PromptCAD Studio doctor")
    print(f"Python: {sys.version.split()[0]}")
    print(f"Data dir: {settings.data_dir.resolve()}")
    print(f"Planner mode: {settings.planner_mode}")
    print(f"LLM configured: {settings.llm_is_configured}")
    print(f"CadQuery: {'available' if cadquery else 'not installed'}")
    print(f"OpenSCAD: {openscad or 'not installed'}")
    core_path, core_reason = locate_core_binary(settings)
    print(f"promptcad-core: {core_path or f'not found ({core_reason})'}")
    if core_path is None:
        print(
            "Result: promptcad-core is REQUIRED for validation and generation but missing. "
            "Run `make core` (cargo build --release --locked --bins in crates/promptcad-core) "
            "or set PROMPTCAD_CORE_BIN."
        )
        return 1
    if not cadquery and not openscad:
        print("Result: source-only mode. Use Docker/conda for STEP/STL/DXF.")
        return 1
    print("Result: CAD renderer available.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
