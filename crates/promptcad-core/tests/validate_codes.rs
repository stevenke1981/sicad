//! One focused positive test per issue code the validator emits, plus negative controls.

use std::path::PathBuf;

use promptcad_core::canonical::parse_example;
use promptcad_core::model::{CadDocument2, Code, Layer, Role, Severity, ValidationIssue};
use promptcad_core::validate::{
    blocking_issues, compute_root, validate, ValidateOptions, Validation, SCHEMA_INVALID,
};
use serde_json::{json, Value};

// ---- builders -------------------------------------------------------------------------------

fn plate(id: &str, l: f64, w: f64, t: f64) -> Value {
    json!({"id": id, "kind": "volume", "shape": {"kind": "plate", "length": l, "width": w, "thickness": t}})
}

fn cylinder(id: &str, d: f64, h: f64) -> Value {
    json!({"id": id, "kind": "volume", "shape": {"kind": "cylinder", "diameter": d, "height": h}})
}

fn empty_box(id: &str, role: &str, l: f64, w: f64, h: f64) -> Value {
    json!({"id": id, "kind": "empty", "role": role, "shape": {"kind": "box", "length": l, "width": w, "height": h}})
}

fn empty_cyl(id: &str, role: &str, d: f64, h: f64) -> Value {
    json!({"id": id, "kind": "empty", "role": role, "shape": {"kind": "cylinder", "diameter": d, "height": h}})
}

fn dot(id: &str) -> Value {
    json!({"id": id, "kind": "dot", "shape": {"kind": "point"}})
}

fn hole(id: &str, d: f64, hole_type: &str, depth: Option<f64>) -> Value {
    json!({"id": id, "kind": "empty", "role": "cut", "shape": {
        "kind": "hole", "axis": "z", "diameter": d, "hole_type": hole_type, "depth": depth,
        "thread": null, "counterbore_diameter": null, "counterbore_depth": null,
        "countersink_diameter": null, "countersink_angle": null}})
}

fn fixed(id: &str, port: &str, origin: [f64; 3]) -> Value {
    json!({"id": id, "kind": "fixed", "a": port, "frame": {"origin": origin, "x_axis": "+x", "z_axis": "+z"}})
}

fn fixed_rot(id: &str, port: &str, origin: [f64; 3], x: &str, z: &str) -> Value {
    json!({"id": id, "kind": "fixed", "a": port, "frame": {"origin": origin, "x_axis": x, "z_axis": z}})
}

fn k(id: &str, kind: &str, a: &str, b: &str) -> Value {
    json!({"id": id, "kind": kind, "a": a, "b": b})
}

fn on_face(id: &str, a: &str, b: &str, u: f64, v: f64) -> Value {
    json!({"id": id, "kind": "on_face", "a": a, "b": b, "u": u, "v": v})
}

fn part(id: &str, body: &[(&str, &str)]) -> Value {
    let body: Vec<Value> = body
        .iter()
        .map(|(op, c)| json!({"op": op, "component": c}))
        .collect();
    json!({"id": id, "name": id, "quantity": 1, "source": "made", "body": body})
}

fn asm(id: &str, children: &[&str]) -> Value {
    json!({"id": id, "name": id, "children": children})
}

fn doc(components: Value, constraints: Value, parts: Value, assemblies: Value) -> Value {
    json!({
        "schema_version": "2.0",
        "source_prompt": "t",
        "revision": 3,
        "components": components,
        "constraints": constraints,
        "parts": parts,
        "assemblies": assemblies,
        "planner": {"planner": "t"}
    })
}

/// A 100×100×10 plate `v_a` fixed at the origin, part `p_a`, `root = p_a`.
fn base() -> Value {
    let mut d = doc(
        json!([plate("v_a", 100.0, 100.0, 10.0)]),
        json!([fixed("k_a", "v_a.origin", [0.0; 3])]),
        json!([part("p_a", &[("add", "v_a")])]),
        json!([]),
    );
    d["root"] = json!("p_a");
    d
}

fn push(v: &mut Value, key: &str, item: Value) {
    v[key].as_array_mut().unwrap().push(item);
}

fn parse(v: Value) -> CadDocument2 {
    serde_json::from_value(v).unwrap()
}

fn run(v: Value) -> Validation {
    run_with(v, true, None)
}

fn run_with(v: Value, completed: bool, submitter: Option<Role>) -> Validation {
    validate(
        &parse(v),
        &ValidateOptions {
            completed,
            submitter,
        },
    )
}

fn with_code(v: &Validation, code: Code) -> Vec<&ValidationIssue> {
    v.report
        .issues
        .iter()
        .filter(|i| i.code == code.as_str())
        .collect()
}

/// Exactly one issue with `code`; returns it.
fn one(v: &Validation, code: Code) -> ValidationIssue {
    let found = with_code(v, code);
    assert_eq!(found.len(), 1, "{code}: {:#?}", v.report.issues);
    let i = found[0].clone();
    assert_eq!(i.severity, code.default_severity(), "{code}");
    assert_eq!(i.layer, Layer::Analytic);
    i
}

fn none(v: &Validation, code: Code) {
    assert!(
        with_code(v, code).is_empty(),
        "{code}: {:#?}",
        v.report.issues
    );
}

fn codes(v: &Validation) -> Vec<&str> {
    v.report.issues.iter().map(|i| i.code.as_str()).collect()
}

fn example(name: &str) -> Value {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/cad2")
        .join(name);
    let doc = parse_example(&std::fs::read_to_string(p).unwrap())
        .unwrap()
        .0;
    serde_json::to_value(doc).unwrap()
}

