//! Patch engine (spec §8.2, §8.6, §8.7): op semantics, reject reasons, normalization.

mod patch_support;

use patch_support::*;
use promptcad_core::fields::get_field;
use promptcad_core::model::v2::ProvenanceAgent;
use promptcad_core::model::{CadDocument2, Code, FieldRef, Patch, RejectCode, Role};
use promptcad_core::patch_engine::{apply_patch, undo, PatchOrigin, PatchOutcome};
use promptcad_core::solver;
use promptcad_core::validate::compute_root;
use serde_json::json;

fn fr(s: &str) -> FieldRef {
    s.parse().unwrap()
}

fn field(doc: &CadDocument2, s: &str) -> f64 {
    get_field(doc, &fr(s)).unwrap()
}

fn agent(role: Role) -> PatchOrigin {
    PatchOrigin::Agent(role)
}

/// Applies `patch` as its own agent.
fn run(doc: &CadDocument2, patch: &Patch) -> PatchOutcome {
    apply_patch(doc, patch, agent(patch.agent))
}

fn build_example_a() -> Vec<CadDocument2> {
    let mut docs = vec![empty_a()];
    for (role, p) in example_a_patches() {
        let out = apply_patch(docs.last().unwrap(), &p, agent(role));
        docs.push(accepted(&out));
    }
    docs
}

// ---- end-to-end ------------------------------------------------------------------------------

#[test]
fn example_a_is_built_from_an_empty_document_by_four_roles() {
    let docs = build_example_a();
    let built = docs.last().unwrap();
    let expected = example_a();
    assert_eq!(built.revision, 4);
    // Intermediate revisions are 0..=4.
    let revs: Vec<u64> = docs.iter().map(|d| d.revision).collect();
    assert_eq!(revs, [0, 1, 2, 3, 4]);
    // Everything except `root` (owned by validate::compute_root) is byte-identical: revision,
    // parameters, binding order, provenance {component_smith, "r2"}, labels, assumption, part.
    let mut but_root = built.clone();
    but_root.root = expected.root.clone();
    assert!(
        canon(&but_root) == canon(&expected),
        "built document differs from examples/cad2/plate-four-holes.cad2.json"
    );
    // `root` recomputed per §6.3 after the assembler's patch.
    assert_eq!(
        built.root.as_ref().map(|r| r.as_str()),
        Some("p_plate"),
        "root must be recomputed by validate::compute_root (spec §6.3)"
    );
    // Exact equality, and therefore the same solver output.
    assert_eq!(built, &expected);
    assert_eq!(solver::solve(built), solver::solve(&expected));
}

#[test]
fn accepted_outcome_carries_metadata_and_only_design_issues() {
    let base = empty_a();
    let (_, p) = example_a_patches().remove(0);
    let out = apply_patch(&base, &p, agent(Role::SpacePlanner));
    let a = out.accepted().expect("accepted");
    assert_eq!((a.revision_before, a.revision_after), (0, 1));
    assert_eq!(a.patch_id, "r1");
    assert_eq!(a.patch_sha256.len(), 64);
    assert_ne!(a.doc_sha256_before, a.doc_sha256_after);
    assert!(a.created.is_empty());
    assert!(a
        .validation
        .report
        .issues
        .iter()
        .all(|i| !i.code().is_some_and(Code::is_patch_blocking)));
    // Pure: the input is untouched.
    assert_eq!(base, empty_a());

    // D-class issues are returned, not rejected: remove the fixed root.
    let doc = example_a();
    let p = patch(
        Role::Joiner,
        4,
        json!([{"op": "remove_constraint", "id": "k_root"}]),
    );
    let out = run(&doc, &p);
    accepted(&out);
    assert!(out.issues().iter().any(|i| i.code == "no_fixed_root"));
}

// ---- op semantics ----------------------------------------------------------------------------

