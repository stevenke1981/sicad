//! Canonical serialization (spec §1.4) and example-file helpers (§11).
//!
//! Canonical form = `serde_json` pretty printing (2-space indent, UTF-8, no trailing newline)
//! of the typed value: every field written in struct (= spec table) order, `null`s and empty
//! lists included. All document numbers except integer-typed fields (`revision`, `quantity`,
//! `base_revision`, `feature_index`) are `f64`, so `120` in an input file is written back as
//! `120.0`; compare documents with [`normalize_numbers`] when the input was hand-written.

use serde::Serialize;
use serde_json::{Map, Value};

use crate::model::schema::LoadError;
use crate::model::v2::CadDocument2;

/// Top-level keys that exist only in example files and must be removed before typed parsing.
pub const EXAMPLE_ONLY_KEYS: &[&str] = &["expected_resolved"];

/// Canonical pretty JSON of a 2.0 document.
pub fn to_canonical_json(doc: &CadDocument2) -> String {
    to_canonical_string(doc)
}

/// Canonical pretty JSON of any model value (patches, 1.0 documents, reports, `resolved.json`).
pub fn to_canonical_string<T: Serialize + ?Sized>(value: &T) -> String {
    serde_json::to_string_pretty(value).expect("model types always serialize to JSON")
}

/// The canonical form as a `serde_json::Value`.
pub fn to_canonical_value<T: Serialize + ?Sized>(value: &T) -> Value {
    serde_json::to_value(value).expect("model types always serialize to JSON")
}

/// Lower-case hex SHA-256 of `text` (e.g. `patch_sha256` over a canonical patch).
pub fn sha256_hex(text: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(text.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// SHA-256 of the canonical JSON of `value`.
pub fn canonical_sha256<T: Serialize + ?Sized>(value: &T) -> String {
    sha256_hex(&to_canonical_string(value))
}

/// Removes [`EXAMPLE_ONLY_KEYS`] from a top-level JSON object and returns them.
pub fn strip_example_keys(value: &mut Value) -> Map<String, Value> {
    let mut removed = Map::new();
    if let Value::Object(map) = value {
        for key in EXAMPLE_ONLY_KEYS {
            if let Some(v) = map.remove(*key) {
                removed.insert((*key).to_owned(), v);
            }
        }
    }
    removed
}

/// Parses an example file: strips example-only keys, then parses and schema-checks the
/// document. Returns the document and the `expected_resolved` annotation, if any.
pub fn parse_example(s: &str) -> Result<(CadDocument2, Option<Value>), LoadError> {
    let mut value: Value = serde_json::from_str(s)?;
    let mut removed = strip_example_keys(&mut value);
    let doc = CadDocument2::from_value(value)?;
    Ok((doc, removed.remove("expected_resolved")))
}

/// Returns a copy of `value` with every number converted to an `f64` number, so that `120`
/// and `120.0` compare equal.
pub fn normalize_numbers(value: &Value) -> Value {
    match value {
        Value::Number(n) => n
            .as_f64()
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number)
            .unwrap_or_else(|| value.clone()),
        Value::Array(items) => Value::Array(items.iter().map(normalize_numbers).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), normalize_numbers(v)))
                .collect(),
        ),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sha256_known_vector() {
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(canonical_sha256(&json!({"a": 1})).len(), 64);
    }

    #[test]
    fn strip_and_normalize() {
        let mut v = json!({"a": 1, "expected_resolved": {"x": 1}});
        let removed = strip_example_keys(&mut v);
        assert_eq!(v, json!({"a": 1}));
        assert_eq!(removed["expected_resolved"], json!({"x": 1}));
        assert_ne!(json!([1, 2.5]), json!([1.0, 2.5]));
        assert_eq!(
            normalize_numbers(&json!([1, {"b": 2.5}])),
            normalize_numbers(&json!([1.0, {"b": 2.5}]))
        );
    }
}
