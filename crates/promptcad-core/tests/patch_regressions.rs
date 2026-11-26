//! Regressions: the patch engine never panics on untrusted input; revision overflow; forged
//! `resolves` (§8.1 / §8.5).

mod patch_support;

use patch_support::*;
use promptcad_core::model::{Patch, RejectCode, Role};
use promptcad_core::oplog::Session;
use promptcad_core::patch_engine::{apply_patch, PatchOrigin};
use proptest::prelude::*;
use serde_json::{json, Value};

fn nesting_patch(depth: usize) -> Patch {
    // a1 -> a2 -> ... -> a<depth> -> p_plate
    let ops: Vec<Value> = (1..=depth)
        .map(|i| {
            let child = if i == depth {
                "p_plate".to_owned()
            } else {
                format!("a{}", i + 1)
            };
            json!({"op": "add_assembly", "assembly":
                {"id": format!("a{i}"), "name": format!("a{i}"), "children": [child]}})
        })
        .collect();
    patch(Role::Assembler, 4, json!(ops))
}

fn assembler() -> PatchOrigin {
    PatchOrigin::Agent(Role::Assembler)
}

#[test]
fn five_deep_assembly_nesting_is_rejected_not_a_panic() {
    let doc = example_a();
    let before = canon(&doc);
    let out = apply_patch(&doc, &nesting_patch(5), assembler());
    rejected_with(&out, RejectCode::SchemaInvalid);
    assert!(!out.issues().is_empty(), "the schema issue stays attached");
    assert_eq!(canon(&doc), before);
    // Four deep is still fine.
    accepted(&apply_patch(&doc, &nesting_patch(4), assembler()));
}

#[test]
fn five_deep_assembly_nesting_through_session_is_logged_and_leaves_document_alone() {
    let doc = example_a();
    let before = canon(&doc);
    let mut s = Session::new(doc);
    let out = s.submit(&nesting_patch(5), assembler());
    rejected_with(out, RejectCode::SchemaInvalid);
    assert_eq!(s.log().len(), 1);
    let e = s.log().last().unwrap();
    assert!(!e.accepted);
    assert_eq!(e.reject_code(), Some("schema_invalid"));
    assert_eq!(canon(s.document()), before);
    assert_eq!(s.undo_depth(), 0);
}

fn max_rev_doc() -> promptcad_core::model::CadDocument2 {
    let mut doc = example_a();
    doc.revision = u64::MAX;
    doc
}

fn note_patch(role: Role, base: u64) -> Patch {
    patch(role, base, json!([{"op": "add_note", "text": "hi"}]))
}

#[test]
fn revision_u64_max_rejects_instead_of_overflowing() {
    let doc = max_rev_doc();
    let before = canon(&doc);
    let p = note_patch(Role::ComponentSmith, u64::MAX);
    let out = apply_patch(&doc, &p, PatchOrigin::Agent(Role::ComponentSmith));
    rejected_with(&out, RejectCode::SchemaInvalid);
    assert!(out.reason().unwrap().message.contains("overflow"));
    rejected_with(
        &apply_patch(&doc, &p, PatchOrigin::Human),
        RejectCode::SchemaInvalid,
    );
    assert_eq!(canon(&doc), before);

    // Through a session: logged, document untouched.
    let mut s = Session::new(doc);
    rejected_with(
        s.submit(&p, PatchOrigin::Agent(Role::ComponentSmith)),
        RejectCode::SchemaInvalid,
    );
    assert_eq!(s.log().len(), 1);
    assert_eq!(canon(s.document()), before);

    // One below the maximum still works.
    let mut doc = example_a();
    doc.revision = u64::MAX - 1;
    let p = note_patch(Role::ComponentSmith, u64::MAX - 1);
    let new = accepted(&apply_patch(
        &doc,
        &p,
        PatchOrigin::Agent(Role::ComponentSmith),
    ));
    assert_eq!(new.revision, u64::MAX);
}

// ---- forged `resolves` ------------------------------------------------------------------------

fn base() -> promptcad_core::model::CadDocument2 {
    example_a()
}

fn margin_op() -> Value {
    json!([{"op": "set_parameter", "parameter":
        {"name": "margin", "value": 11, "min": null, "max": null, "note": null}}])
}

#[test]
fn unknown_resolves_entry_is_schema_invalid_for_everyone() {
    let doc = base();
    for origin in [
        PatchOrigin::Agent(Role::ComponentSmith),
        PatchOrigin::Agent(Role::SpacePlanner),
        PatchOrigin::Human,
    ] {
        let agent = origin.role().unwrap_or(Role::ComponentSmith);
        let p = patch_resolving(agent, doc.revision, &["x"], margin_op());
        let out = apply_patch(&doc, &p, origin);
        rejected_with(&out, RejectCode::SchemaInvalid);
        assert!(!out.reason().unwrap().message.contains("\"x\""));
    }
}