#[test]
fn later_ops_see_earlier_ops_remove_binding_then_set_field() {
    let doc = example_a();
    let p = patch(
        Role::ComponentSmith,
        4,
        json!([
            {"op": "remove_binding", "target": "v_plate:length"},
            {"op": "set_field", "target": "v_plate:length", "value": 130}
        ]),
    );
    let new = accepted(&run(&doc, &p));
    assert_eq!(field(&new, "v_plate:length"), 130.0);
    assert!(new.binding(&fr("v_plate:length")).is_none());
    assert_eq!(new.bindings.len(), doc.bindings.len() - 1);

    // Reversed order fails: the field is still bound when set_field runs.
    let p = patch(
        Role::ComponentSmith,
        4,
        json!([
            {"op": "set_field", "target": "v_plate:length", "value": 130},
            {"op": "remove_binding", "target": "v_plate:length"}
        ]),
    );
    let out = run(&doc, &p);
    rejected_with(&out, RejectCode::FieldIsBound);
    assert_eq!(out.reason().unwrap().op_index, Some(0));
}

#[test]
fn remove_binding_keeps_the_cached_value() {
    let doc = example_a();
    let p = patch(
        Role::Joiner,
        4,
        json!([{"op": "remove_binding", "target": "k_d1:u"}]),
    );
    let new = accepted(&run(&doc, &p));
    assert_eq!(field(&new, "k_d1:u"), -50.0);
    // Changing `len` no longer moves k_d1:u, but still moves the bound k_d2:u.
    let p = patch(
        Role::SpacePlanner,
        5,
        json!([{"op": "set_parameter", "parameter":
            {"name": "len", "value": 140, "min": null, "max": null, "note": "長"}}]),
    );
    let newer = accepted(&run(&new, &p));
    assert_eq!(field(&newer, "k_d1:u"), -50.0);
    assert_eq!(field(&newer, "k_d2:u"), 60.0);
    assert_eq!(field(&newer, "v_plate:length"), 140.0);
}

#[test]
fn set_binding_adds_or_replaces_and_evaluates_immediately() {
    let doc = example_a();
    let p = patch(
        Role::ComponentSmith,
        4,
        json!([{"op": "set_binding", "target": "v_plate:length", "expr": "len + 10"}]),
    );
    let new = accepted(&run(&doc, &p));
    assert_eq!(field(&new, "v_plate:length"), 130.0);
    assert_eq!(new.bindings.len(), doc.bindings.len());
    assert_eq!(new.binding(&fr("v_plate:length")).unwrap().expr, "len + 10");
    // Bound order is preserved (replace in place).
    assert_eq!(new.bindings[0].target.as_str(), "v_plate:length");

    // New binding on an unbound field: appended and evaluated.
    let p = patch(
        Role::ComponentSmith,
        4,
        json!([plate("v_extra", 50.0, 20.0, 5.0),
               {"op": "set_binding", "target": "v_extra:length", "expr": "len / 2"}]),
    );
    let new = accepted(&run(&doc, &p));
    assert_eq!(field(&new, "v_extra:length"), 60.0);
    assert_eq!(
        new.bindings.last().unwrap().target.as_str(),
        "v_extra:length"
    );
}

#[test]
fn set_parameter_adds_or_replaces_by_name_and_bindings_follow() {
    let doc = example_a();
    let p = patch(
        Role::SpacePlanner,
        4,
        json!([
            {"op": "set_parameter", "parameter":
                {"name": "len", "value": 150, "min": null, "max": null, "note": "長"}},
            {"op": "set_parameter", "parameter":
                {"name": "gap", "value": 1, "min": 0, "max": null, "note": null}}
        ]),
    );
    let new = accepted(&run(&doc, &p));
    assert_eq!(new.parameters.len(), 5);
    assert_eq!(new.parameters[0].value, 150.0);
    assert_eq!(new.parameters[4].name.as_str(), "gap");
    assert_eq!(field(&new, "v_plate:length"), 150.0);
    assert_eq!(field(&new, "k_d1:u"), -65.0);
    assert_eq!(field(&new, "k_d3:u"), 65.0);
}

