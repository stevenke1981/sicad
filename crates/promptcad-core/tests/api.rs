//! `promptcad_core::api`: the transport-neutral request/response contract.
//!
//! Every response produced here is checked by `api_support::call_text`: it parses into the
//! typed `CoreResponse`, re-serializes byte-identically and conforms to the exported response
//! schema.

mod api_support;

use api_support::*;
use promptcad_core::api::{
    self, CoreRequest, CoreResponse, CoreResult, ErrorCode, PatchOutcomeView, MAX_REQUEST_BYTES,
};
use promptcad_core::canonical::canonical_sha256;
use promptcad_core::model::CadDocument2;
use promptcad_core::oplog::{OpLog, OpLogEntry};
use promptcad_core::patch_engine::PatchOrigin;
use proptest::prelude::*;
use serde_json::{json, Value};

const ALL_EXAMPLES: [&str; 3] = [PLATE_2, ENCLOSURE_2, PLATE_1];

fn is_v1(rel: &str) -> bool {
    rel == PLATE_1
}

/// Every request the tests exercise for `doc` (one per op).
fn requests_for(doc: &Value) -> Vec<Value> {
    vec![
        request("validate", json!({"document": doc})),
        request("validate", json!({"document": doc, "completed": false})),
        request("solve", json!({"document": doc})),
        request("normalize", json!({"document": doc})),
        request("upgrade", json!({"document": doc})),
        request("downgrade", json!({"document": doc})),
        request(
            "apply_patch",
            json!({"document": doc, "patch": note_patch(doc, "assembler", "n"), "origin": "assembler"}),
        ),
        request("describe", json!({"document": doc})),
        request("version", json!({})),
    ]
}

// ---- every op on every example ---------------------------------------------------------------

#[test]
fn validate_all_examples() {
    for rel in ALL_EXAMPLES {
        let doc = example(rel);
        let (_, _, v) = call(&request("validate", json!({"document": doc})));
        let r = ok_result(&v);
        assert_eq!(r["op"], "validate");
        assert_eq!(r["report"]["valid"], true, "{rel}: {}", r["report"]);
        assert_eq!(r["input_version"], if is_v1(rel) { "1.0" } else { "2.0" });
        assert_eq!(r["document"]["schema_version"], "2.0", "{rel}");
        assert!(!r["resolved"]["components"].as_object().unwrap().is_empty());
        // the returned document is canonical and loads again
        CadDocument2::from_value(r["document"].clone()).expect("returned document loads");
    }
}

#[test]
fn validate_draft_skips_completed_rules() {
    let mut doc = example(PLATE_2);
    doc["root"] = Value::Null;
    let (_, _, completed) = call(&request("validate", json!({"document": doc})));
    let completed = ok_result(&completed);
    assert_eq!(completed["report"]["valid"], false);
    assert!(completed["report"]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["code"] == "invalid_root"));
    let (_, _, draft) = call(&request(
        "validate",
        json!({"document": doc, "completed": false}),
    ));
    assert_eq!(ok_result(&draft)["report"]["valid"], true);
}

#[test]
fn solve_all_examples() {
    for rel in ALL_EXAMPLES {
        let (_, _, v) = call(&request("solve", json!({"document": example(rel)})));
        let r = ok_result(&v);
        assert_eq!(r["op"], "solve");
        assert_eq!(r["resolved"]["schema_version"], "2.0");
        assert!(r["resolved"]["issues"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["severity"] != "error"));
    }
    let (_, _, v) = call(&request("solve", json!({"document": example(PLATE_2)})));
    assert!(ok_result(&v)["resolved"]["components"]
        .get("v_plate")
        .is_some());
}

#[test]
fn normalize_all_examples_is_idempotent() {
    for rel in ALL_EXAMPLES {
        let (_, _, v) = call(&request("normalize", json!({"document": example(rel)})));
        let r = ok_result(&v);
        assert_eq!(r["issues"], json!([]), "{rel}");
        assert_eq!(r["document"]["schema_version"], "2.0");
        let once = r["document"].clone();
        let (_, _, again) = call(&request("normalize", json!({"document": once})));
        assert_eq!(ok_result(&again)["document"], once, "{rel}: idempotent");
        assert_eq!(ok_result(&again)["input_version"], "2.0");
    }
}

#[test]
fn normalize_recomputes_bindings_and_root() {
    let mut doc = example(ENCLOSURE_2);
    doc["components"][1]["shape"]["height"] = json!(40.0); // stale cache of v_body:height
    doc["root"] = Value::Null;
    let (_, _, v) = call(&request("normalize", json!({"document": doc})));
    let r = ok_result(&v);
    assert_eq!(r["document"]["components"][1]["shape"]["height"], 33.0);
    assert_eq!(r["document"]["root"], "a_main");
}

#[test]
fn normalize_reports_binding_errors() {
    let mut doc = example(ENCLOSURE_2);
    doc["bindings"][0]["expr"] = json!("wall + nope");
    let (_, _, v) = call(&request("normalize", json!({"document": doc})));
    let r = ok_result(&v);
    let issues = r["issues"].as_array().unwrap();
    assert!(
        issues.iter().any(|i| i["code"] == "binding_unknown_name"),
        "{issues:?}"
    );
    // the field keeps its cached value
    assert_eq!(r["document"]["components"][1]["shape"]["height"], 33.0);
}

