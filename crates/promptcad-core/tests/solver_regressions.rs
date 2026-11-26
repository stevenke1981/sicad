//! Regression tests for reviewer-confirmed solver defects:
//!
//! 1. exponential evaluation time on `section` / `extrude` chains;
//! 2. orientation not propagating through derived Volumes (§5.4 steps 4–5);
//! 3. holes ignoring the rotation of a derived `body[0]` (§5.4 step 3, §7.1 step 3);
//!
//! plus the (documented, unchanged) behaviour of `inside` / `clearance` on a region that does
//! not resolve.

use std::time::{Duration, Instant};

use promptcad_core::model::geom::{AxisDir, Orientation};
use promptcad_core::model::resolved::{Resolved, ResolvedPort};
use promptcad_core::model::{CadDocument2, PortRef, Severity};
use promptcad_core::solver::{self, WorldView};
use promptcad_core::validate::{validate, ValidateOptions};
use serde_json::{json, Value};

fn doc(components: Value, constraints: Value, parts: Value) -> CadDocument2 {
    let root = parts
        .as_array()
        .and_then(|p| (p.len() == 1).then(|| p[0]["id"].clone()))
        .unwrap_or(Value::Null);
    CadDocument2::from_value(json!({
        "schema_version": "2.0", "name": "t", "source_prompt": "t", "unit": "mm",
        "revision": 1, "parameters": [], "bindings": [],
        "components": components, "constraints": constraints, "parts": parts,
        "assemblies": [], "root": root, "assumptions": [], "notes": [],
        "planner": {"planner": "test", "confidence": 1, "review_required": false}
    }))
    .unwrap_or_else(|e| panic!("{e}"))
}

fn fixed(id: &str, port: &str, origin: [f64; 3], x: &str, z: &str) -> Value {
    json!({"id": id, "kind": "fixed", "a": port,
           "frame": {"origin": origin, "x_axis": x, "z_axis": z}})
}

fn plate(id: &str, l: f64, w: f64, t: f64) -> Value {
    json!({"id": id, "kind": "volume", "shape": {"kind": "plate", "length": l, "width": w, "thickness": t}})
}

fn rect(id: &str, l: f64, w: f64) -> Value {
    json!({"id": id, "kind": "plane", "shape": {"kind": "rect", "length": l, "width": w}})
}

fn extrude(id: &str, profile: &str, h: f64) -> Value {
    json!({"id": id, "kind": "volume", "shape": {"kind": "extrude", "profile": profile, "height": h}})
}

fn section(id: &str, source: &str, offset: f64) -> Value {
    json!({"id": id, "kind": "plane", "shape": {"kind": "section", "source": source, "offset": offset}})
}

fn dot(id: &str) -> Value {
    json!({"id": id, "kind": "dot", "shape": {"kind": "point"}})
}

fn through_hole(id: &str, axis: &str) -> Value {
    json!({"id": id, "kind": "empty", "role": "cut", "shape": {
        "kind": "hole", "axis": axis, "diameter": 5, "hole_type": "through", "depth": null,
        "thread": null, "counterbore_diameter": null, "counterbore_depth": null,
        "countersink_diameter": null, "countersink_angle": null}})
}

fn part(id: &str, body: &[(&str, &str)]) -> Value {
    let body: Vec<Value> = body
        .iter()
        .map(|(op, c)| json!({"op": op, "component": c}))
        .collect();
    json!({"id": id, "name": id, "quantity": 1, "source": "made", "body": body})
}

fn codes(r: &Resolved) -> Vec<&str> {
    r.issues.iter().map(|i| i.code.as_str()).collect()
}

fn orientation(r: &Resolved, id: &str) -> Orientation {
    r.components[id].orientation().unwrap()
}

fn o(x: AxisDir, z: AxisDir) -> Orientation {
    Orientation::new(x, z).unwrap()
}