#[test]
fn remove_component_drops_its_bindings_but_never_cascades() {
    let doc = example_a();
    let p = patch(
        Role::ComponentSmith,
        4,
        json!([plate("v_extra", 50.0, 20.0, 5.0),
               {"op": "set_binding", "target": "v_extra:length", "expr": "len / 2"}]),
    );
    let with_extra = accepted(&run(&doc, &p));
    let p = patch(
        Role::ComponentSmith,
        5,
        json!([{"op": "remove_component", "id": "v_extra"}]),
    );
    let removed = accepted(&run(&with_extra, &p));
    assert!(removed.component("v_extra").is_none());
    assert_eq!(removed.bindings, doc.bindings);

    // Referenced from a binding expression elsewhere -> dangling_reference.
    let p = patch(
        Role::ComponentSmith,
        5,
        json!([plate("v_other", 50.0, 20.0, 5.0),
               {"op": "set_binding", "target": "v_other:length", "expr": "v_extra:width * 2"},
               {"op": "remove_component", "id": "v_extra"}]),
    );
    let out = run(&with_extra, &p);
    rejected_with(&out, Code::DanglingReference);
    assert_eq!(out.reason().unwrap().op_index, Some(2));
}

#[test]
fn remove_constraint_drops_bindings_targeting_it() {
    let doc = example_a();
    let p = patch(
        Role::Joiner,
        4,
        json!([{"op": "remove_constraint", "id": "k_d1"}]),
    );
    let new = accepted(&run(&doc, &p));
    assert!(new.constraint("k_d1").is_none());
    assert_eq!(new.bindings.len(), doc.bindings.len() - 2);
    assert!(new.bindings.iter().all(|b| b.target.id_str() != "k_d1"));
}

#[test]
fn set_body_add_part_add_assembly_and_root_recomputation() {
    let doc = example_a();
    let smith = patch(
        Role::ComponentSmith,
        4,
        json!([plate("v_two", 40.0, 40.0, 5.0)]),
    );
    let doc = accepted(&run(&doc, &smith));
    // Two parts, no assembly: root cannot be determined (invalid_root only when completed).
    let parts = patch(
        Role::Assembler,
        5,
        json!([{"op": "add_part", "part": {"id": "p_two", "name": "second", "material": null,
            "quantity": 2, "source": "made", "body": [{"op": "add", "component": "v_two"}],
            "fillets": [], "chamfers": []}}]),
    );
    let two_parts = accepted(&run(&doc, &parts));
    assert_eq!(two_parts.root, None);
    assert_eq!(two_parts.root, compute_root(&two_parts));
    let asm = patch(
        Role::Assembler,
        6,
        json!([{"op": "add_assembly", "assembly":
            {"id": "a_main", "name": "main", "children": ["p_plate", "p_two"]}}]),
    );
    let with_asm = accepted(&run(&two_parts, &asm));
    assert_eq!(with_asm.root.as_ref().map(|r| r.as_str()), Some("a_main"));

    // set_body replaces the body of an existing part.
    let body = patch(
        Role::Assembler,
        7,
        json!([{"op": "set_body", "part_id": "p_plate", "body": [
            {"op": "add", "component": "v_plate"},
            {"op": "cut", "component": "e_h1"},
            {"op": "cut", "component": "e_h2"}
        ]}]),
    );
    let new = accepted(&run(&with_asm, &body));
    assert_eq!(new.part("p_plate").unwrap().body.len(), 3);

    // Root recomputed while building example A: absent until the part exists.
    let docs = build_example_a();
    assert!(docs[..4].iter().all(|d| d.root.is_none()));
    assert_eq!(docs[4].root.as_ref().map(|r| r.as_str()), Some("p_plate"));
}

