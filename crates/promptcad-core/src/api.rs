//! Transport-neutral request/response contract (ADR 0008 rule 1; integration ADR 0012).
//!
//! One dispatcher, three hosts:
//!
//! * the Tauri desktop shell calls [`handle`] in-process (a `core_request` command);
//! * server mode spawns `promptcad-core rpc` per call and exchanges one JSON request / one JSON
//!   response over stdin/stdout ([`handle_json`]);
//! * the CLI subcommands build a [`CoreRequest`] and print parts of the result.
//!
//! # Wire format
//!
//! A request is one JSON object tagged by `op` ([`CoreRequest`]); unknown fields are rejected.
//! Document and patch inputs (`document`, `patch`) are *raw JSON values*: they are parsed into
//! the typed model inside the dispatcher, so an untrusted document that does not parse yields a
//! structured [`CoreResponse::Error`] (with a JSON pointer) instead of a request-level failure.
//!
//! A response is tagged by `status`:
//!
//! * `ok` — `{"status": "ok", "result": {"op": "<same op>", ...}}` ([`CoreResult`]). Domain
//!   outcomes — an invalid validation report, a rejected patch, a document that cannot be
//!   downgraded — are `ok` results.
//! * `error` — `{"status": "error", "code", "message", "path", "violations"}` ([`CoreError`])
//!   for requests that cannot be processed at all; `code` is the closed [`ErrorCode`] enum.
//!
//! Paths in errors are JSON pointers relative to the *request* (`/document/components/3/shape`,
//! `/patch/ops/0`, `/document/schema_version`).
//!
//! [`handle_json`] returns compact single-line JSON (no trailing newline), keys in struct
//! order, maps sorted; the output is a pure function of the input bytes.
//!
//! # Size bounds
//!
//! Requests are capped at [`MAX_REQUEST_BYTES`]; every response that leaves the core through
//! [`CoreResponse::to_json`] (and so [`handle_json`], `promptcad-core rpc` and the Tauri
//! command) is capped at [`MAX_RESPONSE_BYTES`] and replaced by an
//! [`ErrorCode::ResponseTooLarge`] error beyond it. Within the schema limits no op comes
//! close: the largest responses are `validate` / `apply_patch` of a document whose 255
//! `reserve` Empties all overlap pairwise (one `reserve_overlap` issue per pair, ~11 MB and
//! ~15 MB); every other op is linear in the document size. The `describe` tree expands each
//! part / assembly at most once (later occurrences are [`TreeNode::repeated`]
//! back-references) and is additionally capped at [`MAX_TREE_NODES`]. See
//! `tests/api_regressions.rs` for the per-op worst cases.
//!
//! Documents are not stripped of example-only keys (`expected_resolved`): callers that read
//! example files remove them first (see [`crate::canonical::strip_example_keys`]).

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::panic::{self, AssertUnwindSafe};

use schemars::generate::SchemaSettings;
use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::bindings::recompute_bindings;
use crate::canonical::canonical_sha256;
use crate::fields::{component_field, constraint_field};
use crate::migrate::{downgrade_2_to_1, upgrade_1_to_2};
use crate::model::ids::{FieldRef, Id, PortRef};
use crate::model::issue::{ValidationIssue, ValidationReport};
use crate::model::resolved::{PortType, Resolved};
use crate::model::schema::SchemaViolation;
use crate::model::v1::CadDocument1;
use crate::model::v2::{
    BodyOp, CadDocument2, Component, ComponentKind, Constraint, ConstraintKind, EmptyRole,
    Material, Parameter, PartSource, Provenance, ShapeKind,
};
use crate::model::Patch;
use crate::oplog::OpLogEntry;
use crate::patch_engine::{apply_patch, patch_sha256, PatchOrigin, PatchOutcome, RejectReason};
use crate::ports::component_port_table;
use crate::solver::solve;
use crate::validate::{compute_root, validate, ValidateOptions};

/// Maximum accepted size of a JSON request in bytes (8 MiB) for [`handle_json`] and
/// `promptcad-core rpc`. A maximal 2.0 document (256 components, 512 constraints, …) is far
/// below this; larger inputs get a [`ErrorCode::TooLarge`] error.
pub const MAX_REQUEST_BYTES: usize = 8 * 1024 * 1024;

/// Maximum size of a serialized response in bytes (24 MiB). Larger responses are replaced by an
/// [`ErrorCode::ResponseTooLarge`] error (see [`CoreResponse::to_json`]). The legitimate
/// maximum within the schema limits is ~15 MB (`apply_patch` on a document with 255 pairwise
/// overlapping `reserve` Empties); the server's stdout cap (32 MiB) stays above this limit
/// plus the trailing newline of `promptcad-core rpc`.
pub const MAX_RESPONSE_BYTES: usize = 24 * 1024 * 1024;

/// Maximum number of nodes in a `describe` tree. Each part / assembly is expanded at most
/// once, so a schema-valid document has at most `assemblies + parts + Σ children` ≤ 560
/// nodes; this is a second line of defence, exceeding it is an [`ErrorCode::Internal`]
/// error.
pub const MAX_TREE_NODES: usize = 4096;

/// Version of this request/response contract. Bumped on any incompatible wire change.
pub const API_VERSION: u32 = 1;

/// JSON pointer of the `document` input.
const DOC: &str = "/document";
/// JSON pointer of the `patch` input.
const PATCH: &str = "/patch";

// =============================================================================================
// request
// =============================================================================================

fn default_true() -> bool {
    true
}

/// Schema of the raw `document` input: any JSON object (parsed by the dispatcher).
fn raw_document(_: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "type": "object",
        "additionalProperties": true,
        "description": "A CadDocument 1.0 or 2.0 JSON object (schemas/cad2/cad-document-1.schema.json, cad-document-2.schema.json), dispatched on its `schema_version`. Parsed by the core: a document that does not parse yields an `error` response, not a request failure."
    })
}

