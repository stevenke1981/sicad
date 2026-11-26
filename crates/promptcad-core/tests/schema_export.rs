//! JSON Schema export (ADR 0008): every object schema is closed (`additionalProperties: false`).

use promptcad_core::model::{CadDocument1, CadDocument2, Patch, Resolved, ValidationReport};
use serde_json::Value;

/// Visits every sub-schema object that declares `properties`.
fn walk<'a>(v: &'a Value, path: String, out: &mut Vec<(String, &'a Value)>) {
    match v {
        Value::Object(map) => {
            if map.contains_key("properties") {
                out.push((path.clone(), v));
            }
            for (k, child) in map {
                walk(child, format!("{path}/{k}"), out);
            }
        }
        Value::Array(items) => {
            for (i, child) in items.iter().enumerate() {
                walk(child, format!("{path}/{i}"), out);
            }
        }
        _ => {}
    }
}

fn assert_closed(name: &str, schema: &Value) -> usize {
    let mut objects = Vec::new();
    walk(schema, String::new(), &mut objects);
    assert!(!objects.is_empty(), "{name}: no object schemas");
    for (path, obj) in &objects {
        assert_eq!(
            obj.get("additionalProperties"),
            Some(&Value::Bool(false)),
            "{name}: object at {path} is not closed"
        );
    }
    objects.len()
}

fn schema_json<T: schemars::JsonSchema>() -> Value {
    serde_json::to_value(schemars::schema_for!(T)).unwrap()
}

#[test]
fn document_schemas_are_closed() {
    let doc2 = schema_json::<CadDocument2>();
    let n = assert_closed("CadDocument2", &doc2);
    assert!(n > 30, "CadDocument2 has {n} object schemas");
    assert_closed("CadDocument1", &schema_json::<CadDocument1>());
    assert_closed("Patch", &schema_json::<Patch>());
    assert_closed("Resolved", &schema_json::<Resolved>());
    assert_closed("ValidationReport", &schema_json::<ValidationReport>());
}

#[test]
fn schema_carries_patterns_and_ranges() {
    let doc2 = schema_json::<CadDocument2>();
    let defs = &doc2["$defs"];
    assert_eq!(defs["Id"]["pattern"], "^[a-z][a-z0-9_]{0,31}$");
    assert_eq!(
        defs["PortRef"]["pattern"],
        r"^[a-z][a-z0-9_]{0,31}\.[a-z][a-z0-9_]{0,31}$"
    );
    assert_eq!(
        defs["FieldRef"]["pattern"],
        "^[a-z][a-z0-9_]{0,31}:[a-z][a-z0-9_]{0,31}$"
    );
    let text = doc2.to_string();
    assert!(
        text.contains("\"exclusiveMinimum\":0.0"),
        "Dim lower bound exported"
    );
    assert!(
        text.contains("\"maximum\":100000.0"),
        "Dim upper bound exported"
    );
    let thread = serde_json::to_string(r"^M\d{1,3}(\.\d)?(x\d(\.\d{1,2})?)?$").unwrap();
    assert!(text.contains(&thread), "thread pattern exported");
    // a component variant carries `role` only for empties
    let comp = &defs["Component"];
    let variants = comp["oneOf"].as_array().expect("Component is a oneOf");
    assert_eq!(variants.len(), 5);
    for v in variants {
        let kind = v["properties"]["kind"]["const"].as_str().unwrap();
        assert_eq!(
            v["properties"].get("role").is_some(),
            kind == "empty",
            "{kind}"
        );
        assert!(v["properties"].get("origin").is_none());
    }

    let patch = schema_json::<Patch>();
    let ops = patch["$defs"]["PatchOp"]["oneOf"].as_array().unwrap();
    assert_eq!(ops.len(), 14);
    let input = &patch["$defs"]["ComponentInput"]["oneOf"];
    for v in input.as_array().unwrap() {
        assert!(v["properties"].get("provenance").is_none());
    }
}
