//! Property tests: `validate` never panics and its report is internally consistent, on
//! arbitrary upgraded 1.0 documents and on randomly mutated 2.0 examples.

use std::path::PathBuf;

use promptcad_core::canonical::parse_example;
use promptcad_core::migrate::upgrade_1_to_2;
use promptcad_core::model::{CadDocument1, CadDocument2, Role, Severity};
use promptcad_core::validate::{blocking_issues, compute_root, validate, ValidateOptions};
use proptest::prelude::*;
use serde_json::{json, Value};

fn check_invariants(doc: &CadDocument2) {
    for (completed, submitter) in [(true, None), (false, Some(Role::Joiner))] {
        let opts = ValidateOptions {
            completed,
            submitter,
        };
        let v = validate(doc, &opts);
        let has_error = v
            .report
            .issues
            .iter()
            .any(|i| i.severity == Severity::Error);
        assert_eq!(v.report.valid, !has_error);
        if has_error {
            assert!(v.report.review_required);
        }
        let blocking = blocking_issues(&v.report);
        assert!(blocking.iter().all(|b| v.report.issues.contains(b)));
        for i in &blocking {
            if i.code != "invalid_root" {
                assert_eq!(i.owner, submitter, "{}", i.code);
            }
        }
        assert_eq!(v.resolved.revision, doc.revision);
        // deterministic
        assert_eq!(v, validate(doc, &opts));
    }
    let _ = compute_root(doc);
}

// ---- arbitrary 1.0 documents ----------------------------------------------------------------

fn base_strategy() -> impl Strategy<Value = Value> {
    prop_oneof![
        (1.0..300.0f64, 1.0..300.0f64, 0.5..50.0f64).prop_map(
            |(l, w, t)| json!({"kind": "plate", "length": l, "width": w, "thickness": t})
        ),
        (1.0..200.0f64, 0.5..100.0f64)
            .prop_map(|(d, h)| json!({"kind": "cylinder", "diameter": d, "height": h})),
        (2.0..200.0f64, 0.05..0.95f64, 0.5..50.0f64).prop_map(|(od, f, h)| {
            json!({"kind": "ring", "outer_diameter": od, "inner_diameter": od * f, "height": h})
        }),
        (1.0..200.0f64, 2.0..100.0f64, 2.0..100.0f64, 0.05..0.95f64).prop_map(|(w, d, vh, f)| {
            json!({"kind": "l_bracket", "width": w, "depth": d, "vertical_height": vh,
                   "thickness": d.min(vh) * f})
        }),
        (2.0..300.0f64, 2.0..300.0f64, 1.0..100.0f64, 0.05..0.45f64).prop_map(|(l, w, h, f)| {
            json!({"kind": "enclosure", "length": l, "width": w, "height": h,
                   "wall_thickness": (l.min(w) * f).min(h * 0.9)})
        }),
    ]
}

fn hole_strategy() -> impl Strategy<Value = Value> {
    (
        (-150.0..150.0f64, -150.0..150.0f64, -50.0..80.0f64),
        prop_oneof![Just("x"), Just("y"), Just("z")],
        0.5..40.0f64,
        0usize..6,
        (0.5..60.0f64, 1.05..3.0f64, 10.0..170.0f64),
        prop_oneof![Just(Value::Null), Just(json!("M6")), Just(json!(" m8x1.25 ")), Just(json!("junk"))],
    )
        .prop_map(|((x, y, z), axis, d, t, (depth, k, angle), thread)| {
            let types = ["through", "blind", "clearance", "tapped", "counterbore", "countersink"];
            let ty = types[t];
            json!({
                "kind": "hole", "x": x, "y": y, "z": z, "axis": axis, "diameter": d,
                "hole_type": ty,
                "depth": if ty == "blind" { json!(depth) } else { Value::Null },
                "thread": thread,
                "counterbore_diameter": if ty == "counterbore" { json!(d * k) } else { Value::Null },
                "counterbore_depth": if ty == "counterbore" { json!(depth) } else { Value::Null },
                "countersink_diameter": if ty == "countersink" { json!(d * k) } else { Value::Null },
                "countersink_angle": if ty == "countersink" { json!(angle) } else { Value::Null },
            })
        })
}

fn edge_strategy(kind: &'static str) -> impl Strategy<Value = Value> {
    (
        0.1..60.0f64,
        prop_oneof![Just("all"), Just("vertical"), Just("top"), Just("bottom")],
    )
        .prop_map(move |(r, sel)| {
            let key = if kind == "fillet" {
                "radius"
            } else {
                "distance"
            };
            json!({"kind": kind, key: r, "selector": sel})
        })
}

fn v1_strategy() -> impl Strategy<Value = Value> {
    (
        base_strategy(),
        prop::collection::vec(hole_strategy(), 0..8),
        prop::collection::vec(edge_strategy("fillet"), 0..3),
        prop::collection::vec(edge_strategy("chamfer"), 0..3),
        prop::collection::vec(Just("a".to_owned()), 0..3),
    )
        .prop_map(|(base, holes, fillets, chamfers, assumptions)| {
            json!({
                "schema_version": "1.0", "name": "p", "source_prompt": "p", "unit": "mm",
                "material": null, "base": base, "holes": holes, "fillets": fillets,
                "chamfers": chamfers, "assumptions": assumptions, "notes": [],
                "planner": {"planner": "t", "confidence": 1.0, "review_required": false}
            })
        })
}

// ---- mutated examples -----------------------------------------------------------------------