/// Schema of the raw `patch` input.
fn raw_patch(_: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "type": "object",
        "additionalProperties": true,
        "description": "A Patch JSON object (schemas/cad2/patch.schema.json). Parsed by the core: a patch that does not parse yields an `invalid_patch` error; schema-level patch violations are a `schema_invalid` rejection."
    })
}

/// One request, tagged by `op`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum CoreRequest {
    /// Validate a 1.0 or 2.0 document (1.0 is upgraded first).
    Validate {
        /// Raw 1.0 or 2.0 document.
        #[schemars(schema_with = "raw_document")]
        document: Value,
        /// Check the completed-document rules (components ≥ 1, parts ≥ 1, root). Default `true`.
        #[serde(default = "default_true")]
        completed: bool,
    },
    /// Solve a 1.0 or 2.0 document (`resolved.json`).
    Solve {
        /// Raw 1.0 or 2.0 document.
        #[schemars(schema_with = "raw_document")]
        document: Value,
    },
    /// Canonical 2.0 form: upgrade 1.0, recompute bindings, recompute `root`.
    Normalize {
        /// Raw 1.0 or 2.0 document.
        #[schemars(schema_with = "raw_document")]
        document: Value,
    },
    /// Upgrade a 1.0 document to 2.0 (a missing `schema_version` counts as 1.0).
    Upgrade {
        /// Raw 1.0 document.
        #[schemars(schema_with = "raw_document")]
        document: Value,
    },
    /// Downgrade a 2.0 document to 1.0 when it is expressible there.
    Downgrade {
        /// Raw 2.0 document.
        #[schemars(schema_with = "raw_document")]
        document: Value,
    },
    /// Apply a patch to a 2.0 document (spec §8.6).
    ApplyPatch {
        /// Raw 2.0 document (the current state).
        #[schemars(schema_with = "raw_document")]
        document: Value,
        /// Raw patch.
        #[schemars(schema_with = "raw_patch")]
        patch: Value,
        /// Who submitted the patch, as determined by the host (not by the patch body).
        origin: PatchOrigin,
        /// Caller-supplied timestamp copied into the op-log entry (the core reads no clock).
        #[serde(default)]
        timestamp: Option<String>,
        /// `seq` of the op-log entry: the number of entries already in the host's
        /// `oplog.jsonl`. Default 0.
        #[serde(default)]
        log_seq: u64,
    },
    /// Read model for UIs: components with fields and ports, parts, assemblies, tree.
    Describe {
        /// Raw 1.0 or 2.0 document.
        #[schemars(schema_with = "raw_document")]
        document: Value,
    },
    /// Crate version and supported schema versions.
    Version {},
}

impl CoreRequest {
    /// The `op` string.
    pub fn op(&self) -> &'static str {
        match self {
            CoreRequest::Validate { .. } => "validate",
            CoreRequest::Solve { .. } => "solve",
            CoreRequest::Normalize { .. } => "normalize",
            CoreRequest::Upgrade { .. } => "upgrade",
            CoreRequest::Downgrade { .. } => "downgrade",
            CoreRequest::ApplyPatch { .. } => "apply_patch",
            CoreRequest::Describe { .. } => "describe",
            CoreRequest::Version {} => "version",
        }
    }
}

// =============================================================================================
// response
// =============================================================================================

/// One response, tagged by `status`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum CoreResponse {
    /// The request was processed; `result.op` equals the request `op`.
    Ok {
        /// Op-specific result.
        result: CoreResult,
    },
    /// The request could not be processed at all.
    Error(CoreError),
}

impl CoreResponse {
    /// The error, if any.
    pub fn error(&self) -> Option<&CoreError> {
        match self {
            CoreResponse::Ok { .. } => None,
            CoreResponse::Error(e) => Some(e),
        }
    }

    /// The result, if any.
    pub fn result(&self) -> Option<&CoreResult> {
        match self {
            CoreResponse::Ok { result } => Some(result),
            CoreResponse::Error(_) => None,
        }
    }

    /// Compact single-line JSON (the [`handle_json`] output format), at most
    /// [`MAX_RESPONSE_BYTES`] long: a larger response is replaced by an
    /// [`ErrorCode::ResponseTooLarge`] error. Every host emits responses through this method.
    pub fn to_json(&self) -> String {
        self.to_json_limited(MAX_RESPONSE_BYTES)
    }

    /// [`to_json`](Self::to_json) with an explicit limit of `limit` bytes. Serializes into a
    /// bounded buffer, so an oversized response is never fully materialized as text. Hosts
    /// use [`to_json`](Self::to_json); this exists so the cap can be exercised in tests.
    pub fn to_json_limited(&self, limit: usize) -> String {
        let mut writer = CappedWriter {
            buf: Vec::new(),
            limit,
        };
        match serde_json::to_writer(&mut writer, self) {
            // serde_json only writes valid UTF-8.
            Ok(()) => {
                String::from_utf8(writer.buf).unwrap_or_else(|_| INTERNAL_FALLBACK.to_owned())
            }
            Err(e) if e.is_io() => {
                let error = CoreResponse::Error(response_too_large(limit));
                serde_json::to_string(&error).unwrap_or_else(|_| INTERNAL_FALLBACK.to_owned())
            }
            Err(_) => INTERNAL_FALLBACK.to_owned(),
        }
    }
}

/// A `Vec` writer that fails as soon as more than `limit` bytes would be written.
struct CappedWriter {
    buf: Vec<u8>,
    limit: usize,
}

impl std::io::Write for CappedWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        if self.buf.len().saturating_add(data.len()) > self.limit {
            return Err(std::io::Error::other("response size limit exceeded"));
        }
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Used only if serializing a response fails (never expected).
const INTERNAL_FALLBACK: &str = r#"{"status":"error","code":"internal","message":"response serialization failed","path":null,"violations":[]}"#;

