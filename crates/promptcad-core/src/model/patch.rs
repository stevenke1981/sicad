//! Patch protocol types (spec §8.1–§8.2). Applying patches lives in [`crate::patch_engine`].

use std::borrow::Cow;
use std::fmt;

use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};

use super::ids::{FieldRef, Id};
pub use super::issue::Role;
use super::schema::{char_len, Checker, LoadError, SchemaViolation};
use super::v2::{check_body, Assembly, BodyOp, ComponentInput, Constraint, Parameter, Part};

/// The submitting role (§8.1). `human` is not a patch agent; the coordinator stamps it.
pub type PatchAgent = Role;

/// Patch size limits (§8.1).
pub mod limits {
    /// `ops` min length.
    pub const OPS_MIN: usize = 1;
    /// `ops` max length.
    pub const OPS_MAX: usize = 64;
    /// `resolves` max length.
    pub const RESOLVES: usize = 16;
}

/// Error for a text newtype whose length is out of bounds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextLengthError {
    /// Minimum chars.
    pub min: usize,
    /// Maximum chars.
    pub max: usize,
    /// Actual chars.
    pub len: usize,
}

impl fmt::Display for TextLengthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "string length must be between {} and {} (got {})",
            self.min, self.max, self.len
        )
    }
}

impl std::error::Error for TextLengthError {}

macro_rules! bounded_text {
    ($(#[$meta:meta])* $name:ident, $min:expr, $max:expr) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            /// Minimum length in chars.
            pub const MIN: usize = $min;
            /// Maximum length in chars.
            pub const MAX: usize = $max;

            /// Validates the length (in Unicode scalar values) and wraps `s`.
            pub fn new(s: impl Into<String>) -> Result<Self, TextLengthError> {
                let s = s.into();
                let len = char_len(&s);
                if (Self::MIN..=Self::MAX).contains(&len) {
                    Ok(Self(s))
                } else {
                    Err(TextLengthError { min: Self::MIN, max: Self::MAX, len })
                }
            }

            /// The text.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = TextLengthError;
            fn try_from(s: String) -> Result<Self, Self::Error> {
                Self::new(s)
            }
        }

        impl From<$name> for String {
            fn from(t: $name) -> String {
                t.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl JsonSchema for $name {
            fn schema_name() -> Cow<'static, str> {
                stringify!($name).into()
            }
            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                json_schema!({"type": "string", "minLength": $min, "maxLength": $max})
            }
        }
    };
}

bounded_text!(
    /// Assumption / note text in a patch op, 1–300 chars.
    Text300,
    1,
    300
);
bounded_text!(
    /// Patch `rationale`, 1–500 chars.
    Rationale,
    1,
    500
);
bounded_text!(
    /// Binding expression source, 1–160 chars (grammar §4.3, parsed by [`crate::expr`]).
    ExprText,
    1,
    160
);

/// A patch (§8.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Patch {
    /// Submitting role.
    pub agent: PatchAgent,
    /// Must equal the document's current `revision`.
    pub base_revision: u64,
    /// 1–500 chars.
    pub rationale: Rationale,
    /// Issue codes this patch addresses, ≤ 16.
    #[schemars(length(max = 16))]
    pub resolves: Vec<String>,
    /// 1–64 ops, applied in order.
    #[schemars(length(min = 1, max = 64))]
    pub ops: Vec<PatchOp>,
}

