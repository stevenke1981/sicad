"""Cad2Compiler: determinism, safety, supported subset and explicit unsupported errors.

Placement data comes from the ``expected_resolved`` block of the two ``examples/cad2`` files
(the same shape as the core's ``resolved``), so most tests need no core binary; the last test
runs the real core when it is available.
"""

from __future__ import annotations

import ast
import asyncio
import copy
import json
from pathlib import Path

import pytest

from app.core.config import Settings
from app.services.compiler2 import Cad2Compiler, CompileUnsupported
from app.services.core_bridge import CoreBridge

CAD2 = Path(__file__).resolve().parents[1] / "examples" / "cad2"
MARKER = "INJECT_MARKER_7f3a91"
EVIL = f'"""\nimport os; os.system("{MARKER}")\n#'


def load(name: str) -> tuple[dict, dict]:
    document = json.loads((CAD2 / f"{name}.cad2.json").read_text(encoding="utf-8"))
    expected = document.pop("expected_resolved")
    resolved = {
        "schema_version": "2.0",
        "revision": document["revision"],
        "components": expected["components"],
        "ports": expected["ports"],
        "holes": expected["holes"],
        "issues": [],
    }
    return document, resolved


@pytest.fixture
def plate():
    return load("plate-four-holes")


@pytest.fixture
def enclosure():
    return load("enclosure-with-lid")


def component(document: dict, cid: str) -> dict:
    return next(c for c in document["components"] if c["id"] == cid)


def hole(document: dict, cid: str) -> dict:
    return component(document, cid)["shape"]


def test_compile_is_deterministic_and_parses(plate):
    first = Cad2Compiler().compile(*plate)
    second = Cad2Compiler().compile(*copy.deepcopy(plate))
    assert first == second
    ast.parse(first)
    assert "\r" not in first


def test_model_py_contract(plate):
    code = Cad2Compiler().compile(*plate)
    for needle in (
        "def build():",
        "return result, warnings",
        "def export_all(result, output_dir: Path, formats: set[str], warnings: list[str]):",
        "--output-dir",
        "--formats",
        "render-warnings.json",
        "if __name__ == '__main__':",
        "import cadquery as cq",
    ):
        assert needle in code
    assert "subprocess" not in code
    assert code.count("cq.Solid.makeCylinder(3.3, 12,") == 4
    assert ".edges('|Z').fillet(5)" in code


def test_document_text_never_reaches_the_code(plate):
    document, resolved = plate
    document["name"] = EVIL
    document["source_prompt"] = EVIL
    document["assumptions"] = [EVIL]
    document["notes"] = [EVIL]
    for comp in document["components"]:
        comp["label"] = EVIL
    for part in document["parts"]:
        part["name"] = EVIL
    code = Cad2Compiler().compile(document, resolved)
    assert MARKER not in code
    ast.parse(code)


def test_non_numeric_dimension_is_unsupported_not_injected(plate):
    document, resolved = plate
    component(document, "v_plate")["shape"]["length"] = f"__import__('os').system('{MARKER}')"
    with pytest.raises(CompileUnsupported) as info:
        Cad2Compiler().compile(document, resolved)
    assert info.value.items[0][0] == "v_plate"


def test_enclosure_with_lid_is_one_compound_in_world_coordinates(enclosure):
    code = Cad2Compiler().compile(*enclosure)
    ast.parse(code)
    assert "box(100, 70, 33, centered=(True, True, False))" in code
    assert "box(96, 66, 31.2," in code  # inner cavity keeps the 1.0 +0.2 overshoot
    assert ".translate((0, 0, 33))" in code  # lid placed from resolved.components
    assert "cq.Compound.makeCompound(solids)" in code
    assert "e_pcb" not in code  # the PCB reserve is intent only


def test_single_part_result_is_the_part(plate):
    code = Cad2Compiler().compile(*plate)
    assert "result = part_0" in code
    assert "makeCompound" not in code


def test_rotated_pose_uses_placement_helper(enclosure):
    document, resolved = enclosure
    resolved["components"]["v_lid"].update(x_axis="+y", z_axis="-x", origin=[1.5, 2, 3])
    code = Cad2Compiler().compile(document, resolved)
    assert "place(b_1_0, (1.5, 2, 3), (0, 1, 0), (-1, 0, 0))" in code


def test_counterbore_and_countersink_follow_section_7_2(plate):
    document, resolved = plate
    hole(document, "e_h1").update(
        hole_type="counterbore", counterbore_diameter=11, counterbore_depth=4
    )
    hole(document, "e_h2").update(
        hole_type="countersink", countersink_diameter=12, countersink_angle=90
    )
    code = Cad2Compiler().compile(document, resolved)
    # entry (-50, -20, 10), inward -z: back off 0.1 -> z 10.1, depth + 0.2
    assert "cq.Solid.makeCylinder(5.5, 4.2, cq.Vector(-50, -20, 10.1), cq.Vector(0, 0, -1))" in code
    # the cone starts at the entry (no back-off), height (12 - 6.6) / (2 * tan 45 deg) = 2.7
    assert "cq.Solid.makeCone(6, 3.3, 2.7, cq.Vector(50, -20, 10), cq.Vector(0, 0, -1))" in code