#[test]
fn provenance_is_stamped_once_and_never_overwritten() {
    let docs = build_example_a();
    let a = docs.last().unwrap();
    for c in &a.components {
        let p = c.provenance.as_ref().expect("stamped");
        assert_eq!(p.agent, ProvenanceAgent::ComponentSmith);
        assert_eq!(p.patch_id.as_deref(), Some("r2"));
    }
    // A later patch touching v_plate and adding d_new: only d_new gets r5.
    let p = patch(
        Role::ComponentSmith,
        4,
        json!([{"op": "remove_binding", "target": "v_plate:width"},
               {"op": "set_field", "target": "v_plate:width", "value": 70},
               dot("d_new")]),
    );
    let out = run(a, &p);
    let new = accepted(&out);
    assert_eq!(
        out.accepted()
            .unwrap()
            .created
            .iter()
            .map(|i| i.as_str())
            .collect::<Vec<_>>(),
        ["d_new"]
    );
    let pv = |id: &str| new.component(id).unwrap().provenance.clone().unwrap();
    assert_eq!(pv("v_plate").patch_id.as_deref(), Some("r2"));
    assert_eq!(pv("d_new").patch_id.as_deref(), Some("r5"));
    assert_eq!(pv("d_new").agent, ProvenanceAgent::ComponentSmith);

    // Human origin stamps `human`, whatever patch.agent says.
    let p = patch(Role::Joiner, 5, json!([dot("d_human")]));
    let newer = accepted(&apply_patch(&new, &p, PatchOrigin::Human));
    let h = newer
        .component("d_human")
        .unwrap()
        .provenance
        .clone()
        .unwrap();
    assert_eq!(h.agent, ProvenanceAgent::Human);
    assert_eq!(h.patch_id.as_deref(), Some("r6"));
    assert_eq!(
        newer.component("d_new").unwrap().provenance,
        new.component("d_new").unwrap().provenance
    );
}

#[test]
fn annotations_and_assumption_removal() {
    let doc = example_a();
    let text = doc.assumptions[0].clone();
    let p = patch(
        Role::ComponentSmith,
        4,
        json!([{"op": "remove_assumption", "text": text},
               {"op": "add_note", "text": "n"},
               dot("d_x")]),
    );
    let new = accepted(&run(&doc, &p));
    assert!(new.assumptions.is_empty());
    assert_eq!(new.notes, ["n"]);

    // An assumption added earlier in the same patch can be removed by a later op.
    let p = patch(
        Role::Joiner,
        4,
        json!([{"op": "add_assumption", "text": "tmp"},
               {"op": "remove_binding", "target": "k_d1:u"},
               {"op": "remove_assumption", "text": "tmp"}]),
    );
    let new = accepted(&run(&doc, &p));
    assert_eq!(new.assumptions, doc.assumptions);
}

#[test]
fn list_caps_are_respected() {
    let doc = example_a();
    let ops: Vec<_> = (0..33)
        .map(|i| json!({"op": "add_note", "text": format!("n{i}")}))
        .collect();
    let p = patch(Role::Joiner, 4, json!(ops));
    let out = run(&doc, &p);
    rejected_with(&out, RejectCode::SchemaInvalid);
    assert_eq!(out.reason().unwrap().op_index, Some(32));
}

#[test]
fn atomicity_third_op_failure_changes_nothing() {
    let doc = example_a();
    let before = canon(&doc);
    let p = patch(
        Role::ComponentSmith,
        4,
        json!([dot("d_a"), {"op": "add_note", "text": "x"},
               {"op": "set_field", "target": "zz:length", "value": 1}, dot("d_b")]),
    );
    let out = run(&doc, &p);
    rejected_with(&out, RejectCode::UnknownTarget);
    assert_eq!(out.reason().unwrap().op_index, Some(2));
    assert_eq!(canon(&doc), before);
}