/// Four Dots fixed at the corners of a 20 × 10 rectangle centered on `(3, 0, 0)`, joined by a
/// closed polyline (`ccw` seen from `+z` → normal `+z`, otherwise `−z`), a `profile` and an
/// `extrude` of height 5.
fn dot_extrude(ccw: bool) -> (Vec<Value>, Vec<Value>) {
    let mut pts = [(-10.0, -5.0), (10.0, -5.0), (10.0, 5.0), (-10.0, 5.0)];
    if !ccw {
        pts.reverse();
    }
    let mut comps: Vec<Value> = (1..=4).map(|i| dot(&format!("d{i}"))).collect();
    comps.push(json!({"id": "l_out", "kind": "line",
        "shape": {"kind": "polyline", "points": ["d1", "d2", "d3", "d4"], "closed": true}}));
    comps.push(json!({"id": "s_prof", "kind": "plane",
        "shape": {"kind": "profile", "outer": "l_out", "inners": []}}));
    comps.push(extrude("v_ext", "s_prof", 5.0));
    let ks = pts
        .iter()
        .enumerate()
        .map(|(i, (x, y))| {
            fixed(
                &format!("k_d{}", i + 1),
                &format!("d{}.point", i + 1),
                [x + 3.0, *y, 0.0],
                "+x",
                "+z",
            )
        })
        .collect();
    (comps, ks)
}

// =============================================================================================
// 1. section / extrude chains are evaluated in linear time
// =============================================================================================

/// `r0` fixed; `e0 = extrude(r0, 10)`; `s_i = section(e_{i−1}, 5)`; `e_i = extrude(s_i, 10)`.
fn chain(n: usize) -> CadDocument2 {
    let mut comps = vec![rect("r0", 10.0, 10.0), extrude("e0", "r0", 10.0)];
    for i in 1..=n {
        comps.push(section(&format!("s{i}"), &format!("e{}", i - 1), 5.0));
        comps.push(extrude(&format!("e{i}"), &format!("s{i}"), 10.0));
    }
    let body: Vec<(String, String)> = (0..=n)
        .map(|i| ("add".to_owned(), format!("e{i}")))
        .collect();
    let body: Vec<(&str, &str)> = body.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    doc(
        Value::Array(comps),
        json!([fixed("k_root", "r0.center", [0.0; 3], "+x", "+z")]),
        json!([part("p", &body)]),
    )
}

#[test]
fn section_extrude_chain_at_the_component_cap_is_fast_and_exact() {
    // 2 + 2·127 = 256 components: the longest chain the component cap allows. The derivation
    // graph has no depth limit any more, so every level resolves (the old 64-level recursion
    // guard silently dropped the geometry of the deep end of such chains).
    let n = 127;
    let d = chain(n);
    assert_eq!(d.components.len(), 256);

    let start = Instant::now();
    let r = solver::solve(&d);
    let v = validate(&d, &ValidateOptions::default());
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_secs(5),
        "solve + validate of a {n}-level chain took {elapsed:?}"
    );

    assert!(r.issues.is_empty(), "{:?}", r.issues);
    assert_eq!(r, v.resolved);
    assert!(
        v.report
            .issues
            .iter()
            .all(|i| i.severity != Severity::Error),
        "{:?}",
        v.report.issues
    );
    // every component (derived ones included) has a pose
    assert_eq!(r.components.len(), 256);
    for i in 0..=n {
        let e = &r.components[format!("e{i}").as_str()];
        assert_eq!(e.origin, [0.0, 0.0, 5.0 * i as f64], "e{i}");
        assert_eq!(e.orientation(), Some(Orientation::IDENTITY), "e{i}");
    }
    let view = WorldView::new(&d, &r);
    assert_eq!(
        view.component_port(&format!("e{n}"), "top"),
        Some(ResolvedPort::Face {
            origin: [0.0, 0.0, 5.0 * n as f64 + 10.0],
            normal: AxisDir::PosZ,
            u: AxisDir::PosX,
            v: AxisDir::PosY,
            extent: [10.0, 10.0],
        })
    );
    let bb = view.part_bbox("p").unwrap();
    assert_eq!(bb.min, [-5.0, -5.0, 0.0]);
    assert_eq!(bb.max, [5.0, 5.0, 5.0 * n as f64 + 10.0]);
}