/// Closed set of request-level error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The request is not valid JSON, has an unknown `op`, an unknown / missing field or a
    /// field of the wrong type (including an unknown `origin`), or is not UTF-8.
    BadRequest,
    /// The request exceeds [`MAX_REQUEST_BYTES`].
    TooLarge,
    /// `document.schema_version` is missing, not a string, or not `"1.0"` / `"2.0"`.
    UnsupportedSchemaVersion,
    /// A supported `schema_version` that this op does not accept (`upgrade` needs 1.0,
    /// `downgrade` and `apply_patch` need 2.0).
    WrongSchemaVersion,
    /// The document does not deserialize into the typed model (not an object, wrong type,
    /// unknown field, malformed id or reference, missing required field, …).
    InvalidDocument,
    /// The document parsed but violates schema-level constraints (ranges, lengths, list caps,
    /// cross-field shape rules); `violations` lists them all, `path` is the first.
    SchemaViolation,
    /// The patch does not deserialize into the typed `Patch` model.
    InvalidPatch,
    /// The response would exceed [`MAX_RESPONSE_BYTES`] and was discarded. Not expected for
    /// documents within the schema limits (a safety net against output amplification).
    ResponseTooLarge,
    /// An internal failure (a caught panic, or a broken internal size invariant such as
    /// [`MAX_TREE_NODES`]). Never expected; please report.
    Internal,
}

impl ErrorCode {
    /// The wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::BadRequest => "bad_request",
            ErrorCode::TooLarge => "too_large",
            ErrorCode::UnsupportedSchemaVersion => "unsupported_schema_version",
            ErrorCode::WrongSchemaVersion => "wrong_schema_version",
            ErrorCode::InvalidDocument => "invalid_document",
            ErrorCode::SchemaViolation => "schema_violation",
            ErrorCode::InvalidPatch => "invalid_patch",
            ErrorCode::ResponseTooLarge => "response_too_large",
            ErrorCode::Internal => "internal",
        }
    }
}

/// A request-level error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CoreError {
    /// Error code.
    pub code: ErrorCode,
    /// Human-readable English message.
    pub message: String,
    /// JSON pointer into the request (`/document`, `/document/components/3/shape/diameter`,
    /// `/patch`, …); `null` when the error is not about one location.
    pub path: Option<String>,
    /// All schema violations (`schema_violation` only; empty otherwise), with request-relative
    /// paths.
    pub violations: Vec<SchemaViolation>,
}

impl CoreError {
    fn new(code: ErrorCode, message: impl Into<String>, path: Option<&str>) -> Self {
        CoreError {
            code,
            message: message.into(),
            path: path.map(str::to_owned),
            violations: Vec::new(),
        }
    }

    fn schema(base: &str, violations: Vec<SchemaViolation>) -> Self {
        let violations: Vec<SchemaViolation> = violations
            .into_iter()
            .map(|v| SchemaViolation {
                path: format!("{base}{}", v.path),
                message: v.message,
            })
            .collect();
        let message = format!(
            "{} schema violation(s): {}",
            violations.len(),
            violations
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ")
        );
        CoreError {
            code: ErrorCode::SchemaViolation,
            message,
            path: violations.first().map(|v| v.path.clone()),
            violations,
        }
    }
}

impl std::fmt::Display for CoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for CoreError {}

impl From<CoreError> for CoreResponse {
    fn from(e: CoreError) -> Self {
        CoreResponse::Error(e)
    }
}

/// A document `schema_version`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum DocumentVersion {
    /// `"1.0"`
    #[serde(rename = "1.0")]
    V1_0,
    /// `"2.0"`
    #[serde(rename = "2.0")]
    V2_0,
}

/// Op-specific result, tagged by `op` (the request's op name).
// Payloads are boxed (they carry whole documents); the box is invisible on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum CoreResult {
    /// Result of `validate`.
    Validate(Box<ValidateResult>),
    /// Result of `solve`.
    Solve(Box<SolveResult>),
    /// Result of `normalize`.
    Normalize(Box<NormalizeResult>),
    /// Result of `upgrade`.
    Upgrade(Box<UpgradeResult>),
    /// Result of `downgrade`.
    Downgrade(Box<DowngradeResult>),
    /// Result of `apply_patch`.
    ApplyPatch(Box<ApplyPatchResult>),
    /// Result of `describe`.
    Describe(Box<DescribeResult>),
    /// Result of `version`.
    Version(VersionResult),
}

/// `validate` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ValidateResult {
    /// `schema_version` of the input document.
    pub input_version: DocumentVersion,
    /// Analytic validation report (all issues, including the solver's).
    pub report: ValidationReport,
    /// Solver output (empty when the solver was gated, see `validate` module docs).
    pub resolved: Resolved,
    /// The canonical 2.0 document that was validated (the upgraded one for 1.0 input).
    pub document: CadDocument2,
}

/// `solve` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SolveResult {
    /// `schema_version` of the input document.
    pub input_version: DocumentVersion,
    /// `resolved.json`.
    pub resolved: Resolved,
}

/// `normalize` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NormalizeResult {
    /// `schema_version` of the input document.
    pub input_version: DocumentVersion,
    /// The canonical 2.0 document with bindings and `root` recomputed. Bindings with errors
    /// keep their cached value. If the recomputed document violates the schema, the response
    /// is a `schema_violation` error instead.
    pub document: CadDocument2,
    /// Binding issues of the recomputation (empty when all bindings evaluated).
    pub issues: Vec<ValidationIssue>,
}

/// `upgrade` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpgradeResult {
    /// The canonical 2.0 document.
    pub document: CadDocument2,
}

/// `downgrade` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DowngradeResult {
    /// Whether the document is expressible as CadDocument 1.0 (spec §10.2).
    pub expressible: bool,
    /// The 1.0 document; `null` when not expressible.
    pub document: Option<CadDocument1>,
}

/// `apply_patch` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyPatchResult {
    /// Accepted or rejected.
    pub outcome: PatchOutcomeView,
}

/// Outcome of `apply_patch`, tagged by `status`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PatchOutcomeView {
    /// The patch was accepted; the host replaces its document with `document`.
    Accepted(Box<AcceptedPatchView>),
    /// The patch was rejected; the host keeps its document unchanged.
    Rejected(Box<RejectedPatchView>),
}

