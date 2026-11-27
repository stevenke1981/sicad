//! Shared helpers for the API tests: example loading, request round trips and a small
//! structural JSON Schema checker for the exported request / response schemas.

#![allow(dead_code)]

use std::fs;
use std::path::Path;

use promptcad_core::api::{self, CoreResponse};
use serde_json::{json, Value};

pub const PLATE_2: &str = "examples/cad2/plate-four-holes.cad2.json";
pub const ENCLOSURE_2: &str = "examples/cad2/enclosure-with-lid.cad2.json";
pub const PLATE_1: &str = "examples/generated/plate-four-holes/spec.json";

/// An example document with its example-only keys removed.
pub fn example(rel: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel);
    let mut value: Value =
        serde_json::from_str(&fs::read_to_string(&path).expect("read example")).expect("JSON");
    promptcad_core::canonical::strip_example_keys(&mut value);
    value
}

/// A request object `{"op": op, ...fields}`.
pub fn request(op: &str, fields: Value) -> Value {
    let mut map = serde_json::Map::new();
    map.insert("op".into(), json!(op));
    if let Value::Object(f) = fields {
        map.extend(f);
    }
    Value::Object(map)
}

/// The `add_note` patch for `doc` as `agent`.
pub fn note_patch(doc: &Value, agent: &str, text: &str) -> Value {
    json!({
        "agent": agent,
        "base_revision": doc["revision"],
        "rationale": "api test note",
        "resolves": [],
        "ops": [{"op": "add_note", "text": text}],
    })
}

/// Sends `input` through [`api::handle_json`] and checks the invariants every response must
/// satisfy: it parses into [`CoreResponse`], re-serializes to the same bytes, and conforms
/// to the exported response schema. Returns the raw text, the typed and the JSON response.
pub fn call_text(input: &str) -> (String, CoreResponse, Value) {
    let out = api::handle_json(input);
    assert!(!out.contains('\n'), "response must be a single line");
    let typed: CoreResponse = serde_json::from_str(&out)
        .unwrap_or_else(|e| panic!("response does not parse as CoreResponse: {e}\n{out}"));
    assert_eq!(
        typed.to_json(),
        out,
        "typed round trip must be byte-identical"
    );
    let value: Value = serde_json::from_str(&out).unwrap();
    let errors = check_schema(&response_schema(), &value);
    assert!(errors.is_empty(), "response violates schema: {errors:#?}");
    (out, typed, value)
}

/// [`call_text`] on a JSON request value, which must itself conform to the request schema.
pub fn call(req: &Value) -> (String, CoreResponse, Value) {
    let errors = check_schema(&request_schema(), req);
    assert!(errors.is_empty(), "request violates schema: {errors:#?}");
    call_text(&req.to_string())
}

/// The `result` object of an `ok` response (panics on `error`).
pub fn ok_result(value: &Value) -> &Value {
    assert_eq!(value["status"], "ok", "expected ok, got {value}");
    &value["result"]
}

/// The error object of an `error` response.
pub fn error_of(value: &Value) -> &Value {
    assert_eq!(value["status"], "error", "expected error, got {value}");
    value
}

pub fn request_schema() -> Value {
    serde_json::to_value(api::request_schema()).unwrap()
}

pub fn response_schema() -> Value {
    serde_json::to_value(api::response_schema()).unwrap()
}

// ---- structural JSON Schema checker -----------------------------------------------------------

/// Checks `value` against `schema` (a root schema with `$defs`). Supports the keywords
/// schemars emits: `$ref`, `oneOf`, `anyOf`, `allOf`, `type`, `const`, `enum`, `properties`,
/// `required`, `additionalProperties`, `patternProperties` (id / port-ref patterns), `items`, `minItems`, `maxItems`, `minimum`,
/// `maximum`, `exclusiveMinimum`, `exclusiveMaximum`, `minLength`, `maxLength`.
/// `pattern` and `format` are not checked. Returns the violations (empty = conforms).
pub fn check_schema(root: &Value, value: &Value) -> Vec<String> {
    let mut errors = Vec::new();
    check(root, root, value, "", &mut errors);
    errors
}

fn resolve<'a>(root: &'a Value, reference: &str) -> &'a Value {
    let name = reference
        .strip_prefix("#/$defs/")
        .unwrap_or_else(|| panic!("unsupported $ref {reference}"));
    root["$defs"]
        .get(name)
        .unwrap_or_else(|| panic!("missing definition {name}"))
}

fn matches(root: &Value, schema: &Value, value: &Value) -> bool {
    let mut errors = Vec::new();
    check(root, schema, value, "", &mut errors);
    errors.is_empty()
}
/// The only `patternProperties` patterns schemars emits here are the id and port-ref
/// patterns; check them with the crate's own parsers instead of a regex engine.
fn pattern_matches(pattern: &str, key: &str) -> bool {
    use promptcad_core::model::{Id, PortRef};
    match pattern {
        "^[a-z][a-z0-9_]{0,31}$" => Id::new(key).is_ok(),
        r"^[a-z][a-z0-9_]{0,31}\.[a-z][a-z0-9_]{0,31}$" => PortRef::new(key).is_ok(),
        other => panic!("unsupported patternProperties pattern {other}"),
    }
}