// ---- one test per reject reason -------------------------------------------------------------

#[test]
fn reject_schema_invalid() {
    let doc = example_a();
    // Empty ops list.
    let p = patch(Role::Joiner, 4, json!([]));
    rejected_with(&run(&doc, &p), RejectCode::SchemaInvalid);
    // Payload rule: negative Dim in add_component.
    let p = patch(
        Role::ComponentSmith,
        4,
        json!([plate("v_bad", -1.0, 10.0, 1.0)]),
    );
    let out = run(&doc, &p);
    rejected_with(&out, RejectCode::SchemaInvalid);
    assert_eq!(out.reason().unwrap().op_index, Some(0));
    // Non-finite set_field value (not expressible in JSON, but in Rust).
    let mut p = patch(
        Role::ComponentSmith,
        4,
        json!([{"op": "set_field", "target": "e_h1:diameter", "value": 1}]),
    );
    if let promptcad_core::model::PatchOp::SetField { value, .. } = &mut p.ops[0] {
        *value = f64::NAN;
    }
    rejected_with(&run(&doc, &p), RejectCode::SchemaInvalid);
    // Op outside the role's schema subset (joiner cannot add parts).
    let p = patch(
        Role::Joiner,
        4,
        json!([{"op": "set_body", "part_id": "p_plate", "body": [{"op": "add", "component": "v_plate"}]}]),
    );
    rejected_with(&run(&doc, &p), RejectCode::SchemaInvalid);
    // The patched document violates the schema (a Dim set to a negative value).
    let p = patch(
        Role::ComponentSmith,
        4,
        json!([{"op": "set_field", "target": "e_h1:diameter", "value": -2}]),
    );
    rejected_with(&run(&doc, &p), RejectCode::SchemaInvalid);
}

#[test]
fn reject_stale_revision() {
    let doc = example_a();
    for base in [0, 3, 5] {
        let p = patch(Role::Joiner, base, json!([{"op": "add_note", "text": "x"}]));
        rejected_with(&run(&doc, &p), RejectCode::StaleRevision);
        rejected_with(
            &apply_patch(&doc, &p, PatchOrigin::Human),
            RejectCode::StaleRevision,
        );
    }
}

#[test]
fn reject_unknown_target() {
    let doc = example_a();
    let cases = [
        (
            Role::ComponentSmith,
            json!({"op": "set_field", "target": "zz:length", "value": 1}),
        ),
        (
            Role::ComponentSmith,
            json!({"op": "set_binding", "target": "zz:length", "expr": "len"}),
        ),
        (
            Role::ComponentSmith,
            json!({"op": "remove_binding", "target": "e_h1:diameter"}),
        ),
        (
            Role::ComponentSmith,
            json!({"op": "remove_component", "id": "zz"}),
        ),
        (
            Role::ComponentSmith,
            json!({"op": "remove_component", "id": "k_root"}),
        ),
        (
            Role::Joiner,
            json!({"op": "remove_constraint", "id": "v_plate"}),
        ),
        (
            Role::Assembler,
            json!({"op": "set_body", "part_id": "p_none",
            "body": [{"op": "add", "component": "v_plate"}]}),
        ),
    ];
    for (role, op) in cases {
        let p = patch(role, 4, json!([op.clone()]));
        let out = run(&doc, &p);
        rejected_with(&out, RejectCode::UnknownTarget);
        assert_eq!(out.reason().unwrap().op_index, Some(0), "{op}");
    }
}

#[test]
fn reject_unknown_field() {
    let doc = example_a();
    for target in ["v_plate:height", "e_h1:depth", "k_e1:value"] {
        let role = if target.starts_with('k') {
            Role::Joiner
        } else {
            Role::ComponentSmith
        };
        let p = patch(
            role,
            4,
            json!([{"op": "set_field", "target": target, "value": 1}]),
        );
        rejected_with(&run(&doc, &p), Code::UnknownField);
    }
}