#[test]
fn negative_control_base_is_clean() {
    let v = run(base());
    assert!(v.report.issues.is_empty(), "{:#?}", v.report.issues);
    assert!(v.report.valid && !v.report.review_required);
    assert!(v.resolved.components.contains_key("v_a"));
}

// ---- schema ---------------------------------------------------------------------------------

#[test]
fn schema_violations_short_circuit() {
    let mut d = base();
    d["components"][0]["shape"]["thickness"] = json!(-1.0);
    push(
        &mut d,
        "constraints",
        k("k_bad", "coincident", "v_x.origin", "v_a.origin"),
    );
    let v = run_with(d, true, Some(Role::ComponentSmith));
    assert_eq!(codes(&v), [SCHEMA_INVALID]);
    let i = &v.report.issues[0];
    assert_eq!(i.severity, Severity::Error);
    assert_eq!(i.target.as_deref(), Some("/components/0/shape/thickness"));
    assert_eq!(i.owner, Some(Role::ComponentSmith));
    assert!(!v.report.valid);
    assert!(v.resolved.components.is_empty() && v.resolved.issues.is_empty());
    assert_eq!(v.resolved.revision, 3);
    assert_eq!(blocking_issues(&v.report).len(), 1);
}

#[test]
fn assembly_depth_over_four_is_a_schema_violation() {
    let mut d = base();
    d["assemblies"] = json!([
        asm("a1", &["a2"]),
        asm("a2", &["a3"]),
        asm("a3", &["a4"]),
        asm("a4", &["a5"]),
        asm("a5", &["p_a"])
    ]);
    d["root"] = json!("a1");
    let v = run(d.clone());
    assert_eq!(codes(&v), [SCHEMA_INVALID]);
    assert!(v.report.issues[0].message.contains("depth"));
    // depth 4 is fine
    d["assemblies"] = json!([
        asm("a1", &["a2"]),
        asm("a2", &["a3"]),
        asm("a3", &["a4"]),
        asm("a4", &["p_a"])
    ]);
    let v = run(d);
    assert!(v.report.valid, "{:#?}", v.report.issues);
}

// ---- structure and references ---------------------------------------------------------------

#[test]
fn duplicate_id_n1_and_n2() {
    let mut d = base();
    push(&mut d, "constraints", fixed("v_a", "v_a.origin", [0.0; 3]));
    let v = run_with(d, false, Some(Role::Joiner));
    let i = one(&v, Code::DuplicateId);
    assert_eq!(i.target.as_deref(), Some("v_a"));
    assert_eq!(i.owner, Some(Role::Joiner));
    // gating: a duplicate N1 id skips the solver and the design rules
    assert!(v.resolved.components.is_empty());

    let mut d = base();
    push(&mut d, "assemblies", asm("p_a", &["p_a"]));
    let v = run(d);
    assert!(with_code(&v, Code::DuplicateId)
        .iter()
        .any(|i| i.message.contains("N2")));
}

#[test]
fn duplicate_parameter_name_comes_from_bindings() {
    let mut d = base();
    d["parameters"] = json!([{"name": "len", "value": 1}, {"name": "len", "value": 2}]);
    let v = run(d);
    one(&v, Code::DuplicateId);
}

#[test]
fn dangling_references_everywhere() {
    // shape source
    let mut d = base();
    push(
        &mut d,
        "components",
        json!({"id": "l_s", "kind": "line", "shape": {"kind": "segment", "start": "d_x", "end": "d_y"}}),
    );
    let v = run(d);
    assert_eq!(with_code(&v, Code::DanglingReference).len(), 2);
    // constraint port
    let mut d = base();
    push(
        &mut d,
        "constraints",
        k("k_c", "coincident", "d_x.point", "v_a.origin"),
    );
    let i = one(&run(d), Code::DanglingReference);
    assert_eq!(i.target.as_deref(), Some("k_c"));
    assert_eq!(i.related, ["d_x"]);
    // body
    let mut d = base();
    d["parts"][0]["body"] =
        json!([{"op": "add", "component": "v_a"}, {"op": "cut", "component": "e_x"}]);
    assert_eq!(
        one(&run(d), Code::DanglingReference).target.as_deref(),
        Some("p_a")
    );
    // children and root
    let mut d = base();
    d["assemblies"] = json!([asm("a_main", &["p_a", "p_x"])]);
    d["root"] = json!("a_main");
    assert_eq!(one(&run(d), Code::DanglingReference).related, ["p_x"]);
    let mut d = base();
    d["root"] = json!("p_zz");
    let v = run(d);
    one(&v, Code::DanglingReference);
    one(&v, Code::InvalidRoot);
    // root problems do not gate the solver
    assert!(v.resolved.components.contains_key("v_a"));
}

#[test]
fn unknown_port_type_mismatch_invalid_fixed_target() {
    let mut d = base();
    push(&mut d, "components", dot("d_p"));
    push(
        &mut d,
        "constraints",
        k("k_u", "coincident", "d_p.point", "v_a.nope"),
    );
    assert_eq!(
        one(&run(d), Code::UnknownPort).target.as_deref(),
        Some("k_u")
    );

    let mut d = base();
    push(&mut d, "components", dot("d_p"));
    push(
        &mut d,
        "constraints",
        k("k_m", "coincident", "d_p.point", "v_a.top"),
    );
    one(&run(d), Code::PortTypeMismatch);

    let mut d = base();
    d["constraints"] = json!([fixed("k_a", "v_a.center", [0.0; 3])]);
    one(&run(d), Code::InvalidFixedTarget);
}

