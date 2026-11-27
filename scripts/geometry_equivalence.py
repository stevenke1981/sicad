"""Geometry-equivalence harness (ADR 0010 phase 2 completion condition).

For every 1.0 document of the corpus it compares

* A: the 1.0 ``CadQueryCompiler`` run directly on the 1.0 document, with
* B: ``Cad2Compiler`` run on the real core's ``validate`` result (the 1.0 document upgraded
  to 2.0 and solved by ``promptcad-core``),

by executing both generated ``model.py`` sources (``__name__ != "__main__"``) and comparing
volume (relative 1e-6), bounding box (absolute 1e-6), centre of mass (absolute 1e-6), the
symmetric-difference volume ``|A - B| + |B - A|`` (< 1e-3 mm^3), the solid count and the
number of warnings. A case where both sides fail is ``both_failed`` (fine); a one-sided
failure is a ``mismatch``. Documents the 1.0 Pydantic model or the core reject outright are
``skipped`` (no 1.0 reference exists to compare against).

Corpus: ``examples/generated/*/spec.json``, the 49 specs of
``crates/promptcad-core/tests/fixtures/v1_parity.json`` and ``frozen_smoke.smoke_cases()``
(deduplicated by content). Specs are compiled regardless of ``report.valid``: many parity
specs are deliberately invalid.

Native 2.0 checks (no 1.0 reference exists): ``plate-four-holes.cad2.json`` against the
1.0 compiler on the core's ``downgrade``; ``enclosure-with-lid.cad2.json`` against analytic
bounding box and volume; an ``l_bracket`` placed in all 24 axis-aligned poses against the
analytically rotated local bounding box and centre of mass (the latter also catches a
mirrored placement).

The core is located the normal way (``PROMPTCAD_CORE_BIN`` or the dev fallback); no token
is needed. Needs CadQuery (use the desktop venv); exits 1 on any mismatch or failed check,
2 when CadQuery is missing. The JSON summary goes to stdout, progress to stderr.

Usage::

    python scripts/geometry_equivalence.py [--case SUBSTRING] [--limit N]
"""

from __future__ import annotations

import argparse
import contextlib
import copy
import itertools
import json
import sys
import time
import traceback
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

VOLUME_REL_TOL = 1e-6
BBOX_ABS_TOL = 1e-6
COM_ABS_TOL = 1e-6
SYMDIFF_TOL = 1e-3

EXAMPLES_DIR = ROOT / "examples" / "generated"
CAD2_DIR = ROOT / "examples" / "cad2"
PARITY_FIXTURE = ROOT / "crates" / "promptcad-core" / "tests" / "fixtures" / "v1_parity.json"

# Explicit allow-list of corpus cases whose difference is a genuine, documented semantic
# difference between the 1.0 compiler and the 2.0 core (case name -> reason). Empty on purpose:
# an entry must justify itself; tolerances are never loosened.
ALLOWED_DIFFERENCES: dict[str, str] = {}

POSES = [
    (x_axis, z_axis)
    for z_axis in ("+z", "-z", "+x", "-x", "+y", "-y")
    for x_axis in ("+x", "-x", "+y", "-y", "+z", "-z")
    if x_axis[1] != z_axis[1]
]
_UNIT = {
    "+x": (1.0, 0.0, 0.0),
    "-x": (-1.0, 0.0, 0.0),
    "+y": (0.0, 1.0, 0.0),
    "-y": (0.0, -1.0, 0.0),
    "+z": (0.0, 0.0, 1.0),
    "-z": (0.0, 0.0, -1.0),
}


# --------------------------------------------------------------------------- pure helpers


