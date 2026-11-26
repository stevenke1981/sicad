//! Reverse deltas for undo (ADR 0005: after undo the canonical serialization is byte-identical
//! to the pre-patch document).
//!
//! A [`Revert`] stores, for every top-level document key whose canonical serialization changed,
//! the key's *previous* JSON value (including `revision`). Applying it to the post-patch
//! document substitutes those keys and re-parses, which yields exactly the pre-image: the
//! typed round trip `CadDocument2 → JSON → CadDocument2` is lossless (`float_roundtrip`), and
//! keys are compared by their serialized text, so even `-0.0` vs `0.0` is preserved.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::canonical::to_canonical_value;
use crate::model::v2::CadDocument2;

/// Previous values of the top-level keys a patch changed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Revert {
    /// Top-level key → value before the patch.
    pub sections: Map<String, Value>,
}

/// Why a [`Revert`] could not be applied.
#[derive(Debug)]
pub struct RevertError(serde_json::Error);

impl std::fmt::Display for RevertError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "revert delta does not produce a valid document: {}",
            self.0
        )
    }
}

impl std::error::Error for RevertError {}

fn top_level(doc: &CadDocument2) -> Map<String, Value> {
    match to_canonical_value(doc) {
        Value::Object(map) => map,
        // A document always serializes to an object; an empty map is the harmless fallback.
        _ => Map::new(),
    }
}

fn same(a: &Value, b: &Value) -> bool {
    // Text comparison: `Value` equality treats -0.0 == 0.0, the canonical text does not.
    serde_json::to_string(a).ok() == serde_json::to_string(b).ok()
}

impl Revert {
    /// The delta that turns `after` back into `before`.
    pub fn between(before: &CadDocument2, after: &CadDocument2) -> Self {
        let old = top_level(before);
        let new = top_level(after);
        let sections = old
            .into_iter()
            .filter(|(k, v)| new.get(k).is_none_or(|n| !same(v, n)))
            .collect();
        Revert { sections }
    }

    /// Applies the delta to `after` (the post-patch document), returning the pre-image.
    pub fn apply(&self, after: &CadDocument2) -> Result<CadDocument2, RevertError> {
        let mut map = top_level(after);
        for (k, v) in &self.sections {
            map.insert(k.clone(), v.clone());
        }
        serde_json::from_value(Value::Object(map)).map_err(RevertError)
    }

    /// Names of the restored top-level keys.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.sections.keys().map(String::as_str)
    }
}