#[test]
fn invalid_promotion_and_open_profile() {
    let mut d = base();
    push(
        &mut d,
        "components",
        json!({"id": "l_s", "kind": "line", "shape": {"kind": "segment", "start": "v_a", "end": "v_a"}}),
    );
    assert_eq!(with_code(&run(d), Code::InvalidPromotion).len(), 2);

    let mut d = base();
    push(&mut d, "components", dot("d_1"));
    push(&mut d, "components", dot("d_2"));
    push(
        &mut d,
        "components",
        json!({"id": "l_s", "kind": "line", "shape": {"kind": "segment", "start": "d_1", "end": "d_2"}}),
    );
    push(
        &mut d,
        "components",
        json!({"id": "p_f", "kind": "plane", "shape": {"kind": "profile", "outer": "l_s", "inners": []}}),
    );
    let v = run(d);
    one(&v, Code::OpenProfile);
    none(&v, Code::InvalidPromotion);

    // of / section / extrude / revolve sources of the wrong kind
    for (shape, field) in [
        (json!({"kind": "of", "source": "d_1"}), "empty"),
        (
            json!({"kind": "section", "source": "d_1", "offset": 0}),
            "plane",
        ),
        (
            json!({"kind": "extrude", "profile": "d_1", "height": 5}),
            "volume",
        ),
    ] {
        let mut d = base();
        push(&mut d, "components", dot("d_1"));
        let mut c = json!({"id": "c_x", "kind": field, "shape": shape});
        if field == "empty" {
            c["role"] = json!("reserve");
        }
        push(&mut d, "components", c);
        one(&run(d), Code::InvalidPromotion);
    }
}

#[test]
fn reference_cycles_in_shapes_and_assemblies() {
    let mut d = base();
    push(
        &mut d,
        "components",
        json!({"id": "p_s", "kind": "plane", "shape": {"kind": "section", "source": "v_x", "offset": 1}}),
    );
    push(
        &mut d,
        "components",
        json!({"id": "v_x", "kind": "volume", "shape": {"kind": "extrude", "profile": "p_s", "height": 4}}),
    );
    let i = one(&run(d), Code::ReferenceCycle);
    assert_eq!(i.target.as_deref(), Some("p_s"));
    assert_eq!(i.related, ["v_x"]);

    let mut d = base();
    d["assemblies"] = json!([asm("a1", &["a2", "p_a"]), asm("a2", &["a1"])]);
    let v = run(d);
    one(&v, Code::ReferenceCycle);
    // no top-level assembly → no root
    one(&v, Code::InvalidRoot);
}

#[test]
fn multi_owner_bodies_and_assembly_parents() {
    let mut d = base();
    push(&mut d, "parts", part("p_b", &[("add", "v_a")]));
    let i = one(&run_with(d, false, None), Code::MultiOwner);
    assert_eq!(i.target.as_deref(), Some("v_a"));
    assert_eq!(i.related, ["p_a", "p_b"]);
    assert_eq!(i.owner, None, "P-class owner is the submitter (none here)");

    // envelopes may be shared
    let mut d = base();
    push(&mut d, "components", plate("v_b", 10.0, 10.0, 10.0));
    push(
        &mut d,
        "components",
        empty_box("e_env", "envelope", 500.0, 500.0, 500.0),
    );
    push(
        &mut d,
        "constraints",
        fixed("k_b", "v_b.origin", [0.0, 0.0, 10.0]),
    );
    push(
        &mut d,
        "constraints",
        fixed("k_e", "e_env.origin", [0.0, 0.0, -100.0]),
    );
    d["parts"] = json!([
        part("p_a", &[("add", "v_a"), ("intersect", "e_env")]),
        part("p_b", &[("add", "v_b"), ("intersect", "e_env")])
    ]);
    d["assemblies"] = json!([asm("a_m", &["p_a", "p_b"])]);
    let v = run(d.clone());
    none(&v, Code::MultiOwner);

    d["assemblies"] = json!([
        asm("a_m", &["a_1", "a_2"]),
        asm("a_1", &["p_a"]),
        asm("a_2", &["p_a", "p_b"])
    ]);
    let i = one(&run(d), Code::MultiOwner);
    assert_eq!(i.target.as_deref(), Some("p_a"));
    assert_eq!(i.related, ["a_1", "a_2"]);
}

#[test]
fn body_rules() {
    let mut d = base();
    push(
        &mut d,
        "components",
        empty_box("e_c", "cut", 10.0, 10.0, 10.0),
    );
    push(&mut d, "constraints", fixed("k_c", "e_c.origin", [0.0; 3]));
    d["parts"][0]["body"] =
        json!([{"op": "cut", "component": "e_c"}, {"op": "add", "component": "v_a"}]);
    one(&run(d), Code::BodyFirstNotAdd);

    let mut d = base();
    push(
        &mut d,
        "components",
        empty_box("e_r", "reserve", 10.0, 10.0, 10.0),
    );
    push(
        &mut d,
        "components",
        empty_box("e_k", "keep_out", 10.0, 10.0, 10.0),
    );
    push(&mut d, "components", dot("d_p"));
    push(
        &mut d,
        "constraints",
        fixed("k_r", "e_r.origin", [0.0, 0.0, 50.0]),
    );
    push(
        &mut d,
        "constraints",
        fixed("k_k", "e_k.origin", [0.0, 0.0, 80.0]),
    );
    push(
        &mut d,
        "constraints",
        fixed("k_p", "d_p.point", [0.0, 0.0, 80.0]),
    );
    d["parts"][0]["body"] = json!([
        {"op": "add", "component": "v_a"},
        {"op": "cut", "component": "e_r"},
        {"op": "intersect", "component": "e_k"},
        {"op": "add", "component": "d_p"}
    ]);
    let v = run(d);
    assert_eq!(with_code(&v, Code::InvalidBodyComponent).len(), 3);
}

