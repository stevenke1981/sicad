//! Regressions for output / CPU amplification of the `api` ops (phase 2b review).
//!
//! * `describe` used to expand the assembly graph as a tree of *paths*: an assembly listing
//!   the next one 32 times, four levels deep, turned a 1.4 KB request into a 56 MB response
//!   (five levels: 1.8 GB, 7.3 GB peak memory; sixteen levels never finished). Every part /
//!   assembly is now expanded once; later occurrences are `repeated` leaves.
//! * Every op is checked against adversarial maximal documents (schema limits: 256
//!   components, 512 constraints, 128 bindings, 64 parameters, 32 parts, 16 assemblies) for a
//!   response-size bound and a time bound. Worst cases (release build, measured):
//!
//! | op | worst-case response | driver |
//! |---|---|---|
//! | `version` | constant (~100 B) | — |
//! | `validate` | O(n²) in components: ~11 MB | one `reserve_overlap` issue per pair of 255 overlapping reserves (32 385 issues) + the echoed document (≈ request) + `resolved` (linear) |
//! | `apply_patch` | O(n²): ~15 MB | as `validate` (accepted: report + document + resolved), plus `log_entry` (issue summaries, patch, `revert` ≤ one copy of the document) and `log_line` (the same entry again as an escaped string) |
//! | `solve` | linear, < request | one placement + port table per component |
//! | `normalize` | linear, ≈ request | the document + ≤ 128 binding issues |
//! | `describe` | linear, ≈ request | component views (fields, ports), ≤ 560 tree nodes |
//! | `upgrade` | linear (1.0 caps: 64 holes) | the 2.0 document |
//! | `downgrade` | linear, < request | the 1.0 document or `null` |
//!
//! All of these stay far below `MAX_RESPONSE_BYTES` (24 MiB), the global safety net that
//! replaces any larger response with a `response_too_large` error.

mod api_support;

use std::time::{Duration, Instant};

use api_support::*;
use promptcad_core::api::{
    self, CoreResponse, ErrorCode, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES, MAX_TREE_NODES,
};
use serde_json::{json, Value};

/// Generous per-request time bound (debug build; release is ~20× faster).
const TIME_BOUND: Duration = Duration::from_secs(60);

/// Runs `req` through `handle_json`, asserting the size and time bounds; returns the response.
fn bounded(label: &str, req: &Value, max_bytes: usize) -> Value {
    let text = req.to_string();
    assert!(text.len() <= MAX_REQUEST_BYTES, "{label}: request too big");
    let start = Instant::now();
    let out = api::handle_json(&text);
    let elapsed = start.elapsed();
    eprintln!(
        "{label}: request {} B, response {} B ({:.1}x), {:?}",
        text.len(),
        out.len(),
        out.len() as f64 / text.len() as f64,
        elapsed
    );
    assert!(elapsed < TIME_BOUND, "{label}: took {elapsed:?}");
    assert!(
        out.len() <= max_bytes,
        "{label}: response {} B exceeds the bound {max_bytes} B",
        out.len()
    );
    assert!(out.len() <= MAX_RESPONSE_BYTES, "{label}");
    let value: Value = serde_json::from_str(&out).unwrap();
    assert_ne!(
        value["code"], "response_too_large",
        "{label}: a schema-valid document must never hit the response cap"
    );
    value
}

fn tree_nodes(nodes: &Value) -> usize {
    nodes
        .as_array()
        .unwrap()
        .iter()
        .map(|n| 1 + tree_nodes(&n["children"]))
        .sum()
}

// ---- describe: the exponential tree ------------------------------------------------------------