/// An accepted patch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AcceptedPatchView {
    /// The normalized document at `revision_after`.
    pub document: CadDocument2,
    /// Validation of `document` (`completed = false`); only D-class issues.
    pub report: ValidationReport,
    /// Solver output of `document`.
    pub resolved: Resolved,
    /// Revision of the input document.
    pub revision_before: u64,
    /// `revision_before + 1`.
    pub revision_after: u64,
    /// `r<revision_after>`, the `provenance.patch_id` of created components.
    pub patch_id: String,
    /// Components created by this patch, in document order.
    pub created: Vec<Id>,
    /// SHA-256 of the canonical patch JSON.
    pub patch_sha256: String,
    /// SHA-256 of the canonical input document.
    pub doc_sha256_before: String,
    /// SHA-256 of the canonical `document`.
    pub doc_sha256_after: String,
    /// The op-log entry to append (`seq` = request `log_seq`, `timestamp` = request
    /// `timestamp`; contains the reverse delta for undo).
    pub log_entry: OpLogEntry,
    /// `log_entry` as the exact `oplog.jsonl` line (compact JSON, no trailing newline).
    pub log_line: String,
}

/// A rejected patch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RejectedPatchView {
    /// Why.
    pub reason: RejectReason,
    /// The P-class issues behind a validation rejection (empty for other reasons).
    pub issues: Vec<ValidationIssue>,
    /// SHA-256 of the canonical patch JSON.
    pub patch_sha256: String,
    /// SHA-256 of the canonical input document (unchanged).
    pub doc_sha256_before: String,
    /// The op-log entry to append (rejections are logged too).
    pub log_entry: OpLogEntry,
    /// `log_entry` as the exact `oplog.jsonl` line (compact JSON, no trailing newline).
    pub log_line: String,
}

/// `version` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VersionResult {
    /// `promptcad-core` crate version.
    pub crate_version: String,
    /// [`API_VERSION`] of this contract.
    pub api_version: u32,
    /// Accepted document `schema_version`s.
    pub schema_versions: Vec<DocumentVersion>,
}

// ---- describe -------------------------------------------------------------------------------

/// `shape.kind` of a component (the name; pair with the component `kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ShapeName {
    /// Empty `box`.
    Box,
    /// Empty or Volume `cylinder`.
    Cylinder,
    /// Empty `hole`.
    Hole,
    /// Empty `of`.
    Of,
    /// Dot `point`.
    Point,
    /// Line `segment`.
    Segment,
    /// Line `polyline`.
    Polyline,
    /// Line `arc`.
    Arc,
    /// Line `circle`.
    Circle,
    /// Plane `rect`.
    Rect,
    /// Plane `disc`.
    Disc,
    /// Plane `profile`.
    Profile,
    /// Plane `section`.
    Section,
    /// Volume `plate`.
    Plate,
    /// Volume `ring`.
    Ring,
    /// Volume `l_bracket`.
    LBracket,
    /// Volume `enclosure`.
    Enclosure,
    /// Volume `extrude`.
    Extrude,
    /// Volume `revolve`.
    Revolve,
}

impl From<ShapeKind> for ShapeName {
    fn from(s: ShapeKind) -> Self {
        use ShapeKind::*;
        match s {
            EmptyBox => ShapeName::Box,
            EmptyCylinder | VolumeCylinder => ShapeName::Cylinder,
            EmptyHole => ShapeName::Hole,
            EmptyOf => ShapeName::Of,
            DotPoint => ShapeName::Point,
            LineSegment => ShapeName::Segment,
            LinePolyline => ShapeName::Polyline,
            LineArc => ShapeName::Arc,
            LineCircle => ShapeName::Circle,
            PlaneRect => ShapeName::Rect,
            PlaneDisc => ShapeName::Disc,
            PlaneProfile => ShapeName::Profile,
            PlaneSection => ShapeName::Section,
            VolumePlate => ShapeName::Plate,
            VolumeRing => ShapeName::Ring,
            VolumeLBracket => ShapeName::LBracket,
            VolumeEnclosure => ShapeName::Enclosure,
            VolumeExtrude => ShapeName::Extrude,
            VolumeRevolve => ShapeName::Revolve,
        }
    }
}

/// `describe` result: a read model derived from the document (no solving).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DescribeResult {
    /// `schema_version` of the input document (1.0 input is described after upgrading).
    pub input_version: DocumentVersion,
    /// Document `name`.
    pub name: String,
    /// Document `revision`.
    pub revision: u64,
    /// Stored `root` (part or assembly id).
    pub root: Option<Id>,
    /// The root the coordinator would compute (spec §6.3); `null` when it would be
    /// `invalid_root`.
    pub computed_root: Option<Id>,
    /// Parameters as stored.
    pub parameters: Vec<Parameter>,
    /// Components in document order.
    pub components: Vec<ComponentView>,
    /// Constraints in document order.
    pub constraints: Vec<ConstraintView>,
    /// Parts in document order.
    pub parts: Vec<PartView>,
    /// Assemblies in document order.
    pub assemblies: Vec<AssemblyView>,
    /// Assembly tree: top-level nodes (assemblies that are no assembly's child, then parts
    /// that are no assembly's child), each with its children expanded recursively. Every
    /// part / assembly is expanded at its first occurrence (pre-order) only; later
    /// occurrences are [`TreeNode::repeated`] leaves, so the tree has at most
    /// `assemblies + parts + Σ children` nodes (never more than [`MAX_TREE_NODES`]).
    pub tree: Vec<TreeNode>,
}

/// One component in the read model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ComponentView {
    /// Component id.
    pub id: Id,
    /// Component kind.
    pub kind: ComponentKind,
    /// Empty role (`null` for non-Empties).
    pub role: Option<EmptyRole>,
    /// `shape.kind`.
    pub shape: ShapeName,
    /// Independent (owns a pose) vs derived (§3.2).
    pub independent: bool,
    /// Frame port name of an independent component (`null` for derived ones).
    pub frame_port: Option<String>,
    /// Human-only label.
    pub label: Option<String>,
    /// Provenance stamp.
    pub provenance: Option<Provenance>,
    /// First part (document order) whose `body` references this component, if any.
    pub part: Option<Id>,
    /// Ids referenced from `shape` (promotion sources), in field order.
    pub refs: Vec<ShapeRef>,
    /// Numeric shape fields (§3.3) in spec order with current values and binding state.
    pub fields: Vec<FieldView>,
    /// Ports (§3.4) in table order; Empty `of` uses its source Volume's table (empty when the
    /// source does not resolve).
    pub ports: Vec<PortView>,
}

