//! Schema-level checks that serde cannot express, plus JSON-Schema helper fragments.
//!
//! `check_schema` functions (on [`crate::model::v2::CadDocument2`],
//! [`crate::model::v1::CadDocument1`], [`crate::model::patch::Patch`]) return a list of
//! [`SchemaViolation`]s, each carrying a JSON-pointer path (`/components/3/shape/diameter`) and
//! a fixed-template English message. An empty list means the value is schema-valid.

use std::fmt;

use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};

use super::geom::MAX_DIMENSION_MM;

/// One schema-level violation (range, length, list cap, cross-field shape rule, …).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SchemaViolation {
    /// JSON pointer to the offending value (`""` for the document root).
    pub path: String,
    /// Human-readable, fixed-template message (embeds only ids and numbers).
    pub message: String,
}

impl fmt::Display for SchemaViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let path = if self.path.is_empty() {
            "/"
        } else {
            &self.path
        };
        write!(f, "{path}: {}", self.message)
    }
}

/// Error from a `from_json_str` loader: either JSON/serde parsing or the schema check failed.
#[derive(Debug)]
pub enum LoadError {
    /// JSON syntax, unknown field, bad id/ref, wrong type, …
    Json(serde_json::Error),
    /// The value parsed but violates schema-level constraints.
    Schema(Vec<SchemaViolation>),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Json(e) => write!(f, "invalid JSON document: {e}"),
            LoadError::Schema(v) => {
                write!(f, "schema violations:")?;
                for item in v {
                    write!(f, "\n  {item}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for LoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LoadError::Json(e) => Some(e),
            LoadError::Schema(_) => None,
        }
    }
}

impl From<serde_json::Error> for LoadError {
    fn from(e: serde_json::Error) -> Self {
        LoadError::Json(e)
    }
}

/// Number of Unicode scalar values (matches Python `len(str)` used by the 1.0 Pydantic model).
pub fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// Accumulates [`SchemaViolation`]s.
#[derive(Debug, Default)]
pub(crate) struct Checker {
    pub(crate) out: Vec<SchemaViolation>,
}

impl Checker {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn finish(self) -> Vec<SchemaViolation> {
        self.out
    }

    pub(crate) fn err(&mut self, path: impl Into<String>, message: impl Into<String>) {
        self.out.push(SchemaViolation {
            path: path.into(),
            message: message.into(),
        });
    }

    fn finite(&mut self, path: &str, v: f64) -> bool {
        if v.is_finite() {
            true
        } else {
            self.err(path, "must be a finite number");
            false
        }
    }

    /// Any finite float.
    pub(crate) fn float(&mut self, path: &str, v: f64) {
        self.finite(path, v);
    }

    /// Optional finite float.
    pub(crate) fn opt_float(&mut self, path: &str, v: Option<f64>) {
        if let Some(v) = v {
            self.finite(path, v);
        }
    }

    /// `Dim`: `0 < v ≤ MAX_DIMENSION_MM`.
    pub(crate) fn dim(&mut self, path: &str, v: f64) {
        if self.finite(path, v) && !(v > 0.0 && v <= MAX_DIMENSION_MM) {
            self.err(
                path,
                format!("must satisfy 0 < x <= {MAX_DIMENSION_MM} (got {v})"),
            );
        }
    }

    /// Optional `Dim`.
    pub(crate) fn opt_dim(&mut self, path: &str, v: Option<f64>) {
        if let Some(v) = v {
            self.dim(path, v);
        }
    }

    /// `Coord`: `−MAX_DIMENSION_MM ≤ v ≤ MAX_DIMENSION_MM`.
    pub(crate) fn coord(&mut self, path: &str, v: f64) {
        if self.finite(path, v) && !(-MAX_DIMENSION_MM..=MAX_DIMENSION_MM).contains(&v) {
            self.err(
                path,
                format!("must satisfy -{MAX_DIMENSION_MM} <= x <= {MAX_DIMENSION_MM} (got {v})"),
            );
        }
    }

    /// Open interval `lo < v < hi`.
    pub(crate) fn open_range(&mut self, path: &str, v: f64, lo: f64, hi: f64) {
        if self.finite(path, v) && !(v > lo && v < hi) {
            self.err(path, format!("must satisfy {lo} < x < {hi} (got {v})"));
        }
    }

    /// Half-open interval `lo < v ≤ hi`.
    pub(crate) fn left_open_range(&mut self, path: &str, v: f64, lo: f64, hi: f64) {
        if self.finite(path, v) && !(v > lo && v <= hi) {
            self.err(path, format!("must satisfy {lo} < x <= {hi} (got {v})"));
        }
    }