#[test]
fn invalid_root_and_compute_root() {
    // two parts, no assembly
    let mut d = base();
    push(&mut d, "components", plate("v_b", 10.0, 10.0, 10.0));
    push(
        &mut d,
        "constraints",
        fixed("k_b", "v_b.origin", [0.0, 0.0, 10.0]),
    );
    push(&mut d, "parts", part("p_b", &[("add", "v_b")]));
    assert_eq!(compute_root(&parse(d.clone())), None);
    let i = one(&run(d.clone()), Code::InvalidRoot);
    assert_eq!(i.owner, Some(Role::Assembler));
    assert!(blocking_issues(&run(d.clone()).report).contains(&&i));
    // not checked for intermediate documents
    none(&run_with(d.clone(), false, None), Code::InvalidRoot);
    // with an assembly it is fine, as long as root names it
    d["assemblies"] = json!([asm("a_m", &["p_a", "p_b"])]);
    d["root"] = json!("p_a");
    let i = one(&run(d.clone()), Code::InvalidRoot);
    assert!(i.message.contains("a_m"));
    d["root"] = json!("a_m");
    none(&run(d.clone()), Code::InvalidRoot);
    assert_eq!(compute_root(&parse(d)).unwrap().as_str(), "a_m");
    // root missing on a completed document; single part → that part
    let mut d = base();
    d["root"] = Value::Null;
    assert_eq!(compute_root(&parse(d.clone())).unwrap().as_str(), "p_a");
    let i = one(&run(d.clone()), Code::InvalidRoot);
    assert!(i.message.contains("missing"));
    d["root"] = json!("p_a");
    none(&run(d), Code::InvalidRoot);
    // no parts at all
    let mut d = base();
    d["parts"] = json!([]);
    assert!(!with_code(&run(d), Code::InvalidRoot).is_empty());
}

#[test]
fn orphan_component_is_a_warning() {
    let mut d = base();
    d["root"] = json!("p_a");
    push(&mut d, "components", plate("v_o", 5.0, 5.0, 5.0));
    push(
        &mut d,
        "constraints",
        fixed("k_o", "v_o.origin", [0.0, 0.0, 50.0]),
    );
    let v = run(d);
    let i = one(&v, Code::OrphanComponent);
    assert_eq!(i.owner, Some(Role::Assembler));
    assert!(v.report.valid && v.report.review_required);
    assert!(blocking_issues(&v.report).is_empty());
}

#[test]
fn profile_not_axis_aligned() {
    let mut d = base();
    d["root"] = json!("p_a");
    for (id, p) in [
        ("d_1", [0.0, 0.0, 0.0]),
        ("d_2", [10.0, 0.0, 0.0]),
        ("d_3", [10.0, 10.0, 3.0]),
    ] {
        push(&mut d, "components", dot(id));
        push(
            &mut d,
            "constraints",
            fixed(&format!("k_{id}"), &format!("{id}.point"), p),
        );
    }
    push(
        &mut d,
        "components",
        json!({"id": "l_o", "kind": "line", "shape": {"kind": "polyline", "points": ["d_1", "d_2", "d_3"], "closed": true}}),
    );
    push(
        &mut d,
        "components",
        json!({"id": "p_f", "kind": "plane", "shape": {"kind": "profile", "outer": "l_o", "inners": []}}),
    );
    let v = run(d.clone());
    assert_eq!(
        one(&v, Code::ProfileNotAxisAligned).target.as_deref(),
        Some("p_f")
    );
    // flatten d_3 → planar
    d["constraints"][3]["frame"]["origin"] = json!([10.0, 10.0, 0.0]);
    none(&run(d.clone()), Code::ProfileNotAxisAligned);

    // a diagonal segment used as a revolve axis
    push(
        &mut d,
        "components",
        json!({"id": "l_ax", "kind": "line", "shape": {"kind": "segment", "start": "d_1", "end": "d_3"}}),
    );
    push(
        &mut d,
        "components",
        json!({"id": "v_r", "kind": "volume", "shape": {"kind": "revolve", "profile": "p_f", "axis": "l_ax", "angle": 360}}),
    );
    d["parts"][0]["body"]
        .as_array_mut()
        .unwrap()
        .push(json!({"op": "add", "component": "v_r"}));
    let i = one(&run(d), Code::ProfileNotAxisAligned);
    assert_eq!(i.target.as_deref(), Some("l_ax"));
}

#[test]
fn binding_issues_surface_with_submitter_owner() {
    let mut d = base();
    d["bindings"] = json!([{"target": "v_a:nope", "expr": "1"}]);
    let v = run_with(d, false, Some(Role::ComponentSmith));
    let i = one(&v, Code::UnknownField);
    assert_eq!(i.owner, Some(Role::ComponentSmith));
    // bindings do not gate the solver
    assert!(v.resolved.components.contains_key("v_a"));
}

#[test]
fn structural_errors_gate_the_solver() {
    let mut d = base();
    push(&mut d, "components", plate("v_b", 10.0, 10.0, 10.0));
    push(
        &mut d,
        "constraints",
        k("k_x", "mate", "v_b.bottom", "v_a.nope"),
    );
    d["parts"][0]["body"]
        .as_array_mut()
        .unwrap()
        .push(json!({"op": "add", "component": "v_b"}));
    let v = run_with(d, false, None);
    assert_eq!(codes(&v), ["unknown_port"]);
    assert!(v.resolved.components.is_empty() && v.resolved.ports.is_empty());
    assert_eq!(v.resolved.revision, 3);
}

// ---- solver codes are taken from the solver --------------------------------------------------