#[test]
fn normalize_keeps_out_of_range_bindings_cached() {
    let mut doc = example(ENCLOSURE_2);
    // the binding evaluates to 1.5, below the 2 mm wall: rejected, cached value kept, and the
    // returned document stays schema-valid
    doc["bindings"][0]["expr"] = json!("1.5");
    let (_, _, v) = call(&request("normalize", json!({"document": doc})));
    let r = ok_result(&v);
    assert_eq!(r["issues"][0]["code"], "binding_out_of_range");
    assert_eq!(r["document"]["components"][1]["shape"]["height"], 33.0);
    CadDocument2::from_value(r["document"].clone()).expect("schema-valid");
}

#[test]
fn upgrade_accepts_only_1_0() {
    let (_, _, v) = call(&request("upgrade", json!({"document": example(PLATE_1)})));
    let r = ok_result(&v);
    assert_eq!(r["op"], "upgrade");
    assert_eq!(r["document"]["schema_version"], "2.0");

    // a missing schema_version counts as 1.0 for upgrade
    let mut versionless = example(PLATE_1);
    versionless
        .as_object_mut()
        .unwrap()
        .remove("schema_version");
    let (_, _, v2) = call(&request("upgrade", json!({"document": versionless})));
    assert_eq!(ok_result(&v2)["document"], r["document"]);

    for rel in [PLATE_2, ENCLOSURE_2] {
        let (_, _, v) = call(&request("upgrade", json!({"document": example(rel)})));
        let e = error_of(&v);
        assert_eq!(e["code"], "wrong_schema_version");
        assert_eq!(e["path"], "/document/schema_version");
    }
}

#[test]
fn downgrade_examples() {
    let (_, _, v) = call(&request("downgrade", json!({"document": example(PLATE_2)})));
    let r = ok_result(&v);
    assert_eq!(r["expressible"], true);
    assert_eq!(r["document"]["schema_version"], "1.0");

    let (_, _, v) = call(&request(
        "downgrade",
        json!({"document": example(ENCLOSURE_2)}),
    ));
    let r = ok_result(&v);
    assert_eq!(r["expressible"], false);
    assert_eq!(r["document"], Value::Null);

    let (_, _, v) = call(&request("downgrade", json!({"document": example(PLATE_1)})));
    assert_eq!(error_of(&v)["code"], "wrong_schema_version");
}

#[test]
fn upgrade_then_downgrade_round_trips_1_0() {
    let original = example(PLATE_1);
    let (_, _, up) = call(&request("upgrade", json!({"document": original})));
    let doc2 = ok_result(&up)["document"].clone();
    let (_, _, down) = call(&request("downgrade", json!({"document": doc2})));
    assert_eq!(ok_result(&down)["document"], original);
}

#[test]
fn describe_all_examples() {
    for rel in ALL_EXAMPLES {
        let (_, _, v) = call(&request("describe", json!({"document": example(rel)})));
        let r = ok_result(&v);
        assert_eq!(r["op"], "describe");
        assert!(!r["components"].as_array().unwrap().is_empty(), "{rel}");
        assert!(!r["parts"].as_array().unwrap().is_empty(), "{rel}");
        assert!(r["root"].is_string(), "{rel}");
        assert_eq!(r["root"], r["computed_root"], "{rel}");
        assert_eq!(r["input_version"], if is_v1(rel) { "1.0" } else { "2.0" });
    }
}

fn find<'a>(list: &'a Value, id: &str) -> &'a Value {
    list.as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == id)
        .unwrap_or_else(|| panic!("{id} missing"))
}

