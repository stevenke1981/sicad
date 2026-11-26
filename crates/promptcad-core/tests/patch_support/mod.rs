//! Shared fixtures for the patch-engine and op-log tests.
#![allow(dead_code)]

use std::path::PathBuf;

use promptcad_core::canonical::{parse_example, to_canonical_json};
use promptcad_core::model::{CadDocument2, Patch, Role};
use promptcad_core::patch_engine::{PatchOutcome, ReasonCode};
use serde_json::{json, Value};

pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Loads an example from `examples/cad2/` (the `expected_resolved` annotation is dropped).
pub fn example(name: &str) -> CadDocument2 {
    let path = repo_root().join("examples/cad2").join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
    parse_example(&text).expect("example parses").0
}

pub fn example_a() -> CadDocument2 {
    example("plate-four-holes.cad2.json")
}

pub fn example_b() -> CadDocument2 {
    example("enclosure-with-lid.cad2.json")
}

/// The revision-0 document example A starts from: top-level metadata only.
pub fn empty_a() -> CadDocument2 {
    serde_json::from_value(json!({
        "schema_version": "2.0",
        "name": "plate-four-holes",
        "source_prompt": "鋁合金固定板，長120mm、寬60mm、厚10mm，四角 M6 通孔，孔中心離邊緣10mm，四角 R5。",
        "unit": "mm",
        "revision": 0,
        "parameters": [],
        "bindings": [],
        "components": [],
        "constraints": [],
        "parts": [],
        "assemblies": [],
        "root": null,
        "assumptions": [],
        "notes": [],
        "planner": {"planner": "agent_team", "confidence": 0.9, "review_required": true}
    }))
    .unwrap()
}

/// Builds a patch from JSON ops.
pub fn patch(agent: Role, base_revision: u64, ops: Value) -> Patch {
    patch_resolving(agent, base_revision, &[], ops)
}

/// Builds a patch with a `resolves` list.
pub fn patch_resolving(agent: Role, base_revision: u64, resolves: &[&str], ops: Value) -> Patch {
    serde_json::from_value(json!({
        "agent": agent,
        "base_revision": base_revision,
        "rationale": "test patch",
        "resolves": resolves,
        "ops": ops,
    }))
    .expect("test patch parses")
}

pub fn dot(id: &str) -> Value {
    json!({"op": "add_component", "component":
        {"id": id, "kind": "dot", "shape": {"kind": "point"}, "label": null}})
}

pub fn plate(id: &str, length: f64, width: f64, thickness: f64) -> Value {
    json!({"op": "add_component", "component":
        {"id": id, "kind": "volume",
         "shape": {"kind": "plate", "length": length, "width": width, "thickness": thickness},
         "label": null}})
}

pub fn box_empty(id: &str, role: &str, l: f64, w: f64, h: f64) -> Value {
    json!({"op": "add_component", "component":
        {"id": id, "kind": "empty", "role": role,
         "shape": {"kind": "box", "length": l, "width": w, "height": h}, "label": null}})
}

pub fn hole(id: &str, hole_type: &str, diameter: f64, thread: Option<&str>) -> Value {
    json!({"op": "add_component", "component":
        {"id": id, "kind": "empty", "role": "cut",
         "shape": {"kind": "hole", "axis": "z", "diameter": diameter, "hole_type": hole_type,
                   "depth": null, "thread": thread,
                   "counterbore_diameter": null, "counterbore_depth": null,
                   "countersink_diameter": null, "countersink_angle": null},
         "label": null}})
}

fn param(name: &str, value: f64, note: &str) -> Value {
    json!({"op": "set_parameter", "parameter":
        {"name": name, "value": value, "min": null, "max": null, "note": note}})
}

fn labelled(mut op: Value, label: &str) -> Value {
    op["component"]["label"] = json!(label);
    op
}

fn on_face(id: &str, dot: &str, u: f64, v: f64) -> Value {
    json!({"op": "add_constraint", "constraint":
        {"id": id, "kind": "on_face", "a": format!("{dot}.point"), "b": "v_plate.top",
         "value": null, "u": u, "v": v, "frame": null}})
}

fn coincident(id: &str, hole: &str, dot: &str) -> Value {
    json!({"op": "add_constraint", "constraint":
        {"id": id, "kind": "coincident", "a": format!("{hole}.entry"), "b": format!("{dot}.point"),
         "value": null, "u": null, "v": null, "frame": null}})
}

fn bind(target: &str, expr: &str) -> Value {
    json!({"op": "set_binding", "target": target, "expr": expr})
}