#[test]
fn smith_repair_unlock_requires_an_issue_present_in_the_report() {
    let doc = base();
    let smith = PatchOrigin::Agent(Role::ComponentSmith);
    // Valid code, but the document has no such issue: forged.
    let forged = patch_resolving(
        Role::ComponentSmith,
        doc.revision,
        &["low_edge_clearance"],
        margin_op(),
    );
    let out = apply_patch(&doc, &forged, smith);
    rejected_with(&out, RejectCode::PermissionDenied);
    assert_eq!(out.reason().unwrap().op_index, Some(0));
    // Present in the pre-patch report (example A has thread_annotation_only): unlocked.
    let genuine = patch_resolving(
        Role::ComponentSmith,
        doc.revision,
        &["low_edge_clearance", "thread_annotation_only"],
        margin_op(),
    );
    accepted(&apply_patch(&doc, &genuine, smith));
    // Humans and the space planner are not subject to the presence check.
    accepted(&apply_patch(&doc, &forged, PatchOrigin::Human));
    let planner = patch_resolving(
        Role::SpacePlanner,
        doc.revision,
        &["low_edge_clearance"],
        margin_op(),
    );
    accepted(&apply_patch(
        &doc,
        &planner,
        PatchOrigin::Agent(Role::SpacePlanner),
    ));
    // Empty resolves: component_smith still has no set_parameter.
    let none = patch(Role::ComponentSmith, doc.revision, margin_op());
    rejected_with(
        &apply_patch(&doc, &none, smith),
        RejectCode::PermissionDenied,
    );
}

// ---- arbitrary patches ------------------------------------------------------------------------

const IDS: [&str; 12] = [
    "v_plate", "d_h1", "e_h1", "k_d1", "k_root", "p_plate", "a1", "x_new", "e_pcb", "len",
    "margin", "",
];
const FIELDS: [&str; 6] = ["length", "u", "v", "x", "value", ""];
const EXPRS: [&str; 5] = ["len", "margin * 2", "1 / 0", "(", "k_d1"];
const RESOLVES: [&str; 6] = [
    "thin_wall",
    "low_edge_clearance",
    "thread_annotation_only",
    "binding_cycle",
    "nonsense",
    "schema_invalid",
];

fn id() -> impl Strategy<Value = String> {
    prop::sample::select(IDS.to_vec()).prop_map(str::to_owned)
}

fn field_ref() -> impl Strategy<Value = String> {
    (id(), prop::sample::select(FIELDS.to_vec())).prop_map(|(i, f)| format!("{i}:{f}"))
}

fn op() -> impl Strategy<Value = Value> {
    prop_oneof![
        id().prop_map(|i| dot(&i)),
        (id(), -5.0f64..200.0).prop_map(|(i, l)| plate(&i, l, l, l)),
        (id(), -5.0f64..50.0).prop_map(|(i, l)| box_empty(&i, "reserve", l, l, l)),
        (id(), 0.0f64..20.0).prop_map(|(i, d)| hole(&i, "clearance", d, Some("M6"))),
        id().prop_map(|i| json!({"op": "remove_component", "id": i})),
        id().prop_map(|i| json!({"op": "remove_constraint", "id": i})),
        (field_ref(), -1e9f64..1e9)
            .prop_map(|(t, v)| json!({"op": "set_field", "target": t, "value": v})),
        (field_ref(), prop::sample::select(EXPRS.to_vec()))
            .prop_map(|(t, e)| json!({"op": "set_binding", "target": t, "expr": e})),
        field_ref().prop_map(|t| json!({"op": "remove_binding", "target": t})),
        (id(), -10.0f64..10.0).prop_map(|(n, v)| json!({"op": "set_parameter", "parameter":
            {"name": n, "value": v, "min": null, "max": null, "note": null}})),
        (id(), id()).prop_map(|(a, c)| json!({"op": "add_assembly", "assembly":
            {"id": a, "name": "n", "children": [c]}})),
        (id(), id()).prop_map(|(p, c)| json!({"op": "set_body", "part_id": p,
            "body": [{"op": "add", "component": c}]})),
        Just(json!({"op": "add_note", "text": "n"})),
        prop::sample::select(vec!["a", "n"])
            .prop_map(|t| json!({"op": "remove_assumption", "text": t})),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn arbitrary_patches_never_panic_and_rejections_leave_input_untouched(
        which in 0usize..2,
        role in prop::sample::select(Role::ALL.to_vec()),
        origin in 0usize..5,
        rev_choice in 0u8..4,
        resolves in prop::collection::vec(prop::sample::select(RESOLVES.to_vec()), 0..3),
        ops in prop::collection::vec(op(), 0..7),
    ) {
        let mut doc = if which == 0 { example_a() } else { example_b() };
        if rev_choice == 3 {
            doc.revision = u64::MAX;
        }
        let base = if rev_choice == 2 { doc.revision.wrapping_add(1) } else { doc.revision };
        let Ok(p) = serde_json::from_value::<Patch>(json!({
            "agent": role, "base_revision": base, "rationale": "r",
            "resolves": resolves, "ops": ops,
        })) else {
            return Ok(());
        };
        let origin = if origin == 4 {
            PatchOrigin::Human
        } else {
            PatchOrigin::Agent(Role::ALL[origin])
        };
        let before = canon(&doc);
        let out = apply_patch(&doc, &p, origin);
        prop_assert_eq!(canon(&doc), before);
        if let Some(a) = out.accepted() {
            prop_assert_eq!(Some(a.document.revision), doc.revision.checked_add(1));
        }
        // The same through a session: the outcome is always logged.
        let mut s = Session::new(doc);
        s.submit(&p, origin);
        prop_assert_eq!(s.log().len(), 1);
    }
}