#[test]
fn rotated_chain_follows_the_root_frame() {
    // the same chain under a rotated root: each level steps 5 mm along the root's local z
    // (world +x); orientation knowledge is static, so ports are known per axis
    let mut v = serde_json::to_value(chain(40)).unwrap();
    v["constraints"][0]["frame"] = json!({"origin": [1, 2, 3], "x_axis": "+y", "z_axis": "+x"});
    let d = CadDocument2::from_value(v).unwrap();
    let r = solver::solve(&d);
    assert!(r.issues.is_empty(), "{:?}", r.issues);
    let e = &r.components["e40"];
    assert_eq!(e.origin, [201.0, 2.0, 3.0]);
    assert_eq!(e.orientation(), Some(o(AxisDir::PosY, AxisDir::PosX)));
}

// =============================================================================================
// 2. orientation propagates through derived Volumes
// =============================================================================================

#[test]
fn mate_against_extrude_of_rect_flips_the_lid() {
    // reviewer repro: rect r fixed (identity); v_x = extrude(r, 5); plate v_lid; mate
    // v_lid.top ↔ v_x.top. v_lid must get the first canonical orientation satisfying the
    // mate, (+x, −z), exactly like against a plain 5 mm plate — not identity + a false
    // `orientation_unsatisfiable`.
    let lid = |base: Value, extra: Vec<Value>| {
        let mut ks = vec![
            fixed("k1", "r.center", [0.0; 3], "+x", "+z"),
            json!({"id": "k5", "kind": "mate", "a": "v_lid.top", "b": "v_x.top"}),
        ];
        ks.extend(extra);
        let mut comps = vec![rect("r", 10.0, 10.0), base, plate("v_lid", 10.0, 10.0, 2.0)];
        if comps[1]["shape"]["kind"] == "plate" {
            // the plain-plate twin is rooted by itself instead of by the rect
            comps.remove(0);
            ks[0] = fixed("k1", "v_x.origin", [0.0; 3], "+x", "+z");
        }
        doc(
            Value::Array(comps),
            Value::Array(ks),
            json!([part("p", &[("add", "v_x")]), part("q", &[("add", "v_lid")])]),
        )
    };
    let flipped = o(AxisDir::PosX, AxisDir::NegZ);

    // the bare repro: only the translation along x/y is left open
    let r = solver::solve(&lid(extrude("v_x", "r", 5.0), vec![]));
    assert_eq!(codes(&r), ["under_constrained"], "{:?}", r.issues);
    assert_eq!(orientation(&r, "v_lid"), flipped);
    assert_eq!(r.components["v_lid"].origin[2], 7.0);

    // fully constrained, and identical to the plain-plate twin
    let coax =
        || vec![json!({"id": "k6", "kind": "coaxial", "a": "v_lid.axis_z", "b": "v_x.axis_z"})];
    let r = solver::solve(&lid(extrude("v_x", "r", 5.0), coax()));
    assert!(r.issues.is_empty(), "{:?}", r.issues);
    let twin = solver::solve(&lid(plate("v_x", 10.0, 10.0, 5.0), coax()));
    assert!(twin.issues.is_empty(), "{:?}", twin.issues);
    for r in [&r, &twin] {
        assert_eq!(orientation(r, "v_lid"), flipped);
        assert_eq!(r.components["v_lid"].origin, [0.0, 0.0, 7.0]);
    }
}