def compare_metrics(
    a: dict[str, Any], b: dict[str, Any], symdiff: float | None = None
) -> list[str]:
    """Differences between two metric dicts (empty list = equivalent).

    A metric dict has ``volume``, ``bbox`` (xmin, ymin, zmin, xmax, ymax, zmax), ``com``
    (x, y, z), ``solids`` and ``warnings`` (a count). ``symdiff`` is the symmetric-difference
    volume of the two shapes when it was computed.
    """
    diffs: list[str] = []
    va, vb = float(a["volume"]), float(b["volume"])
    if abs(va - vb) > VOLUME_REL_TOL * max(abs(va), abs(vb), 1e-12):
        diffs.append(f"volume {va!r} vs {vb!r}")
    for name, tol in (("bbox", BBOX_ABS_TOL), ("com", COM_ABS_TOL)):
        for index, (x, y) in enumerate(zip(a[name], b[name], strict=True)):
            if abs(float(x) - float(y)) > tol:
                diffs.append(f"{name}[{index}] {x!r} vs {y!r}")
    if a.get("solids") != b.get("solids"):
        diffs.append(f"solid count {a.get('solids')} vs {b.get('solids')}")
    if a.get("warnings") != b.get("warnings"):
        diffs.append(f"warning count {a.get('warnings')} vs {b.get('warnings')}")
    if symdiff is not None and symdiff >= SYMDIFF_TOL:
        diffs.append(f"symmetric difference {symdiff!r} mm^3")
    return diffs


def classify(
    a: dict[str, Any], b: dict[str, Any], symdiff: float | None = None
) -> tuple[str, list[str]]:
    """``(status, details)`` for two outcomes ``{"ok": bool, "error": str} | {"ok": True, **metrics}``."""
    if not a["ok"] and not b["ok"]:
        return "both_failed", [f"A: {a['error']}", f"B: {b['error']}"]
    if not a["ok"]:
        return "mismatch", [f"only A failed: {a['error']}"]
    if not b["ok"]:
        return "mismatch", [f"only B failed: {b['error']}"]
    diffs = compare_metrics(a, b, symdiff)
    return ("mismatch", diffs) if diffs else ("equal", [])


def rotate(
    x_axis: str, z_axis: str, local: tuple[float, float, float]
) -> tuple[float, float, float]:
    """World direction of ``local`` under the pose (columns x_axis, y = z x x, z_axis)."""
    xa, za = _UNIT[x_axis], _UNIT[z_axis]
    ya = (
        za[1] * xa[2] - za[2] * xa[1],
        za[2] * xa[0] - za[0] * xa[2],
        za[0] * xa[1] - za[1] * xa[0],
    )
    return tuple(xa[i] * local[0] + ya[i] * local[1] + za[i] * local[2] for i in range(3))  # type: ignore[return-value]


def rotated_bbox(
    x_axis: str,
    z_axis: str,
    origin: tuple[float, float, float],
    local_min: tuple[float, float, float],
    local_max: tuple[float, float, float],
) -> list[float]:
    """Analytic world bbox (xmin, ymin, zmin, xmax, ymax, zmax) of a local box under a pose."""
    corners = [
        rotate(x_axis, z_axis, (cx, cy, cz))
        for cx, cy, cz in itertools.product(*zip(local_min, local_max, strict=True))
    ]
    mins = [min(c[i] for c in corners) + origin[i] for i in range(3)]
    maxs = [max(c[i] for c in corners) + origin[i] for i in range(3)]
    return mins + maxs


# --------------------------------------------------------------------------- corpus


def corpus() -> tuple[list[tuple[str, dict[str, Any]]], int]:
    """All 1.0 cases as ``(name, spec)``, deduplicated by content; and the duplicate count."""
    cases: list[tuple[str, dict[str, Any]]] = []
    for spec_path in sorted(EXAMPLES_DIR.glob("*/spec.json")):
        cases.append(
            (f"example:{spec_path.parent.name}", json.loads(spec_path.read_text(encoding="utf-8")))
        )
    if PARITY_FIXTURE.is_file():
        for entry in json.loads(PARITY_FIXTURE.read_text(encoding="utf-8")):
            cases.append((f"parity:{entry['name']}", entry["spec"]))
    from scripts.frozen_smoke import smoke_cases

    for name, spec in smoke_cases():
        cases.append((f"smoke:{name}", spec))
    seen: set[str] = set()
    unique: list[tuple[str, dict[str, Any]]] = []
    for name, spec in cases:
        key = json.dumps(spec, sort_keys=True, ensure_ascii=False)
        if key in seen:
            continue
        seen.add(key)
        unique.append((name, spec))
    return unique, len(cases) - len(unique)


# --------------------------------------------------------------------------- execution