/// The reviewer's input: `a_i.children = [a_{i+1}] * fan`, the last level lists one part
/// `fan` times.
fn chain_doc(levels: usize, fan: usize) -> Value {
    let assemblies: Vec<Value> = (0..levels)
        .map(|i| {
            let child = if i + 1 < levels {
                format!("a{}", i + 1)
            } else {
                "p_x".to_owned()
            };
            json!({"id": format!("a{i}"), "name": "n", "children": vec![child; fan]})
        })
        .collect();
    json!({
        "schema_version": "2.0", "source_prompt": "x", "revision": 0,
        "components": [{"id": "v_x", "kind": "volume",
                        "shape": {"kind": "plate", "length": 1, "width": 1, "thickness": 1}}],
        "parts": [{"id": "p_x", "name": "p", "quantity": 1, "source": "made",
                   "body": [{"op": "add", "component": "v_x"}]}],
        "assemblies": assemblies,
        "planner": {"planner": "agent_team", "confidence": 0.9, "review_required": true},
    })
}

#[test]
fn describe_of_a_fan_out_chain_is_linear() {
    // 4 levels: was 56 MB; 5: 1.8 GB; 16: never finished. Now each is a few KB.
    for levels in [4, 5, 16] {
        let doc = chain_doc(levels, 32);
        let req = request("describe", json!({"document": doc}));
        let v = bounded(&format!("describe chain {levels}x32"), &req, 64 * 1024);
        let tree = &ok_result(&v)["tree"];
        // a0 is the only top-level node; each assembly is expanded once with 32 children,
        // the first is the next level, the other 31 are repeated back-references.
        assert_eq!(tree_nodes(tree), 1 + 32 * levels, "{levels} levels");
        let mut node = &tree[0];
        for i in 0..levels {
            assert_eq!(node["id"], format!("a{i}"));
            assert_eq!(node["repeated"], false);
            let children = node["children"].as_array().unwrap();
            assert_eq!(children.len(), 32);
            for c in &children[1..] {
                assert_eq!(c["repeated"], true, "{c}");
                assert_eq!(c["children"], json!([]));
            }
            node = &children[0];
        }
        assert_eq!(node["id"], "p_x");
        assert_eq!(node["node"], "part");
        assert_eq!(node["repeated"], false);
    }
}

#[test]
fn describe_of_the_reviewer_input_conforms_to_the_schema() {
    let (_, _, v) = call(&request("describe", json!({"document": chain_doc(5, 32)})));
    assert_eq!(ok_result(&v)["computed_root"], "a0");
}

#[test]
fn describe_marks_duplicate_and_multi_parent_children_as_repeated() {
    let mut doc = example(ENCLOSURE_2);
    // p_body listed twice in one assembly; a_sub is a child of both a_main and a_other.
    doc["assemblies"] = json!([
        {"id": "a_main", "name": "main", "children": ["p_body", "p_body", "a_sub"]},
        {"id": "a_sub", "name": "sub", "children": ["p_lid"]},
        {"id": "a_other", "name": "other", "children": ["a_sub"]},
    ]);
    let (_, _, v) = call(&request("describe", json!({"document": doc})));
    let tree = &ok_result(&v)["tree"];
    let sub = json!({"id": "a_sub", "node": "assembly", "name": "sub", "repeated": false,
        "children": [{"id": "p_lid", "node": "part", "name": "上蓋", "repeated": false, "children": []}]});
    assert_eq!(
        tree,
        &json!([
            {"id": "a_main", "node": "assembly", "name": "main", "repeated": false, "children": [
                {"id": "p_body", "node": "part", "name": "外殼本體", "repeated": false, "children": []},
                {"id": "p_body", "node": "part", "name": "外殼本體", "repeated": true, "children": []},
                sub,
            ]},
            {"id": "a_other", "node": "assembly", "name": "other", "repeated": false, "children": [
                {"id": "a_sub", "node": "assembly", "name": "sub", "repeated": true, "children": []},
            ]},
        ])
    );
}