#[test]
fn reject_field_is_bound() {
    let doc = example_a();
    for (role, target) in [
        (Role::ComponentSmith, "v_plate:length"),
        (Role::Joiner, "k_d3:v"),
    ] {
        let p = patch(
            role,
            4,
            json!([{"op": "set_field", "target": target, "value": 100}]),
        );
        let out = run(&doc, &p);
        rejected_with(&out, RejectCode::FieldIsBound);
        assert_eq!(out.reason().unwrap().target.as_deref(), Some(target));
    }
}

#[test]
fn reject_assumption_not_found() {
    let doc = example_a();
    let p = patch(
        Role::ComponentSmith,
        4,
        json!([dot("d_x"), {"op": "remove_assumption", "text": "no such assumption"}]),
    );
    let out = run(&doc, &p);
    rejected_with(&out, RejectCode::AssumptionNotFound);
    assert_eq!(out.reason().unwrap().op_index, Some(1));
    // The message never echoes the free text (§9.1).
    assert!(!out.reason().unwrap().message.contains("no such assumption"));
    // Also for humans.
    rejected_with(
        &apply_patch(&doc, &p, PatchOrigin::Human),
        RejectCode::AssumptionNotFound,
    );
}

#[test]
fn reject_annotation_only_removal() {
    let doc = example_a();
    let text = doc.assumptions[0].clone();
    for role in Role::ALL {
        let p = patch(
            role,
            4,
            json!([{"op": "add_note", "text": "resolved"}, {"op": "remove_assumption", "text": text}]),
        );
        let out = run(&doc, &p);
        rejected_with(&out, RejectCode::AnnotationOnlyRemoval);
        assert_eq!(out.reason().unwrap().op_index, Some(1));
    }
    // Humans may remove an assumption on its own (they are the reviewers).
    let p = patch(
        Role::Joiner,
        4,
        json!([{"op": "remove_assumption", "text": text}]),
    );
    let new = accepted(&apply_patch(&doc, &p, PatchOrigin::Human));
    assert!(new.assumptions.is_empty());
}

#[test]
fn reject_standards_mismatch() {
    let doc = example_a();
    // Modified hole: M6 clearance must stay 6.6.
    let p = patch(
        Role::ComponentSmith,
        4,
        json!([{"op": "set_field", "target": "e_h1:diameter", "value": 6.5}]),
    );
    let out = run(&doc, &p);
    rejected_with(&out, RejectCode::StandardsMismatch);
    assert_eq!(out.reason().unwrap().target.as_deref(), Some("e_h1"));
    // Added holes: tapped M6 uses the 5.0 tap drill; M8 clearance is 9.0.
    for (ty, d, thread, ok) in [
        ("tapped", 5.0, "M6x1", true),
        ("tapped", 5.2, "M6x1", false),
        ("clearance", 9.0, "M8", true),
        ("clearance", 8.4, "M8", false),
        ("clearance", 7.7, "M7", true), // not in the table
        ("through", 6.0, "M6", true),   // not a checked hole type
    ] {
        let p = patch(
            Role::ComponentSmith,
            4,
            json!([hole("e_new", ty, d, Some(thread))]),
        );
        let out = run(&doc, &p);
        if ok {
            accepted(&out);
        } else {
            rejected_with(&out, RejectCode::StandardsMismatch);
        }
    }
    // Unchanged holes are not re-checked (a pre-existing off-table hole stays acceptable).
    let mut odd = doc.clone();
    if let Some(c) = odd.component_mut("e_h4") {
        if let promptcad_core::model::Geometry::Empty {
            shape: promptcad_core::model::v2::EmptyShape::Hole(h),
            ..
        } = &mut c.geometry
        {
            h.diameter = 6.4;
        }
    }
    let p = patch(Role::Joiner, 4, json!([{"op": "add_note", "text": "x"}]));
    accepted(&run(&odd, &p));
}