#[test]
fn describe_enclosure_facts() {
    let (_, _, v) = call(&request(
        "describe",
        json!({"document": example(ENCLOSURE_2)}),
    ));
    let r = ok_result(&v);
    assert_eq!(r["name"], "enclosure-with-lid");
    assert_eq!(r["revision"], 5);
    assert_eq!(r["root"], "a_main");
    assert_eq!(r["computed_root"], "a_main");
    assert_eq!(r["parameters"].as_array().unwrap().len(), 3);

    // v_body: an enclosure Volume of p_body whose height is bound
    let body = find(&r["components"], "v_body");
    assert_eq!(body["kind"], "volume");
    assert_eq!(body["role"], Value::Null);
    assert_eq!(body["shape"], "enclosure");
    assert_eq!(body["independent"], true);
    assert_eq!(body["frame_port"], "origin");
    assert_eq!(body["part"], "p_body");
    let fields: Vec<&str> = body["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap())
        .collect();
    assert_eq!(fields, ["length", "width", "height", "wall_thickness"]);
    let height = find_field(body, "height");
    assert_eq!(
        *height,
        json!({"name": "height", "value": 33.0, "bound": true, "expr": "wall + standoff_h + pcb_stack + 1"})
    );
    let length = find_field(body, "length");
    assert_eq!(length["bound"], false);
    assert_eq!(length["expr"], Value::Null);
    let cavity = find_port(body, "cavity_floor");
    assert_eq!(cavity["type"], "face");

    // e_pcb: a reserve Empty box with no part
    let pcb = find(&r["components"], "e_pcb");
    assert_eq!(pcb["kind"], "empty");
    assert_eq!(pcb["role"], "reserve");
    assert_eq!(pcb["shape"], "box");
    assert_eq!(pcb["part"], Value::Null);
    assert_eq!(pcb["provenance"]["agent"], "space_planner");
    let ports: Vec<(&str, &str)> = pcb["ports"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (p["name"].as_str().unwrap(), p["type"].as_str().unwrap()))
        .collect();
    assert_eq!(
        ports,
        [
            ("origin", "point"),
            ("center", "point"),
            ("axis_z", "axis"),
            ("top", "face"),
            ("bottom", "face"),
            ("x_pos", "face"),
            ("x_neg", "face"),
            ("y_pos", "face"),
            ("y_neg", "face"),
            ("region", "region"),
        ]
    );

    // constraints carry their numeric fields
    let pcb_z = find(&r["constraints"], "k_pcb_z");
    assert_eq!(pcb_z["kind"], "distance");
    assert_eq!(pcb_z["a"], "e_pcb.bottom");
    assert_eq!(pcb_z["b"], "v_body.cavity_floor");
    assert_eq!(
        pcb_z["fields"],
        json!([{"name": "value", "value": 5.0, "bound": false, "expr": null}])
    );
    assert_eq!(find(&r["constraints"], "k_root")["fields"], json!([]));

    // parts, assemblies and the tree
    let p_body = find(&r["parts"], "p_body");
    assert_eq!(
        p_body["body"],
        json!([{"op": "add", "component": "v_body"}])
    );
    assert_eq!(p_body["parent"], "a_main");
    assert_eq!(p_body["material"], "plastic");
    let a_main = find(&r["assemblies"], "a_main");
    assert_eq!(a_main["children"], json!(["p_body", "p_lid"]));
    assert_eq!(a_main["parent"], Value::Null);
    assert_eq!(
        r["tree"],
        json!([{
            "id": "a_main", "node": "assembly", "name": "電子外殼", "repeated": false,
            "children": [
                {"id": "p_body", "node": "part", "name": "外殼本體", "repeated": false, "children": []},
                {"id": "p_lid", "node": "part", "name": "上蓋", "repeated": false, "children": []},
            ],
        }])
    );
}

fn find_field<'a>(component: &'a Value, name: &str) -> &'a Value {
    component["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == name)
        .unwrap()
}

fn find_port<'a>(component: &'a Value, name: &str) -> &'a Value {
    component["ports"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == name)
        .unwrap()
}

#[test]
fn describe_tree_survives_cycles_and_dangling_children() {
    let mut doc = example(ENCLOSURE_2);
    doc["assemblies"] = json!([
        {"id": "a_main", "name": "main", "children": ["a_sub", "p_body"]},
        {"id": "a_sub", "name": "sub", "children": ["a_main", "p_lid", "p_gone"]},
    ]);
    let (_, _, v) = call(&request("describe", json!({"document": doc})));
    let r = ok_result(&v);
    // both assemblies are someone's child: no assembly is top-level, and no part either
    assert_eq!(r["tree"], json!([]));
    assert_eq!(find(&r["assemblies"], "a_sub")["parent"], "a_main");

    doc["assemblies"] = json!([
        {"id": "a_main", "name": "main", "children": ["a_sub", "p_body"]},
        {"id": "a_sub", "name": "sub", "children": ["a_sub", "p_gone"]},
    ]);
    let (_, _, v) = call(&request("describe", json!({"document": doc})));
    let tree = &ok_result(&v)["tree"];
    let names: Vec<&str> = tree
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["id"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["a_main", "p_lid"]);
    let sub = &tree[0]["children"][0];
    assert_eq!(sub["id"], "a_sub");
    assert_eq!(sub["id"], "a_sub");
    assert_eq!(sub["repeated"], false);
    assert_eq!(sub["children"][0]["id"], "a_sub");
    assert_eq!(
        sub["children"][0]["repeated"], true,
        "cycle is a back-reference"
    );
    assert_eq!(
        sub["children"][0]["children"],
        json!([]),
        "cycle not expanded"
    );
    assert_eq!(sub["children"][1]["node"], "missing");
    assert_eq!(sub["children"][1]["repeated"], false);
    assert_eq!(sub["children"][1]["name"], Value::Null);
}

#[test]
fn describe_of_empty_uses_source_ports() {
    let mut doc = example(ENCLOSURE_2);
    doc["components"].as_array_mut().unwrap().push(json!({
        "id": "e_of", "kind": "empty", "role": "keep_out",
        "shape": {"kind": "of", "source": "v_lid"}, "label": null, "provenance": null,
    }));
    let (_, _, v) = call(&request("describe", json!({"document": doc})));
    let of = find(&ok_result(&v)["components"], "e_of");
    assert_eq!(of["shape"], "of");
    assert_eq!(of["independent"], false);
    assert_eq!(of["frame_port"], Value::Null);
    assert_eq!(of["refs"], json!([{"field": "source", "id": "v_lid"}]));
    assert_eq!(
        of["ports"].as_array().unwrap().len(),
        10,
        "plate port table"
    );
    assert_eq!(of["fields"], json!([]));
}