#[test]
fn extrude_of_section_of_rotated_plate_drives_step_5() {
    // v_base rotated (x +y, z +x); s = section(v_base, 10); e = extrude(s, 5): e.top points
    // along v_base's local z = world +x, so v_cap.top must be −x → first canonical
    // orientation with z_axis −x is (+y, −x)
    let d = doc(
        json!([
            plate("v_base", 40.0, 30.0, 10.0),
            section("s", "v_base", 10.0),
            extrude("e", "s", 5.0),
            plate("v_cap", 10.0, 10.0, 2.0)
        ]),
        json!([
            fixed("k0", "v_base.origin", [0.0; 3], "+y", "+x"),
            {"id": "k1", "kind": "mate", "a": "v_cap.top", "b": "e.top"},
            {"id": "k2", "kind": "coaxial", "a": "v_cap.axis_z", "b": "e.axis_z"}
        ]),
        json!([
            part("p", &[("add", "v_base"), ("add", "e")]),
            part("q", &[("add", "v_cap")])
        ]),
    );
    let r = solver::solve(&d);
    assert!(r.issues.is_empty(), "{:?}", r.issues);
    assert_eq!(orientation(&r, "v_cap"), o(AxisDir::PosY, AxisDir::NegX));
    // e.top at world x = 10 + 5; the flipped cap's top (local z = 2) lies on it
    assert_eq!(r.components["v_cap"].origin, [17.0, 0.0, 0.0]);
}

#[test]
fn unfixed_rect_is_oriented_through_its_extrude() {
    // the rect has no frame of its own: step 5 orients it (the root of the extrude's
    // orientation) so that e.top mates v_base.top. Translations never flow into a derived
    // end (§5.5 step 3), so the rect is placed through a Dot; the mate and coaxial are then
    // satisfied checks.
    let d = doc(
        json!([
            plate("v_base", 40.0, 30.0, 10.0),
            rect("r", 10.0, 10.0),
            extrude("e", "r", 5.0),
            dot("d_r")
        ]),
        json!([
            fixed("k0", "v_base.origin", [0.0; 3], "+x", "+z"),
            {"id": "k1", "kind": "mate", "a": "e.top", "b": "v_base.top"},
            {"id": "k2", "kind": "coaxial", "a": "e.axis_z", "b": "v_base.axis_z"},
            fixed("k3", "d_r.point", [0.0, 0.0, 15.0], "+x", "+z"),
            {"id": "k4", "kind": "coincident", "a": "r.center", "b": "d_r.point"}
        ]),
        json!([part("p", &[("add", "v_base")]), part("q", &[("add", "e")])]),
    );
    let r = solver::solve(&d);
    assert_eq!(
        codes(&r),
        ["redundant_constraint", "redundant_constraint"],
        "{:?}",
        r.issues
    );
    assert_eq!(orientation(&r, "r"), o(AxisDir::PosX, AxisDir::NegZ));
    assert_eq!(orientation(&r, "e"), o(AxisDir::PosX, AxisDir::NegZ));
    // e.top (local z = 5) lies on z = 10 with the rect at z = 15
    assert_eq!(
        r.ports[&"e.top".parse::<PortRef>().unwrap()],
        ResolvedPort::Face {
            origin: [0.0, 0.0, 10.0],
            normal: AxisDir::NegZ,
            u: AxisDir::PosX,
            v: AxisDir::NegY,
            extent: [10.0, 10.0],
        }
    );
}

#[test]
fn position_dependent_extrude_flips_a_plate_in_the_second_pass() {
    // the profile normal comes from Dot positions, so the first pass cannot use it; the second
    // pass evaluates it on the solved positions and orients the plate. Before the fix the
    // plate stayed identity and step 7 reported a false `orientation_unsatisfiable`.
    let flipped = o(AxisDir::PosX, AxisDir::NegZ);
    for (ccw, a_port, z) in [(true, "v_top.top", 7.0), (false, "v_top.bottom", -5.0)] {
        let (mut comps, mut ks) = dot_extrude(ccw);
        comps.push(plate("v_top", 20.0, 10.0, 2.0));
        ks.insert(
            0,
            json!({"id": "k_m", "kind": "mate", "a": a_port, "b": "v_ext.top"}),
        );
        ks.push(json!({"id": "k_ax", "kind": "coaxial", "a": "v_top.axis_z", "b": "v_ext.axis_z"}));
        let r = solver::solve(&doc(Value::Array(comps), Value::Array(ks), json!([])));
        assert!(r.issues.is_empty(), "ccw={ccw}: {:?}", r.issues);
        assert_eq!(orientation(&r, "v_top"), flipped, "ccw={ccw}");
        assert_eq!(r.components["v_top"].origin, [3.0, 0.0, z], "ccw={ccw}");
    }
}