/// A reference from a shape to another component.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShapeRef {
    /// Shape field holding the reference (`source`, `start`, `points`, `profile`, …).
    pub field: String,
    /// Referenced component id.
    pub id: Id,
}

/// A numeric (bindable) field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FieldView {
    /// Field name (`length`, `value`, `u`, …); `id:name` is its field reference.
    pub name: String,
    /// Current (cached) value; `null` for an optional field that is null (not bindable).
    pub value: Option<f64>,
    /// Driven by a binding.
    pub bound: bool,
    /// The binding expression when bound.
    pub expr: Option<String>,
}

/// A port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PortView {
    /// Port name; `id.name` is its port reference.
    pub name: String,
    /// Port type.
    #[serde(rename = "type")]
    pub port_type: PortType,
}

/// One constraint in the read model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConstraintView {
    /// Constraint id.
    pub id: Id,
    /// Constraint kind.
    pub kind: ConstraintKind,
    /// Port `a`.
    pub a: PortRef,
    /// Port `b` (`null` for `fixed`).
    pub b: Option<PortRef>,
    /// Numeric fields of this kind (`value`, or `u`/`v`) with §5.1 defaults applied.
    pub fields: Vec<FieldView>,
}

/// One part in the read model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PartView {
    /// Part id.
    pub id: Id,
    /// Part name.
    pub name: String,
    /// Material.
    pub material: Option<Material>,
    /// Quantity.
    pub quantity: u32,
    /// `made` / `purchased`.
    pub source: PartSource,
    /// Boolean recipe.
    pub body: Vec<BodyOp>,
    /// First assembly (document order) listing this part as a child.
    pub parent: Option<Id>,
}

/// One assembly in the read model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AssemblyView {
    /// Assembly id.
    pub id: Id,
    /// Assembly name.
    pub name: String,
    /// Child ids (parts or assemblies) as stored.
    pub children: Vec<Id>,
    /// First assembly (document order) listing this assembly as a child.
    pub parent: Option<Id>,
}

/// What a tree node refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TreeNodeKind {
    /// An assembly.
    Assembly,
    /// A part.
    Part,
    /// A child id that is neither (dangling reference).
    Missing,
}

/// A node of the assembly tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TreeNode {
    /// Part or assembly id.
    pub id: Id,
    /// Node kind.
    pub node: TreeNodeKind,
    /// Part / assembly name (`null` for `missing`).
    pub name: Option<String>,
    /// `true` when this part / assembly already occurs earlier in the tree (pre-order): the
    /// node is a back-reference to that first occurrence and has no children. Only documents
    /// whose assembly graph is not a tree have repeated nodes: a child listed twice in one
    /// assembly, a child of two assemblies (`multi_owner`) or a cycle (`reference_cycle`).
    /// Always `false` for `missing` nodes.
    pub repeated: bool,
    /// Children (an assembly's first occurrence only; empty for parts, `missing` and
    /// repeated nodes).
    pub children: Vec<TreeNode>,
}

// =============================================================================================
// schemas
// =============================================================================================

/// JSON Schema of [`CoreRequest`] (`schemas/cad2/core-request.schema.json`), describing what
/// the core *accepts* (fields with defaults are optional).
pub fn request_schema() -> Schema {
    SchemaSettings::draft2020_12()
        .for_deserialize()
        .into_generator()
        .into_root_schema_for::<CoreRequest>()
}

/// JSON Schema of [`CoreResponse`] (`schemas/cad2/core-response.schema.json`), describing what
/// the core *emits*: generated for serialization, so every field the core always writes
/// (including `null`s and empty lists) is `required`.
pub fn response_schema() -> Schema {
    SchemaSettings::draft2020_12()
        .for_serialize()
        .into_generator()
        .into_root_schema_for::<CoreResponse>()
}

// =============================================================================================
// document loading
// =============================================================================================

/// Which document versions an op accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Accept {
    Any,
    V1Only,
    V2Only,
}

/// A loaded document of either version.
enum Loaded {
    V1(CadDocument1),
    V2(CadDocument2),
}

fn load(value: Value, accept: Accept) -> Result<Loaded, CoreError> {
    let version_path = "/document/schema_version";
    let Value::Object(map) = &value else {
        return Err(CoreError::new(
            ErrorCode::InvalidDocument,
            "document must be a JSON object",
            Some(DOC),
        ));
    };
    let version = match map.get("schema_version") {
        None if accept == Accept::V1Only => DocumentVersion::V1_0,
        None => {
            return Err(CoreError::new(
                ErrorCode::UnsupportedSchemaVersion,
                "document has no schema_version (expected \"1.0\" or \"2.0\")",
                Some(version_path),
            ))
        }
        Some(Value::String(s)) if s == "1.0" => DocumentVersion::V1_0,
        Some(Value::String(s)) if s == "2.0" => DocumentVersion::V2_0,
        Some(_) => {
            return Err(CoreError::new(
                ErrorCode::UnsupportedSchemaVersion,
                "unsupported schema_version (expected \"1.0\" or \"2.0\")",
                Some(version_path),
            ))
        }
    };
    let wrong = |expected: &str| {
        CoreError::new(
            ErrorCode::WrongSchemaVersion,
            format!("this op expects a CadDocument {expected} document"),
            Some(version_path),
        )
    };
    match (version, accept) {
        (DocumentVersion::V2_0, Accept::V1Only) => Err(wrong("1.0")),
        (DocumentVersion::V1_0, Accept::V2Only) => Err(wrong("2.0")),
        (DocumentVersion::V1_0, _) => {
            let doc: CadDocument1 = serde_json::from_value(value).map_err(invalid_document)?;
            let violations = doc.check_schema();
            if violations.is_empty() {
                Ok(Loaded::V1(doc))
            } else {
                Err(CoreError::schema(DOC, violations))
            }
        }
        (DocumentVersion::V2_0, _) => {
            let doc: CadDocument2 = serde_json::from_value(value).map_err(invalid_document)?;
            let violations = doc.check_schema();
            if violations.is_empty() {
                Ok(Loaded::V2(doc))
            } else {
                Err(CoreError::schema(DOC, violations))
            }
        }
    }
}

