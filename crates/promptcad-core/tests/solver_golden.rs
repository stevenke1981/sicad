//! Solver golden tests against `expected_resolved` in `examples/cad2/*.cad2.json`, plus
//! focused tests for §5.4, §5.5, §7 behaviour.

use std::collections::BTreeSet;
use std::path::PathBuf;

use promptcad_core::canonical::{parse_example, to_canonical_string};
use promptcad_core::model::geom::{AxisDir, Orientation};
use promptcad_core::model::resolved::{Resolved, ResolvedPort};
use promptcad_core::model::{CadDocument2, PortRef};
use promptcad_core::ports::frame_port_ref;
use promptcad_core::solver::{self, WorldView};
use serde_json::{json, Value};

const TOL: f64 = 1e-6;
const PLATE: &str = "examples/cad2/plate-four-holes.cad2.json";
const ENCLOSURE: &str = "examples/cad2/enclosure-with-lid.cad2.json";

fn read(rel: &str) -> String {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn load(rel: &str) -> (CadDocument2, Value) {
    let (doc, expected) = parse_example(&read(rel)).unwrap();
    (doc, expected.unwrap())
}

/// The example as a mutable JSON value (example-only keys removed).
fn example_value(rel: &str) -> Value {
    let (doc, _) = load(rel);
    serde_json::to_value(&doc).unwrap()
}

fn doc_from(v: Value) -> CadDocument2 {
    CadDocument2::from_value(v).unwrap_or_else(|e| panic!("{e}"))
}

fn pr(s: &str) -> PortRef {
    s.parse().unwrap()
}

/// Deep comparison with numeric tolerance; objects must have the same keys.
fn assert_close(actual: &Value, expected: &Value, path: &str) {
    match (actual, expected) {
        (Value::Number(a), Value::Number(e)) => {
            let (a, e) = (a.as_f64().unwrap(), e.as_f64().unwrap());
            assert!((a - e).abs() <= TOL, "{path}: {a} != {e}");
        }
        (Value::Array(a), Value::Array(e)) => {
            assert_eq!(a.len(), e.len(), "{path}: length");
            for (i, (x, y)) in a.iter().zip(e).enumerate() {
                assert_close(x, y, &format!("{path}[{i}]"));
            }
        }
        (Value::Object(a), Value::Object(e)) => {
            let ka: BTreeSet<_> = a.keys().collect();
            let ke: BTreeSet<_> = e.keys().collect();
            assert_eq!(ka, ke, "{path}: keys");
            for (k, v) in e {
                assert_close(&a[k], v, &format!("{path}.{k}"));
            }
        }
        _ => assert_eq!(actual, expected, "{path}"),
    }
}

fn codes(r: &Resolved) -> Vec<&str> {
    r.issues.iter().map(|i| i.code.as_str()).collect()
}

fn port_json(p: &ResolvedPort) -> Value {
    serde_json::to_value(p).unwrap()
}

// =============================================================================================
// golden
// =============================================================================================

fn check_golden(rel: &str) {
    let (doc, expected) = load(rel);
    let resolved = solver::solve(&doc);
    let actual = serde_json::to_value(&resolved).unwrap();

    assert_eq!(resolved.revision, doc.revision);
    for key in ["components", "holes", "issues"] {
        assert_close(&actual[key], &expected[key], &format!("{rel}:{key}"));
    }

    // every expected port, resolved through the world view (expected_resolved also lists
    // ports that are neither constraint-referenced nor frame ports, e.g. regions)
    let view = WorldView::new(&doc, &resolved);
    for (name, exp) in expected["ports"].as_object().unwrap() {
        let port = pr(name);
        let got = view
            .port(&port)
            .unwrap_or_else(|| panic!("{rel}: {name} unresolved"));
        assert_close(&port_json(&got), exp, &format!("{rel}:ports.{name}"));
        if let Some(stored) = resolved.ports.get(&port) {
            assert_close(
                &port_json(stored),
                exp,
                &format!("{rel}:resolved.ports.{name}"),
            );
        }
        assert_eq!(
            solver::world_port(&doc, &resolved, &port).as_ref(),
            Some(&got)
        );
    }

    // Resolved.ports = constraint-referenced ports ∪ frame ports (§5.6)
    let mut want: BTreeSet<String> = doc
        .constraints
        .iter()
        .flat_map(|k| k.ports().map(|p| p.to_string()).collect::<Vec<_>>())
        .collect();
    want.extend(
        doc.components
            .iter()
            .filter_map(frame_port_ref)
            .map(|p| p.to_string()),
    );
    let got: BTreeSet<String> = resolved.ports.keys().map(|p| p.to_string()).collect();
    assert_eq!(got, want, "{rel}: Resolved.ports keys");
}

#[test]
fn golden_plate_four_holes() {
    check_golden(PLATE);
}

#[test]
fn golden_enclosure_with_lid() {
    check_golden(ENCLOSURE);
}

#[test]
fn determinism_identical_bytes() {
    for rel in [PLATE, ENCLOSURE] {
        let (doc, _) = load(rel);
        let a = to_canonical_string(&solver::solve(&doc));
        let b = to_canonical_string(&solver::solve(&doc.clone()));
        assert_eq!(a, b);
        assert!(!a.contains("-0.0"), "{rel}: negative zero in output");
        // round trip through resolved.json
        let back: Resolved = serde_json::from_str(&a).unwrap();
        assert_eq!(to_canonical_string(&back), a);
    }
}

// =============================================================================================
// §5.5
// =============================================================================================

#[test]
fn constraint_cycle_is_redundant_not_error() {
    let mut v = example_value(ENCLOSURE);
    v["constraints"].as_array_mut().unwrap().push(json!(
        {"id": "k_lid_z", "kind": "flush", "a": "v_lid.top", "b": "v_body.top", "value": 2}
    ));
    let r = solver::solve(&doc_from(v.clone()));
    assert_eq!(codes(&r), ["redundant_constraint"]);
    assert_eq!(r.issues[0].target.as_deref(), Some("k_lid_z"));
    assert!(r.issues[0].message.contains('z'), "{}", r.issues[0].message);

    v["constraints"][5]["value"] = json!(3);
    let r = solver::solve(&doc_from(v));
    assert_eq!(codes(&r), ["conflicting_constraints"]);
    let i = &r.issues[0];
    assert_eq!(i.target.as_deref(), Some("k_lid_z"));
    assert_eq!(i.related, ["k_lid_mate", "k_root"]);
    assert!(i.message.contains("residual 1 mm"), "{}", i.message);
    // the solved positions are those of the first (assigning) constraints
    assert_eq!(r.components["v_lid"].origin, [0.0, 0.0, 33.0]);
}

#[test]
fn duplicate_fixed_is_checked_in_step_5() {
    let mut v = example_value(PLATE);
    v["constraints"].as_array_mut().unwrap().push(json!(
        {"id": "k_root2", "kind": "fixed", "a": "v_plate.origin",
         "frame": {"origin": [0, 0, 1], "x_axis": "+x", "z_axis": "+z"}}
    ));
    let r = solver::solve(&doc_from(v.clone()));
    assert_eq!(codes(&r), ["conflicting_constraints"]);
    assert_eq!(r.issues[0].related, ["k_root"]);
    v["constraints"][9]["frame"]["origin"] = json!([0, 0, 0]);
    assert_eq!(
        codes(&solver::solve(&doc_from(v))),
        ["redundant_constraint"]
    );
}

#[test]
fn unconstrained_component_defaults_to_origin() {
    let mut v = example_value(ENCLOSURE);
    v["constraints"]
        .as_array_mut()
        .unwrap()
        .retain(|k| k["id"] != "k_pcb_z");
    let r = solver::solve(&doc_from(v.clone()));
    assert_eq!(codes(&r), ["under_constrained"]);
    assert_eq!(r.issues[0].target.as_deref(), Some("e_pcb"));
    assert_eq!(
        serde_json::to_value(&r.components["e_pcb"]).unwrap()["defaulted_axes"],
        json!(["z"])
    );
    assert_eq!(r.components["e_pcb"].origin, [0.0, 0.0, 0.0]);

    // a completely free component: all three axes, one warning per component, in doc order
    v["components"].as_array_mut().unwrap().push(json!(
        {"id": "e_free", "kind": "empty", "role": "keep_out",
         "shape": {"kind": "box", "length": 1, "width": 1, "height": 1}}
    ));
    let r = solver::solve(&doc_from(v));
    assert_eq!(codes(&r), ["under_constrained", "under_constrained"]);
    let targets: Vec<_> = r.issues.iter().map(|i| i.target.as_deref()).collect();
    assert_eq!(targets, [Some("e_pcb"), Some("e_free")]);
    assert_eq!(
        serde_json::to_value(&r.components["e_free"]).unwrap()["defaulted_axes"],
        json!(["x", "y", "z"])
    );
}

#[test]
fn defaulting_re_propagates() {
    // e_b depends on e_a, which is free: defaulting e_a then propagates to e_b (no warning)
    let mut v = example_value(ENCLOSURE);
    let comps = v["components"].as_array_mut().unwrap();
    for id in ["e_a", "e_b"] {
        comps.push(json!({"id": id, "kind": "empty", "role": "keep_out",
                          "shape": {"kind": "box", "length": 2, "width": 2, "height": 2}}));
    }
    v["constraints"].as_array_mut().unwrap().push(json!(
        {"id": "k_ab", "kind": "coincident", "a": "e_b.center", "b": "e_a.origin"}
    ));
    let r = solver::solve(&doc_from(v));
    assert_eq!(codes(&r), ["under_constrained"]);
    assert_eq!(r.issues[0].target.as_deref(), Some("e_a"));
    assert_eq!(r.components["e_b"].origin, [0.0, 0.0, -1.0]);
    assert!(r.components["e_b"].defaulted_axes.is_empty());
}

#[test]
fn no_fixed_root() {
    let mut v = example_value(ENCLOSURE);
    v["constraints"]
        .as_array_mut()
        .unwrap()
        .retain(|k| k["kind"] != "fixed");
    let r = solver::solve(&doc_from(v));
    assert_eq!(codes(&r), ["no_fixed_root"]);
    assert!(r.components.is_empty() && r.ports.is_empty() && r.holes.is_empty());
}

// =============================================================================================
// §5.4
// =============================================================================================

fn base_doc(components: Value, constraints: Value, parts: Value) -> CadDocument2 {
    doc_from(json!({
        "schema_version": "2.0", "name": "t", "source_prompt": "t", "unit": "mm",
        "revision": 1, "parameters": [], "bindings": [],
        "components": components, "constraints": constraints, "parts": parts,
        "assemblies": [], "root": null, "assumptions": [], "notes": [],
        "planner": {"planner": "test", "confidence": 1, "review_required": false}
    }))
}

fn plate(id: &str, l: f64, w: f64, t: f64) -> Value {
    json!({"id": id, "kind": "volume", "shape": {"kind": "plate", "length": l, "width": w, "thickness": t}})
}

fn root_fixed(port: &str) -> Value {
    json!({"id": "k_root", "kind": "fixed", "a": port,
           "frame": {"origin": [0, 0, 0], "x_axis": "+x", "z_axis": "+z"}})
}

fn part(id: &str, body: &[(&str, &str)]) -> Value {
    let body: Vec<Value> = body
        .iter()
        .map(|(op, c)| json!({"op": op, "component": c}))
        .collect();
    json!({"id": id, "name": id, "quantity": 1, "source": "made", "body": body})
}

#[test]
fn mate_flips_a_part() {
    // the cap's own top face mates against the base top: the cap must be upside down
    let doc = base_doc(
        json!([
            plate("v_base", 100.0, 60.0, 10.0),
            plate("v_cap", 100.0, 60.0, 4.0)
        ]),
        json!([
            root_fixed("v_base.origin"),
            {"id": "k_ax", "kind": "coaxial", "a": "v_cap.axis_z", "b": "v_base.axis_z"},
            {"id": "k_m", "kind": "mate", "a": "v_cap.top", "b": "v_base.top", "value": 1}
        ]),
        json!([
            part("p_base", &[("add", "v_base")]),
            part("p_cap", &[("add", "v_cap")])
        ]),
    );
    let r = solver::solve(&doc);
    assert_eq!(codes(&r), Vec::<&str>::new());
    let cap = &r.components["v_cap"];
    // first canonical orientation with local +z → world −z is (+x, −z)
    assert_eq!(
        cap.orientation(),
        Orientation::new(AxisDir::PosX, AxisDir::NegZ)
    );
    // cap top (local z = 4) sits 1 mm above the base top (z = 10): t_z − 4 = 11
    assert_eq!(cap.origin, [0.0, 0.0, 15.0]);
    match &r.ports[&pr("v_cap.top")] {
        ResolvedPort::Face { origin, normal, .. } => {
            assert_eq!(*normal, AxisDir::NegZ);
            assert_eq!(*origin, [0.0, 0.0, 11.0]);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn orientation_unsatisfiable_in_step_5_and_step_7() {
    // step 5: mate and flush on the same face pair cannot both hold
    let doc = base_doc(
        json!([
            plate("v_base", 10.0, 10.0, 2.0),
            plate("v_cap", 10.0, 10.0, 2.0)
        ]),
        json!([
            root_fixed("v_base.origin"),
            {"id": "k_m", "kind": "mate", "a": "v_cap.top", "b": "v_base.top"},
            {"id": "k_f", "kind": "flush", "a": "v_cap.top", "b": "v_base.top"}
        ]),
        json!([]),
    );
    let r = solver::solve(&doc);
    let orient: Vec<_> = r
        .issues
        .iter()
        .filter(|i| i.code == "orientation_unsatisfiable")
        .collect();
    assert_eq!(orient.len(), 1, "{:?}", r.issues);
    assert_eq!(orient[0].target.as_deref(), Some("v_cap"));
    assert_eq!(orient[0].related, ["k_m", "k_f"]);
    assert_eq!(
        r.components["v_cap"].orientation(),
        Some(Orientation::IDENTITY)
    );

    // step 7: both orientations fixed, the mate cannot hold
    let doc = base_doc(
        json!([
            plate("v_base", 10.0, 10.0, 2.0),
            plate("v_cap", 10.0, 10.0, 2.0)
        ]),
        json!([
            root_fixed("v_base.origin"),
            {"id": "k_cap", "kind": "fixed", "a": "v_cap.origin",
             "frame": {"origin": [0, 0, 2], "x_axis": "+x", "z_axis": "+z"}},
            {"id": "k_m", "kind": "mate", "a": "v_cap.bottom", "b": "v_base.bottom"}
        ]),
        json!([]),
    );
    let r = solver::solve(&doc);
    let orient: Vec<_> = r
        .issues
        .iter()
        .filter(|i| i.code == "orientation_unsatisfiable")
        .collect();
    assert_eq!(orient.len(), 1, "{:?}", r.issues);
    assert_eq!(orient[0].target.as_deref(), Some("k_m"));
    assert_eq!(orient[0].related, ["v_cap", "v_base"]);
}

#[test]
fn rotated_fixed_frame_transforms_ports() {
    // local X → world +Y, local Z → world +X (so local Y → world +Z)
    let doc = base_doc(
        json!([plate("v_p", 40.0, 20.0, 5.0), {"id": "d_a", "kind": "dot", "shape": {"kind": "point"}}]),
        json!([
            {"id": "k_root", "kind": "fixed", "a": "v_p.origin",
             "frame": {"origin": [10, 20, 30], "x_axis": "+y", "z_axis": "+x"}},
            {"id": "k_d", "kind": "on_face", "a": "d_a.point", "b": "v_p.top", "u": 5, "v": 3}
        ]),
        json!([]),
    );
    let r = solver::solve(&doc);
    assert!(r.issues.is_empty(), "{:?}", r.issues);
    let p = &r.components["v_p"];
    assert_eq!((p.x_axis, p.z_axis), (AxisDir::PosY, AxisDir::PosX));
    assert_eq!(p.origin, [10.0, 20.0, 30.0]);
    assert_eq!(
        r.ports[&pr("v_p.top")],
        ResolvedPort::Face {
            origin: [15.0, 20.0, 30.0],
            normal: AxisDir::PosX,
            u: AxisDir::PosY,
            v: AxisDir::PosZ,
            extent: [40.0, 20.0],
        }
    );
    assert_eq!(r.components["d_a"].origin, [15.0, 25.0, 33.0]);
    let view = WorldView::new(&doc, &r);
    let region = view.region("v_p").unwrap();
    assert_eq!(region.min, [10.0, 0.0, 20.0]);
    assert_eq!(region.max, [15.0, 40.0, 40.0]);
    assert_eq!(
        view.component_port("v_p", "y_pos"),
        Some(ResolvedPort::Face {
            origin: [12.5, 20.0, 40.0],
            normal: AxisDir::PosZ,
            u: AxisDir::NegY,
            v: AxisDir::PosX,
            extent: [40.0, 5.0],
        })
    );
    let pose = solver::resolved_pose(&r, "v_p").unwrap();
    assert_eq!(pose.apply([0.0, 0.0, 5.0]), [15.0, 20.0, 30.0]);
}

// =============================================================================================
// §7
// =============================================================================================

fn hole(id: &str, axis: &str, hole_type: &str, extra: Value) -> Value {
    let mut shape = json!({"kind": "hole", "axis": axis, "diameter": 6.6, "hole_type": hole_type});
    for (k, v) in extra.as_object().unwrap() {
        shape[k] = v.clone();
    }
    json!({"id": id, "kind": "empty", "role": "cut", "shape": shape})
}

fn dot(id: &str) -> Value {
    json!({"id": id, "kind": "dot", "shape": {"kind": "point"}})
}

fn hole_doc(h: Value, constraints: Value) -> CadDocument2 {
    let mut ks = vec![root_fixed("v_plate.origin")];
    ks.extend(constraints.as_array().unwrap().iter().cloned());
    base_doc(
        json!([plate("v_plate", 100.0, 60.0, 10.0), dot("d_h"), h]),
        Value::Array(ks),
        json!([part("p", &[("add", "v_plate"), ("cut", "e_h")])]),
    )
}

#[test]
fn hole_direction_from_side_face() {
    let doc = hole_doc(
        hole("e_h", "x", "through", json!({})),
        json!([
            {"id": "k_d", "kind": "on_face", "a": "d_h.point", "b": "v_plate.x_pos", "u": 0, "v": 0},
            {"id": "k_e", "kind": "coincident", "a": "e_h.entry", "b": "d_h.point"}
        ]),
    );
    let r = solver::solve(&doc);
    assert!(r.issues.is_empty(), "{:?}", r.issues);
    let h = &r.holes["e_h"];
    assert_eq!(h.direction, AxisDir::NegX);
    assert_eq!(h.cutter_start, [51.0, 0.0, 5.0]);
    assert_eq!(h.cutter_end, [-51.0, 0.0, 5.0]);
    let view = WorldView::new(&doc, &r);
    assert_eq!(
        view.component_port("e_h", "axis"),
        Some(ResolvedPort::Axis {
            position: [50.0, 0.0, 5.0],
            direction: AxisDir::NegX
        })
    );
    let region = view.region("e_h").unwrap();
    assert!((region.min[0] + 50.0).abs() < TOL && (region.max[0] - 50.0).abs() < TOL);
    assert!((region.min[1] + 3.3).abs() < TOL && (region.max[2] - 8.3).abs() < TOL);
    assert_eq!(view.hole_part("e_h"), Some("p"));
    assert_eq!(view.part_bbox("p").unwrap().max, [50.0, 30.0, 10.0]);
}

#[test]
fn hole_direction_conflict_and_axis_mismatch() {
    // entry directly on the top face, and via the coincident Dot on the x_pos face
    let doc = hole_doc(
        hole("e_h", "z", "through", json!({})),
        json!([
            {"id": "k_top", "kind": "on_face", "a": "e_h.entry", "b": "v_plate.top"},
            {"id": "k_d", "kind": "on_face", "a": "d_h.point", "b": "v_plate.x_pos"},
            {"id": "k_e", "kind": "coincident", "a": "d_h.point", "b": "e_h.entry"}
        ]),
    );
    let r = solver::solve(&doc);
    let i = r
        .issues
        .iter()
        .find(|i| i.code == "hole_direction_conflict")
        .unwrap_or_else(|| panic!("{:?}", r.issues));
    assert_eq!(i.target.as_deref(), Some("e_h"));
    assert_eq!(i.related, ["k_top", "k_d"]);
    // the first candidate wins: −(+z)
    assert_eq!(r.holes["e_h"].direction, AxisDir::NegZ);
    assert!(!codes(&r).contains(&"hole_axis_mismatch"));

    // z-axis hole placed on a side face: direction −x is not parallel to z
    let doc = hole_doc(
        hole("e_h", "z", "through", json!({})),
        json!([
            {"id": "k_d", "kind": "on_face", "a": "d_h.point", "b": "v_plate.x_pos"},
            {"id": "k_e", "kind": "coincident", "a": "e_h.entry", "b": "d_h.point"}
        ]),
    );
    let r = solver::solve(&doc);
    assert_eq!(codes(&r), ["hole_axis_mismatch"]);
    assert_eq!(r.issues[0].target.as_deref(), Some("e_h"));
    assert_eq!(
        r.issues[0].owner.map(|o| o.as_str()),
        Some("component_smith")
    );
}

#[test]
fn blind_counterbore_countersink_cutters() {
    let on_top = json!([
        {"id": "k_d", "kind": "on_face", "a": "d_h.point", "b": "v_plate.top", "u": 10, "v": 0},
        {"id": "k_e", "kind": "coincident", "a": "e_h.entry", "b": "d_h.point"}
    ]);
    let close = |a: [f64; 3], b: [f64; 3]| (0..3).all(|i| (a[i] - b[i]).abs() < TOL);

    let doc = hole_doc(
        hole("e_h", "z", "blind", json!({"depth": 4})),
        on_top.clone(),
    );
    let r = solver::solve(&doc);
    assert!(r.issues.is_empty(), "{:?}", r.issues);
    let h = &r.holes["e_h"];
    assert_eq!(h.direction, AxisDir::NegZ);
    assert!(
        close(h.cutter_start, [10.0, 0.0, 10.1]),
        "{:?}",
        h.cutter_start
    );
    assert!(close(h.cutter_end, [10.0, 0.0, 5.9]), "{:?}", h.cutter_end);
    let region = solver::component_region(&doc, &r, "e_h").unwrap();
    assert!(close(region.min, [6.7, -3.3, 6.0]) && close(region.max, [13.3, 3.3, 10.0]));

    let doc = hole_doc(
        hole(
            "e_h",
            "z",
            "counterbore",
            json!({"counterbore_diameter": 11, "counterbore_depth": 6.5}),
        ),
        on_top.clone(),
    );
    let r = solver::solve(&doc);
    let c = solver::hole_cutters(&doc, &r, "e_h").unwrap();
    assert!(close(c.main.start, [10.0, 0.0, 11.0]) && close(c.main.end, [10.0, 0.0, -1.0]));
    let cb = c.counterbore.unwrap();
    assert!(close(cb.start, [10.0, 0.0, 10.1]) && close(cb.end, [10.0, 0.0, 3.4]));
    assert_eq!(cb.diameter, 11.0);
    assert!(c.countersink.is_none());
    // hole region uses the largest diameter
    let region = solver::component_region(&doc, &r, "e_h").unwrap();
    assert!(close(region.min, [4.5, -5.5, 0.0]));

    let doc = hole_doc(
        hole(
            "e_h",
            "z",
            "countersink",
            json!({"countersink_diameter": 12, "countersink_angle": 90}),
        ),
        on_top,
    );
    let r = solver::solve(&doc);
    let cs = solver::hole_cutters(&doc, &r, "e_h")
        .unwrap()
        .countersink
        .unwrap();
    assert!(close(cs.start, [10.0, 0.0, 10.0]) && close(cs.end, [10.0, 0.0, 7.3]));
    assert_eq!((cs.outer_diameter, cs.inner_diameter), (12.0, 6.6));
}

// =============================================================================================
// §5.5 step 6
// =============================================================================================

#[test]
fn inside_and_clearance_checks() {
    let mut v = example_value(ENCLOSURE);
    v["constraints"].as_array_mut().unwrap().extend([
        json!({"id": "k_in_ok", "kind": "inside", "a": "e_pcb.region", "b": "v_body.cavity"}),
        json!({"id": "k_in_bad", "kind": "inside", "a": "v_lid.region", "b": "v_body.cavity"}),
        json!({"id": "k_cl_ok", "kind": "clearance", "a": "e_pcb.region", "b": "v_lid.region", "value": 1}),
        json!({"id": "k_cl_bad", "kind": "clearance", "a": "e_pcb.region", "b": "v_lid.region", "value": 2}),
    ]);
    let r = solver::solve(&doc_from(v));
    assert_eq!(codes(&r), ["inside_violated", "clearance_violated"]);
    assert_eq!(r.issues[0].target.as_deref(), Some("k_in_bad"));
    assert_eq!(r.issues[0].related, ["v_lid", "v_body"]);
    assert!(
        r.issues[0].message.contains("2 mm"),
        "{}",
        r.issues[0].message
    );
    assert_eq!(r.issues[1].target.as_deref(), Some("k_cl_bad"));
    assert!(
        r.issues[1].message.contains("1 mm apart"),
        "{}",
        r.issues[1].message
    );
}

// =============================================================================================
// derived components and robustness
// =============================================================================================

#[test]
fn extrude_from_dots_then_plate_on_top() {
    // a 20 × 10 rectangle at z = 0 (counter-clockwise from +z), extruded 5 mm; a plate mated
    // on the extrude top is placed only once the profile frame is computable
    let pts = [(-10, -5), (10, -5), (10, 5), (-10, 5)];
    let mut comps: Vec<Value> = (1..=4).map(|i| dot(&format!("d{i}"))).collect();
    comps.push(json!({"id": "l_out", "kind": "line",
        "shape": {"kind": "polyline", "points": ["d1", "d2", "d3", "d4"], "closed": true}}));
    comps.push(json!({"id": "s_prof", "kind": "plane", "shape": {"kind": "profile", "outer": "l_out", "inners": []}}));
    comps.push(json!({"id": "v_ext", "kind": "volume", "shape": {"kind": "extrude", "profile": "s_prof", "height": 5}}));
    comps.push(plate("v_top", 20.0, 10.0, 2.0));
    let mut ks: Vec<Value> = pts
        .iter()
        .enumerate()
        .map(|(i, (x, y))| {
            json!({"id": format!("k_d{}", i + 1), "kind": "fixed", "a": format!("d{}.point", i + 1),
                   "frame": {"origin": [x + 3, y, 0], "x_axis": "+x", "z_axis": "+z"}})
        })
        .collect();
    // listed before the dots are known only in the sense of document order: the mate is first
    ks.insert(
        0,
        json!({"id": "k_m", "kind": "mate", "a": "v_top.bottom", "b": "v_ext.top"}),
    );
    ks.push(json!({"id": "k_ax", "kind": "coaxial", "a": "v_top.axis_z", "b": "v_ext.axis_z"}));
    let doc = base_doc(Value::Array(comps), Value::Array(ks), json!([]));
    let r = solver::solve(&doc);
    assert!(r.issues.is_empty(), "{:?}", r.issues);
    assert_eq!(r.components["v_top"].origin, [3.0, 0.0, 5.0]);
    assert_eq!(r.components["v_ext"].origin, [3.0, 0.0, 0.0]);
    assert_eq!(
        r.ports[&pr("v_ext.top")],
        ResolvedPort::Face {
            origin: [3.0, 0.0, 5.0],
            normal: AxisDir::PosZ,
            u: AxisDir::PosX,
            v: AxisDir::PosY,
            extent: [20.0, 10.0],
        }
    );
    let view = WorldView::new(&doc, &r);
    assert_eq!(
        view.component_port("s_prof", "normal"),
        Some(ResolvedPort::Axis {
            position: [3.0, 0.0, 0.0],
            direction: AxisDir::PosZ
        })
    );
    assert_eq!(
        view.component_port("l_out", "end"),
        Some(ResolvedPort::Point {
            position: [-7.0, 5.0, 0.0]
        })
    );

    // the identity orientation satisfies the mate above; these variants require a flip. The
    // profile normal depends on Dot positions, so step 5 can only use it in the positional
    // second pass (before, the plate stayed identity and step 7 reported a false
    // `orientation_unsatisfiable`).
    let flipped = Orientation::new(AxisDir::PosX, AxisDir::NegZ).unwrap();
    for (a_port, reverse, z) in [
        // the plate's own top on the extrude top: upside down, top at z = 5
        ("v_top.top", false, 7.0),
        // clockwise outline → normal −z, the extrude grows downwards: the plate's bottom
        // (local −z) must face +z
        ("v_top.bottom", true, -5.0),
    ] {
        let mut v = serde_json::to_value(&doc).unwrap();
        v["constraints"][0]["a"] = json!(a_port);
        if reverse {
            let pts = v["components"][4]["shape"]["points"]
                .as_array_mut()
                .unwrap();
            pts.reverse();
        }
        let r = solver::solve(&doc_from(v));
        assert!(r.issues.is_empty(), "{a_port}: {:?}", r.issues);
        assert_eq!(
            r.components["v_top"].orientation(),
            Some(flipped),
            "{a_port}"
        );
        assert_eq!(r.components["v_top"].origin, [3.0, 0.0, z], "{a_port}");
    }
}

#[test]
fn invalid_input_never_panics_and_is_skipped() {
    let mut v = example_value(PLATE);
    v["constraints"].as_array_mut().unwrap().extend([
        json!({"id": "k_dangle", "kind": "coincident", "a": "zz_none.point", "b": "d_h1.point"}),
        json!({"id": "k_unknown", "kind": "coincident", "a": "v_plate.nope", "b": "d_h1.point"}),
        json!({"id": "k_mismatch", "kind": "coaxial", "a": "v_plate.top", "b": "e_h1.axis"}),
        json!({"id": "k_badfix", "kind": "fixed", "a": "v_plate.top",
               "frame": {"origin": [5, 5, 5], "x_axis": "+x", "z_axis": "+z"}}),
        json!({"id": "k_ofx", "kind": "mate", "a": "e_of.top", "b": "v_plate.bottom"}),
    ]);
    let comps = v["components"].as_array_mut().unwrap();
    // reference cycle between derived components, an `of` of a non-Volume, a duplicate id
    comps.push(json!({"id": "s_cyc", "kind": "plane", "shape": {"kind": "section", "source": "v_cyc", "offset": 1}}));
    comps.push(json!({"id": "v_cyc", "kind": "volume", "shape": {"kind": "extrude", "profile": "s_cyc", "height": 1}}));
    comps.push(json!({"id": "e_of", "kind": "empty", "role": "keep_out", "shape": {"kind": "of", "source": "d_h1"}}));
    comps.push(plate("v_plate", 1.0, 1.0, 1.0));
    let doc = doc_from(v);
    let r = solver::solve(&doc);
    // the valid part of the document still solves exactly as before
    let (_, expected) = load(PLATE);
    let actual = serde_json::to_value(&r).unwrap();
    assert_close(
        &actual["components"]["d_h3"],
        &expected["components"]["d_h3"],
        "d_h3",
    );
    assert_close(&actual["holes"], &expected["holes"], "holes");
    assert!(r.issues.is_empty(), "{:?}", r.issues);
    let view = WorldView::new(&doc, &r);
    assert!(view.component_port("v_cyc", "top").is_none());
    assert!(view.component_port("e_of", "top").is_none());
    assert!(view.port(&pr("zz_none.point")).is_none());
}
