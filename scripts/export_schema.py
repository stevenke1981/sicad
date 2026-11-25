"""Export the Pydantic models as one JSON Schema document (ADR 0008).

Usage:
    python scripts/export_schema.py            # write schemas/promptcad.schema.json
    python scripts/export_schema.py --check    # exit 1 if the committed file has drifted

The output is deterministic (sorted keys, LF line endings, trailing newline) so the
TypeScript types generated from it can be drift-checked in CI.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from pydantic import BaseModel  # noqa: E402
from pydantic.json_schema import models_json_schema  # noqa: E402

from app import __version__  # noqa: E402
from app.models.api import (  # noqa: E402
    Artifact,
    CapabilityResponse,
    GenerateFromSpecRequest,
    GenerateRequest,
    JobListItem,
    JobManifest,
    PlanRequest,
    PlanResponse,
)
from app.models.cad import CadDocument, ValidationReport  # noqa: E402

DEFAULT_OUTPUT = ROOT / "schemas" / "promptcad.schema.json"

MODELS: tuple[type[BaseModel], ...] = (
    CadDocument,
    ValidationReport,
    PlanRequest,
    GenerateRequest,
    GenerateFromSpecRequest,
    PlanResponse,
    Artifact,
    JobManifest,
    JobListItem,
    CapabilityResponse,
)


def build_schema() -> dict:
    # mode="validation" describes request bodies; responses share the same shapes here.
    _, top = models_json_schema(
        [(model, "validation") for model in MODELS],
        ref_template="#/$defs/{model}",
        title="PromptCAD Studio",
    )
    defs = top.get("$defs", {})
    missing = [model.__name__ for model in MODELS if model.__name__ not in defs]
    if missing:  # pragma: no cover - defensive, guards against pydantic renaming
        raise RuntimeError(f"Models missing from $defs: {missing}")
    return {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "PromptCAD Studio",
        "description": (
            "Generated from app/models by scripts/export_schema.py. Do not edit by hand."
        ),
        "x-promptcad-version": __version__,
        "$defs": defs,
    }


def render_schema() -> str:
    return json.dumps(build_schema(), indent=2, sort_keys=True, ensure_ascii=False) + "\n"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--check", action="store_true", help="fail if the file is out of date")
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    args = parser.parse_args(argv)

    content = render_schema()
    output: Path = args.output

    if args.check:
        current = output.read_bytes().decode("utf-8") if output.exists() else None
        if current != content:
            print(
                f"{output} is out of date; run `python scripts/export_schema.py`.",
                file=sys.stderr,
            )
            return 1
        print(f"{output} is up to date.")
        return 0

    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_bytes(content.encode("utf-8"))
    print(f"Wrote {output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