#[test]
fn version_op() {
    let (_, typed, v) = call(&request("version", json!({})));
    let r = ok_result(&v);
    assert_eq!(r["crate_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(r["api_version"], api::API_VERSION);
    assert_eq!(r["schema_versions"], json!(["1.0", "2.0"]));
    assert!(matches!(typed.result(), Some(CoreResult::Version(_))));
}

// ---- apply_patch -------------------------------------------------------------------------------

#[test]
fn apply_patch_accepted() {
    for rel in [PLATE_2, ENCLOSURE_2] {
        let doc = example(rel);
        let revision = doc["revision"].as_u64().unwrap();
        let (_, typed, v) = call(&request(
            "apply_patch",
            json!({
                "document": doc,
                "patch": note_patch(&doc, "assembler", "api note"),
                "origin": "assembler",
                "timestamp": "2026-11-26T00:00:00Z",
                "log_seq": 7,
            }),
        ));
        let r = ok_result(&v);
        assert_eq!(r["op"], "apply_patch");
        let o = &r["outcome"];
        assert_eq!(o["status"], "accepted", "{rel}: {o}");
        assert_eq!(o["revision_before"], revision);
        assert_eq!(o["revision_after"], revision + 1);
        assert_eq!(o["patch_id"], format!("r{}", revision + 1));
        assert_eq!(o["created"], json!([]));
        assert_eq!(o["document"]["revision"], revision + 1);
        assert!(o["document"]["notes"]
            .as_array()
            .unwrap()
            .contains(&json!("api note")));
        assert_eq!(o["report"]["valid"], true);
        assert!(o["resolved"]["components"].as_object().unwrap().len() > 1);

        let before = CadDocument2::from_value(doc.clone()).unwrap();
        assert_eq!(o["doc_sha256_before"], canonical_sha256(&before));
        let after = CadDocument2::from_value(o["document"].clone()).unwrap();
        assert_eq!(o["doc_sha256_after"], canonical_sha256(&after));

        let entry = &o["log_entry"];
        assert_eq!(entry["seq"], 7);
        assert_eq!(entry["timestamp"], "2026-11-26T00:00:00Z");
        assert_eq!(entry["accepted"], true);
        assert_eq!(entry["origin"], "assembler");
        assert_eq!(entry["patch_sha256"], o["patch_sha256"]);
        assert!(entry["revert"]["sections"].get("notes").is_some());
        let line = o["log_line"].as_str().unwrap();
        let parsed: OpLogEntry = serde_json::from_str(line).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), *entry);

        let PatchOutcomeView::Accepted(a) = &typed.result().map(apply_outcome).unwrap() else {
            panic!("accepted")
        };
        assert_eq!(a.log_entry, parsed);
    }
}

fn apply_outcome(r: &CoreResult) -> PatchOutcomeView {
    match r {
        CoreResult::ApplyPatch(a) => a.outcome.clone(),
        other => panic!("not apply_patch: {other:?}"),
    }
}

#[test]
fn apply_patch_log_line_replays_with_oplog() {
    let doc = example(PLATE_2);
    let req = request(
        "apply_patch",
        json!({"document": doc, "patch": note_patch(&doc, "assembler", "x"), "origin": "assembler"}),
    );
    let (_, _, v) = call(&req);
    let o = &ok_result(&v)["outcome"];
    let log = OpLog::from_jsonl(&format!("{}\n", o["log_line"].as_str().unwrap())).unwrap();
    let initial = CadDocument2::from_value(doc).unwrap();
    let replayed = promptcad_core::oplog::replay(&initial, &log).unwrap();
    assert_eq!(serde_json::to_value(&replayed).unwrap(), o["document"]);
}

#[test]
fn apply_patch_human_origin() {
    let doc = example(PLATE_2);
    // patch.agent is ignored for human origins
    let (_, _, v) = call(&request(
        "apply_patch",
        json!({"document": doc, "patch": note_patch(&doc, "joiner", "h"), "origin": "human"}),
    ));
    let o = &ok_result(&v)["outcome"];
    assert_eq!(o["status"], "accepted");
    assert_eq!(o["log_entry"]["origin"], "human");
}

#[test]
fn apply_patch_rejected_stale_revision() {
    let doc = example(PLATE_2);
    let mut patch = note_patch(&doc, "assembler", "late");
    patch["base_revision"] = json!(doc["revision"].as_u64().unwrap() + 99);
    let (_, _, v) = call(&request(
        "apply_patch",
        json!({"document": doc, "patch": patch, "origin": "assembler"}),
    ));
    let o = &ok_result(&v)["outcome"];
    assert_eq!(o["status"], "rejected");
    assert_eq!(o["reason"]["code"], "stale_revision");
    assert_eq!(o["issues"], json!([]));
    assert_eq!(o["log_entry"]["accepted"], false);
    assert_eq!(o["log_entry"]["revision_after"], Value::Null);
    assert_eq!(o["log_entry"]["reject"], o["reason"]);
    assert!(o.get("document").is_none(), "no document on rejection");
    let before = CadDocument2::from_value(doc).unwrap();
    assert_eq!(o["doc_sha256_before"], canonical_sha256(&before));
}

#[test]
fn apply_patch_rejected_permission_and_identity() {
    let doc = example(PLATE_2);
    // origin differs from patch.agent
    let (_, _, v) = call(&request(
        "apply_patch",
        json!({"document": doc, "patch": note_patch(&doc, "joiner", "x"), "origin": "assembler"}),
    ));
    let o = &ok_result(&v)["outcome"];
    assert_eq!(o["status"], "rejected");
    assert_eq!(o["reason"]["code"], "permission_denied");
}