def _short_error() -> str:
    return "".join(traceback.format_exception_only(*sys.exc_info()[:2])).strip()[:400]


def run_source(source: str) -> tuple[Any, list[str]]:
    """Execute generated ``model.py`` text as a module and call ``build()``."""
    namespace: dict[str, Any] = {"__name__": "generated_model"}
    exec(compile(source, "<model.py>", "exec"), namespace)  # noqa: S102
    result, warnings = namespace["build"]()
    return result, list(warnings)


def metrics_of(result: Any, warnings: list[str]) -> dict[str, Any]:
    shape = result.val()
    box = shape.BoundingBox()
    com = shape.Center()
    return {
        "ok": True,
        "volume": shape.Volume(),
        "bbox": [box.xmin, box.ymin, box.zmin, box.xmax, box.ymax, box.zmax],
        "com": [com.x, com.y, com.z],
        "solids": len(result.solids().vals()),
        "warnings": len(warnings),
        "shape_valid": bool(shape.isValid()),
    }


def build_outcome(source_factory: Any) -> tuple[dict[str, Any], Any]:
    """Compile+run; ``(outcome, result Workplane | None)``; never raises."""
    try:
        result, warnings = run_source(source_factory())
        return metrics_of(result, warnings), result
    except Exception:
        return {"ok": False, "error": _short_error()}, None


def symmetric_difference(a: Any, b: Any) -> float:
    def vol(workplane: Any) -> float:
        vals = workplane.vals()
        return sum(v.Volume() for v in vals) if vals else 0.0

    return vol(a.cut(b)) + vol(b.cut(a))


class Core:
    """Synchronous access to ``promptcad-core rpc`` through the normal bridge lookup."""

    def __init__(self) -> None:
        from app.core.config import Settings
        from app.services.core_bridge import CoreBridge

        self.bridge = CoreBridge(Settings())

    def call(self, request: dict[str, Any]) -> dict[str, Any]:
        raw = self.bridge.call_bytes(json.dumps(request, ensure_ascii=False).encode("utf-8"))
        return json.loads(raw)

    def validate(self, document: dict[str, Any]) -> dict[str, Any]:
        response = self.call({"op": "validate", "document": document})
        if response.get("status") != "ok":
            raise RuntimeError(f"core rejected the document: {response}")
        return response["result"]

    def shutdown(self) -> None:
        self.bridge.shutdown()


def compile_v2(core: Core, document: dict[str, Any]) -> str:
    from app.services.compiler2 import Cad2Compiler

    result = core.validate(document)
    return Cad2Compiler().compile(result["document"], result["resolved"])


# --------------------------------------------------------------------------- corpus cases


def run_corpus_case(core: Core, name: str, spec: dict[str, Any]) -> dict[str, Any]:
    from app.models.cad import CadDocument
    from app.services.compiler import CadQueryCompiler

    entry: dict[str, Any] = {"name": name}
    try:
        doc1 = CadDocument.model_validate(spec)
    except Exception:
        return {**entry, "status": "skipped", "details": ["1.0 pydantic model rejects the spec"]}
    try:
        core_result = core.validate(spec)
    except Exception:
        return {**entry, "status": "skipped", "details": [f"core: {_short_error()}"]}
    entry["valid"] = bool(core_result["report"]["valid"])

    a, a_result = build_outcome(lambda: CadQueryCompiler().compile(doc1))

    def source_b() -> str:
        from app.services.compiler2 import Cad2Compiler

        return Cad2Compiler().compile(core_result["document"], core_result["resolved"])

    b, b_result = build_outcome(source_b)
    symdiff = None
    if a_result is not None and b_result is not None:
        if not (a["shape_valid"] and b["shape_valid"]):
            # Booleans on an invalid solid (e.g. an over-large fillet) are undefined; the other
            # metrics are still compared.
            entry["symdiff_skipped"] = "invalid shape"
        else:
            try:
                symdiff = symmetric_difference(a_result, b_result)
            except Exception:
                symdiff = None
                b = {**b, "ok": False, "error": f"symmetric difference failed: {_short_error()}"}
    status, details = classify(a, b, symdiff)
    entry.update(status=status, details=details)
    if symdiff is not None:
        entry["symdiff"] = symdiff
    if a["ok"] and b["ok"]:
        entry["volume_delta"] = abs(a["volume"] - b["volume"])
        entry["volume_a"] = a["volume"]
    if status == "mismatch" and name in ALLOWED_DIFFERENCES:
        entry["status"] = "allowed_difference"
        entry["allowed_reason"] = ALLOWED_DIFFERENCES[name]
    return entry