#[test]
fn describe_tree_stays_below_the_node_cap_for_maximal_assembly_graphs() {
    // 16 assemblies × 32 children (the schema maximum), every assembly listing every other
    // assembly and parts: the worst case for the tree builder.
    let mut doc = example(ENCLOSURE_2);
    let mut children: Vec<String> = (1..16).map(|i| format!("a{i}")).collect();
    children.extend(["p_body", "p_lid"].iter().map(|s| (*s).to_owned()));
    while children.len() < 32 {
        children.push("p_lid".to_owned());
    }
    let assemblies: Vec<Value> = (0..16)
        .map(|i| json!({"id": format!("a{i}"), "name": "n", "children": children}))
        .collect();
    doc["assemblies"] = json!(assemblies);
    let req = request("describe", json!({"document": doc}));
    let v = bounded("describe dense 16x32", &req, 256 * 1024);
    let nodes = tree_nodes(&ok_result(&v)["tree"]);
    // a0 is top-level (a1..a15 are children of a0); every assembly expands once: 1 + 16 * 32.
    assert_eq!(nodes, 1 + 16 * 32);
    assert!(nodes <= 16 + 32 + 16 * 32);
    const { assert!(16 + 32 + 16 * 32 < MAX_TREE_NODES) };
}

// ---- spec §6.2: duplicate children -------------------------------------------------------------