#[test]
fn solver_issues_are_included() {
    let mut d = base();
    d["root"] = json!("p_a");
    d["constraints"] = json!([]);
    let v = run(d);
    one(&v, Code::NoFixedRoot);

    let mut d = base();
    d["root"] = json!("p_a");
    push(
        &mut d,
        "components",
        empty_box("e_r", "reserve", 10.0, 10.0, 10.0),
    );
    push(
        &mut d,
        "constraints",
        fixed("k_r", "e_r.origin", [0.0, 0.0, 50.0]),
    );
    push(
        &mut d,
        "constraints",
        json!({"id": "k_in", "kind": "inside", "a": "e_r.region", "b": "v_a.region"}),
    );
    let v = run(d);
    one(&v, Code::InsideViolated);
    assert_eq!(v.resolved.issues.len(), 1);
}

// ---- empties --------------------------------------------------------------------------------

fn with_zone(role: &str, z: f64) -> Value {
    let mut d = base();
    d["root"] = json!("p_a");
    push(
        &mut d,
        "components",
        empty_box("e_z", role, 20.0, 20.0, 20.0),
    );
    push(
        &mut d,
        "constraints",
        fixed("k_z", "e_z.origin", [0.0, 0.0, z]),
    );
    d
}

#[test]
fn reserve_and_keep_out_violated() {
    let v = run(with_zone("reserve", 5.0));
    let i = one(&v, Code::ReserveViolated);
    assert_eq!(i.target.as_deref(), Some("e_z"));
    assert_eq!(i.related, ["p_a", "v_a"]);
    assert!(i.message.contains("5 mm along +z"), "{}", i.message);
    assert_eq!(i.owner, Some(Role::ComponentSmith));
    // touching the top face is not a violation
    none(&run(with_zone("reserve", 10.0)), Code::ReserveViolated);
    one(&run(with_zone("keep_out", 2.0)), Code::KeepOutViolated);
    none(&run(with_zone("keep_out", 10.0)), Code::KeepOutViolated);
}

#[test]
fn reserve_respected_by_a_cut_pocket() {
    // the pocket cut removes exactly the intruded material
    let mut d = with_zone("reserve", 5.0);
    push(
        &mut d,
        "components",
        empty_box("e_pocket", "cut", 20.0, 20.0, 5.0),
    );
    push(
        &mut d,
        "constraints",
        fixed("k_p", "e_pocket.origin", [0.0, 0.0, 5.0]),
    );
    d["parts"][0]["body"]
        .as_array_mut()
        .unwrap()
        .push(json!({"op": "cut", "component": "e_pocket"}));
    let v = run(d);
    none(&v, Code::ReserveViolated);
    none(&v, Code::EmptyCutNoEffect);
}

#[test]
fn envelope_exceeded() {
    let mut d = base();
    d["root"] = json!("p_a");
    push(
        &mut d,
        "components",
        empty_box("e_env", "envelope", 90.0, 200.0, 50.0),
    );
    push(
        &mut d,
        "constraints",
        fixed("k_e", "e_env.origin", [0.0, 0.0, -1.0]),
    );
    let i = one(&run(d.clone()), Code::EnvelopeExceeded);
    assert!(i.message.contains("5 mm"), "{}", i.message);
    d["components"][1]["shape"]["length"] = json!(100.0);
    none(&run(d.clone()), Code::EnvelopeExceeded);
    // intersecting with the envelope clips the part inside it
    d["components"][1]["shape"]["length"] = json!(90.0);
    d["parts"][0]["body"]
        .as_array_mut()
        .unwrap()
        .push(json!({"op": "intersect", "component": "e_env"}));
    none(&run(d), Code::EnvelopeExceeded);

    // cylinder envelope: the plate corners stick out radially
    let mut d = base();
    d["root"] = json!("p_a");
    push(
        &mut d,
        "components",
        empty_cyl("e_env", "envelope", 120.0, 20.0),
    );
    push(
        &mut d,
        "constraints",
        fixed("k_e", "e_env.origin", [0.0, 0.0, -1.0]),
    );
    let i = one(&run(d.clone()), Code::EnvelopeExceeded);
    assert!(i.message.contains("radially"), "{}", i.message);
    d["components"][1]["shape"]["diameter"] = json!(150.0);
    none(&run(d), Code::EnvelopeExceeded);
}

#[test]
fn empty_cut_no_effect_and_removes_all() {
    let cut_at = |size: f64, z: f64| {
        let mut d = base();
        d["root"] = json!("p_a");
        push(
            &mut d,
            "components",
            empty_box("e_c", "cut", size, size, size),
        );
        push(
            &mut d,
            "constraints",
            fixed("k_c", "e_c.origin", [0.0, 0.0, z]),
        );
        d["parts"][0]["body"]
            .as_array_mut()
            .unwrap()
            .push(json!({"op": "cut", "component": "e_c"}));
        run(d)
    };
    let v = cut_at(10.0, 30.0);
    assert_eq!(
        one(&v, Code::EmptyCutNoEffect).target.as_deref(),
        Some("e_c")
    );
    none(&v, Code::EmptyCutRemovesAll);
    let v = cut_at(10.0, 5.0);
    none(&v, Code::EmptyCutNoEffect);
    none(&v, Code::EmptyCutRemovesAll);
    let v = cut_at(200.0, -50.0);
    let i = one(&v, Code::EmptyCutRemovesAll);
    assert_eq!(i.target.as_deref(), Some("p_a"));
    assert_eq!(i.related, ["e_c"]);
    assert!(!v.report.valid);
}