#[test]
fn apply_patch_schema_violations_are_rejections_not_errors() {
    let doc = example(PLATE_2);
    let mut patch = note_patch(&doc, "assembler", "x");
    patch["ops"] = json!([]);
    let (_, _, v) = call(&request(
        "apply_patch",
        json!({"document": doc, "patch": patch, "origin": "assembler"}),
    ));
    let o = &ok_result(&v)["outcome"];
    assert_eq!(o["status"], "rejected");
    assert_eq!(o["reason"]["code"], "schema_invalid");
    assert!(o["log_entry"]["patch"].is_object(), "the patch is logged");
}

#[test]
fn apply_patch_validation_rejection_attaches_issues() {
    let doc = example(ENCLOSURE_2);
    let patch = json!({
        "agent": "joiner",
        "base_revision": doc["revision"],
        "rationale": "dangling",
        "resolves": [],
        "ops": [{"op": "add_constraint", "constraint": {
            "id": "k_bad", "kind": "coincident", "a": "v_lid.origin", "b": "v_nope.origin",
            "value": null, "u": null, "v": null, "frame": null
        }}],
    });
    let (_, _, v) = call(&request(
        "apply_patch",
        json!({"document": doc, "patch": patch, "origin": "joiner"}),
    ));
    let o = &ok_result(&v)["outcome"];
    assert_eq!(o["status"], "rejected", "{o}");
    assert_eq!(o["reason"]["code"], "dangling_reference");
    assert!(!o["issues"].as_array().unwrap().is_empty());
    assert_eq!(o["log_entry"]["issues"][0]["code"], "dangling_reference");
}

#[test]
fn apply_patch_needs_2_0_and_a_parsable_patch() {
    let doc1 = example(PLATE_1);
    let (_, _, v) = call(&request(
        "apply_patch",
        json!({"document": doc1, "patch": {}, "origin": "human"}),
    ));
    assert_eq!(error_of(&v)["code"], "wrong_schema_version");

    let doc = example(PLATE_2);
    let (_, _, v) = call(&request(
        "apply_patch",
        json!({"document": doc, "patch": {"agent": "assembler"}, "origin": "human"}),
    ));
    let e = error_of(&v);
    assert_eq!(e["code"], "invalid_patch");
    assert_eq!(e["path"], "/patch");

    let mut bad = note_patch(&doc, "assembler", "x");
    bad["ops"][0]["op"] = json!("explode");
    let (_, _, v) = call(&request(
        "apply_patch",
        json!({"document": doc, "patch": bad, "origin": "assembler"}),
    ));
    assert_eq!(error_of(&v)["code"], "invalid_patch");

    // unknown origin: the request itself is malformed
    let req = request(
        "apply_patch",
        json!({"document": doc, "patch": note_patch(&doc, "assembler", "x"), "origin": "robot"}),
    );
    let (_, _, v) = call_text(&req.to_string());
    assert_eq!(error_of(&v)["code"], "bad_request");
}

// ---- error responses ---------------------------------------------------------------------------

fn error_code(input: &str) -> (Value, String) {
    let (_, typed, v) = call_text(input);
    assert!(typed.error().is_some(), "expected error for {input:.200}");
    let code = v["code"].as_str().unwrap().to_owned();
    (v, code)
}

#[test]
fn malformed_requests_are_bad_request() {
    for input in [
        "",
        "garbage",
        "{",
        "[]",
        "null",
        "42",
        "{}",
        r#"{"op": "nope"}"#,
        r#"{"op": 5}"#,
        r#"{"op": "version", "extra": 1}"#,
        r#"{"op": "validate"}"#,
        r#"{"op": "validate", "document": {}, "completed": "yes"}"#,
        r#"{"op": "validate", "document": {}, "docment": {}}"#,
    ] {
        let (v, code) = error_code(input);
        assert_eq!(code, "bad_request", "{input}: {v}");
        assert_eq!(v["path"], Value::Null);
        assert_eq!(v["violations"], json!([]));
    }
}

#[test]
fn unsupported_schema_version() {
    for doc in [
        json!({"schema_version": "3.0"}),
        json!({"schema_version": 2.0}),
        json!({"name": "no version"}),
    ] {
        for op in ["validate", "solve", "normalize", "describe", "downgrade"] {
            let (_, _, v) = call(&request(op, json!({"document": doc})));
            let e = error_of(&v);
            assert_eq!(e["code"], "unsupported_schema_version", "{op} {doc}");
            assert_eq!(e["path"], "/document/schema_version");
        }
    }
    let (_, _, v) = call(&request(
        "upgrade",
        json!({"document": {"schema_version": "3.0"}}),
    ));
    assert_eq!(error_of(&v)["code"], "unsupported_schema_version");
}

#[test]
fn invalid_document() {
    // a non-object document
    let (v, code) = error_code(&request("validate", json!({"document": [1, 2]})).to_string());
    assert_eq!(code, "invalid_document");
    assert_eq!(v["path"], "/document");

    // an example file that still carries `expected_resolved`
    let raw: Value = serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join(ENCLOSURE_2),
        )
        .unwrap(),
    )
    .unwrap();
    let (_, _, v) = call(&request("validate", json!({"document": raw})));
    let e = error_of(&v);
    assert_eq!(e["code"], "invalid_document");
    assert!(e["message"].as_str().unwrap().contains("expected_resolved"));

    // a malformed id
    let mut doc = example(PLATE_2);
    doc["components"][0]["id"] = json!("Not An Id");
    let (_, _, v) = call(&request("solve", json!({"document": doc})));
    assert_eq!(error_of(&v)["code"], "invalid_document");
}