fn examples() -> Vec<Value> {
    ["plate-four-holes.cad2.json", "enclosure-with-lid.cad2.json"]
        .iter()
        .map(|n| {
            let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/cad2")
                .join(n);
            let doc = parse_example(&std::fs::read_to_string(p).unwrap())
                .unwrap()
                .0;
            serde_json::to_value(doc).unwrap()
        })
        .collect()
}

#[derive(Debug, Clone)]
enum Mutation {
    DeleteComponent(usize),
    DeleteConstraint(usize),
    RetargetPort { k: usize, b: bool, to: usize },
    RetargetBody { p: usize, op: usize, to: usize },
    RetargetChild { to: usize },
    DuplicateComponent(usize),
    ScaleField { c: usize, f: f64 },
    DropParts,
    MoveRoot(usize),
}

fn mutation() -> impl Strategy<Value = Mutation> {
    prop_oneof![
        (0usize..16).prop_map(Mutation::DeleteComponent),
        (0usize..16).prop_map(Mutation::DeleteConstraint),
        (0usize..16, any::<bool>(), 0usize..16).prop_map(|(k, b, to)| Mutation::RetargetPort {
            k,
            b,
            to
        }),
        (0usize..4, 0usize..8, 0usize..16).prop_map(|(p, op, to)| Mutation::RetargetBody {
            p,
            op,
            to
        }),
        (0usize..16).prop_map(|to| Mutation::RetargetChild { to }),
        (0usize..16).prop_map(Mutation::DuplicateComponent),
        (0usize..16, 0.01..20.0f64).prop_map(|(c, f)| Mutation::ScaleField { c, f }),
        Just(Mutation::DropParts),
        (0usize..16).prop_map(Mutation::MoveRoot),
    ]
}

fn ids(v: &Value) -> Vec<String> {
    v["components"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap().to_owned())
        .collect()
}

fn apply(v: &mut Value, m: &Mutation) {
    let ids = ids(v);
    let pick = |i: usize| ids.get(i % ids.len().max(1)).cloned();
    match *m {
        Mutation::DeleteComponent(i) => {
            let a = v["components"].as_array_mut().unwrap();
            if !a.is_empty() {
                let n = a.len();
                a.remove(i % n);
            }
        }
        Mutation::DeleteConstraint(i) => {
            let a = v["constraints"].as_array_mut().unwrap();
            if !a.is_empty() {
                let n = a.len();
                a.remove(i % n);
            }
        }
        Mutation::RetargetPort { k, b, to } => {
            let Some(id) = pick(to) else { return };
            let a = v["constraints"].as_array_mut().unwrap();
            if a.is_empty() {
                return;
            }
            let n = a.len();
            let key = if b { "b" } else { "a" };
            if let Some(port) = a[k % n][key].as_str().map(str::to_owned) {
                let p = port.split('.').nth(1).unwrap_or("origin").to_owned();
                a[k % n][key] = json!(format!("{id}.{p}"));
            }
        }
        Mutation::RetargetBody { p, op, to } => {
            let Some(id) = pick(to) else { return };
            let parts = v["parts"].as_array_mut().unwrap();
            if parts.is_empty() {
                return;
            }
            let n = parts.len();
            let body = parts[p % n]["body"].as_array_mut().unwrap();
            let m = body.len();
            body[op % m]["component"] = json!(id);
        }
        Mutation::RetargetChild { to } => {
            let targets = ["p_body", "p_lid", "p_plate", "a_main", "zz"];
            if let Some(a) = v["assemblies"].as_array_mut().unwrap().first_mut() {
                a["children"][0] = json!(targets[to % targets.len()]);
            }
        }
        Mutation::DuplicateComponent(i) => {
            let a = v["components"].as_array_mut().unwrap();
            if !a.is_empty() {
                let n = a.len();
                let c = a[i % n].clone();
                a.push(c);
            }
        }
        Mutation::ScaleField { c, f } => {
            let a = v["components"].as_array_mut().unwrap();
            if a.is_empty() {
                return;
            }
            let n = a.len();
            if let Some(shape) = a[c % n]["shape"].as_object_mut() {
                for (_, val) in shape.iter_mut() {
                    if let Some(x) = val.as_f64() {
                        *val = json!(x * f);
                    }
                }
            }
        }
        Mutation::DropParts => v["parts"] = json!([]),
        Mutation::MoveRoot(i) => {
            v["root"] = pick(i).map_or(Value::Null, Value::String);
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, ..ProptestConfig::default() })]

    #[test]
    fn never_panics_on_upgraded_v1_documents(spec in v1_strategy()) {
        let Ok(doc1) = CadDocument1::from_json_str(&spec.to_string()) else {
            return Ok(());
        };
        let doc2 = upgrade_1_to_2(&doc1);
        check_invariants(&doc2);
        // upgraded documents are structurally sound: no P-class issue
        let v = validate(&doc2, &ValidateOptions { completed: true, submitter: None });
        prop_assert!(blocking_issues(&v.report).is_empty(), "{:#?}", v.report.issues);
    }

    #[test]
    fn never_panics_on_mutated_examples(
        which in 0usize..2,
        muts in prop::collection::vec(mutation(), 1..6),
    ) {
        let mut v = examples().swap_remove(which);
        for m in &muts {
            apply(&mut v, m);
        }
        if let Ok(doc) = serde_json::from_value::<CadDocument2>(v) {
            check_invariants(&doc);
        }
    }
}