fn invalid_document(e: serde_json::Error) -> CoreError {
    CoreError::new(
        ErrorCode::InvalidDocument,
        format!("invalid JSON document: {e}"),
        Some(DOC),
    )
}

/// Parses a raw 1.0 or 2.0 document into the 2.0 model (1.0 is upgraded), exactly as every
/// op that accepts both versions does. Errors carry request-relative paths (`/document/…`).
pub fn parse_any_document(value: Value) -> Result<(CadDocument2, DocumentVersion), CoreError> {
    match load(value, Accept::Any)? {
        Loaded::V1(d) => Ok((upgrade_1_to_2(&d), DocumentVersion::V1_0)),
        Loaded::V2(d) => Ok((d, DocumentVersion::V2_0)),
    }
}

/// Parses a raw 2.0 document (a 1.0 document is [`ErrorCode::WrongSchemaVersion`]).
pub fn parse_document_v2(value: Value) -> Result<CadDocument2, CoreError> {
    match load(value, Accept::V2Only)? {
        Loaded::V2(d) => Ok(d),
        Loaded::V1(_) => Err(CoreError::new(
            ErrorCode::Internal,
            "unexpected 1.0 document",
            Some(DOC),
        )),
    }
}

fn parse_document_v1(value: Value) -> Result<CadDocument1, CoreError> {
    match load(value, Accept::V1Only)? {
        Loaded::V1(d) => Ok(d),
        Loaded::V2(_) => Err(CoreError::new(
            ErrorCode::Internal,
            "unexpected 2.0 document",
            Some(DOC),
        )),
    }
}

/// Parses a raw patch with serde only. Schema-level violations are left to the patch engine,
/// which rejects them as `schema_invalid` (a logged domain outcome).
fn parse_patch(value: Value) -> Result<Patch, CoreError> {
    if !value.is_object() {
        return Err(CoreError::new(
            ErrorCode::InvalidPatch,
            "patch must be a JSON object",
            Some(PATCH),
        ));
    }
    serde_json::from_value(value).map_err(|e| {
        CoreError::new(
            ErrorCode::InvalidPatch,
            format!("invalid patch: {e}"),
            Some(PATCH),
        )
    })
}

// =============================================================================================
// dispatch
// =============================================================================================

/// Handles one request. Pure and deterministic; never panics (a panic inside the core is
/// caught and reported as [`ErrorCode::Internal`], see below). Applies no request size limit
/// (see [`handle_json`]); the response limit applies when the response is serialized
/// ([`CoreResponse::to_json`]).
///
/// **The panic guard needs unwinding.** `catch_unwind` (here and in [`handle_json`]) only
/// catches panics in a binary built with `panic = "unwind"`: the default, used by
/// `promptcad-core rpc`, the CLI and all tests. The Tauri desktop shell's *release* profile
/// sets `panic = "abort"` (`src-tauri/Cargo.toml`); in that build a panic aborts the whole
/// process before this guard runs, so a latent core panic kills the desktop app instead of
/// becoming an `internal` response. (The shell's debug build unwinds, so the guard works
/// there.)
pub fn handle(request: CoreRequest) -> CoreResponse {
    // No-op under `panic = "abort"` (the Tauri release build), see above.
    match panic::catch_unwind(AssertUnwindSafe(|| dispatch(request))) {
        Ok(Ok(result)) => CoreResponse::Ok { result },
        Ok(Err(error)) => CoreResponse::Error(error),
        Err(payload) => CoreResponse::Error(internal(payload)),
    }
}

/// Handles one JSON request and returns the JSON response (compact, single line, no trailing
/// newline). A request over [`MAX_REQUEST_BYTES`] yields `too_large`; a request that does not
/// deserialize into [`CoreRequest`] yields `bad_request`; a response over
/// [`MAX_RESPONSE_BYTES`] is replaced by `response_too_large`. Never panics (in an unwinding
/// build; see [`handle`] for `panic = "abort"`).
pub fn handle_json(input: &str) -> String {
    handle_json_limited(input, MAX_RESPONSE_BYTES)
}

/// [`handle_json`] with an explicit response limit (see [`CoreResponse::to_json_limited`]);
/// for tests of the response cap, including host parity tests.
#[doc(hidden)]
pub fn handle_json_limited(input: &str, response_limit: usize) -> String {
    // No-op under `panic = "abort"` (the Tauri release build), see `handle`.
    let response = panic::catch_unwind(AssertUnwindSafe(|| {
        if input.len() > MAX_REQUEST_BYTES {
            return CoreResponse::Error(too_large(input.len()));
        }
        match serde_json::from_str::<CoreRequest>(input) {
            Ok(request) => handle(request),
            Err(e) => CoreResponse::Error(bad_request(&e.to_string())),
        }
    }))
    .unwrap_or_else(|payload| CoreResponse::Error(internal(payload)));
    response.to_json_limited(response_limit)
}

/// The `bad_request` error for an unparsable request (used by hosts that reject the request
/// before calling [`handle`], e.g. non-UTF-8 input).
pub fn bad_request(detail: &str) -> CoreError {
    CoreError::new(
        ErrorCode::BadRequest,
        format!("invalid request: {detail}"),
        None,
    )
}

/// The `response_too_large` error for a response limit of `limit` bytes.
pub fn response_too_large(limit: usize) -> CoreError {
    CoreError::new(
        ErrorCode::ResponseTooLarge,
        format!("the response exceeds the limit of {limit} bytes and was discarded"),
        None,
    )
}

