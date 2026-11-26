//! Role schema subsets (§8.3) and the permission matrix (§8.5), table-driven.

mod patch_support;

use patch_support::*;
use promptcad_core::model::{CadDocument2, Patch, RejectCode, Role};
use promptcad_core::patch_engine::{
    apply_patch, op_class, permits, role_allows_op, OpClass, PatchOrigin, PatchOutcome,
};
use serde_json::{json, Value};

/// §8.5 transcribed: rows × [space_planner, component_smith, joiner, assembler].
const MATRIX: [(OpClass, [bool; 4]); 10] = [
    (OpClass::SetParameter, [true, false, false, false]),
    (OpClass::SetParameterRepair, [true, true, false, false]),
    (OpClass::AddReservedComponent, [true, false, false, false]),
    (OpClass::AddPhysicalComponent, [false, true, false, false]),
    (OpClass::EditReserved, [true, false, false, false]),
    (OpClass::EditPhysical, [false, true, false, false]),
    (OpClass::Constraint, [false, false, true, false]),
    (OpClass::Assembly, [false, false, false, true]),
    (OpClass::Annotation, [true, true, true, true]),
    (OpClass::RemoveAssumption, [true, true, true, true]),
];

#[test]
fn permission_matrix_is_exactly_spec_8_5() {
    assert_eq!(MATRIX.len(), OpClass::ALL.len());
    for (class, row) in MATRIX {
        for (i, role) in Role::ALL.into_iter().enumerate() {
            assert_eq!(permits(role, class), row[i], "{role} × {class:?}");
        }
    }
}

/// Example A plus a keep-out Empty (`e_keep`, reserved class), a free Dot (`d_free`) and an
/// unowned Volume (`v_free`), added by a human so every op class has a target.
fn base() -> CadDocument2 {
    let doc = example_a();
    let p = patch(
        Role::SpacePlanner,
        4,
        json!([
            box_empty("e_keep", "keep_out", 20.0, 20.0, 5.0),
            dot("d_free"),
            plate("v_free", 30.0, 30.0, 3.0)
        ]),
    );
    accepted(&apply_patch(&doc, &p, PatchOrigin::Human))
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Expect {
    /// Accepted.
    A,
    /// `schema_invalid`: the op is not in the role's schema subset (§8.3).
    S,
    /// `permission_denied`: in the subset, but the matrix denies this target class (§8.5).
    P,
}
use Expect::{A, P, S};

/// A permitted design op of each role (used to make `remove_assumption` patches legal).
fn own_design_op(role: Role) -> Value {
    match role {
        Role::SpacePlanner => box_empty("e_res2", "reserve", 10.0, 10.0, 10.0),
        Role::ComponentSmith => dot("d_more"),
        Role::Joiner => json!({"op": "remove_binding", "target": "k_d1:u"}),
        Role::Assembler => json!({"op": "set_body", "part_id": "p_plate",
            "body": [{"op": "add", "component": "v_plate"}, {"op": "cut", "component": "e_h1"},
                     {"op": "cut", "component": "e_h2"}, {"op": "cut", "component": "e_h3"},
                     {"op": "cut", "component": "e_h4"}]}),
    }
}

struct Case {
    name: &'static str,
    class: OpClass,
    resolves: &'static [&'static str],
    ops: fn(Role, &CadDocument2) -> Vec<Value>,
    expect: [Expect; 4],
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "set_parameter",
            class: OpClass::SetParameter,
            resolves: &[],
            ops: |_, _| {
                vec![json!({"op": "set_parameter", "parameter":
                {"name": "margin", "value": 11, "min": null, "max": null, "note": "孔邊距"}})]
            },
            expect: [A, P, S, S],
        },
        Case {
            name: "set_parameter (repair)",
            class: OpClass::SetParameterRepair,
            resolves: &["thread_annotation_only"],
            ops: |_, _| {
                vec![json!({"op": "set_parameter", "parameter":
                {"name": "margin", "value": 11, "min": null, "max": null, "note": "孔邊距"}})]
            },
            expect: [A, A, S, S],
        },
        Case {
            name: "add_component reserved",
            class: OpClass::AddReservedComponent,
            resolves: &[],
            ops: |_, _| vec![box_empty("e_new", "reserve", 10.0, 10.0, 10.0)],
            expect: [A, S, S, S],
        },
        Case {
            name: "add_component physical",
            class: OpClass::AddPhysicalComponent,
            resolves: &[],
            ops: |_, _| vec![dot("d_new")],
            expect: [S, A, S, S],
        },
        Case {
            name: "set_field reserved",
            class: OpClass::EditReserved,
            resolves: &[],
            ops: |_, _| vec![json!({"op": "set_field", "target": "e_keep:length", "value": 30})],
            expect: [A, P, P, S],
        },
        Case {
            name: "set_binding reserved",
            class: OpClass::EditReserved,
            resolves: &[],
            ops: |_, _| {
                vec![json!({"op": "set_binding", "target": "e_keep:height", "expr": "thk"})]
            },
            expect: [A, P, P, S],
        },
        Case {
            name: "remove_component reserved",
            class: OpClass::EditReserved,
            resolves: &[],
            ops: |_, _| vec![json!({"op": "remove_component", "id": "e_keep"})],
            expect: [A, P, S, S],
        },
        Case {
            name: "set_field physical",
            class: OpClass::EditPhysical,
            resolves: &[],
            ops: |_, _| vec![json!({"op": "set_field", "target": "e_h1:diameter", "value": 6.6})],
            expect: [P, A, P, S],
        },
        Case {
            name: "remove_binding physical",
            class: OpClass::EditPhysical,
            resolves: &[],
            ops: |_, _| vec![json!({"op": "remove_binding", "target": "v_plate:thickness"})],
            expect: [P, A, P, S],
        },
        Case {
            name: "remove_component physical",
            class: OpClass::EditPhysical,
            resolves: &[],
            ops: |_, _| vec![json!({"op": "remove_component", "id": "d_free"})],
            expect: [P, A, S, S],
        },
        Case {
            name: "add_constraint",
            class: OpClass::Constraint,
            resolves: &[],
            ops: |_, _| {
                vec![json!({"op": "add_constraint", "constraint":
                {"id": "k_keep", "kind": "fixed", "a": "e_keep.origin", "b": null, "value": null,
                 "u": null, "v": null,
                 "frame": {"origin": [0, 0, 40], "x_axis": "+x", "z_axis": "+z"}}})]
            },
            expect: [S, S, A, S],
        },
        Case {
            name: "remove_constraint",
            class: OpClass::Constraint,
            resolves: &[],
            ops: |_, _| vec![json!({"op": "remove_constraint", "id": "k_d4"})],
            expect: [S, S, A, S],
        },
        Case {
            name: "set_field / remove_binding constraint",
            class: OpClass::Constraint,
            resolves: &[],
            ops: |_, _| {
                vec![
                    json!({"op": "remove_binding", "target": "k_d1:u"}),
                    json!({"op": "set_field", "target": "k_d1:u", "value": -45}),
                ]
            },
            expect: [P, P, A, S],
        },
        Case {
            name: "add_part",
            class: OpClass::Assembly,
            resolves: &[],
            ops: |_, _| {
                vec![
                    json!({"op": "add_part", "part": {"id": "p_free", "name": "f",
                "material": null, "quantity": 1, "source": "purchased",
                "body": [{"op": "add", "component": "v_free"}], "fillets": [], "chamfers": []}}),
                ]
            },
            expect: [S, S, S, A],
        },
        Case {
            name: "set_body",
            class: OpClass::Assembly,
            resolves: &[],
            ops: |_, _| vec![own_design_op(Role::Assembler)],
            expect: [S, S, S, A],
        },
        Case {
            name: "add_note / add_assumption",
            class: OpClass::Annotation,
            resolves: &[],
            ops: |_, _| {
                vec![
                    json!({"op": "add_note", "text": "n"}),
                    json!({"op": "add_assumption", "text": "a"}),
                ]
            },
            expect: [A, A, A, A],
        },
        Case {
            name: "remove_assumption (+ own design op)",
            class: OpClass::RemoveAssumption,
            resolves: &[],
            ops: |r, d| {
                vec![
                    json!({"op": "remove_assumption", "text": d.assumptions[0]}),
                    own_design_op(r),
                ]
            },
            expect: [A, A, A, A],
        },
    ]
}