/// A patch operation: closed union discriminated by `op` (§8.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum PatchOp {
    /// Add a component (`provenance` is stamped by the coordinator).
    AddComponent {
        /// The component.
        component: ComponentInput,
    },
    /// Remove a component (and bindings targeting it).
    RemoveComponent {
        /// Component id.
        id: Id,
    },
    /// Set a component numeric field or constraint `value`/`u`/`v`.
    SetField {
        /// `id:field`.
        target: FieldRef,
        /// New value.
        value: f64,
    },
    /// Add or replace the binding of `target`.
    SetBinding {
        /// `id:field`.
        target: FieldRef,
        /// Expression source.
        expr: ExprText,
    },
    /// Remove the binding of `target` (the field keeps its cached value).
    RemoveBinding {
        /// `id:field`.
        target: FieldRef,
    },
    /// Add a constraint.
    AddConstraint {
        /// The constraint.
        constraint: Constraint,
    },
    /// Remove a constraint (and bindings targeting it).
    RemoveConstraint {
        /// Constraint id.
        id: Id,
    },
    /// Add or replace a parameter by `name`.
    SetParameter {
        /// The parameter.
        parameter: Parameter,
    },
    /// Add a part.
    AddPart {
        /// The part.
        part: Part,
    },
    /// Replace an existing part's `body`.
    SetBody {
        /// Part id.
        part_id: Id,
        /// New body, 1–128 ops.
        #[schemars(length(min = 1, max = 128))]
        body: Vec<BodyOp>,
    },
    /// Add an assembly.
    AddAssembly {
        /// The assembly.
        assembly: Assembly,
    },
    /// Append an assumption.
    AddAssumption {
        /// 1–300 chars.
        text: Text300,
    },
    /// Remove an identical assumption (§8.7).
    RemoveAssumption {
        /// 1–300 chars.
        text: Text300,
    },
    /// Append a note.
    AddNote {
        /// 1–300 chars.
        text: Text300,
    },
}

impl PatchOp {
    /// The `op` tag string.
    pub fn op_name(&self) -> &'static str {
        match self {
            PatchOp::AddComponent { .. } => "add_component",
            PatchOp::RemoveComponent { .. } => "remove_component",
            PatchOp::SetField { .. } => "set_field",
            PatchOp::SetBinding { .. } => "set_binding",
            PatchOp::RemoveBinding { .. } => "remove_binding",
            PatchOp::AddConstraint { .. } => "add_constraint",
            PatchOp::RemoveConstraint { .. } => "remove_constraint",
            PatchOp::SetParameter { .. } => "set_parameter",
            PatchOp::AddPart { .. } => "add_part",
            PatchOp::SetBody { .. } => "set_body",
            PatchOp::AddAssembly { .. } => "add_assembly",
            PatchOp::AddAssumption { .. } => "add_assumption",
            PatchOp::RemoveAssumption { .. } => "remove_assumption",
            PatchOp::AddNote { .. } => "add_note",
        }
    }

    /// `add_assumption`, `remove_assumption`, `add_note` are annotation ops; everything else is
    /// a design op (§8.7).
    pub fn is_annotation(&self) -> bool {
        matches!(
            self,
            PatchOp::AddAssumption { .. }
                | PatchOp::RemoveAssumption { .. }
                | PatchOp::AddNote { .. }
        )
    }

    pub(crate) fn check(&self, c: &mut Checker, path: &str) {
        match self {
            PatchOp::AddComponent { component } => component.check(c, &format!("{path}/component")),
            PatchOp::SetField { value, .. } => c.float(&format!("{path}/value"), *value),
            PatchOp::AddConstraint { constraint } => {
                constraint.check(c, &format!("{path}/constraint"))
            }
            PatchOp::SetParameter { parameter } => parameter.check(c, &format!("{path}/parameter")),
            PatchOp::AddPart { part } => part.check(c, &format!("{path}/part")),
            PatchOp::SetBody { body, .. } => check_body(c, &format!("{path}/body"), body),
            PatchOp::AddAssembly { assembly } => assembly.check(c, &format!("{path}/assembly")),
            PatchOp::RemoveComponent { .. }
            | PatchOp::SetBinding { .. }
            | PatchOp::RemoveBinding { .. }
            | PatchOp::RemoveConstraint { .. }
            | PatchOp::AddAssumption { .. }
            | PatchOp::RemoveAssumption { .. }
            | PatchOp::AddNote { .. } => {}
        }
    }
}

