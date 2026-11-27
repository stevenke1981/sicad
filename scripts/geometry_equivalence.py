"""Geometry-equivalence harness (ADR 0010 phase 2 completion condition).

Golden mode (the only mode): ``Cad2Compiler`` runs on the real core's ``validate`` result for
every corpus case (the 1.0 documents upgraded to 2.0 and solved by ``promptcad-core``) and the
generated ``model.py`` is executed (``__name__ != "__main__"``). Its metrics are compared with
the frozen oracle ``tests/fixtures/v1_geometry_golden.json``: volume (relative 1e-6), surface
area (relative 1e-6), bounding box (absolute 1e-6), centre of mass (absolute 1e-6), solid
count, warning count and ``shape_valid``. Where the golden records ``shape_valid: false`` only
``shape_valid``, solids, warnings and the bounding box are compared. Cases are keyed by name
plus ``spec_sha256``; a missing, extra or hash-changed case is a mismatch.

``--write-golden PATH`` re-baselines the oracle from the current Cad2Compiler output: per case
it writes the metrics, ``spec_sha256`` and the ``model_sha256`` of the generated source, plus
the ``native`` entry and a ``meta`` block. It prints a reminder to stderr and refuses to write
unless every native check passes and every corpus case compiles. Differences from the
checked-in golden are reported as ``rebaselined``. Justify any re-baseline in the PR.

Corpus: ``examples/generated/*/spec.json``, the 49 specs of
``crates/promptcad-core/tests/fixtures/v1_parity.json`` and ``frozen_smoke.smoke_cases()``
(deduplicated by content). Specs are compiled regardless of ``report.valid``: many parity
specs are deliberately invalid.

Native 2.0 checks: ``plate-four-holes.cad2.json`` against its core ``downgrade`` ->
``validate`` -> ``Cad2Compiler`` round trip and the frozen ``native`` golden entry;
``enclosure-with-lid.cad2.json`` against analytic bounding box and volume; an ``l_bracket``
placed in all 24 axis-aligned poses against the analytically rotated local bounding box and
centre of mass (the latter also catches a mirrored placement).

The core is located the normal way (``PROMPTCAD_CORE_BIN`` or the dev fallback); no token
is needed. Exits 1 on any mismatch or failed check, 2 when CadQuery is missing. The JSON
summary goes to stdout, progress to stderr.

Usage::

    python scripts/geometry_equivalence.py [--case SUBSTRING] [--limit N]
    python scripts/geometry_equivalence.py --write-golden PATH
"""

from __future__ import annotations

import argparse
import contextlib
import copy
import hashlib
import itertools
import json
import platform
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
AREA_REL_TOL = 1e-6

EXAMPLES_DIR = ROOT / "examples" / "generated"
CAD2_DIR = ROOT / "examples" / "cad2"
GOLDEN_PATH = ROOT / "tests" / "fixtures" / "v1_geometry_golden.json"
GOLDEN_VERSION = 1
NATIVE_PLATE = "native:plate-four-holes.cad2"
METRIC_KEYS = ("volume", "area", "bbox", "com", "solids", "warnings", "shape_valid")
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
    a: dict[str, Any],
    b: dict[str, Any],
    symdiff: float | None = None,
    *,
    reduced: bool = False,
) -> list[str]:
    """Differences between two metric dicts (empty list = equivalent).

    A metric dict has ``volume``, ``bbox`` (xmin, ymin, zmin, xmax, ymax, zmax), ``com``
    (x, y, z), optionally ``area``, ``solids`` and ``warnings`` (a count). ``symdiff`` is the
    symmetric-difference volume of the two shapes when it was computed. ``reduced`` compares
    only bbox, solids and warnings (shapes whose solid is invalid). Details carry the deltas.
    """
    diffs: list[str] = []
    if not reduced:
        va, vb = float(a["volume"]), float(b["volume"])
        if abs(va - vb) > VOLUME_REL_TOL * max(abs(va), abs(vb), 1e-12):
            diffs.append(f"volume {va!r} vs {vb!r} (delta {vb - va:+.3e})")
        if a.get("area") is not None and b.get("area") is not None:
            aa, ab = float(a["area"]), float(b["area"])
            if abs(aa - ab) > AREA_REL_TOL * max(abs(aa), abs(ab), 1e-12):
                diffs.append(f"area {aa!r} vs {ab!r} (delta {ab - aa:+.3e})")
    for name, tol in (("bbox", BBOX_ABS_TOL), ("com", COM_ABS_TOL)):
        if reduced and name == "com":
            continue
        for index, (x, y) in enumerate(zip(a[name], b[name], strict=True)):
            if abs(float(x) - float(y)) > tol:
                diffs.append(f"{name}[{index}] {x!r} vs {y!r} (delta {float(y) - float(x):+.3e})")
    if a.get("solids") != b.get("solids"):
        diffs.append(f"solid count {a.get('solids')} vs {b.get('solids')}")
    if a.get("warnings") != b.get("warnings"):
        diffs.append(f"warning count {a.get('warnings')} vs {b.get('warnings')}")
    if symdiff is not None and symdiff >= SYMDIFF_TOL:
        diffs.append(f"symmetric difference {symdiff!r} mm^3")
    return diffs