#[test]
fn schema_violation_carries_paths() {
    let mut doc = example(ENCLOSURE_2);
    doc["components"][2]["shape"]["thickness"] = json!(-2.0);
    doc["components"][0]["shape"]["length"] = json!(0.0);
    for op in ["validate", "solve", "normalize", "describe", "downgrade"] {
        let (_, _, v) = call(&request(op, json!({"document": doc})));
        let e = error_of(&v);
        assert_eq!(e["code"], "schema_violation", "{op}");
        assert_eq!(e["path"], "/document/components/0/shape/length");
        let paths: Vec<&str> = e["violations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["path"].as_str().unwrap())
            .collect();
        assert_eq!(
            paths,
            [
                "/document/components/0/shape/length",
                "/document/components/2/shape/thickness"
            ]
        );
    }

    // 1.0 documents too
    let mut doc1 = example(PLATE_1);
    doc1["base"]["length"] = json!(-1.0);
    let (_, _, v) = call(&request("validate", json!({"document": doc1})));
    let e = error_of(&v);
    assert_eq!(e["code"], "schema_violation");
    assert_eq!(e["path"], "/document/base/length");
}

#[test]
fn oversize_input_is_too_large() {
    let mut input = String::with_capacity(MAX_REQUEST_BYTES + 16);
    input.push_str(r#"{"op":"version","pad":""#);
    while input.len() <= MAX_REQUEST_BYTES {
        input.push_str("xxxxxxxxxxxxxxxx");
    }
    input.push_str("\"}");
    let (v, code) = error_code(&input);
    assert_eq!(code, "too_large");
    assert!(v["message"].as_str().unwrap().contains("8388608"));

    // exactly at the limit is parsed (and then rejected for its unknown field)
    let mut at_limit = String::from(r#"{"op":"version","pad":""#);
    let fill = MAX_REQUEST_BYTES - at_limit.len() - 2;
    at_limit.push_str(&"x".repeat(fill));
    at_limit.push_str("\"}");
    assert_eq!(at_limit.len(), MAX_REQUEST_BYTES);
    assert_eq!(error_code(&at_limit).1, "bad_request");
}

#[test]
fn error_codes_are_closed() {
    let schema = response_schema();
    let codes: Vec<&str> = schema["$defs"]["ErrorCode"]["oneOf"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["const"].as_str().unwrap())
        .collect();
    assert_eq!(
        codes,
        [
            "bad_request",
            "too_large",
            "unsupported_schema_version",
            "wrong_schema_version",
            "invalid_document",
            "schema_violation",
            "invalid_patch",
            "response_too_large",
            "internal"
        ]
    );
}

// ---- typed entry point and determinism ---------------------------------------------------------

#[test]
fn handle_matches_handle_json() {
    let doc = example(ENCLOSURE_2);
    for req in requests_for(&doc) {
        let typed_req: CoreRequest = serde_json::from_value(req.clone()).unwrap();
        let direct = api::handle(typed_req).to_json();
        assert_eq!(direct, api::handle_json(&req.to_string()), "{}", req["op"]);
    }
    let r = api::handle(CoreRequest::ApplyPatch {
        document: doc.clone(),
        patch: note_patch(&doc, "assembler", "typed"),
        origin: PatchOrigin::Human,
        timestamp: None,
        log_seq: 0,
    });
    assert!(matches!(r, CoreResponse::Ok { .. }));
}

#[test]
fn every_op_is_deterministic() {
    for rel in ALL_EXAMPLES {
        let doc = example(rel);
        for req in requests_for(&doc) {
            let text = req.to_string();
            let (first, _, _) = call_text(&text);
            let (second, _, _) = call_text(&text);
            assert_eq!(first, second, "{rel} {}", req["op"]);
        }
    }
}

#[test]
fn result_op_matches_request_op() {
    let doc = example(PLATE_2);
    for req in requests_for(&doc) {
        let (_, _, v) = call(&req);
        if v["status"] == "ok" {
            assert_eq!(v["result"]["op"], req["op"]);
        }
    }
}

// ---- schemas -----------------------------------------------------------------------------------

/// Visits every schema node (root, definitions, property / item / branch schemas).
fn walk<'a>(v: &'a Value, path: String, out: &mut Vec<(String, &'a Value)>) {
    out.push((path.clone(), v));
    let Value::Object(map) = v else { return };
    for key in ["$defs", "properties", "patternProperties"] {
        if let Some(Value::Object(children)) = map.get(key) {
            for (k, child) in children {
                walk(child, format!("{path}/{key}/{k}"), out);
            }
        }
    }
    for key in ["oneOf", "anyOf", "allOf"] {
        if let Some(Value::Array(children)) = map.get(key) {
            for (i, child) in children.iter().enumerate() {
                walk(child, format!("{path}/{key}/{i}"), out);
            }
        }
    }
    for key in ["items", "additionalProperties"] {
        if let Some(child @ Value::Object(_)) = map.get(key) {
            walk(child, format!("{path}/{key}"), out);
        }
    }
}

fn assert_tagged(schema: &Value, union: &Value, tag: &str, expected: &[&str]) {
    let branches = union["oneOf"].as_array().expect("tagged union is a oneOf");
    let tags: Vec<&str> = branches
        .iter()
        .map(|b| {
            let b = match b.get("$ref") {
                Some(Value::String(r)) => &schema["$defs"][r.trim_start_matches("#/$defs/")],
                _ => b,
            };
            assert_eq!(b["type"], "object");
            assert!(
                b["required"].as_array().unwrap().contains(&json!(tag)),
                "{tag} required"
            );
            assert_eq!(b["properties"][tag]["type"], "string");
            b["properties"][tag]["const"].as_str().expect("const tag")
        })
        .collect();
    assert_eq!(tags, expected);
}

const OPS: [&str; 8] = [
    "validate",
    "solve",
    "normalize",
    "upgrade",
    "downgrade",
    "apply_patch",
    "describe",
    "version",
];

#[test]
fn schemas_are_tagged_unions_with_named_definitions() {
    let req = request_schema();
    assert_eq!(req["title"], "CoreRequest");
    assert_tagged(&req, &req, "op", &OPS);
    let req_defs: Vec<&String> = req["$defs"].as_object().unwrap().keys().collect();
    assert_eq!(req_defs, ["PatchOrigin"]);

    let resp = response_schema();
    assert_eq!(resp["title"], "CoreResponse");
    assert_tagged(&resp, &resp, "status", &["ok", "error"]);
    let defs = &resp["$defs"];
    assert_tagged(&resp, &defs["CoreResult"], "op", &OPS);
    assert_tagged(
        &resp,
        &defs["PatchOutcomeView"],
        "status",
        &["accepted", "rejected"],
    );
    for name in [
        "CoreResult",
        "ErrorCode",
        "PatchOutcomeView",
        "DocumentVersion",
        "ComponentView",
        "ConstraintView",
        "FieldView",
        "PortView",
        "PartView",
        "AssemblyView",
        "TreeNode",
        "TreeNodeKind",
        "ShapeName",
        "ShapeRef",
        "OpLogEntry",
        "RejectReason",
        "ReasonCode",
        "PatchOrigin",
        "SchemaViolation",
        "CadDocument2",
        "CadDocument1",
        "Resolved",
        "ValidationReport",
    ] {
        assert!(defs.get(name).is_some(), "definition {name} missing");
    }
}

#[test]
fn no_definition_degenerates_to_any() {
    for (name, schema) in [
        ("request", request_schema()),
        ("response", response_schema()),
    ] {
        for (def, body) in schema["$defs"].as_object().unwrap() {
            assert!(
                body.is_object() && !body.as_object().unwrap().is_empty(),
                "{name}: definition {def} is `{body}`"
            );
        }
        let mut nodes = Vec::new();
        walk(&schema, String::new(), &mut nodes);
        assert!(nodes.len() > 20, "{name}: walked {} nodes", nodes.len());
        for (path, node) in nodes {
            let map = node
                .as_object()
                .unwrap_or_else(|| panic!("{name}: schema at {path} is `{node}`"));
            assert!(
                ["type", "$ref", "oneOf", "anyOf", "allOf", "const", "enum"]
                    .iter()
                    .any(|k| map.contains_key(*k)),
                "{name}: untyped schema at {path}: {node}"
            );
            if map.contains_key("properties") {
                assert_eq!(
                    map.get("additionalProperties"),
                    Some(&Value::Bool(false)),
                    "{name}: object at {path} is not closed"
                );
            }
            if map.get("type") == Some(&json!("object")) && !map.contains_key("properties") {
                // open objects: typed maps, the raw request inputs and the op-log reverse delta
                let allowed = map.contains_key("patternProperties")
                    || map
                        .get("additionalProperties")
                        .is_some_and(Value::is_object)
                    || path.ends_with("/properties/document")
                    || path.ends_with("/properties/patch")
                    || path == "/$defs/OpLogEntry/properties/patch"
                    || path == "/$defs/Revert/properties/sections";
                assert!(allowed, "{name}: open object at {path}");
            }
            if map.get("additionalProperties") == Some(&Value::Bool(true)) {
                // the deliberately raw spots, and nothing else
                let raw = path.ends_with("/properties/document")
                    || path.ends_with("/properties/patch")
                    || path == "/$defs/Revert/properties/sections";
                assert!(raw, "{name}: arbitrary-JSON object at {path}");
            }
        }
    }
}

#[test]
fn request_schema_documents_raw_inputs_as_objects() {
    let req = request_schema();
    for branch in req["oneOf"].as_array().unwrap() {
        for key in ["document", "patch"] {
            if let Some(p) = branch["properties"].get(key) {
                assert_eq!(p["type"], "object", "{key}");
                assert!(p.get("properties").is_none());
            }
        }
    }
    let validate = &req["oneOf"][0];
    assert_eq!(validate["properties"]["completed"]["default"], true);
    assert_eq!(validate["required"], json!(["op", "document"]));
}

#[test]
fn response_schema_requires_always_written_fields() {
    let resp = response_schema();
    let error = &resp["oneOf"][1];
    assert_eq!(
        error["required"],
        json!(["status", "code", "message", "path", "violations"])
    );
    let field = &resp["$defs"]["FieldView"];
    assert_eq!(field["required"], json!(["name", "value", "bound", "expr"]));
}

// ---- properties --------------------------------------------------------------------------------

/// Arbitrary JSON values (bounded depth and size).
fn arb_json() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(|n| json!(n)),
        (-1e6f64..1e6).prop_map(|x| json!(x)),
        prop_oneof![
            Just(-0.0f64),
            Just(1e300),
            Just(-1e300),
            Just(5e-324),
            Just(100000.0),
            Just(100000.5),
        ]
        .prop_map(|x| json!(x)),
        "[a-z_]{0,8}".prop_map(Value::String),
        prop_oneof![
            Just("1.0"),
            Just("2.0"),
            Just("v_body"),
            Just("v_body.top"),
            Just("v_body:height"),
            Just("human"),
            Just("assembler"),
            Just("add_note"),
            Just("validate"),
        ]
        .prop_map(|s| Value::String(s.into())),
    ];
    leaf.prop_recursive(4, 32, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..6).prop_map(Value::Array),
            prop::collection::btree_map("[a-z_]{1,10}", inner, 0..6)
                .prop_map(|m| Value::Object(m.into_iter().collect())),
        ]
    })
}