#[test]
fn reserve_overlap() {
    let mut d = with_zone("reserve", 50.0);
    push(
        &mut d,
        "components",
        empty_box("e_y", "reserve", 20.0, 20.0, 20.0),
    );
    push(
        &mut d,
        "constraints",
        fixed("k_y", "e_y.origin", [5.0, 5.0, 55.0]),
    );
    let i = one(&run(d.clone()), Code::ReserveOverlap);
    assert_eq!(i.target.as_deref(), Some("e_z"));
    assert_eq!(i.related, ["e_y"]);
    assert_eq!(i.owner, Some(Role::SpacePlanner));
    d["constraints"][2]["frame"]["origin"] = json!([30.0, 0.0, 50.0]);
    none(&run(d), Code::ReserveOverlap);
}

#[test]
fn reserve_unreachable_only_when_closed() {
    let v = run(example("enclosure-with-lid.cad2.json"));
    one(&v, Code::ReserveUnreachable);
    // without the lid the top is open
    let mut d = example("enclosure-with-lid.cad2.json");
    d["parts"].as_array_mut().unwrap().pop();
    d["assemblies"] = json!([asm("a_main", &["p_body"])]);
    let v = run(d);
    none(&v, Code::ReserveUnreachable);
    // a hole through the lid above the reserve is an opening
    let mut d = example("enclosure-with-lid.cad2.json");
    push(&mut d, "components", hole("e_vent", 5.0, "through", None));
    push(&mut d, "components", dot("d_vent"));
    push(
        &mut d,
        "constraints",
        on_face("k_dv", "d_vent.point", "v_lid.top", 0.0, 0.0),
    );
    push(
        &mut d,
        "constraints",
        k("k_ev", "coincident", "e_vent.entry", "d_vent.point"),
    );
    d["parts"][1]["body"]
        .as_array_mut()
        .unwrap()
        .push(json!({"op": "cut", "component": "e_vent"}));
    let v = run(d);
    none(&v, Code::ReserveUnreachable);
    assert!(v.report.valid, "{:#?}", v.report.issues);
}

// ---- between parts --------------------------------------------------------------------------

/// Plate `v_a` with a d = 6.6 through hole at the center; a pin of diameter `pin_d` in a
/// second part, coaxial with the hole.
fn pin_in_hole(pin_d: f64) -> Value {
    let mut d = base();
    push(&mut d, "components", dot("d_h"));
    push(&mut d, "components", hole("e_h", 6.6, "clearance", None));
    push(&mut d, "components", cylinder("v_pin", pin_d, 30.0));
    push(
        &mut d,
        "constraints",
        on_face("k_dh", "d_h.point", "v_a.top", 0.0, 0.0),
    );
    push(
        &mut d,
        "constraints",
        k("k_eh", "coincident", "e_h.entry", "d_h.point"),
    );
    push(
        &mut d,
        "constraints",
        k("k_pin", "coaxial", "v_pin.axis", "e_h.axis"),
    );
    push(
        &mut d,
        "constraints",
        json!({"id": "k_pz", "kind": "distance", "a": "v_pin.bottom", "b": "v_a.bottom", "value": 10.0}),
    );
    d["parts"][0]["body"]
        .as_array_mut()
        .unwrap()
        .push(json!({"op": "cut", "component": "e_h"}));
    push(&mut d, "parts", part("p_pin", &[("add", "v_pin")]));
    d["assemblies"] = json!([asm("a_m", &["p_a", "p_pin"])]);
    d["root"] = json!("a_m");
    d
}

#[test]
fn part_interference_respects_holes() {
    let v = run(pin_in_hole(6.0));
    none(&v, Code::PartInterference);
    none(&v, Code::FloatingPart);
    assert!(v.report.valid, "{:#?}", v.report.issues);
    let v = run(pin_in_hole(8.0));
    let i = one(&v, Code::PartInterference);
    assert_eq!(i.target.as_deref(), Some("p_a"));
    assert_eq!(i.related, ["p_pin"]);
    assert_eq!(i.owner, Some(Role::Joiner));
    assert!(v.report.valid, "analytic interference is a warning");
    // a pin exactly the hole size is undecidable analytically: no warning
    none(&run(pin_in_hole(6.6)), Code::PartInterference);
}

#[test]
fn floating_part() {
    let mut d = base();
    push(&mut d, "components", plate("v_b", 10.0, 10.0, 10.0));
    push(
        &mut d,
        "constraints",
        fixed("k_b", "v_b.origin", [0.0, 0.0, 10.0]),
    );
    push(&mut d, "parts", part("p_b", &[("add", "v_b")]));
    d["assemblies"] = json!([asm("a_m", &["p_a", "p_b"])]);
    d["root"] = json!("a_m");
    let v = run(d.clone());
    let f = with_code(&v, Code::FloatingPart);
    assert_eq!(f.len(), 2, "{:#?}", v.report.issues);
    assert_eq!(f[0].target.as_deref(), Some("p_a"));
    assert_eq!(f[1].target.as_deref(), Some("p_b"));
    // mate p_b onto p_a: joined (the redundant z equation becomes a solver info)
    push(
        &mut d,
        "constraints",
        k("k_mate", "mate", "v_b.bottom", "v_a.top"),
    );
    none(&run(d.clone()), Code::FloatingPart);
    // joined through a Dot that belongs to no part
    let mut d2 = d.clone();
    d2["constraints"].as_array_mut().unwrap().pop();
    push(&mut d2, "components", dot("d_j"));
    push(
        &mut d2,
        "constraints",
        on_face("k_j1", "d_j.point", "v_a.top", 0.0, 0.0),
    );
    push(
        &mut d2,
        "constraints",
        k("k_j2", "coincident", "v_b.origin", "d_j.point"),
    );
    let v = run(d2);
    none(&v, Code::FloatingPart);
    // a single-part tree never floats
    none(
        &run(example("plate-four-holes.cad2.json")),
        Code::FloatingPart,
    );
}