def compare_golden(golden: dict[str, Any], b: dict[str, Any]) -> list[str]:
    """Differences between a frozen golden entry and a fresh B outcome (empty = equal)."""
    if not b["ok"]:
        return [f"B failed: {b['error']}"]
    diffs: list[str] = []
    if golden.get("shape_valid") != b.get("shape_valid"):
        diffs.append(f"shape_valid {golden.get('shape_valid')} vs {b.get('shape_valid')}")
    reduced = golden.get("shape_valid") is False
    return diffs + compare_metrics(golden, b, reduced=reduced)


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


def spec_sha256(spec: dict[str, Any]) -> str:
    """Content key of a corpus spec (same canonical form as the corpus de-duplication)."""
    text = json.dumps(spec, sort_keys=True, ensure_ascii=False)
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def source_sha256(source: str) -> str:
    return hashlib.sha256(source.encode("utf-8")).hexdigest()


def dump_golden(golden: dict[str, Any]) -> str:
    """Canonical golden text: sorted keys, indent 2, trailing newline (written with LF)."""
    return json.dumps(golden, indent=2, sort_keys=True, ensure_ascii=False) + "\n"


def write_text_lf(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8", newline="\n") as handle:
        handle.write(text)


def load_golden(path: Path = GOLDEN_PATH) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def golden_metrics(outcome: dict[str, Any]) -> dict[str, Any]:
    return {key: outcome[key] for key in METRIC_KEYS}


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
        "area": shape.Area(),
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
    return _compile_result(core.validate(document))


# --------------------------------------------------------------------------- corpus cases


def run_golden_case(
    core: Core,
    name: str,
    spec: dict[str, Any],
    golden_cases: dict[str, Any],
    *,
    rebaseline: bool = False,
) -> dict[str, Any]:
    """B (Cad2Compiler on the core's validate result) against the frozen entry.

    With ``rebaseline`` the frozen entry only reports what changed; the fresh metrics and source
    hash are returned in ``_golden`` for ``--write-golden``.
    """
    entry: dict[str, Any] = {"name": name}
    golden = golden_cases.get(name)
    spec_hash = spec_sha256(spec)
    if not rebaseline:
        if golden is None:
            return {**entry, "status": "mismatch", "details": ["case missing from the golden"]}
        if golden.get("spec_sha256") != spec_hash:
            return {
                **entry,
                "status": "mismatch",
                "details": [
                    "spec_sha256 changed since the golden was written (re-baseline needed)"
                ],
            }
    try:
        core_result = core.validate(spec)
    except Exception:
        return {**entry, "status": "mismatch", "details": [f"core: {_short_error()}"]}
    entry["valid"] = bool(core_result["report"]["valid"])
    sources: list[str] = []

    def source_b() -> str:
        sources.append(_compile_result(core_result))
        return sources[0]

    b, _ = build_outcome(source_b)
    if golden is None:
        diffs = ["case missing from the golden"]
    elif golden.get("spec_sha256") != spec_hash:
        diffs = ["spec_sha256 changed since the golden was written"]
    else:
        diffs = compare_golden(golden, b)
    status = "mismatch" if diffs else "equal"
    if rebaseline and diffs and b["ok"]:
        status = "rebaselined"
    entry.update(status=status, details=diffs)
    if golden is not None and golden.get("shape_valid") is False:
        entry["reduced_comparison"] = "invalid shape"
    if b["ok"] and golden is not None:
        entry["volume_delta"] = abs(b["volume"] - golden["volume"])
        entry["volume_a"] = golden["volume"]
    if entry["status"] == "mismatch" and name in ALLOWED_DIFFERENCES:
        entry["status"] = "allowed_difference"
        entry["allowed_reason"] = ALLOWED_DIFFERENCES[name]
    if b["ok"]:
        entry["_golden"] = {
            **golden_metrics(b),
            "spec_sha256": spec_hash,
            "model_sha256": source_sha256(sources[0]),
        }
    return entry


def _compile_result(core_result: dict[str, Any]) -> str:
    from app.services.compiler2 import Cad2Compiler

    return Cad2Compiler().compile(core_result["document"], core_result["resolved"])


# --------------------------------------------------------------------------- 2.0-native checks


def _load_cad2(filename: str) -> dict[str, Any]:
    document = json.loads((CAD2_DIR / filename).read_text(encoding="utf-8"))
    document.pop("expected_resolved", None)
    return document


def _plate_b(core: Core, document: dict[str, Any]) -> tuple[dict[str, Any], Any, list[str]]:
    sources: list[str] = []

    def factory() -> str:
        sources.append(compile_v2(core, document))
        return sources[0]

    outcome, result = build_outcome(factory)
    return outcome, result, sources


def check_plate_roundtrip(
    core: Core,
    frozen: dict[str, Any] | None = None,
    capture: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """The 2.0 plate against its downgrade -> validate -> Cad2Compiler round trip (metrics and
    symmetric difference), then against the frozen ``native`` golden entry. With ``capture``
    (re-baseline) the direct metrics and source hash are stored there instead."""
    check = {"name": "plate-four-holes round trip via downgrade and frozen golden"}
    document = _load_cad2("plate-four-holes.cad2.json")
    down = core.call({"op": "downgrade", "document": document})["result"]
    if not down.get("expressible"):
        return {**check, "status": "mismatch", "details": ["downgrade is not expressible"]}
    direct, direct_result, direct_sources = _plate_b(core, document)
    trip, trip_result, _ = _plate_b(core, down["document"])
    symdiff = (
        symmetric_difference(direct_result, trip_result)
        if direct_result is not None and trip_result is not None
        else None
    )
    status, details = classify(direct, trip, symdiff)
    if status == "both_failed":
        status = "mismatch"
    if capture is not None:
        if status == "equal":
            capture[NATIVE_PLATE] = {
                **golden_metrics(direct),
                "model_sha256": source_sha256(direct_sources[0]),
            }
    elif frozen is None:
        details = [*details, f"no frozen {NATIVE_PLATE} entry in the golden"]
        status = "mismatch"
    else:
        frozen_diffs = compare_golden(frozen, direct)
        if frozen_diffs:
            details = [*details, *(f"frozen: {d}" for d in frozen_diffs)]
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
        counts[case["status"]] = counts.get(case["status"], 0) + 1
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


def _versions() -> dict[str, str]:
    import cadquery

    versions = {"cadquery": str(getattr(cadquery, "__version__", "unknown"))}
    try:
        import OCP

        versions["ocp"] = str(getattr(OCP, "__version__", "unknown"))
    except ImportError:
        pass
    return versions


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--case", help="only corpus cases whose name contains this text")
    parser.add_argument("--limit", type=int, help="at most this many corpus cases")
    parser.add_argument("--skip-native", action="store_true", help="skip the 2.0-native checks")
    parser.add_argument(
        "--write-golden",
        metavar="PATH",
        help="re-baseline the golden from the current Cad2Compiler output (full corpus only; "
        "refused unless every native check passes and every case compiles)",
    )
    args = parser.parse_args(argv)
    rebaseline = bool(args.write_golden)
    if rebaseline and (args.case or args.limit is not None or args.skip_native):
        parser.error("--write-golden needs the full corpus (no --case/--limit/--skip-native)")

    try:
        import cadquery  # noqa: F401
    except ImportError:
        print(
            "geometry_equivalence: CadQuery is not importable in this interpreter", file=sys.stderr
        )
        return 2

    if rebaseline:
        print(
            "geometry_equivalence: re-baselining the geometry golden: justify this in the PR",
            file=sys.stderr,
        )
    golden: dict[str, Any] = {}
    try:
        golden = load_golden()
    except (OSError, ValueError) as exc:
        if not rebaseline:
            print(f"geometry_equivalence: cannot read {GOLDEN_PATH}: {exc}", file=sys.stderr)
            return 1
        print(f"geometry_equivalence: no readable golden to compare with ({exc})", file=sys.stderr)

    started = time.perf_counter()
    cases_in, duplicates = corpus()
    full_corpus = [name for name, _ in cases_in]
    filtered = bool(args.case) or args.limit is not None
    if args.case:
        cases_in = [c for c in cases_in if args.case in c[0]]
    if args.limit is not None:
        cases_in = cases_in[: args.limit]

    core = Core()
    captured: dict[str, Any] = {}
    native_captured: dict[str, Any] = {}

    def plate(c: Core) -> dict[str, Any]:
        return check_plate_roundtrip(
            c,
            golden.get("native", {}).get(NATIVE_PLATE),
            native_captured if rebaseline else None,
        )

    try:
        results = []
        for name, spec in cases_in:
            entry = run_golden_case(
                core,
                name,
                copy.deepcopy(spec),
                golden.get("cases", {}),
                rebaseline=rebaseline,
            )
            if "_golden" in entry:
                captured[name] = entry.pop("_golden")
            print(f"[{entry['status']}] {name}", file=sys.stderr)
            results.append(entry)
        if not rebaseline and not filtered:
            for extra in sorted(set(golden.get("cases", {})) - set(full_corpus)):
                print(f"[mismatch] {extra} (only in the golden)", file=sys.stderr)
                results.append(
                    {
                        "name": extra,
                        "status": "mismatch",
                        "details": ["case in the golden but not in the corpus"],
                    }
                )
        natives: list[dict[str, Any]] = []
        if not args.skip_native and not args.case:
            for check in (plate, check_enclosure_with_lid, check_l_bracket_poses):
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
        mode="golden",
        duplicates_skipped=duplicates,
        runtime_seconds=round(time.perf_counter() - started, 1),
    )
    failed = bool(summary["counts"]["mismatch"] or any(n["status"] != "equal" for n in natives))

    if rebaseline:
        complete = (
            not failed
            and len(natives) == 3
            and NATIVE_PLATE in native_captured
            and len(results) == len(captured) == len(full_corpus)
        )
        if not complete:
            print(
                "geometry_equivalence: not writing the golden: a native check or a corpus case "
                "did not pass",
                file=sys.stderr,
            )
            summary["golden_written"] = False
            failed = True
        else:
            data = {
                "meta": {
                    **_versions(),
                    "case_count": len(captured),
                    "format": GOLDEN_VERSION,
                    "platform": platform.platform(),
                },
                "native": native_captured,
                "cases": captured,
            }
            write_text_lf(Path(args.write_golden), dump_golden(data))
            summary["golden_written"] = args.write_golden
            print(f"geometry_equivalence: wrote {args.write_golden}", file=sys.stderr)

    print(json.dumps(summary, indent=2, ensure_ascii=False))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
