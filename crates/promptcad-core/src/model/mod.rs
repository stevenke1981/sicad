//! The document model: CadDocument 2.0 (normative, `docs/CAD_DSL_2.md`), CadDocument 1.0
//! (compatibility), patches, validation issues and solver output.

pub mod common;
pub mod geom;
pub mod ids;
pub mod issue;
pub mod patch;
pub mod resolved;
pub mod schema;
pub mod v1;
pub mod v2;

pub use geom::{Axis, AxisDir, Frame, Orientation, Vec3, MAX_DIMENSION_MM, TOL};
pub use ids::{FieldRef, Id, IdError, PortRef};
pub use issue::codes::{Code, CodeClass, CodeInfo, DefaultOwner, RejectCode};
pub use issue::{Layer, Role, Severity, ValidationIssue, ValidationReport};
pub use patch::{Patch, PatchAgent, PatchOp};
pub use resolved::{PortType, Resolved, ResolvedComponent, ResolvedHole, ResolvedPort};
pub use schema::{LoadError, SchemaViolation};
pub use v1::CadDocument1;
pub use v2::{
    CadDocument2, Component, ComponentInput, ComponentKind, Constraint, Geometry, ShapeKind,
};