/// The four-role patch sequence of concept §12.1 (example A), in the frozen 2.0 op syntax.
pub fn example_a_patches() -> Vec<(Role, Patch)> {
    let r1 = patch(
        Role::SpacePlanner,
        0,
        json!([
            param("len", 120.0, "長"),
            param("wid", 60.0, "寬"),
            param("thk", 10.0, "厚"),
            param("margin", 10.0, "孔邊距"),
        ]),
    );
    let mut ops = vec![
        labelled(plate("v_plate", 120.0, 60.0, 10.0), "固定板"),
        labelled(dot("d_h1"), "左下角孔心"),
        labelled(dot("d_h2"), "右下角孔心"),
        labelled(dot("d_h3"), "右上角孔心"),
        labelled(dot("d_h4"), "左上角孔心"),
    ];
    for i in 1..=4 {
        ops.push(hole(&format!("e_h{i}"), "clearance", 6.6, Some("M6")));
    }
    ops.extend([
        bind("v_plate:length", "len"),
        bind("v_plate:width", "wid"),
        bind("v_plate:thickness", "thk"),
        json!({"op": "add_assumption", "text": "M6 未提供實際孔徑，暫用一般間隙孔 6.6 mm（表列值）"}),
    ]);
    let r2 = patch(Role::ComponentSmith, 1, Value::Array(ops));

    let corners = [(-50.0, -20.0), (50.0, -20.0), (50.0, 20.0), (-50.0, 20.0)];
    let mut ops = vec![json!({"op": "add_constraint", "constraint":
        {"id": "k_root", "kind": "fixed", "a": "v_plate.origin", "b": null,
         "value": null, "u": null, "v": null,
         "frame": {"origin": [0, 0, 0], "x_axis": "+x", "z_axis": "+z"}}})];
    for (i, (u, v)) in corners.iter().enumerate() {
        let n = i + 1;
        ops.push(on_face(&format!("k_d{n}"), &format!("d_h{n}"), *u, *v));
    }
    for n in 1..=4 {
        ops.push(coincident(
            &format!("k_e{n}"),
            &format!("e_h{n}"),
            &format!("d_h{n}"),
        ));
    }
    let exprs = [
        ("margin - len / 2", "margin - wid / 2"),
        ("len / 2 - margin", "margin - wid / 2"),
        ("len / 2 - margin", "wid / 2 - margin"),
        ("margin - len / 2", "wid / 2 - margin"),
    ];
    for (i, (u, v)) in exprs.iter().enumerate() {
        let n = i + 1;
        ops.push(bind(&format!("k_d{n}:u"), u));
        ops.push(bind(&format!("k_d{n}:v"), v));
    }
    let r3 = patch(Role::Joiner, 2, Value::Array(ops));

    let r4 = patch(
        Role::Assembler,
        3,
        json!([{"op": "add_part", "part": {
            "id": "p_plate", "name": "固定板", "material": "aluminum", "quantity": 1,
            "source": "made",
            "body": [
                {"op": "add", "component": "v_plate"},
                {"op": "cut", "component": "e_h1"},
                {"op": "cut", "component": "e_h2"},
                {"op": "cut", "component": "e_h3"},
                {"op": "cut", "component": "e_h4"}
            ],
            "fillets": [{"kind": "fillet", "radius": 5, "selector": "vertical"}],
            "chamfers": []
        }}]),
    );
    vec![
        (Role::SpacePlanner, r1),
        (Role::ComponentSmith, r2),
        (Role::Joiner, r3),
        (Role::Assembler, r4),
    ]
}

/// Asserts acceptance and returns the new document.
#[track_caller]
pub fn accepted(outcome: &PatchOutcome) -> CadDocument2 {
    match outcome {
        PatchOutcome::Accepted(a) => a.document.clone(),
        PatchOutcome::Rejected(r) => panic!(
            "expected acceptance, got {} (op {:?}, target {:?}); issues: {:?}",
            r.reason,
            r.reason.op_index,
            r.reason.target,
            r.issues.iter().map(|i| &i.code).collect::<Vec<_>>()
        ),
    }
}

/// Asserts rejection with `code`.
#[track_caller]
pub fn rejected_with(outcome: &PatchOutcome, code: impl Into<ReasonCode>) -> ReasonCode {
    let code = code.into();
    match outcome {
        PatchOutcome::Accepted(_) => panic!("expected rejection with {code}, got acceptance"),
        PatchOutcome::Rejected(r) => {
            assert_eq!(r.reason.code, code, "reason: {}", r.reason);
            code
        }
    }
}

pub fn canon(doc: &CadDocument2) -> String {
    to_canonical_json(doc)
}