/// The `too_large` error for an input of `len` bytes.
pub fn too_large(len: usize) -> CoreError {
    CoreError::new(
        ErrorCode::TooLarge,
        format!("request is {len} bytes; the limit is {MAX_REQUEST_BYTES} bytes"),
        None,
    )
}

fn internal(payload: Box<dyn std::any::Any + Send>) -> CoreError {
    let detail: Cow<'_, str> = if let Some(s) = payload.downcast_ref::<&str>() {
        Cow::Borrowed(s)
    } else if let Some(s) = payload.downcast_ref::<String>() {
        Cow::Borrowed(s.as_str())
    } else {
        Cow::Borrowed("unknown panic")
    };
    CoreError::new(
        ErrorCode::Internal,
        format!("internal error: {detail}"),
        None,
    )
}

fn dispatch(request: CoreRequest) -> Result<CoreResult, CoreError> {
    Ok(match request {
        CoreRequest::Validate {
            document,
            completed,
        } => {
            let (doc, input_version) = parse_any_document(document)?;
            let validation = validate(
                &doc,
                &ValidateOptions {
                    completed,
                    submitter: None,
                },
            );
            CoreResult::Validate(Box::new(ValidateResult {
                input_version,
                report: validation.report,
                resolved: validation.resolved,
                document: doc,
            }))
        }
        CoreRequest::Solve { document } => {
            let (doc, input_version) = parse_any_document(document)?;
            CoreResult::Solve(Box::new(SolveResult {
                input_version,
                resolved: solve(&doc),
            }))
        }
        CoreRequest::Normalize { document } => {
            let (mut doc, input_version) = parse_any_document(document)?;
            let issues = recompute_bindings(&mut doc);
            doc.root = compute_root(&doc);
            // Recomputed values can break cross-field shape rules (e.g. a bound enclosure
            // height below its wall thickness); never return a schema-invalid document.
            let violations = doc.check_schema();
            if !violations.is_empty() {
                let mut error = CoreError::schema(DOC, violations);
                error.message = format!("after recomputing bindings: {}", error.message);
                return Err(error);
            }
            CoreResult::Normalize(Box::new(NormalizeResult {
                input_version,
                document: doc,
                issues,
            }))
        }
        CoreRequest::Upgrade { document } => {
            let doc1 = parse_document_v1(document)?;
            CoreResult::Upgrade(Box::new(UpgradeResult {
                document: upgrade_1_to_2(&doc1),
            }))
        }
        CoreRequest::Downgrade { document } => {
            let doc = parse_document_v2(document)?;
            let doc1 = downgrade_2_to_1(&doc);
            CoreResult::Downgrade(Box::new(DowngradeResult {
                expressible: doc1.is_some(),
                document: doc1,
            }))
        }
        CoreRequest::ApplyPatch {
            document,
            patch,
            origin,
            timestamp,
            log_seq,
        } => {
            let doc = parse_document_v2(document)?;
            let patch = parse_patch(patch)?;
            CoreResult::ApplyPatch(Box::new(run_apply_patch(
                &doc, &patch, origin, timestamp, log_seq,
            )))
        }
        CoreRequest::Describe { document } => {
            let (doc, input_version) = parse_any_document(document)?;
            CoreResult::Describe(Box::new(describe(&doc, input_version)?))
        }
        CoreRequest::Version {} => CoreResult::Version(VersionResult {
            crate_version: env!("CARGO_PKG_VERSION").to_owned(),
            api_version: API_VERSION,
            schema_versions: vec![DocumentVersion::V1_0, DocumentVersion::V2_0],
        }),
    })
}

fn run_apply_patch(
    doc: &CadDocument2,
    patch: &Patch,
    origin: PatchOrigin,
    timestamp: Option<String>,
    log_seq: u64,
) -> ApplyPatchResult {
    let outcome = apply_patch(doc, patch, origin);
    let mut log_entry = OpLogEntry::for_patch(doc, patch, origin, &outcome, timestamp);
    log_entry.seq = log_seq;
    let log_line = serde_json::to_string(&log_entry).unwrap_or_default();
    let outcome = match outcome {
        PatchOutcome::Accepted(a) => {
            let a = *a;
            PatchOutcomeView::Accepted(Box::new(AcceptedPatchView {
                document: a.document,
                report: a.validation.report,
                resolved: a.validation.resolved,
                revision_before: a.revision_before,
                revision_after: a.revision_after,
                patch_id: a.patch_id,
                created: a.created,
                patch_sha256: a.patch_sha256,
                doc_sha256_before: a.doc_sha256_before,
                doc_sha256_after: a.doc_sha256_after,
                log_entry,
                log_line,
            }))
        }
        PatchOutcome::Rejected(r) => PatchOutcomeView::Rejected(Box::new(RejectedPatchView {
            reason: r.reason,
            issues: r.issues,
            patch_sha256: patch_sha256(patch),
            doc_sha256_before: canonical_sha256(doc),
            log_entry,
            log_line,
        })),
    };
    ApplyPatchResult { outcome }
}

// =============================================================================================
// describe
// =============================================================================================

fn field_view(doc: &CadDocument2, id: &Id, name: &str, value: Option<f64>) -> FieldView {
    let binding = Id::new(name)
        .ok()
        .and_then(|f| doc.binding(&FieldRef::from_parts(id, &f)));
    FieldView {
        name: name.to_owned(),
        value,
        bound: binding.is_some(),
        expr: binding.map(|b| b.expr.clone()),
    }
}