/// JSON pointers of every node of `v`.
fn pointers(v: &Value, path: String, out: &mut Vec<String>) {
    out.push(path.clone());
    match v {
        Value::Object(map) => {
            for (k, child) in map {
                pointers(child, format!("{path}/{k}"), out);
            }
        }
        Value::Array(items) => {
            for (i, child) in items.iter().enumerate() {
                pointers(child, format!("{path}/{i}"), out);
            }
        }
        _ => {}
    }
}

#[derive(Debug, Clone)]
enum Mutation {
    Replace(Value),
    Remove,
    Scale(f64),
}

fn arb_mutation() -> impl Strategy<Value = Mutation> {
    prop_oneof![
        3 => arb_json().prop_map(Mutation::Replace),
        1 => Just(Mutation::Remove),
        2 => prop_oneof![Just(-1.0), Just(0.0), Just(0.5), Just(2.0), Just(1e6), Just(1e-9)]
            .prop_map(Mutation::Scale),
    ]
}

fn mutate(base: &Value, index: usize, mutation: &Mutation) -> Value {
    let mut v = base.clone();
    let mut ptrs = Vec::new();
    pointers(&v, String::new(), &mut ptrs);
    let ptr = &ptrs[index % ptrs.len()];
    if ptr.is_empty() {
        return match mutation {
            Mutation::Replace(x) => x.clone(),
            _ => v,
        };
    }
    let (parent, key) = ptr.rsplit_once('/').unwrap();
    match mutation {
        Mutation::Replace(x) => *v.pointer_mut(ptr).unwrap() = x.clone(),
        Mutation::Scale(f) => {
            let node = v.pointer_mut(ptr).unwrap();
            if let Some(n) = node.as_f64() {
                *node = json!(n * f);
            } else {
                *node = json!(f);
            }
        }
        Mutation::Remove => match v.pointer_mut(parent).unwrap() {
            Value::Object(map) => {
                map.remove(key);
            }
            Value::Array(items) => {
                let i: usize = key.parse().unwrap();
                items.remove(i);
            }
            _ => unreachable!(),
        },
    }
    v
}