fn reject_code(out: &PatchOutcome) -> Option<String> {
    out.reason().map(|r| r.code.to_string())
}

#[test]
fn every_role_times_every_op_class_end_to_end() {
    let doc = base();
    for case in cases() {
        for (i, role) in Role::ALL.into_iter().enumerate() {
            let p: Patch = patch_resolving(
                role,
                doc.revision,
                case.resolves,
                json!((case.ops)(role, &doc)),
            );
            // The class the engine computes is the row under test (first op).
            assert_eq!(
                op_class(&doc, &p, &p.ops[0]),
                Some(case.class),
                "{}",
                case.name
            );
            let in_schema = p.ops.iter().all(|op| role_allows_op(role, op));
            let expect = case.expect[i];
            assert_eq!(
                in_schema,
                expect != S,
                "{} × {role}: schema subset",
                case.name
            );
            assert_eq!(
                permits(role, case.class),
                expect == A,
                "{} × {role}: matrix",
                case.name
            );
            let out = apply_patch(&doc, &p, PatchOrigin::Agent(role));
            let got = reject_code(&out);
            let want = match expect {
                A => None,
                S => Some(RejectCode::SchemaInvalid.as_str().to_owned()),
                P => Some(RejectCode::PermissionDenied.as_str().to_owned()),
            };
            assert_eq!(got, want, "{} × {role}: {:?}", case.name, out.reason());
            if expect == P {
                assert_eq!(out.reason().unwrap().op_index, Some(0));
            }
        }
    }
}