fn component_view(doc: &CadDocument2, c: &Component) -> ComponentView {
    let shape = c.shape_kind();
    let fields = shape
        .numeric_fields()
        .iter()
        .map(|f| field_view(doc, &c.id, f, component_field(c, f).flatten()))
        .collect();
    let any_port = c.id.port(&c.id);
    let ports = component_port_table(doc, c, &any_port)
        .map(|table| {
            table
                .iter()
                .map(|(name, t)| PortView {
                    name: (*name).to_owned(),
                    port_type: *t,
                })
                .collect()
        })
        .unwrap_or_default();
    let part = doc
        .parts
        .iter()
        .find(|p| p.body.iter().any(|op| op.component == c.id))
        .map(|p| p.id.clone());
    ComponentView {
        id: c.id.clone(),
        kind: c.kind(),
        role: c.role(),
        shape: shape.into(),
        independent: shape.is_independent(),
        frame_port: shape.frame_port().map(str::to_owned),
        label: c.label.clone(),
        provenance: c.provenance.clone(),
        part,
        refs: c
            .geometry
            .shape_refs()
            .into_iter()
            .map(|(field, id)| ShapeRef {
                field: field.to_owned(),
                id: id.clone(),
            })
            .collect(),
        fields,
        ports,
    }
}

fn constraint_view(doc: &CadDocument2, k: &Constraint) -> ConstraintView {
    ConstraintView {
        id: k.id.clone(),
        kind: k.kind,
        a: k.a.clone(),
        b: k.b.clone(),
        fields: k
            .kind
            .numeric_fields()
            .iter()
            .map(|f| field_view(doc, &k.id, f, constraint_field(k, f).flatten()))
            .collect(),
    }
}

fn parent_of(doc: &CadDocument2, id: &Id) -> Option<Id> {
    doc.assemblies
        .iter()
        .find(|a| a.children.contains(id))
        .map(|a| a.id.clone())
}

/// Builds the `describe` tree in O(document size): every part / assembly is expanded at its
/// first occurrence only (later ones are `repeated` leaves), and the node count is capped.
/// Recursion depth is bounded by the number of assemblies (each is expanded at most once).
struct TreeBuilder<'a> {
    doc: &'a CadDocument2,
    seen: BTreeSet<&'a str>,
    nodes: usize,
}

impl<'a> TreeBuilder<'a> {
    fn node(&mut self, id: &'a Id) -> Result<TreeNode, CoreError> {
        self.nodes += 1;
        if self.nodes > MAX_TREE_NODES {
            return Err(CoreError::new(
                ErrorCode::Internal,
                format!("internal error: the assembly tree exceeds {MAX_TREE_NODES} nodes"),
                Some(DOC),
            ));
        }
        let doc = self.doc;
        Ok(if let Some(a) = doc.assembly(id.as_str()) {
            let repeated = !self.seen.insert(id.as_str());
            let children = if repeated {
                Vec::new()
            } else {
                a.children
                    .iter()
                    .map(|child| self.node(child))
                    .collect::<Result<_, _>>()?
            };
            TreeNode {
                id: id.clone(),
                node: TreeNodeKind::Assembly,
                name: Some(a.name.clone()),
                repeated,
                children,
            }
        } else if let Some(p) = doc.part(id.as_str()) {
            TreeNode {
                id: id.clone(),
                node: TreeNodeKind::Part,
                name: Some(p.name.clone()),
                repeated: !self.seen.insert(id.as_str()),
                children: Vec::new(),
            }
        } else {
            TreeNode {
                id: id.clone(),
                node: TreeNodeKind::Missing,
                name: None,
                repeated: false,
                children: Vec::new(),
            }
        })
    }
}

fn describe(
    doc: &CadDocument2,
    input_version: DocumentVersion,
) -> Result<DescribeResult, CoreError> {
    let is_child = |id: &Id| doc.assemblies.iter().any(|a| a.children.contains(id));
    let mut builder = TreeBuilder {
        doc,
        seen: BTreeSet::new(),
        nodes: 0,
    };
    let tree = doc
        .assemblies
        .iter()
        .map(|a| &a.id)
        .chain(doc.parts.iter().map(|p| &p.id))
        .filter(|id| !is_child(id))
        .map(|id| builder.node(id))
        .collect::<Result<_, _>>()?;
    Ok(DescribeResult {
        input_version,
        name: doc.name.clone(),
        revision: doc.revision,
        root: doc.root.clone(),
        computed_root: compute_root(doc),
        parameters: doc.parameters.clone(),
        components: doc
            .components
            .iter()
            .map(|c| component_view(doc, c))
            .collect(),
        constraints: doc
            .constraints
            .iter()
            .map(|k| constraint_view(doc, k))
            .collect(),
        parts: doc
            .parts
            .iter()
            .map(|p| PartView {
                id: p.id.clone(),
                name: p.name.clone(),
                material: p.material,
                quantity: p.quantity,
                source: p.source,
                body: p.body.clone(),
                parent: parent_of(doc, &p.id),
            })
            .collect(),
        assemblies: doc
            .assemblies
            .iter()
            .map(|a| AssemblyView {
                id: a.id.clone(),
                name: a.name.clone(),
                children: a.children.clone(),
                parent: parent_of(doc, &a.id),
            })
            .collect(),
        tree,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shape_names_match_wire_names() {
        for s in ShapeKind::ALL {
            let name = serde_json::to_value(ShapeName::from(s)).unwrap();
            assert_eq!(name, s.shape_name(), "{s:?}");
        }
    }

    #[test]
    fn error_codes_match_wire_names() {
        for c in [
            ErrorCode::BadRequest,
            ErrorCode::TooLarge,
            ErrorCode::UnsupportedSchemaVersion,
            ErrorCode::WrongSchemaVersion,
            ErrorCode::InvalidDocument,
            ErrorCode::SchemaViolation,
            ErrorCode::InvalidPatch,
            ErrorCode::ResponseTooLarge,
            ErrorCode::Internal,
        ] {
            assert_eq!(serde_json::to_value(c).unwrap(), c.as_str());
        }
    }

    #[test]
    fn internal_fallback_parses() {
        let r: CoreResponse = serde_json::from_str(INTERNAL_FALLBACK).unwrap();
        assert_eq!(r.error().unwrap().code, ErrorCode::Internal);
    }

    #[test]
    fn version_request_round_trips() {
        let out = handle_json(r#"{"op":"version"}"#);
        let r: CoreResponse = serde_json::from_str(&out).unwrap();
        match r.result() {
            Some(CoreResult::Version(v)) => assert_eq!(v.api_version, API_VERSION),
            other => panic!("unexpected {other:?}"),
        }
    }
}