fn base_requests() -> Vec<Value> {
    let plate = example(PLATE_2);
    let enclosure = example(ENCLOSURE_2);
    vec![
        request("validate", json!({"document": enclosure})),
        request("describe", json!({"document": enclosure})),
        request("normalize", json!({"document": example(PLATE_1)})),
        request("upgrade", json!({"document": example(PLATE_1)})),
        request(
            "validate",
            json!({"document": example(PLATE_1), "completed": false}),
        ),
        request("downgrade", json!({"document": plate})),
        request(
            "apply_patch",
            json!({"document": plate, "patch": note_patch(&plate, "assembler", "p"), "origin": "assembler"}),
        ),
        request(
            "apply_patch",
            json!({
                "document": enclosure,
                "patch": {
                    "agent": "assembler",
                    "base_revision": 5,
                    "rationale": "set",
                    "resolves": [],
                    "ops": [
                        {"op": "set_field", "target": "v_lid:thickness", "value": 3.0},
                        {"op": "add_note", "text": "t"}
                    ]
                },
                "origin": "human"
            }),
        ),
    ]
}

fn assert_never_internal(input: &str) {
    let (_, typed, _) = call_text(input);
    if let Some(e) = typed.error() {
        assert_ne!(e.code, ErrorCode::Internal, "internal error: {}", e.message);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn arbitrary_strings_never_panic(input in any::<String>()) {
        assert_never_internal(&input);
    }

    #[test]
    fn arbitrary_json_requests_never_panic(v in arb_json(), op in prop::sample::select(OPS.to_vec())) {
        assert_never_internal(&v.to_string());
        assert_never_internal(&request(op, json!({"document": v})).to_string());
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(192))]

    #[test]
    fn mutated_valid_requests_never_panic(
        which in 0usize..8,
        mutations in prop::collection::vec((any::<usize>(), arb_mutation()), 1..4),
    ) {
        let mut req = base_requests().swap_remove(which);
        for (index, mutation) in &mutations {
            req = mutate(&req, *index, mutation);
        }
        assert_never_internal(&req.to_string());
    }
}