fn type_ok(t: &str, value: &Value) -> bool {
    match t {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value.is_i64() || value.is_u64(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        other => panic!("unknown type {other}"),
    }
}

fn check(root: &Value, schema: &Value, value: &Value, path: &str, errors: &mut Vec<String>) {
    let map = match schema {
        Value::Bool(true) => return,
        Value::Bool(false) => {
            errors.push(format!("{path}: schema `false`"));
            return;
        }
        Value::Object(map) => map,
        other => panic!("not a schema: {other}"),
    };
    if let Some(Value::String(r)) = map.get("$ref") {
        check(root, resolve(root, r), value, path, errors);
    }
    if let Some(t) = map.get("type") {
        let ok = match t {
            Value::String(t) => type_ok(t, value),
            Value::Array(ts) => ts.iter().any(|t| type_ok(t.as_str().unwrap(), value)),
            _ => panic!("bad type"),
        };
        if !ok {
            errors.push(format!("{path}: expected type {t}, got {value}"));
            return;
        }
    }
    if let Some(c) = map.get("const") {
        if c != value {
            errors.push(format!("{path}: expected const {c}, got {value}"));
        }
    }
    if let Some(Value::Array(options)) = map.get("enum") {
        if !options.contains(value) {
            errors.push(format!("{path}: {value} not in enum"));
        }
    }
    if let Some(Value::Array(subs)) = map.get("oneOf") {
        let results: Vec<Vec<String>> = subs
            .iter()
            .map(|s| {
                let mut e = Vec::new();
                check(root, s, value, path, &mut e);
                e
            })
            .collect();
        let n = results.iter().filter(|e| e.is_empty()).count();
        if n != 1 {
            errors.push(format!("{path}: matches {n} oneOf branches"));
            if n == 0 {
                // report the closest branch, preferring one whose discriminator matched
                let score = |e: &Vec<String>| {
                    e.len() + 1000 * e.iter().filter(|m| m.contains("expected const")).count()
                };
                if let Some(best) = results.into_iter().min_by_key(score) {
                    errors.extend(best);
                }
            }
        }
    }
    if let Some(Value::Array(subs)) = map.get("anyOf") {
        if !subs.iter().any(|s| matches(root, s, value)) {
            errors.push(format!("{path}: matches no anyOf branch"));
        }
    }
    if let Some(Value::Array(subs)) = map.get("allOf") {
        for s in subs {
            check(root, s, value, path, errors);
        }
    }
    if let Value::Object(obj) = value {
        let props = map.get("properties").and_then(Value::as_object);
        if let Some(Value::Array(required)) = map.get("required") {
            for r in required {
                if !obj.contains_key(r.as_str().unwrap()) {
                    errors.push(format!("{path}: missing required {r}"));
                }
            }
        }
        let patterns = map.get("patternProperties").and_then(Value::as_object);
        for (k, v) in obj {
            let child = format!("{path}/{k}");
            let by_pattern = patterns.and_then(|p| {
                p.iter()
                    .find(|(pattern, _)| pattern_matches(pattern, k))
                    .map(|(_, s)| s)
            });
            match props.and_then(|p| p.get(k)).or(by_pattern) {
                Some(s) => check(root, s, v, &child, errors),
                None => match map.get("additionalProperties") {
                    Some(Value::Bool(false)) => errors.push(format!("{child}: not allowed")),
                    Some(s @ Value::Object(_)) => check(root, s, v, &child, errors),
                    _ => {}
                },
            }
        }
    }
    if let Value::Array(items) = value {
        if let Some(s) = map.get("items") {
            for (i, item) in items.iter().enumerate() {
                check(root, s, item, &format!("{path}/{i}"), errors);
            }
        }
        if let Some(n) = map.get("minItems").and_then(Value::as_u64) {
            if (items.len() as u64) < n {
                errors.push(format!("{path}: fewer than {n} items"));
            }
        }
        if let Some(n) = map.get("maxItems").and_then(Value::as_u64) {
            if items.len() as u64 > n {
                errors.push(format!("{path}: more than {n} items"));
            }
        }
    }
    if let Some(x) = value.as_f64() {
        let bound = |k: &str| map.get(k).and_then(Value::as_f64);
        if bound("minimum").is_some_and(|m| x < m)
            || bound("maximum").is_some_and(|m| x > m)
            || bound("exclusiveMinimum").is_some_and(|m| x <= m)
            || bound("exclusiveMaximum").is_some_and(|m| x >= m)
        {
            errors.push(format!("{path}: {x} out of range"));
        }
    }
    if let Value::String(s) = value {
        let n = s.chars().count() as u64;
        if map
            .get("minLength")
            .and_then(Value::as_u64)
            .is_some_and(|m| n < m)
            || map
                .get("maxLength")
                .and_then(Value::as_u64)
                .is_some_and(|m| n > m)
        {
            errors.push(format!("{path}: string length {n} out of range"));
        }
    }
}