/// §6.2 says "組裝圖必須是樹（每個子項最多一個父組裝）". A child listed twice in the *same*
/// assembly still has exactly one parent, so the frozen §9.2 codes do not cover it: the
/// validator deduplicates parents and reports nothing. This pins that behaviour (changing it
/// is a spec decision); `describe` shows the second occurrence as `repeated`.
#[test]
fn duplicate_children_within_one_assembly_are_not_a_validation_issue() {
    let mut doc = example(ENCLOSURE_2);
    doc["assemblies"][0]["children"] = json!(["p_body", "p_lid", "p_body", "p_lid"]);
    let (_, _, v) = call(&request("validate", json!({"document": doc})));
    let report = &ok_result(&v)["report"];
    assert_eq!(report["valid"], true, "{report}");
    let codes: Vec<&str> = report["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["code"].as_str().unwrap())
        .collect();
    assert!(!codes.contains(&"multi_owner"), "{codes:?}");
}

#[test]
fn multi_parent_children_are_multi_owner_and_deep_chains_are_schema_invalid() {
    // A child of two assemblies is `multi_owner` (§6.2 / §9.2) ...
    let mut doc = example(ENCLOSURE_2);
    doc["assemblies"] = json!([
        {"id": "a_main", "name": "main", "children": ["a_x", "a_y"]},
        {"id": "a_x", "name": "x", "children": ["p_body", "p_lid"]},
        {"id": "a_y", "name": "y", "children": ["p_lid"]},
    ]);
    doc["root"] = json!("a_main");
    let (_, _, v) = call(&request("validate", json!({"document": doc})));
    let issues = ok_result(&v)["report"]["issues"]
        .as_array()
        .unwrap()
        .clone();
    let multi: Vec<&Value> = issues
        .iter()
        .filter(|i| i["code"] == "multi_owner")
        .collect();
    assert_eq!(multi.len(), 1, "{issues:?}");
    assert_eq!(multi[0]["target"], "p_lid");

    // ... and the reviewer's 5-level chain breaks the depth ≤ 4 cap (a validator rule that
    // `describe` does not run; `describe` is bounded regardless).
    let (_, _, v) = call(&request("validate", json!({"document": chain_doc(5, 32)})));
    let report = &ok_result(&v)["report"];
    assert_eq!(report["valid"], false);
    assert!(
        report["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["code"] == "schema_invalid"
                && i["message"].as_str().unwrap().contains("depth"))
    );
}

// ---- the global response cap -------------------------------------------------------------------

#[test]
fn oversize_responses_become_response_too_large() {
    let doc = example(PLATE_2);
    let input = request("validate", json!({"document": doc})).to_string();
    let full = api::handle_json(&input);
    assert!(full.starts_with(r#"{"status":"ok""#));

    // exactly at the limit: unchanged; one byte below: the error envelope
    assert_eq!(api::handle_json_limited(&input, full.len()), full);
    let capped = api::handle_json_limited(&input, full.len() - 1);
    let typed: CoreResponse = serde_json::from_str(&capped).unwrap();
    let error = typed.error().expect("error envelope");
    assert_eq!(error.code, ErrorCode::ResponseTooLarge);
    assert_eq!(error.path, None);
    assert!(error.violations.is_empty());
    assert_eq!(
        capped,
        CoreResponse::Error(api::response_too_large(full.len() - 1)).to_json()
    );
    // the envelope conforms to the response schema and is deterministic
    let value: Value = serde_json::from_str(&capped).unwrap();
    assert!(check_schema(&response_schema(), &value).is_empty());
    assert_eq!(api::handle_json_limited(&input, full.len() - 1), capped);
    // to_json uses MAX_RESPONSE_BYTES
    let response: CoreResponse = serde_json::from_str(&full).unwrap();
    assert_eq!(response.to_json(), full);
    assert_eq!(
        response.to_json_limited(MAX_RESPONSE_BYTES),
        response.to_json()
    );
    const { assert!(MAX_RESPONSE_BYTES < 32 * 1024 * 1024) };
}

#[test]
fn response_cap_error_fits_any_sane_limit() {
    // The error envelope itself is small: even a tiny limit yields a parseable error.
    let out = api::handle_json_limited(r#"{"op":"version"}"#, 10);
    let typed: CoreResponse = serde_json::from_str(&out).unwrap();
    assert_eq!(typed.error().unwrap().code, ErrorCode::ResponseTooLarge);
}

// ---- amplification audit: adversarial maximal documents ----------------------------------------

/// A 32-char id (`ID_MAX_LEN`).
fn id(prefix: &str, i: usize) -> String {
    let mut s = format!("{prefix}_{i}");
    while s.len() < 32 {
        s.push('x');
    }
    s
}

/// Every top-level list and string at (or one below, for the lists patches append to) its
/// schema limit.
fn base_doc() -> Value {
    let parameters: Vec<Value> = (0..64)
        .map(|i| {
            let mut name = format!("p{i}");
            while name.len() < 32 {
                name.push('q');
            }
            json!({"name": name, "value": 1 + i, "min": null, "max": null, "note": "註".repeat(120)})
        })
        .collect();
    json!({
        "schema_version": "2.0", "name": "n".repeat(80), "source_prompt": "字".repeat(20_000),
        "unit": "mm", "revision": 9_223_372_036_854_775_808_u64,
        "parameters": parameters, "bindings": [], "components": [], "constraints": [],
        "parts": [], "assemblies": [], "root": null,
        "assumptions": vec!["假".repeat(300); 31], "notes": vec!["記".repeat(300); 31],
        "planner": {"planner": "agent_team", "confidence": 0.9, "review_required": true},
    })
}

fn fixed(i: usize, component: &str) -> Value {
    json!({"id": id("k", i), "kind": "fixed", "a": format!("{component}.origin"), "b": null,
           "value": null, "u": null, "v": null,
           "frame": {"origin": [0, 0, 0], "x_axis": "+x", "z_axis": "+z"}})
}

fn push(doc: &mut Value, key: &str, item: Value) {
    doc[key].as_array_mut().unwrap().push(item);
}

/// 255 reserve boxes stacked at the origin (pairwise overlapping) + one plate part: the
/// quadratic worst case of the validator (`reserve_overlap` per pair), with 512 constraints
/// and 128 bindings.
fn reserves_doc() -> Value {
    let mut doc = base_doc();
    let base = id("v_base", 0);
    push(
        &mut doc,
        "components",
        json!({"id": base, "kind": "volume",
        "shape": {"kind": "plate", "length": 100, "width": 100, "thickness": 5},
        "label": "標".repeat(80), "provenance": {"agent": "component_smith", "patch_id": "r".repeat(64)}}),
    );
    push(&mut doc, "constraints", fixed(0, &base));
    let reserves: Vec<String> = (0..255).map(|i| id("e_r", i)).collect();
    for (i, r) in reserves.iter().enumerate() {
        push(
            &mut doc,
            "components",
            json!({"id": r, "kind": "empty", "role": "reserve",
            "shape": {"kind": "box", "length": 10, "width": 10, "height": 10},
            "label": "標".repeat(80), "provenance": {"agent": "space_planner", "patch_id": "r".repeat(64)}}),
        );
        push(&mut doc, "constraints", fixed(i + 1, r));
    }
    for j in 0..256 {
        let (a, b) = (&reserves[j % 255], &reserves[(j + 1) % 255]);
        push(
            &mut doc,
            "constraints",
            json!({"id": id("k_d", j), "kind": "distance",
            "a": format!("{a}.top"), "b": format!("{b}.top"),
            "value": 0, "u": null, "v": null, "frame": null}),
        );
    }
    let names: Vec<String> = (0..4)
        .map(|k| doc["parameters"][k]["name"].as_str().unwrap().to_owned())
        .collect();
    for r in reserves.iter().take(128) {
        push(
            &mut doc,
            "bindings",
            json!({"target": format!("{r}:length"), "expr": names.join("+")}),
        );
    }
    push(
        &mut doc,
        "parts",
        json!({"id": id("p", 0), "name": "n".repeat(80), "material": "aluminum",
        "quantity": 999, "source": "made", "body": [{"op": "add", "component": base}],
        "fillets": [], "chamfers": []}),
    );
    doc
}

/// One plate with 127 through holes at the same spot (`overlapping_holes` per pair).
fn holes_doc() -> Value {
    let mut doc = base_doc();
    let base = id("v_base", 0);
    push(
        &mut doc,
        "components",
        json!({"id": base, "kind": "volume",
        "shape": {"kind": "plate", "length": 100, "width": 100, "thickness": 5}}),
    );
    push(&mut doc, "constraints", fixed(0, &base));
    let mut body = vec![json!({"op": "add", "component": base})];
    for i in 0..127 {
        let h = id("e_h", i);
        push(
            &mut doc,
            "components",
            json!({"id": h, "kind": "empty", "role": "cut",
            "shape": {"kind": "hole", "diameter": 6, "hole_type": "through", "depth": null,
                      "counterbore_diameter": null, "counterbore_depth": null,
                      "countersink_diameter": null, "countersink_angle": null,
                      "thread": null, "axis": "z"}}),
        );
        push(
            &mut doc,
            "constraints",
            json!({"id": id("k_h", i), "kind": "on_face",
            "a": format!("{h}.entry"), "b": format!("{base}.top"),
            "value": null, "u": 50, "v": 50, "frame": null}),
        );
        body.push(json!({"op": "cut", "component": h}));
    }
    push(
        &mut doc,
        "parts",
        json!({"id": id("p", 0), "name": "n", "quantity": 1, "source": "made", "body": body}),
    );
    doc
}

/// 32 coincident plate parts (`part_interference` per pair) in 16 assemblies × 32 children.
fn parts_doc() -> Value {
    let mut doc = base_doc();
    for i in 0..32 {
        let c = id("v_p", i);
        push(
            &mut doc,
            "components",
            json!({"id": c, "kind": "volume",
            "shape": {"kind": "plate", "length": 100, "width": 100, "thickness": 5}}),
        );
        push(&mut doc, "constraints", fixed(i, &c));
        push(
            &mut doc,
            "parts",
            json!({"id": id("p", i), "name": "n", "quantity": 1,
            "source": "made", "body": [{"op": "add", "component": c}]}),
        );
    }
    let mut children: Vec<String> = (1..16).map(|i| id("a", i)).collect();
    children.extend((0..17).map(|i| id("p", i)));
    let assemblies: Vec<Value> = (0..16)
        .map(|i| json!({"id": id("a", i), "name": "n".repeat(80), "children": children}))
        .collect();
    doc["assemblies"] = json!(assemblies);
    doc
}

/// The reviewer's maximal document: 256 plates, 512 random constraints, 128 long bindings,
/// 32 parts × 128 body ops, 16 assemblies × 32 children.
fn reviewer_max_doc() -> Value {
    let mut components = vec![json!({"id": "v_base", "kind": "volume",
        "shape": {"kind": "plate", "length": 100, "width": 100, "thickness": 5}})];
    components.extend((0..255).map(|i| {
        json!({"id": format!("v_p{i}"), "kind": "volume",
        "shape": {"kind": "plate", "length": 10, "width": 10, "thickness": 1}})
    }));
    let ids: Vec<String> = components
        .iter()
        .map(|c| c["id"].as_str().unwrap().to_owned())
        .collect();
    let mut constraints = vec![json!({"id": "k_fix", "kind": "fixed", "a": "v_base.origin",
        "b": null, "value": null, "u": null, "v": null,
        "frame": {"origin": [0, 0, 0], "x_axis": "+x", "z_axis": "+z"}})];
    let kinds = [
        "coincident",
        "mate",
        "flush",
        "distance",
        "coaxial",
        "on_face",
        "inside",
        "clearance",
    ];
    let mut seed: u64 = 1;
    let mut rand = |n: usize| {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((seed >> 33) as usize) % n
    };
    for j in 0..511 {
        let a = rand(256);
        let b = (a + 1 + rand(255)) % 256;
        let k = kinds[rand(kinds.len())];
        let value = if matches!(k, "distance" | "mate" | "clearance") {
            json!(1.0)
        } else {
            Value::Null
        };
        constraints.push(json!({"id": format!("k_{j}"), "kind": k,
            "a": format!("{}.top", ids[a]), "b": format!("{}.bottom", ids[b]),
            "value": value, "u": null, "v": null, "frame": null}));
    }
    let parameters: Vec<Value> = (0..64)
        .map(|i| json!({"name": format!("p{i}"), "value": 1 + i, "min": null, "max": null, "note": null}))
        .collect();
    let bindings: Vec<Value> = (0..128)
        .map(|i| {
            let expr: String = (0..20).map(|k| format!("p{}", (i + k) % 64)).collect::<Vec<_>>().join(" + ");
            json!({"target": format!("v_p{i}:length"), "expr": expr.chars().take(160).collect::<String>()})
        })
        .collect();
    let parts: Vec<Value> = (0..32)
        .map(|i| json!({"id": format!("p_{i}"), "name": "n", "quantity": 1, "source": "made",
            "body": (0..128).map(|k| json!({"op": "add", "component": format!("v_p{}", (i * 8 + k) % 255)})).collect::<Vec<_>>()}))
        .collect();
    let assemblies: Vec<Value> = (0..16)
        .map(|i| {
            json!({"id": format!("a{i}"), "name": "n",
            "children": (0..32).map(|k| format!("p_{k}")).collect::<Vec<_>>()})
        })
        .collect();
    json!({"schema_version": "2.0", "source_prompt": "x".repeat(20_000), "revision": 0,
        "parameters": parameters, "bindings": bindings, "components": components,
        "constraints": constraints, "parts": parts, "assemblies": assemblies,
        "planner": {"planner": "agent_team", "confidence": 0.9, "review_required": true}})
}

/// A 1.0 document at its caps (64 holes, 8 fillets, 8 chamfers, 32 notes / assumptions).
fn max_v1_doc() -> Value {
    let mut doc = example(PLATE_1);
    let hole = doc["holes"][0].clone();
    doc["holes"] = json!((0..64)
        .map(|i| {
            let mut h = hole.clone();
            h["x"] = json!(-50.0 + (i % 8) as f64 * 12.0);
            h["y"] = json!(-20.0 + (i / 8) as f64 * 5.0);
            h
        })
        .collect::<Vec<_>>());
    doc["fillets"] = json!(vec![doc["fillets"][0].clone(); 8]);
    doc["chamfers"] = json!([]);
    doc["source_prompt"] = json!("字".repeat(20_000));
    doc["name"] = json!("n".repeat(80));
    doc["assumptions"] = json!(vec!["假".repeat(200); 32]);
    doc["notes"] = json!(vec!["記".repeat(200); 32]);
    doc
}

/// A patch touching several top-level keys (a large `revert`), with a 500-char rationale.
fn wide_patch(doc: &Value) -> Value {
    let component = doc["components"][0]["id"].as_str().unwrap();
    let mut parameter = doc["parameters"][0].clone();
    parameter["value"] = json!(2);
    json!({
        "agent": "component_smith", "base_revision": doc["revision"],
        "rationale": "理".repeat(500), "resolves": vec!["reserve_overlap"; 16],
        "ops": [
            {"op": "set_field", "target": format!("{component}:length"), "value": 101},
            {"op": "add_assumption", "text": "新".repeat(300)},
            {"op": "add_note", "text": "新".repeat(300)},
            {"op": "set_parameter", "parameter": parameter},
        ],
    })
}

/// Size bound per op: `linear` ops stay within 2× the request + 64 KiB; the quadratic ones
/// (`validate`, `apply_patch`) are bounded by the measured worst case with headroom.
fn bound_for(op: &str, request_len: usize) -> usize {
    match op {
        "validate" => 16 * 1024 * 1024,
        "apply_patch" => 20 * 1024 * 1024,
        _ => 2 * request_len + 64 * 1024,
    }
}

fn audit(label: &str, doc: &Value, ops: &[&str]) -> Vec<(String, usize)> {
    let mut sizes = Vec::new();
    for op in ops {
        let req = match *op {
            "apply_patch" => request(
                "apply_patch",
                json!({"document": doc, "patch": wide_patch(doc),
                "origin": "human", "timestamp": "t".repeat(40), "log_seq": 5}),
            ),
            _ => request(op, json!({"document": doc})),
        };
        let len = req.to_string().len();
        let v = bounded(&format!("{label} {op}"), &req, bound_for(op, len));
        assert_eq!(
            v["status"],
            "ok",
            "{label} {op}: {}",
            &v.to_string()[..300.min(v.to_string().len())]
        );
        sizes.push(((*op).to_owned(), v.to_string().len()));
    }
    sizes
}

const V2_OPS: [&str; 6] = [
    "validate",
    "solve",
    "normalize",
    "describe",
    "downgrade",
    "apply_patch",
];

#[test]
fn amplification_reserves_worst_case() {
    let doc = reserves_doc();
    let sizes = audit("reserves", &doc, &V2_OPS);
    // the quadratic driver is really exercised: ≥ C(255, 2) reserve_overlap issues
    let v = bounded(
        "reserves validate",
        &request("validate", json!({"document": doc})),
        16 << 20,
    );
    let overlaps = ok_result(&v)["report"]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["code"] == "reserve_overlap")
        .count();
    assert_eq!(overlaps, 255 * 254 / 2);
    // and the accepted patch carries report + log_entry + log_line
    let apply = sizes.iter().find(|(op, _)| op == "apply_patch").unwrap().1;
    let validate = sizes.iter().find(|(op, _)| op == "validate").unwrap().1;
    assert!(apply > validate, "{sizes:?}");
}

#[test]
fn amplification_holes_worst_case() {
    let doc = holes_doc();
    audit("holes", &doc, &V2_OPS);
}

#[test]
fn amplification_parts_and_assemblies_worst_case() {
    let doc = parts_doc();
    audit("parts", &doc, &V2_OPS);
}

#[test]
fn amplification_reviewer_max_document() {
    let doc = reviewer_max_doc();
    audit(
        "maxdoc",
        &doc,
        &["validate", "solve", "normalize", "describe", "downgrade"],
    );
}

#[test]
fn amplification_max_1_0_document() {
    let doc = max_v1_doc();
    audit(
        "v1",
        &doc,
        &["validate", "solve", "normalize", "describe", "upgrade"],
    );
}

#[test]
fn amplification_version() {
    bounded("version", &request("version", json!({})), 1024);
}