def test_through_cutter_is_parametrised_along_a_positive_axis(plate):
    code = Cad2Compiler().compile(*plate)
    # resolved says start z=11 -> end z=-1 along -z; emitted from z=-1 along +z (1.0 form)
    assert "cq.Solid.makeCylinder(3.3, 12, cq.Vector(-50, -20, -1), cq.Vector(0, 0, 1))" in code


def test_blind_hole_uses_solved_cutter(plate):
    document, resolved = plate
    hole(document, "e_h1").update(hole_type="blind", depth=5)
    resolved["holes"]["e_h1"].update(cutter_start=[-50, -20, 10.1], cutter_end=[-50, -20, 4.9])
    code = Cad2Compiler().compile(document, resolved)
    assert "cq.Solid.makeCylinder(3.3, 5.2, cq.Vector(-50, -20, 10.1), cq.Vector(0, 0, -1))" in code


def test_purchased_part_adds_a_warning(enclosure):
    document, resolved = enclosure
    document["parts"][1]["source"] = "purchased"
    code = Cad2Compiler().compile(document, resolved)
    assert "part 2 is purchased" in code


def test_intersect_with_envelope_box(plate):
    document, resolved = plate
    document["components"].append(
        {
            "id": "e_env",
            "kind": "empty",
            "role": "envelope",
            "shape": {"kind": "box", "length": 100, "width": 50, "height": 20},
            "label": None,
            "provenance": None,
        }
    )
    resolved["components"]["e_env"] = {
        "origin": [0, 0, -5],
        "x_axis": "+x",
        "z_axis": "+z",
        "defaulted_axes": [],
    }
    document["parts"][0]["body"].append({"op": "intersect", "component": "e_env"})
    code = Cad2Compiler().compile(document, resolved)
    assert "part_0 = part_0.intersect(b_0_5)" in code
    assert ".translate((0, 0, -5))" in code


def test_unsupported_constructs_are_all_reported(plate):
    document, resolved = plate
    document["components"] += [
        {
            "id": "v_ext",
            "kind": "volume",
            "shape": {"kind": "extrude", "profile": "p_x", "height": 3},
            "label": None,
            "provenance": None,
        },
        {
            "id": "e_of",
            "kind": "empty",
            "role": "cut",
            "shape": {"kind": "of", "source": "v_plate"},
            "label": None,
            "provenance": None,
        },
        {
            "id": "e_keep",
            "kind": "empty",
            "role": "keep_out",
            "shape": {"kind": "box", "length": 1, "width": 1, "height": 1},
            "label": None,
            "provenance": None,
        },
    ]
    document["parts"][0]["body"] += [
        {"op": "add", "component": "v_ext"},
        {"op": "cut", "component": "e_of"},
        {"op": "cut", "component": "e_keep"},
        {"op": "cut", "component": "v_missing"},
    ]
    with pytest.raises(CompileUnsupported) as info:
        Cad2Compiler().compile(document, resolved)
    reported = {ident for ident, _ in info.value.items}
    assert reported == {"v_ext", "e_of", "e_keep", "v_missing"}
    assert all(reason for _, reason in info.value.items)
    assert "v_ext" in str(info.value)


def test_fillet_on_rotated_part_is_unsupported(plate):
    document, resolved = plate
    resolved["components"]["v_plate"].update(x_axis="+x", z_axis="-z")
    with pytest.raises(CompileUnsupported) as info:
        Cad2Compiler().compile(document, resolved)
    assert info.value.items == [
        ("p_plate", "fillets/chamfers on a rotated part are not supported (world-frame build)")
    ]


def test_missing_hole_cutter_is_reported(plate):
    document, resolved = plate
    del resolved["holes"]["e_h3"]
    with pytest.raises(CompileUnsupported) as info:
        Cad2Compiler().compile(document, resolved)
    assert [i for i, _ in info.value.items] == ["e_h3"]


def test_unknown_selector_is_reported(plate):
    document, resolved = plate
    document["parts"][0]["fillets"][0]["selector"] = f"x'; {MARKER}"
    with pytest.raises(CompileUnsupported):
        Cad2Compiler().compile(document, resolved)


@pytest.mark.parametrize("resolved", [{}, {"components": {}, "ports": {}, "holes": {}}])
def test_gated_resolved_raises_a_clear_error(plate, resolved):
    with pytest.raises(CompileUnsupported) as info:
        Cad2Compiler().compile(plate[0], resolved)
    assert info.value.items[0][0] == "resolved"
    assert "solver" in info.value.items[0][1]


def test_wrong_schema_version_is_rejected(plate):
    document, resolved = plate
    document["schema_version"] = "1.0"
    with pytest.raises(CompileUnsupported):
        Cad2Compiler().compile(document, resolved)


def test_document_without_parts_is_rejected(plate):
    document, resolved = plate
    document["parts"] = []
    with pytest.raises(CompileUnsupported):
        Cad2Compiler().compile(document, resolved)


def test_compiles_the_real_core_result(core_binary, plate):
    document, _ = plate
    bridge = CoreBridge(Settings(env="test"))
    bridge.override_binary(core_binary)
    try:
        response = asyncio.run(bridge.call({"op": "validate", "document": document}))
    finally:
        bridge.shutdown()
    result = response["result"]
    code = Cad2Compiler().compile(result["document"], result["resolved"])
    ast.parse(code)
    assert code.count("makeCylinder") == 4
