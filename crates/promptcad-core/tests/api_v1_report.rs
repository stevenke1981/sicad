//! `validate` op, 1.0 input: `feature_index` is restored from the migration ids
//! (`e_h{n}` -> `n - 1`) and the fillet / chamfer list index; 2.0 input keeps hole issues
//! unindexed. No wire change: the field already exists in `ValidationIssue`.

mod api_support;

use api_support::*;
use promptcad_core::migrate::hole_feature_index;
use serde_json::{json, Value};

fn hole(x: f64, d: f64) -> Value {
    json!({"kind": "hole", "x": x, "y": 0.0, "z": 0.0, "axis": "z", "diameter": d,
           "hole_type": "through"})
}

fn spec(holes: Vec<Value>, fillets: Value, chamfers: Value) -> Value {
    json!({
        "schema_version": "1.0", "name": "v1report", "source_prompt": "t",
        "base": {"kind": "plate", "length": 40, "width": 30, "thickness": 5},
        "holes": holes, "fillets": fillets, "chamfers": chamfers,
        "assumptions": [], "planner": {"planner": "test"}
    })
}

fn issues(doc: Value) -> Vec<Value> {
    let (_, _, v) = call(&request("validate", json!({"document": doc})));
    ok_result(&v)["report"]["issues"]
        .as_array()
        .unwrap()
        .clone()
}

fn of_code<'a>(all: &'a [Value], code: &str) -> Vec<&'a Value> {
    all.iter().filter(|i| i["code"] == code).collect()
}

#[test]
fn hole_issues_carry_the_1_0_index() {
    // hole 0 fine, hole 1 outside the part (x = 19, d = 6 on a 40 mm plate)
    let all = issues(spec(
        vec![hole(0.0, 4.0), hole(19.0, 6.0)],
        json!([]),
        json!([]),
    ));
    let outside = of_code(&all, "hole_outside_part");
    assert_eq!(outside.len(), 1, "{all:?}");
    assert_eq!(outside[0]["target"], "e_h2");
    assert_eq!(outside[0]["feature_index"], 1);
}

#[test]
fn overlapping_holes_use_the_first_hole() {
    let all = issues(spec(
        vec![hole(-10.0, 4.0), hole(0.0, 10.0), hole(4.0, 10.0)],
        json!([]),
        json!([]),
    ));
    let o = of_code(&all, "overlapping_holes");
    assert_eq!(o.len(), 1, "{all:?}");
    assert_eq!(o[0]["target"], "e_h2");
    assert_eq!(o[0]["feature_index"], 1);
}

#[test]
fn fillet_and_chamfer_use_their_list_index() {
    let all = issues(spec(
        vec![],
        json!([
            {"kind": "fillet", "radius": 0.5, "selector": "all"},
            {"kind": "fillet", "radius": 3, "selector": "all"}
        ]),
        json!([
            {"kind": "chamfer", "distance": 0.5, "selector": "top"},
            {"kind": "chamfer", "distance": 0.5, "selector": "top"},
            {"kind": "chamfer", "distance": 2.5, "selector": "top"}
        ]),
    ));
    let f = of_code(&all, "fillet_too_large");
    assert_eq!((f.len(), &f[0]["feature_index"]), (1, &json!(1)), "{all:?}");
    assert_eq!(f[0]["target"], "p_main");
    let c = of_code(&all, "chamfer_too_large");
    assert_eq!((c.len(), &c[0]["feature_index"]), (1, &json!(2)), "{all:?}");
}

#[test]
fn issues_without_a_hole_target_stay_null() {
    let mut doc = spec(vec![], json!([]), json!([]));
    doc["assumptions"] = json!(["a"]);
    doc["base"] = json!({"kind": "plate", "length": 2000, "width": 1, "thickness": 1});
    let all = issues(doc);
    for code in ["assumption", "extreme_aspect_ratio"] {
        let hits = of_code(&all, code);
        assert!(!hits.is_empty(), "{code}: {all:?}");
        assert!(hits.iter().all(|i| i["feature_index"].is_null()), "{code}");
    }
}

#[test]
fn two_point_zero_hole_issues_keep_null() {
    // the shipped 2.0 example: no hole issue may carry an index
    for i in issues(example(PLATE_2)) {
        if i["target"].as_str().is_some_and(|t| t.starts_with("e_h")) {
            assert!(i["feature_index"].is_null(), "{i}");
        }
    }
    // hole issues must exist in a 2.0 doc for this test to bite: use an upgraded 1.0 doc
    let (_, _, v) = call(&request(
        "validate",
        json!({"document": spec(vec![hole(19.0, 6.0)], json!([]), json!([]))}),
    ));
    let upgraded = ok_result(&v)["document"].clone();
    assert_eq!(upgraded["schema_version"], "2.0");
    let two = issues(upgraded);
    let outside = of_code(&two, "hole_outside_part");
    assert_eq!(outside.len(), 1, "{two:?}");
    assert_eq!(outside[0]["target"], "e_h1");
    assert!(outside[0]["feature_index"].is_null());
}

#[test]
fn hole_feature_index_parses_only_migration_ids() {
    assert_eq!(hole_feature_index("e_h1"), Some(0));
    assert_eq!(hole_feature_index("e_h12"), Some(11));
    for bad in [
        "e_h", "e_h0", "e_h01", "e_h+1", "e_h1x", "d_h1", "p_main", "E_h1", "",
    ] {
        assert_eq!(hole_feature_index(bad), None, "{bad}");
    }
}