impl Patch {
    /// Schema-level checks (list caps and the payload rules of
    /// [`crate::model::v2::CadDocument2::check_schema`] applied to each op). A failure maps to
    /// reject reason `schema_invalid`.
    pub fn check_schema(&self) -> Vec<SchemaViolation> {
        let mut c = Checker::new();
        c.list("/resolves", self.resolves.len(), 0, limits::RESOLVES);
        c.list("/ops", self.ops.len(), limits::OPS_MIN, limits::OPS_MAX);
        for (i, op) in self.ops.iter().enumerate() {
            op.check(&mut c, &format!("/ops/{i}"));
        }
        c.finish()
    }

    /// `true` when at least one op is a design op (§8.7).
    pub fn has_design_op(&self) -> bool {
        self.ops.iter().any(|op| !op.is_annotation())
    }

    /// Parses JSON and runs [`Patch::check_schema`].
    pub fn from_json_str(s: &str) -> Result<Self, LoadError> {
        let p: Self = serde_json::from_str(s)?;
        let v = p.check_schema();
        if v.is_empty() {
            Ok(p)
        } else {
            Err(LoadError::Schema(v))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn text_newtypes() {
        assert!(Text300::new("").is_err());
        assert!(Text300::new("a".repeat(300)).is_ok());
        assert!(Text300::new("一".repeat(301)).is_err());
        assert!(Rationale::new("x".repeat(500)).is_ok());
        assert!(ExprText::new("x".repeat(161)).is_err());
        assert!(serde_json::from_value::<Text300>(json!("")).is_err());
    }

    #[test]
    fn patch_round_trip_and_checks() {
        let j = json!({
            "agent": "component_smith",
            "base_revision": 3,
            "rationale": "bind body height",
            "resolves": ["reserve_violated"],
            "ops": [
                {"op": "set_binding", "target": "v_body:height", "expr": "wall + standoff_h + pcb_stack + 1"},
                {"op": "add_component", "component": {"id": "d_a", "kind": "dot", "shape": {"kind": "point"}, "label": null}},
                {"op": "set_field", "target": "k_d1:u", "value": -50},
                {"op": "set_body", "part_id": "p_main", "body": [{"op": "add", "component": "v_base"}]},
                {"op": "remove_assumption", "text": "old"}
            ]
        });
        let p: Patch = serde_json::from_value(j.clone()).unwrap();
        assert!(p.check_schema().is_empty());
        assert!(p.has_design_op());
        assert_eq!(p.ops[0].op_name(), "set_binding");
        let back = serde_json::to_value(&p).unwrap();
        assert_eq!(
            crate::canonical::normalize_numbers(&back),
            crate::canonical::normalize_numbers(&j)
        );

        // unknown op field, unknown op, provenance in add_component, human agent
        for bad in [
            json!({"agent":"joiner","base_revision":0,"rationale":"r","resolves":[],"ops":[{"op":"add_note","text":"t","x":1}]}),
            json!({"agent":"joiner","base_revision":0,"rationale":"r","resolves":[],"ops":[{"op":"remove_part","id":"p"}]}),
            json!({"agent":"human","base_revision":0,"rationale":"r","resolves":[],"ops":[{"op":"add_note","text":"t"}]}),
            json!({"agent":"joiner","base_revision":0,"rationale":"","resolves":[],"ops":[{"op":"add_note","text":"t"}]}),
            json!({"agent":"component_smith","base_revision":0,"rationale":"r","resolves":[],"ops":[
                {"op":"add_component","component":{"id":"d","kind":"dot","shape":{"kind":"point"},"label":null,
                 "provenance":{"agent":"human","patch_id":null}}}]}),
        ] {
            assert!(
                serde_json::from_value::<Patch>(bad.clone()).is_err(),
                "{bad}"
            );
        }

        let empty: Patch = serde_json::from_value(
            json!({"agent":"joiner","base_revision":0,"rationale":"r","resolves":[],"ops":[]}),
        )
        .unwrap();
        assert_eq!(empty.check_schema()[0].path, "/ops");
        let only_notes: Patch = serde_json::from_value(json!({"agent":"joiner","base_revision":0,
            "rationale":"r","resolves":[],"ops":[{"op":"remove_assumption","text":"t"}]}))
        .unwrap();
        assert!(!only_notes.has_design_op());
    }
}