    /// Closed interval `lo ≤ v ≤ hi`.
    pub(crate) fn closed_range(&mut self, path: &str, v: f64, lo: f64, hi: f64) {
        if self.finite(path, v) && !(lo..=hi).contains(&v) {
            self.err(path, format!("must satisfy {lo} <= x <= {hi} (got {v})"));
        }
    }

    /// String length in characters within `min..=max`.
    pub(crate) fn text(&mut self, path: &str, s: &str, min: usize, max: usize) {
        let n = char_len(s);
        if n < min || n > max {
            self.err(
                path,
                format!("string length must be between {min} and {max} (got {n})"),
            );
        }
    }

    /// Optional string, length in characters ≤ `max`.
    pub(crate) fn opt_text(&mut self, path: &str, s: Option<&str>, max: usize) {
        if let Some(s) = s {
            self.text(path, s, 0, max);
        }
    }

    /// List length within `min..=max`.
    pub(crate) fn list(&mut self, path: &str, len: usize, min: usize, max: usize) {
        if len < min || len > max {
            self.err(
                path,
                format!("list length must be between {min} and {max} (got {len})"),
            );
        }
    }
}

// ---- JSON-Schema fragments used through `#[schemars(schema_with = "...")]` ------------------

/// `Dim`: `0 < x ≤ 100000`.
pub(crate) fn dim(_: &mut SchemaGenerator) -> Schema {
    json_schema!({"type": "number", "exclusiveMinimum": 0.0, "maximum": MAX_DIMENSION_MM})
}

/// `Dim | null`.
pub(crate) fn opt_dim(_: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "type": ["number", "null"],
        "exclusiveMinimum": 0.0,
        "maximum": MAX_DIMENSION_MM
    })
}

/// `Coord`: `−100000 ≤ x ≤ 100000`.
pub(crate) fn coord(_: &mut SchemaGenerator) -> Schema {
    json_schema!({"type": "number", "minimum": -MAX_DIMENSION_MM, "maximum": MAX_DIMENSION_MM})
}

/// `Coord | null`.
pub(crate) fn opt_coord(_: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "type": ["number", "null"],
        "minimum": -MAX_DIMENSION_MM,
        "maximum": MAX_DIMENSION_MM
    })
}

/// `[Coord, Coord, Coord]`.
pub(crate) fn coord3(_: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "type": "array",
        "items": {"type": "number", "minimum": -MAX_DIMENSION_MM, "maximum": MAX_DIMENSION_MM},
        "minItems": 3,
        "maxItems": 3
    })
}

/// 1.0 hole coordinate (`Coord`, default 0).
pub(crate) fn coord_default0(_: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "type": "number",
        "minimum": -MAX_DIMENSION_MM,
        "maximum": MAX_DIMENSION_MM,
        "default": 0.0
    })
}

/// Countersink angle: `0 < x < 180`, nullable.
pub(crate) fn opt_countersink_angle(_: &mut SchemaGenerator) -> Schema {
    json_schema!({"type": ["number", "null"], "exclusiveMinimum": 0.0, "exclusiveMaximum": 180.0})
}

/// Revolve angle: `0 < x ≤ 360`.
pub(crate) fn revolve_angle(_: &mut SchemaGenerator) -> Schema {
    json_schema!({"type": "number", "exclusiveMinimum": 0.0, "maximum": 360.0})
}

/// Planner confidence: `0 ≤ x ≤ 1`, default 1.
pub(crate) fn confidence(_: &mut SchemaGenerator) -> Schema {
    json_schema!({"type": "number", "minimum": 0.0, "maximum": 1.0, "default": 1.0})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checker_ranges() {
        let mut c = Checker::new();
        c.dim("/a", 1.0);
        c.dim("/b", 0.0);
        c.dim("/c", MAX_DIMENSION_MM);
        c.dim("/d", MAX_DIMENSION_MM + 1.0);
        c.dim("/e", f64::NAN);
        c.coord("/f", -MAX_DIMENSION_MM);
        c.coord("/g", -MAX_DIMENSION_MM - 0.5);
        c.text("/h", "ab", 1, 2);
        c.text("/i", "", 1, 2);
        c.text("/j", "一二", 1, 2);
        c.list("/k", 3, 1, 2);
        let paths: Vec<_> = c.finish().into_iter().map(|v| v.path).collect();
        assert_eq!(paths, ["/b", "/d", "/e", "/g", "/i", "/k"]);
    }
}