#[test]
fn matrix_applies_to_the_evolving_document() {
    let doc = example_a();
    // The space planner creates a reserve Empty and edits it in the same patch: allowed,
    // because the class of `e_new` is decided after op 0 ran.
    let p = patch(
        Role::SpacePlanner,
        4,
        json!([box_empty("e_new", "reserve", 10.0, 10.0, 10.0),
               {"op": "set_field", "target": "e_new:height", "value": 12},
               {"op": "set_binding", "target": "e_new:length", "expr": "thk * 2"}]),
    );
    let new = accepted(&apply_patch(
        &doc,
        &p,
        PatchOrigin::Agent(Role::SpacePlanner),
    ));
    assert_eq!(
        new.binding(&"e_new:length".parse().unwrap()).unwrap().expr,
        "thk * 2"
    );
    // ...but the component smith may not touch a reserved Empty it cannot create.
    let p = patch(
        Role::ComponentSmith,
        5,
        json!([dot("d_x"), {"op": "set_field", "target": "e_new:height", "value": 1}]),
    );
    let out = apply_patch(&new, &p, PatchOrigin::Agent(Role::ComponentSmith));
    rejected_with(&out, RejectCode::PermissionDenied);
    assert_eq!(out.reason().unwrap().op_index, Some(1));
    assert_eq!(
        out.reason().unwrap().target.as_deref(),
        Some("e_new:height")
    );
}

#[test]
fn permission_checks_precede_op_failures() {
    let doc = example_a();
    // op 0 fails at step 4 (unknown target), op 1 is denied at step 3: step 3 wins.
    let p = patch(
        Role::SpacePlanner,
        4,
        json!([{"op": "set_field", "target": "zz:length", "value": 1},
               {"op": "set_field", "target": "e_h1:diameter", "value": 6.6}]),
    );
    let out = apply_patch(&doc, &p, PatchOrigin::Agent(Role::SpacePlanner));
    rejected_with(&out, RejectCode::PermissionDenied);
    assert_eq!(out.reason().unwrap().op_index, Some(1));
}

#[test]
fn submitting_role_must_match_patch_agent() {
    let doc = example_a();
    let p = patch(
        Role::SpacePlanner,
        4,
        json!([{"op": "add_note", "text": "x"}]),
    );
    for role in [Role::ComponentSmith, Role::Joiner, Role::Assembler] {
        rejected_with(
            &apply_patch(&doc, &p, PatchOrigin::Agent(role)),
            RejectCode::PermissionDenied,
        );
    }
    accepted(&apply_patch(
        &doc,
        &p,
        PatchOrigin::Agent(Role::SpacePlanner),
    ));
}

#[test]
fn human_is_not_bound_by_the_matrix() {
    let doc = base();
    // One patch mixing every class, labelled with an arbitrary agent: accepted for a human.
    let p = patch(
        Role::Joiner,
        doc.revision,
        json!([
            {"op": "set_parameter", "parameter": {"name": "margin", "value": 11, "min": null, "max": null, "note": null}},
            box_empty("e_new", "envelope", 200.0, 100.0, 50.0),
            dot("d_new"),
            {"op": "set_field", "target": "e_keep:length", "value": 25},
            {"op": "set_field", "target": "e_h1:diameter", "value": 6.6},
            {"op": "remove_binding", "target": "k_d1:v"},
            own_design_op(Role::Assembler),
            {"op": "add_note", "text": "human edit"}
        ]),
    );
    let new = accepted(&apply_patch(&doc, &p, PatchOrigin::Human));
    assert_eq!(new.revision, doc.revision + 1);
    // The same patch from the joiner fails at the schema subset.
    rejected_with(
        &apply_patch(&doc, &p, PatchOrigin::Agent(Role::Joiner)),
        RejectCode::SchemaInvalid,
    );
}

#[test]
fn role_schema_subsets() {
    let reserve = box_empty("e_r", "reserve", 1.0, 1.0, 1.0);
    let cut = hole("e_c", "through", 3.0, None);
    let vol = plate("v_p", 1.0, 1.0, 1.0);
    let op =
        |v: &Value| serde_json::from_value::<promptcad_core::model::PatchOp>(v.clone()).unwrap();
    assert!(role_allows_op(Role::SpacePlanner, &op(&reserve)));
    assert!(!role_allows_op(Role::SpacePlanner, &op(&cut)));
    assert!(!role_allows_op(Role::SpacePlanner, &op(&vol)));
    assert!(!role_allows_op(Role::ComponentSmith, &op(&reserve)));
    assert!(role_allows_op(Role::ComponentSmith, &op(&cut)));
    assert!(role_allows_op(Role::ComponentSmith, &op(&vol)));
    for r in [Role::Joiner, Role::Assembler] {
        assert!(!role_allows_op(r, &op(&reserve)));
        assert!(!role_allows_op(r, &op(&vol)));
    }
    let set_param = json!({"op": "set_parameter", "parameter": {"name": "x", "value": 1, "min": null, "max": null, "note": null}});
    assert!(role_allows_op(Role::ComponentSmith, &op(&set_param)));
    assert!(!role_allows_op(Role::Joiner, &op(&set_param)));
    assert!(!role_allows_op(Role::Assembler, &op(&set_param)));
}
