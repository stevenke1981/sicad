"""Generate ``v1_parity.json``: the ADR 0005 differential corpus for the Rust validator.

Re-run from the repository root with the project venv::

    .venv/Scripts/python.exe crates/promptcad-core/tests/fixtures/gen_v1_parity.py      # Windows
    .venv/bin/python crates/promptcad-core/tests/fixtures/gen_v1_parity.py              # POSIX

For every case the script builds a 1.0 ``CadDocument`` (``app.models.cad``), runs the 1.0
``DesignValidator`` and records ``expected_issues`` = ``[{code, severity, feature_index}]``.
``crates/promptcad-core/tests/validate_parity_v1.rs`` upgrades each ``spec`` with
``upgrade_1_to_2`` and asserts the Rust validator reproduces the same (code, severity, location)
multiset for the carried-over 1.0 codes.

Normalization: ``upgrade_1_to_2`` (spec §10.1) normalizes ``thread`` and drops values that do
not match the 2.0 pattern (the original text goes to ``notes``). The expected verdict is
therefore computed on the §10.3 *normalized* 1.0 document (same thread normalization applied
here). When that differs from the verdict on the raw document, the raw verdict is kept in
``raw_issues`` so the divergence stays visible (only the ``thread`` normalization can cause it).
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT))

from app.models.cad import CadDocument  # noqa: E402
from app.services.validator import DesignValidator  # noqa: E402

OUT = Path(__file__).with_name("v1_parity.json")
THREAD_RE = re.compile(r"^M\d{1,3}(\.\d)?(x\d(\.\d{1,2})?)?$")


def plate(length, w, t):
    return {"kind": "plate", "length": length, "width": w, "thickness": t}


def cyl(d, h):
    return {"kind": "cylinder", "diameter": d, "height": h}


def ring(od, idia, h):
    return {"kind": "ring", "outer_diameter": od, "inner_diameter": idia, "height": h}


def lbr(w, d, vh, t):
    return {"kind": "l_bracket", "width": w, "depth": d, "vertical_height": vh, "thickness": t}


def enc(length, w, h, wall):
    return {"kind": "enclosure", "length": length, "width": w, "height": h, "wall_thickness": wall}


def hole(x=0, y=0, z=0, axis="z", d=4, t="through", **kw):
    h = {"kind": "hole", "x": x, "y": y, "z": z, "axis": axis, "diameter": d, "hole_type": t}
    h.update(kw)
    return h


def doc(base, holes=(), fillets=(), chamfers=(), assumptions=()):
    return {
        "name": "parity",
        "source_prompt": "parity case",
        "base": base,
        "holes": list(holes),
        "fillets": [{"kind": "fillet", "radius": r, "selector": s} for r, s in fillets],
        "chamfers": [{"kind": "chamfer", "distance": r, "selector": s} for r, s in chamfers],
        "assumptions": list(assumptions),
        "planner": {"planner": "test"},
    }


T5 = plate(40, 30, 5)  # tests/test_validation.py make_doc()

CASES = [
    # ---- tests/test_validation.py ----
    ("test_overlapping_holes_are_error", doc(T5, [hole(0, 0, d=10), hole(4, 0, d=10)])),
    ("test_hole_outside_part_is_error", doc(T5, [hole(19, 0, d=6)])),
    ("test_vertical_plate_fillet_can_equal_half_thickness", doc(T5, fillets=[(5, "vertical")])),
    ("test_blind_depth_equal_to_material_is_blocked", doc(T5, [hole(0, 0, d=4, t="blind", depth=5)])),
    (
        "test_counterbore_effective_diameter_respects_edge",
        doc(T5, [hole(15, 0, d=4, t="counterbore", counterbore_diameter=12, counterbore_depth=2)]),
    ),
    # ---- plate ----
    ("plate_clean_through", doc(T5, [hole(0, 0, d=4)])),
    ("plate_low_edge_clearance", doc(T5, [hole(17.5, 0, d=4)])),
    ("plate_extreme_aspect_ratio", doc(plate(2000, 1, 1))),
    ("plate_fillet_all_too_large", doc(T5, fillets=[(3, "all")])),
    ("plate_fillet_vertical_too_large", doc(T5, fillets=[(15, "vertical"), (2, "top")])),
    ("plate_chamfer_top_too_large", doc(T5, chamfers=[(2.5, "top")])),
    (
        "plate_counterbore_depth",
        doc(T5, [hole(0, 0, d=4, t="counterbore", counterbore_diameter=8, counterbore_depth=5)]),
    ),
    (
        "plate_countersink_depth",
        doc(plate(40, 30, 2), [hole(0, 0, d=4, t="countersink", countersink_diameter=10, countersink_angle=90)]),
    ),
    (
        "plate_countersink_ok",
        doc(plate(40, 30, 10), [hole(0, 0, d=4, t="countersink", countersink_diameter=8, countersink_angle=90)]),
    ),
    ("plate_tapped_without_thread", doc(T5, [hole(0, 0, d=5, t="tapped")])),
    ("plate_tapped_with_thread", doc(T5, [hole(0, 0, d=5, t="tapped", thread="M6")])),
    ("plate_thread_normalized", doc(T5, [hole(0, 0, d=5, t="tapped", thread=" m6x1 ")])),
    ("plate_thread_invalid", doc(T5, [hole(0, 0, d=5, t="tapped", thread="M6 tapped")])),
    ("plate_x_axis_blind_too_deep", doc(plate(40, 30, 10), [hole(y=0, z=5, axis="x", t="blind", depth=45)])),
    ("plate_y_axis_hole_outside", doc(plate(40, 30, 10), [hole(x=0, z=9, axis="y")])),
    ("plate_y_axis_low_edge", doc(plate(40, 30, 10), [hole(x=0, z=7.5, axis="y", d=4)])),
    (
        "plate_three_holes_two_overlaps",
        doc(plate(80, 40, 5), [hole(0, 0, d=6), hole(5, 0, d=6), hole(10, 0, d=6)]),
    ),
    (
        "plate_overlap_ignores_other_axis",
        doc(plate(40, 30, 10), [hole(0, 0, d=4), hole(y=0, z=5, axis="x", d=4)]),
    ),
    ("plate_assumptions", doc(T5, assumptions=["a", "b"])),
    # ---- cylinder ----
    ("cylinder_z_outside", doc(cyl(20, 10), [hole(8, 0, d=6)])),
    ("cylinder_z_low_edge", doc(cyl(20, 10), [hole(6, 0, d=6)])),
    ("cylinder_z_clean", doc(cyl(20, 10), [hole(0, 0, d=6)])),
    ("cylinder_x_axis_blind_too_deep", doc(cyl(20, 30), [hole(y=0, z=15, axis="x", t="blind", depth=20)])),
    ("cylinder_y_axis_outside", doc(cyl(20, 10), [hole(x=0, z=9, axis="y")])),
    ("cylinder_vertical_fillet_too_large", doc(cyl(20, 10), fillets=[(10, "vertical")])),
    ("cylinder_z_blind_too_deep", doc(cyl(20, 10), [hole(0, 0, d=4, t="blind", depth=12)])),
    # ---- ring ----
    ("ring_hole_in_inner_void", doc(ring(40, 20, 5), [hole(0, 0, d=6)])),
    ("ring_hole_intersects_inner_void", doc(ring(40, 20, 5), [hole(10, 0, d=6)])),
    ("ring_hole_clean", doc(ring(40, 20, 5), [hole(15, 0, d=4)])),
    ("ring_hole_outside", doc(ring(40, 20, 5), [hole(19, 0, d=4)])),
    ("ring_x_axis_hole", doc(ring(40, 20, 5), [hole(y=0, z=2.5, axis="x", d=2)])),
    ("ring_chamfer_all_too_large", doc(ring(40, 20, 5), chamfers=[(3, "all")])),
    # ---- l_bracket ----
    ("lbracket_thin", doc(lbr(40, 30, 30, 1.5))),
    ("lbracket_x_axis_blind_too_deep", doc(lbr(40, 30, 30, 3), [hole(y=-13.5, z=20, axis="x", d=2, t="blind", depth=40)])),
    ("lbracket_z_blind_too_deep", doc(lbr(40, 30, 30, 3), [hole(0, 5, d=4, t="blind", depth=3)])),
    (
        "lbracket_y_counterbore_too_deep",
        doc(lbr(40, 30, 30, 3), [hole(x=0, z=20, axis="y", d=4, t="counterbore", counterbore_diameter=8, counterbore_depth=3)]),
    ),
    ("lbracket_z_outside", doc(lbr(40, 30, 30, 3), [hole(19, 0, d=4)])),
    # ---- enclosure ----
    ("enclosure_thin_wall", doc(enc(100, 70, 30, 0.6))),
    ("enclosure_z_blind_too_deep", doc(enc(100, 70, 30, 2), [hole(0, 0, d=4, t="blind", depth=2)])),
    ("enclosure_x_hole_zero_clearance", doc(enc(100, 70, 30, 2), [hole(y=33, z=15, axis="x", d=4)])),
    ("enclosure_chamfer_bottom_too_large", doc(enc(100, 70, 30, 2), chamfers=[(15, "bottom")])),
    (
        "enclosure_x_counterbore_too_deep",
        doc(enc(100, 70, 30, 2), [hole(y=0, z=15, axis="x", d=4, t="counterbore", counterbore_diameter=8, counterbore_depth=3)]),
    ),
    ("enclosure_extreme_aspect_ratio", doc(enc(3000, 2.5, 2, 0.5))),
]


def normalize(spec: dict) -> dict:
    out = json.loads(json.dumps(spec))
    for h in out.get("holes", []):
        t = h.get("thread")
        if t is not None:
            n = t.strip().upper().replace("X", "x")
            h["thread"] = n if THREAD_RE.match(n) else None
    return out


def verdict(spec: dict) -> list[dict]:
    report = DesignValidator().validate(CadDocument.model_validate(spec))
    return [
        {"code": i.code, "severity": i.severity.value, "feature_index": i.feature_index}
        for i in report.issues
    ]


def main() -> None:
    example = json.loads((ROOT / "examples/generated/plate-four-holes/spec.json").read_text("utf-8"))
    cases = [("example_plate_four_holes", example)] + CASES
    out = []
    for name, spec in cases:
        full = CadDocument.model_validate(spec).model_dump(mode="json")
        expected = verdict(normalize(full))
        entry = {"name": name, "spec": full, "expected_issues": expected}
        raw = verdict(full)
        if raw != expected:
            entry["raw_issues"] = raw
        out.append(entry)
    OUT.write_text(json.dumps(out, ensure_ascii=False, indent=1) + "\n", "utf-8")
    print(f"wrote {len(out)} cases to {OUT}")


if __name__ == "__main__":
    main()