#[test]
fn genuinely_unsatisfiable_requirement_on_a_derived_volume_is_still_reported() {
    // v_lid is oriented by the mate with v_x (extrude of the fixed rect); a second, fixed
    // plate demands a contradictory flush → reported once, by step 5
    let d = doc(
        json!([
            rect("r", 10.0, 10.0),
            extrude("v_x", "r", 5.0),
            plate("v_lid", 10.0, 10.0, 2.0)
        ]),
        json!([
            fixed("k1", "r.center", [0.0; 3], "+x", "+z"),
            {"id": "k2", "kind": "mate", "a": "v_lid.top", "b": "v_x.top"},
            {"id": "k3", "kind": "flush", "a": "v_lid.top", "b": "v_x.top"},
            {"id": "k4", "kind": "coaxial", "a": "v_lid.axis_z", "b": "v_x.axis_z"}
        ]),
        json!([part("p", &[("add", "v_x")]), part("q", &[("add", "v_lid")])]),
    );
    let r = solver::solve(&d);
    assert_eq!(
        codes(&r),
        ["orientation_unsatisfiable", "redundant_constraint"],
        "{:?}",
        r.issues
    );
    assert_eq!(r.issues[0].target.as_deref(), Some("v_lid"));
    assert_eq!(r.issues[0].related, ["k2", "k3", "k4"]);
}

// =============================================================================================
// 3. holes follow a derived body[0]
// =============================================================================================

/// Reviewer repro, with the hole positioned the way migrated documents do it (a fixed Dot and
/// `coincident`), so that §5.4 step 3 applies.
fn rotated_extrude_with_hole() -> CadDocument2 {
    doc(
        json!([
            rect("r", 40.0, 40.0),
            extrude("v_x", "r", 20.0),
            dot("d_h"),
            through_hole("e1", "z")
        ]),
        json!([
            fixed("k1", "r.center", [0.0; 3], "+y", "+x"),
            fixed("k2", "d_h.point", [20.0, 0.0, 0.0], "+x", "+z"),
            {"id": "k3", "kind": "coincident", "a": "e1.entry", "b": "d_h.point"}
        ]),
        json!([part("p", &[("add", "v_x"), ("cut", "e1")])]),
    )
}

#[test]
fn hole_follows_rotated_extrude_body() {
    let d = rotated_extrude_with_hole();
    let r = solver::solve(&d);
    assert!(r.issues.is_empty(), "{:?}", r.issues);
    // the hole takes v_x's (= r's) orientation: local z → world +x
    assert_eq!(orientation(&r, "e1"), o(AxisDir::PosY, AxisDir::PosX));
    let h = &r.holes["e1"];
    // §7.1 step 3: −(R · +z) = −x
    assert_eq!(h.direction, AxisDir::NegX);
    // part bbox x ∈ [0, 20]; the cutter spans it along x ± 1.0 at the entry
    assert_eq!(h.cutter_start, [21.0, 0.0, 0.0]);
    assert_eq!(h.cutter_end, [-1.0, 0.0, 0.0]);

    let v = validate(&d, &ValidateOptions::default());
    assert!(
        v.report
            .issues
            .iter()
            .all(|i| i.severity != Severity::Error && i.code != "hole_axis_mismatch"),
        "{:?}",
        v.report.issues
    );
    let view = WorldView::new(&d, &v.resolved);
    assert_eq!(view.hole_direction("e1"), Some(AxisDir::NegX));
    assert_eq!(
        view.component_port("e1", "region"),
        Some(ResolvedPort::Region {
            min: [0.0, -2.5, -2.5],
            max: [20.0, 2.5, 2.5]
        })
    );
}