# --------------------------------------------------------------------------- 2.0-native checks


def _load_cad2(filename: str) -> dict[str, Any]:
    document = json.loads((CAD2_DIR / filename).read_text(encoding="utf-8"))
    document.pop("expected_resolved", None)
    return document


def check_plate_vs_downgrade(core: Core) -> dict[str, Any]:
    from app.models.cad import CadDocument
    from app.services.compiler import CadQueryCompiler

    check = {"name": "plate-four-holes vs 1.0 compiler on downgrade"}
    document = _load_cad2("plate-four-holes.cad2.json")
    down = core.call({"op": "downgrade", "document": document})["result"]
    if not down.get("expressible"):
        return {**check, "status": "mismatch", "details": ["downgrade is not expressible"]}
    doc1 = CadDocument.model_validate(down["document"])
    a, a_result = build_outcome(lambda: CadQueryCompiler().compile(doc1))
    b, b_result = build_outcome(lambda: compile_v2(core, document))
    symdiff = (
        symmetric_difference(a_result, b_result)
        if a_result is not None and b_result is not None
        else None
    )
    status, details = classify(a, b, symdiff)
    if status == "both_failed":
        status = "mismatch"
    return {**check, "status": status, "details": details, "symdiff": symdiff}


def check_enclosure_with_lid(core: Core) -> dict[str, Any]:
    check = {"name": "enclosure-with-lid analytic bbox and volume"}
    document = _load_cad2("enclosure-with-lid.cad2.json")
    outcome, _ = build_outcome(lambda: compile_v2(core, document))
    if not outcome["ok"]:
        return {**check, "status": "mismatch", "details": [outcome["error"]]}
    expected = {
        "volume": 100 * 70 * 33 - 96 * 66 * 31 + 100 * 70 * 2,
        "bbox": [-50.0, -35.0, 0.0, 50.0, 35.0, 35.0],
        "com": outcome["com"],
        "solids": 2,
        "warnings": 0,
    }
    diffs = compare_metrics(outcome, expected)
    return {**check, "status": "mismatch" if diffs else "equal", "details": diffs}


def l_bracket_document(x_axis: str, z_axis: str, origin: list[float]) -> dict[str, Any]:
    return {
        "schema_version": "2.0",
        "name": "pose-check",
        "source_prompt": "l_bracket in one of 24 poses",
        "unit": "mm",
        "revision": 1,
        "parameters": [],
        "bindings": [],
        "components": [
            {
                "id": "v_br",
                "kind": "volume",
                "shape": {
                    "kind": "l_bracket",
                    "width": 40,
                    "depth": 30,
                    "vertical_height": 25,
                    "thickness": 5,
                },
                "label": None,
                "provenance": None,
            }
        ],
        "constraints": [
            {
                "id": "k_root",
                "kind": "fixed",
                "a": "v_br.origin",
                "b": None,
                "value": None,
                "u": None,
                "v": None,
                "frame": {"origin": origin, "x_axis": x_axis, "z_axis": z_axis},
            }
        ],
        "parts": [
            {
                "id": "p_br",
                "name": "bracket",
                "material": None,
                "quantity": 1,
                "source": "made",
                "body": [{"op": "add", "component": "v_br"}],
                "fillets": [],
                "chamfers": [],
            }
        ],
        "assemblies": [],
        "root": "p_br",
        "assumptions": [],
        "notes": [],
        "planner": {"planner": "pose-check", "confidence": 1.0, "review_required": False},
    }