// ---- P-class rejections ---------------------------------------------------------------------

#[test]
fn reject_dangling_reference_after_remove_component() {
    let doc = example_a();
    for id in ["d_h1", "v_plate", "e_h2"] {
        let p = patch(
            Role::ComponentSmith,
            4,
            json!([{"op": "remove_component", "id": id}]),
        );
        let out = run(&doc, &p);
        rejected_with(&out, Code::DanglingReference);
        assert_eq!(out.reason().unwrap().target.as_deref(), Some(id));
    }
}

#[test]
fn reject_duplicate_id() {
    let doc = example_a();
    let cases = [
        (Role::ComponentSmith, dot("v_plate")),
        (Role::ComponentSmith, dot("k_root")), // N1 spans components and constraints
        (
            Role::Joiner,
            json!({"op": "add_constraint", "constraint":
            {"id": "e_h1", "kind": "fixed", "a": "d_h1.point", "b": null, "value": null,
             "u": null, "v": null, "frame": {"origin": [0, 0, 0], "x_axis": "+x", "z_axis": "+z"}}}),
        ),
        (
            Role::Assembler,
            json!({"op": "add_assembly", "assembly":
            {"id": "p_plate", "name": "a", "children": ["p_plate"]}}),
        ),
    ];
    for (role, op) in cases {
        let p = patch(role, 4, json!([op]));
        rejected_with(&run(&doc, &p), Code::DuplicateId);
    }
    // Within one patch.
    let p = patch(Role::ComponentSmith, 4, json!([dot("d_x"), dot("d_x")]));
    let out = run(&doc, &p);
    rejected_with(&out, Code::DuplicateId);
    assert_eq!(out.reason().unwrap().op_index, Some(1));
}

#[test]
fn reject_binding_cycle() {
    let doc = example_a();
    let p = patch(
        Role::ComponentSmith,
        4,
        json!([
            {"op": "set_binding", "target": "v_plate:length", "expr": "v_plate:width + 1"},
            {"op": "set_binding", "target": "v_plate:width", "expr": "v_plate:length - 1"}
        ]),
    );
    let out = run(&doc, &p);
    rejected_with(&out, Code::BindingCycle);
    let r = out.rejected().unwrap();
    assert!(!r.issues.is_empty());
    assert!(r
        .issues
        .iter()
        .all(|i| i.code().is_some_and(Code::is_patch_blocking)));
    // P-class issues are owned by the submitter.
    assert!(r
        .issues
        .iter()
        .any(|i| i.owner == Some(Role::ComponentSmith)));
}

#[test]
fn reject_binding_errors_from_validation() {
    let doc = example_a();
    for (expr, code) in [
        ("len +", Code::BindingParseError),
        ("nope * 2", Code::BindingUnknownName),
        ("len / 0", Code::BindingEvalError),
    ] {
        let p = patch(
            Role::ComponentSmith,
            4,
            json!([{"op": "set_binding", "target": "v_plate:length", "expr": expr}]),
        );
        rejected_with(&run(&doc, &p), code);
    }
}

// ---- undo ------------------------------------------------------------------------------------

#[test]
fn undo_restores_the_pre_image_byte_for_byte() {
    let docs = build_example_a();
    for (i, (role, p)) in example_a_patches().into_iter().enumerate() {
        let out = apply_patch(&docs[i], &p, agent(role));
        let a = out.accepted().unwrap();
        let restored = undo(&a.document, &a.revert).unwrap();
        assert_eq!(canon(&restored), canon(&docs[i]));
    }
    // Revert only carries the changed top-level keys.
    let doc = example_a();
    let p = patch(Role::Joiner, 4, json!([{"op": "add_note", "text": "x"}]));
    let out = run(&doc, &p);
    let keys: Vec<&str> = out.accepted().unwrap().revert.keys().collect();
    assert_eq!(keys, ["notes", "revision"]);
}