#[test]
fn hole_with_its_own_fixed_keeps_the_fixed_frame() {
    // documented decision (§5.4 step 1 precedes step 3 and is unconditional): a hole fixed
    // with an identity frame keeps it even though body[0] is rotated
    let mut v = serde_json::to_value(rotated_extrude_with_hole()).unwrap();
    let ks = v["constraints"].as_array_mut().unwrap();
    ks.truncate(1);
    ks.push(fixed("k2", "e1.entry", [20.0, 0.0, 0.0], "+x", "+z"));
    v["components"].as_array_mut().unwrap().remove(2); // the now unused Dot d_h
    let d = CadDocument2::from_value(v).unwrap();
    let r = solver::solve(&d);
    assert!(r.issues.is_empty(), "{:?}", r.issues);
    assert_eq!(orientation(&r, "e1"), Orientation::IDENTITY);
    assert_eq!(r.holes["e1"].direction, AxisDir::NegZ);
    assert_eq!(r.holes["e1"].cutter_start, [20.0, 0.0, 21.0]);
    assert_eq!(r.holes["e1"].cutter_end, [20.0, 0.0, -21.0]);
}

#[test]
fn hole_follows_position_dependent_extrude_body() {
    // body[0] = extrude of a clockwise Dot profile (normal −z, orientation (+x, −z)): its
    // orientation is only known after translation, so the hole takes it in the second pass
    let (mut comps, mut ks) = dot_extrude(false);
    comps.push(dot("d_h"));
    comps.push(through_hole("e_h", "z"));
    ks.push(fixed("k_h", "d_h.point", [3.0, 0.0, -5.0], "+x", "+z"));
    ks.push(json!({"id": "k_e", "kind": "coincident", "a": "e_h.entry", "b": "d_h.point"}));
    let d = doc(
        Value::Array(comps),
        Value::Array(ks),
        json!([part("p", &[("add", "v_ext"), ("cut", "e_h")])]),
    );
    let r = solver::solve(&d);
    assert!(r.issues.is_empty(), "{:?}", r.issues);
    assert_eq!(orientation(&r, "e_h"), o(AxisDir::PosX, AxisDir::NegZ));
    let h = &r.holes["e_h"];
    // entry on the extrude's end face (z = −5), cutting back into the material along +z
    assert_eq!(h.direction, AxisDir::PosZ);
    assert_eq!(h.cutter_start, [3.0, 0.0, -6.0]);
    assert_eq!(h.cutter_end, [3.0, 0.0, 1.0]);
}

// =============================================================================================
// inside / clearance on a region that does not resolve
// =============================================================================================

#[test]
fn check_on_unresolvable_region_yields_no_issue() {
    // a hole cut by no part has no `region` (it spans to the owning part's bounding box);
    // `inside` / `clearance` against it are silently skipped by §5.5 step 6 — documented in
    // the solver module docs, no new code is invented
    let d = doc(
        json!([plate("v_a", 10.0, 10.0, 10.0), through_hole("e_free", "z")]),
        json!([
            fixed("k0", "v_a.origin", [0.0; 3], "+x", "+z"),
            fixed("k1", "e_free.entry", [100.0, 0.0, 0.0], "+x", "+z"),
            {"id": "k_in", "kind": "inside", "a": "e_free.region", "b": "v_a.region"},
            {"id": "k_cl", "kind": "clearance", "a": "e_free.region", "b": "v_a.region", "value": 1000}
        ]),
        json!([part("p", &[("add", "v_a")])]),
    );
    let r = solver::solve(&d);
    assert!(r.issues.is_empty(), "{:?}", r.issues);
    let view = WorldView::new(&d, &r);
    assert!(view.component_port("e_free", "region").is_none());
}