// ---- carried-over 1.0 rules on 2.0 documents --------------------------------------------------

/// Plate fixed with `frame`, one hole placed `on_face` on its top at `(u, v)`.
fn plate_with_hole(x: &str, z: &str, origin: [f64; 3], u: f64, d: f64) -> Value {
    let mut v = doc(
        json!([
            plate("v_a", 40.0, 30.0, 5.0),
            dot("d_h"),
            hole("e_h", d, "clearance", None)
        ]),
        json!([
            fixed_rot("k_a", "v_a.origin", origin, x, z),
            on_face("k_dh", "d_h.point", "v_a.top", u, 0.0),
            k("k_eh", "coincident", "e_h.entry", "d_h.point")
        ]),
        json!([part("p_a", &[("add", "v_a"), ("cut", "e_h")])]),
        json!([]),
    );
    v["root"] = json!("p_a");
    v
}

#[test]
fn hole_rules_are_invariant_under_part_placement() {
    for (x, z, origin) in [
        ("+x", "+z", [0.0, 0.0, 0.0]),
        ("+y", "-x", [100.0, -20.0, 7.0]),
        ("-z", "+y", [-3.0, 50.0, 1.0]),
    ] {
        // d = 6 at u = 19: 1.0 test_hole_outside_part_is_error
        let v = run(plate_with_hole(x, z, origin, 19.0, 6.0));
        assert_eq!(
            one(&v, Code::HoleOutsidePart).target.as_deref(),
            Some("e_h"),
            "{x} {z}"
        );
        none(&v, Code::HoleAxisMismatch);
        let v = run(plate_with_hole(x, z, origin, 16.0, 6.0));
        one(&v, Code::LowEdgeClearance);
        none(&v, Code::HoleOutsidePart);
        let v = run(plate_with_hole(x, z, origin, 0.0, 6.0));
        assert!(
            v.report.issues.is_empty(),
            "{x} {z}: {:#?}",
            v.report.issues
        );
    }
}

#[test]
fn every_carried_over_rule_on_a_2_0_document() {
    // Volume rules
    let mut d = base();
    d["root"] = json!("p_a");
    d["components"] = json!([
        {"id": "v_a", "kind": "volume", "shape": {"kind": "enclosure", "length": 100, "width": 70, "height": 30, "wall_thickness": 0.5}}
    ]);
    one(&run(d), Code::ThinWall);
    let mut d = base();
    d["root"] = json!("p_a");
    d["components"] = json!([
        {"id": "v_a", "kind": "volume", "shape": {"kind": "l_bracket", "width": 40, "depth": 30, "vertical_height": 30, "thickness": 1}}
    ]);
    one(&run(d), Code::ThinBracket);
    let mut d = base();
    d["root"] = json!("p_a");
    d["components"] = json!([plate("v_a", 2000.0, 1.0, 1.0)]);
    one(&run(d), Code::ExtremeAspectRatio);
    // edges
    let mut d = base();
    d["root"] = json!("p_a");
    d["parts"][0]["fillets"] = json!([{"kind": "fillet", "radius": 5, "selector": "all"}]);
    d["parts"][0]["chamfers"] =
        json!([{"kind": "chamfer", "distance": 50, "selector": "vertical"}]);
    let v = run(d);
    assert_eq!(one(&v, Code::FilletTooLarge).target.as_deref(), Some("p_a"));
    assert_eq!(one(&v, Code::ChamferTooLarge).owner, Some(Role::Assembler));
    // assumption text never reaches the message
    let mut d = base();
    d["root"] = json!("p_a");
    d["assumptions"] = json!(["secret free text"]);
    let i = one(&run(d), Code::Assumption);
    assert!(!i.message.contains("secret") && i.owner.is_none());
}

#[test]
fn hole_rules_on_2_0_documents() {
    let with_hole = |h: Value, u: f64, extra: Option<Value>| {
        let mut d = base();
        d["root"] = json!("p_a");
        push(&mut d, "components", dot("d_h"));
        push(&mut d, "components", h);
        push(
            &mut d,
            "constraints",
            on_face("k_dh", "d_h.point", "v_a.top", u, 0.0),
        );
        push(
            &mut d,
            "constraints",
            k("k_eh", "coincident", "e_h.entry", "d_h.point"),
        );
        d["parts"][0]["body"]
            .as_array_mut()
            .unwrap()
            .push(json!({"op": "cut", "component": "e_h"}));
        if let Some(e) = extra {
            push(&mut d, "components", e);
            push(
                &mut d,
                "constraints",
                on_face("k_d2", "d_2.point", "v_a.top", u + 2.0, 0.0),
            );
            push(
                &mut d,
                "constraints",
                k("k_e2", "coincident", "e_2.entry", "d_2.point"),
            );
            d["parts"][0]["body"]
                .as_array_mut()
                .unwrap()
                .push(json!({"op": "cut", "component": "e_2"}));
            push(&mut d, "components", dot("d_2"));
        }
        run(d)
    };
    one(
        &with_hole(hole("e_h", 4.0, "blind", Some(10.0)), 0.0, None),
        Code::BlindDepthExceedsMaterial,
    );
    none(
        &with_hole(hole("e_h", 4.0, "blind", Some(9.0)), 0.0, None),
        Code::BlindDepthExceedsMaterial,
    );
    let mut cb = hole("e_h", 4.0, "counterbore", None);
    cb["shape"]["counterbore_diameter"] = json!(8.0);
    cb["shape"]["counterbore_depth"] = json!(10.0);
    one(
        &with_hole(cb, 0.0, None),
        Code::CounterboreDepthExceedsMaterial,
    );
    let mut cs = hole("e_h", 4.0, "countersink", None);
    cs["shape"]["countersink_diameter"] = json!(30.0);
    cs["shape"]["countersink_angle"] = json!(60.0);
    one(
        &with_hole(cs, 0.0, None),
        Code::CountersinkDepthExceedsMaterial,
    );
    let v = with_hole(hole("e_h", 5.0, "tapped", None), 0.0, None);
    one(&v, Code::TappedHoleWithoutThread);
    let mut t = hole("e_h", 5.0, "tapped", None);
    t["shape"]["thread"] = json!("M6");
    one(&with_hole(t, 0.0, None), Code::ThreadAnnotationOnly);
    let v = with_hole(
        hole("e_h", 4.0, "through", None),
        0.0,
        Some(hole("e_2", 4.0, "through", None)),
    );
    let i = one(&v, Code::OverlappingHoles);
    assert_eq!(i.target.as_deref(), Some("e_h"));
    assert_eq!(i.related[0], "e_2");
}