def check_l_bracket_poses(core: Core) -> dict[str, Any]:
    check = {"name": "l_bracket in 24 poses vs analytic rotation"}
    origin = [10.0, -20.0, 7.0]
    local_min, local_max = (-20.0, -15.0, 0.0), (20.0, 15.0, 30.0)
    base, _ = build_outcome(lambda: compile_v2(core, l_bracket_document("+x", "+z", [0, 0, 0])))
    if not base["ok"]:
        return {**check, "status": "mismatch", "details": [base["error"]]}
    local_com = tuple(base["com"])
    failures: list[str] = []
    for x_axis, z_axis in POSES:
        document = l_bracket_document(x_axis, z_axis, origin)
        outcome, _ = build_outcome(lambda doc=document: compile_v2(core, doc))
        if not outcome["ok"]:
            failures.append(f"{x_axis}/{z_axis}: {outcome['error']}")
            continue
        rotated = rotate(x_axis, z_axis, local_com)
        expected = {
            "volume": base["volume"],
            "bbox": rotated_bbox(x_axis, z_axis, tuple(origin), local_min, local_max),
            "com": [rotated[i] + origin[i] for i in range(3)],
            "solids": 1,
            "warnings": 0,
        }
        failures.extend(f"{x_axis}/{z_axis}: {d}" for d in compare_metrics(outcome, expected))
    return {
        **check,
        "status": "mismatch" if failures else "equal",
        "poses": len(POSES),
        "details": failures,
    }


# --------------------------------------------------------------------------- main


def summarize(
    cases: list[dict[str, Any]], natives: list[dict[str, Any]], **extra: Any
) -> dict[str, Any]:
    counts = {"equal": 0, "both_failed": 0, "mismatch": 0, "skipped": 0, "allowed_difference": 0}
    for case in cases:
        counts[case["status"]] += 1
    deltas = [c["volume_delta"] for c in cases if "volume_delta" in c]
    rel = [c["volume_delta"] / max(abs(c["volume_a"]), 1e-12) for c in cases if "volume_delta" in c]
    symdiffs = [c["symdiff"] for c in cases if c.get("symdiff") is not None]
    return {
        "cases": len(cases),
        "counts": counts,
        "max_volume_delta": max(deltas, default=0.0),
        "max_volume_relative_delta": max(rel, default=0.0),
        "max_symmetric_difference": max(symdiffs, default=0.0),
        "symdiff_skipped_invalid_shape": [c["name"] for c in cases if "symdiff_skipped" in c],
        "native": natives,
        **extra,
        "problems": [c for c in cases if c["status"] == "mismatch"],
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--case", help="only corpus cases whose name contains this text")
    parser.add_argument("--limit", type=int, help="at most this many corpus cases")
    parser.add_argument("--skip-native", action="store_true", help="skip the 2.0-native checks")
    args = parser.parse_args(argv)

    try:
        import cadquery  # noqa: F401
    except ImportError:
        print(
            "geometry_equivalence: CadQuery is not importable in this interpreter", file=sys.stderr
        )
        return 2

    started = time.perf_counter()
    cases_in, duplicates = corpus()
    if args.case:
        cases_in = [c for c in cases_in if args.case in c[0]]
    if args.limit is not None:
        cases_in = cases_in[: args.limit]

    core = Core()
    try:
        results = []
        for name, spec in cases_in:
            entry = run_corpus_case(core, name, copy.deepcopy(spec))
            print(f"[{entry['status']}] {name}", file=sys.stderr)
            results.append(entry)
        natives: list[dict[str, Any]] = []
        if not args.skip_native and not args.case:
            for check in (
                check_plate_vs_downgrade,
                check_enclosure_with_lid,
                check_l_bracket_poses,
            ):
                try:
                    entry = check(core)
                except Exception:
                    entry = {
                        "name": check.__name__,
                        "status": "mismatch",
                        "details": [_short_error()],
                    }
                print(f"[native {entry['status']}] {entry['name']}", file=sys.stderr)
                natives.append(entry)
    finally:
        with contextlib.suppress(Exception):
            core.shutdown()

    summary = summarize(
        results,
        natives,
        duplicates_skipped=duplicates,
        runtime_seconds=round(time.perf_counter() - started, 1),
    )
    print(json.dumps(summary, indent=2, ensure_ascii=False))
    failed = summary["counts"]["mismatch"] or any(n["status"] != "equal" for n in natives)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