#[test]
fn ring_hole_rules_on_2_0_documents() {
    let ring_hole = |u: f64, d: f64| {
        let mut v = doc(
            json!([
                {"id": "v_r", "kind": "volume", "shape": {"kind": "ring", "outer_diameter": 40, "inner_diameter": 20, "height": 5}},
                dot("d_h"),
                hole("e_h", d, "through", None)
            ]),
            json!([
                fixed("k_r", "v_r.origin", [0.0; 3]),
                on_face("k_dh", "d_h.point", "v_r.top", u, 0.0),
                k("k_eh", "coincident", "e_h.entry", "d_h.point")
            ]),
            json!([part("p_r", &[("add", "v_r"), ("cut", "e_h")])]),
            json!([]),
        );
        v["root"] = json!("p_r");
        run(v)
    };
    let v = ring_hole(0.0, 6.0);
    one(&v, Code::HoleInInnerVoid);
    one(&v, Code::EmptyCutNoEffect);
    let v = ring_hole(10.0, 6.0);
    one(&v, Code::HoleIntersectsInnerVoid);
    none(&v, Code::EmptyCutNoEffect);
    one(&ring_hole(19.0, 4.0), Code::HoleOutsidePart);
    assert!(ring_hole(15.0, 4.0).report.issues.is_empty());
}

#[test]
fn derived_volumes_are_conservative() {
    // extrude of a rect: exact box; a too-deep blind hole is still reported
    let mut d = doc(
        json!([
            {"id": "p_r", "kind": "plane", "shape": {"kind": "rect", "length": 40, "width": 30}},
            {"id": "v_x", "kind": "volume", "shape": {"kind": "extrude", "profile": "p_r", "height": 5}},
            dot("d_h"),
            hole("e_h", 4.0, "blind", Some(6.0))
        ]),
        json!([
            fixed("k_r", "p_r.center", [0.0; 3]),
            on_face("k_dh", "d_h.point", "v_x.top", 0.0, 0.0),
            k("k_eh", "coincident", "e_h.entry", "d_h.point")
        ]),
        json!([part("p_x", &[("add", "v_x"), ("cut", "e_h")])]),
        json!([]),
    );
    d["root"] = json!("p_x");
    let v = run(d.clone());
    one(&v, Code::BlindDepthExceedsMaterial);
    none(&v, Code::OrphanComponent);
    // the rect extrude is exact: a reserve in its material is violated…
    push(
        &mut d,
        "components",
        empty_box("e_z", "reserve", 2.0, 2.0, 2.0),
    );
    push(
        &mut d,
        "constraints",
        fixed("k_z", "e_z.origin", [10.0, 0.0, 1.0]),
    );
    one(&run(d.clone()), Code::ReserveViolated);
    // …but not where the blind hole removed the material
    d["constraints"][3]["frame"]["origin"] = json!([0.0, 0.0, 1.0]);
    none(&run(d.clone()), Code::ReserveViolated);

    // a revolve is only bounded by its box (over-approximation): a reserve inside the box is
    // never reported, there is no panic, and the 1.0 rules still run on the box
    let mut d = doc(
        json!([
            dot("d_1"), dot("d_2"), dot("d_3"), dot("d_4"),
            {"id": "l_o", "kind": "line", "shape": {"kind": "polyline", "points": ["d_1", "d_2", "d_3", "d_4"], "closed": true}},
            {"id": "p_f", "kind": "plane", "shape": {"kind": "profile", "outer": "l_o", "inners": []}},
            {"id": "l_ax", "kind": "line", "shape": {"kind": "segment", "start": "d_1", "end": "d_4"}},
            {"id": "v_rv", "kind": "volume", "shape": {"kind": "revolve", "profile": "p_f", "axis": "l_ax", "angle": 360}},
            empty_box("e_z", "reserve", 2.0, 2.0, 2.0)
        ]),
        json!([
            fixed("k_1", "d_1.point", [0.0, 0.0, 0.0]),
            fixed("k_2", "d_2.point", [10.0, 0.0, 0.0]),
            fixed("k_3", "d_3.point", [10.0, 0.0, 20.0]),
            fixed("k_4", "d_4.point", [0.0, 0.0, 20.0]),
            fixed("k_z", "e_z.origin", [0.0, 0.0, 5.0])
        ]),
        json!([part("p_rv", &[("add", "v_rv")])]),
        json!([]),
    );
    d["root"] = json!("p_rv");
    let v = run(d);
    none(&v, Code::ReserveViolated);
    assert!(v.report.valid, "{:#?}", v.report.issues);
}
