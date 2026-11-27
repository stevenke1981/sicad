//! Atomic patch application (spec §8, ADR 0005 item 4).
//!
//! [`apply_patch`] is pure: it never mutates its input document. It runs the §8.6 steps on a
//! copy and returns either the accepted, normalized document (revision + 1) with its
//! validation report, or a structured [`RejectReason`]; an invalid patch therefore never
//! changes document state.
//!
//! Steps (§8.6):
//!
//! 1. Agent origins only: the origin role must equal `patch.agent` (`permission_denied`).
//!    [`Patch::check_schema`] and, for agents, the role's schema subset
//!    ([`role_allows_op`], §8.3) — `schema_invalid`.
//! 2. `base_revision == revision` — `stale_revision`.
//! 3. Agent origins only: §8.7 `annotation_only_removal`; then, per op and against the
//!    document state as it evolves through the ops, the §8.5 matrix ([`permits`] /
//!    [`op_class`]) — `permission_denied`. For every origin: `remove_assumption` text must
//!    exist at that point — `assumption_not_found`.
//! 4. Ops in order on the copy with the §8.2 semantics (`unknown_target`, `field_is_bound`,
//!    `duplicate_id`, `dangling_reference`, `unknown_field`, list caps → `schema_invalid`).
//!    Step-3 failures take precedence over step-4 failures regardless of op order.
//! 5. Normalize: recompute bindings, recompute `root` ([`validate::compute_root`]), stamp
//!    `provenance = {agent, "r<revision_after>"}` on components created by this patch, set
//!    `revision = revision + 1`. The normalized document must pass
//!    [`CadDocument2::check_schema`] (`schema_invalid`, unless a P-class binding issue explains
//!    the violation, in which case that issue's code is the reason).
//! 6. [`validate::validate`] with `completed = false` and `submitter` = the role: any P-class
//!    issue rejects (reason code = the first such issue's code; all P-class issues attached).
//!    D-class issues are returned in the accepted outcome. Standards re-check of added /
//!    modified metric clearance / tapped holes — `standards_mismatch`.
//!
//! # `resolves` and the repair unlock (interpretation of §8.5, stricter than the literal text)
//!
//! §8.5 lets `component_smith` use `set_parameter` only "in a repair patch". The literal text
//! could be read as "any non-empty `resolves`", which an agent can forge. Here (a) every
//! `resolves` entry, for every origin, must be a known §9.2 code (else `schema_invalid`), and
//! (b) for `component_smith` the unlock additionally requires that at least one `resolves` code
//! is present in the validation report of the pre-patch document (`completed = false`, no
//! submitter), else `permission_denied`. `space_planner` may use `set_parameter` regardless and
//! humans are not subject to (b). The report is computed lazily, only when the unlock is needed.
//!
//! Further: if `revision` is `u64::MAX` an otherwise acceptable patch is rejected with
//! `schema_invalid` (the counter cannot be incremented). `apply_patch` never panics.
//!
//! 7. The caller (see [`crate::oplog::Session`]) logs the outcome either way.

mod apply;
mod permissions;
mod revert;
mod standards;

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::bindings::recompute_bindings;
use crate::canonical::{canonical_sha256, to_canonical_string};
use crate::model::ids::Id;
use crate::model::issue::codes::{Code, RejectCode};
use crate::model::issue::{Role, ValidationIssue};
use crate::model::patch::{Patch, PatchOp};
use crate::model::v2::{CadDocument2, Provenance, ProvenanceAgent};
use crate::validate::{self, blocking_issues, ValidateOptions, Validation};

pub use permissions::{
    op_class, permits, role_allows_component, role_allows_op, role_ops, OpClass,
};
pub use revert::{Revert, RevertError};

// ---- reasons ---------------------------------------------------------------------------------

/// A patch reject reason code (§9.2 last paragraph): one of the eight [`RejectCode`]s or any
/// P-class issue [`Code`]. Serializes as its wire string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReasonCode {
    /// A non-issue reject reason.
    Reject(RejectCode),
    /// A P-class validation issue code.
    Issue(Code),
}

impl ReasonCode {
    /// The wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            ReasonCode::Reject(r) => r.as_str(),
            ReasonCode::Issue(c) => c.as_str(),
        }
    }
}

impl From<RejectCode> for ReasonCode {
    fn from(r: RejectCode) -> Self {
        ReasonCode::Reject(r)
    }
}

impl From<Code> for ReasonCode {
    fn from(c: Code) -> Self {
        ReasonCode::Issue(c)
    }
}

impl PartialEq<RejectCode> for ReasonCode {
    fn eq(&self, other: &RejectCode) -> bool {
        *self == ReasonCode::Reject(*other)
    }
}

impl PartialEq<Code> for ReasonCode {
    fn eq(&self, other: &Code) -> bool {
        *self == ReasonCode::Issue(*other)
    }
}

impl fmt::Display for ReasonCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ReasonCode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(r) = RejectCode::ALL.into_iter().find(|r| r.as_str() == s) {
            return Ok(ReasonCode::Reject(r));
        }
        match s.parse::<Code>() {
            Ok(c) if c.is_patch_blocking() => Ok(ReasonCode::Issue(c)),
            Ok(c) => Err(format!(
                "{c} is a design (D-class) code, not a reject reason"
            )),
            Err(e) => Err(e),
        }
    }
}

impl Serialize for ReasonCode {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ReasonCode {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl ReasonCode {
    /// Every reason code: the eight [`RejectCode`]s in spec order, then the P-class issue
    /// [`Code`]s in table order.
    pub fn all() -> Vec<ReasonCode> {
        RejectCode::ALL
            .into_iter()
            .map(ReasonCode::Reject)
            .chain(
                Code::ALL
                    .iter()
                    .copied()
                    .filter(|c| c.is_patch_blocking())
                    .map(ReasonCode::Issue),
            )
            .collect()
    }
}

impl JsonSchema for ReasonCode {
    fn schema_name() -> Cow<'static, str> {
        "ReasonCode".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        let names: Vec<&'static str> = ReasonCode::all()
            .into_iter()
            .map(ReasonCode::as_str)
            .collect();
        json_schema!({
            "description": "Patch reject reason (§9.2 last paragraph): a reject code or a P-class issue code.",
            "type": "string",
            "enum": names,
        })
    }
}

/// Structured reject diagnostics returned to the submitter and written to the op log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RejectReason {
    /// Reason code.
    pub code: ReasonCode,
    /// Index of the offending op, when one op is to blame.
    pub op_index: Option<usize>,
    /// Offending `id` / `id:field`, when known.
    pub target: Option<String>,
    /// Fixed-template message (ids, numbers and template text only; never free text such as
    /// labels, notes or assumptions — §9.1).
    pub message: String,
}

impl RejectReason {
    fn new(code: impl Into<ReasonCode>, message: impl Into<String>) -> Self {
        RejectReason {
            code: code.into(),
            op_index: None,
            target: None,
            message: message.into(),
        }
    }

    fn at(mut self, op_index: usize) -> Self {
        self.op_index = Some(op_index);
        self
    }

    fn with_target(mut self, target: impl Into<String>) -> Self {
        self.target = Some(target.into());
        self
    }
}

impl fmt::Display for RejectReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

// ---- origin ----------------------------------------------------------------------------------

/// Who submitted a patch, as determined by the coordinator from the request source (not from
/// the patch body).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PatchOrigin {
    /// An agent: §8.3 schema subset and §8.5 matrix enforced; must equal `patch.agent`.
    Agent(Role),
    /// A human edit: schema, revision and op semantics enforced, the role schema subset, the
    /// §8.5 matrix and §8.7 `annotation_only_removal` are not; `patch.agent` is ignored;
    /// provenance agent is `human`.
    Human,
}

impl PatchOrigin {
    /// The agent role, `None` for humans.
    pub fn role(self) -> Option<Role> {
        match self {
            PatchOrigin::Agent(r) => Some(r),
            PatchOrigin::Human => None,
        }
    }

    /// The wire string (`"human"` or the role).
    pub fn as_str(self) -> &'static str {
        match self {
            PatchOrigin::Agent(r) => r.as_str(),
            PatchOrigin::Human => "human",
        }
    }

    /// The provenance agent stamped on components this origin creates.
    pub fn provenance_agent(self) -> ProvenanceAgent {
        match self {
            PatchOrigin::Agent(Role::SpacePlanner) => ProvenanceAgent::SpacePlanner,
            PatchOrigin::Agent(Role::ComponentSmith) => ProvenanceAgent::ComponentSmith,
            PatchOrigin::Agent(Role::Joiner) => ProvenanceAgent::Joiner,
            PatchOrigin::Agent(Role::Assembler) => ProvenanceAgent::Assembler,
            PatchOrigin::Human => ProvenanceAgent::Human,
        }
    }
}

impl From<Role> for PatchOrigin {
    fn from(r: Role) -> Self {
        PatchOrigin::Agent(r)
    }
}

impl fmt::Display for PatchOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for PatchOrigin {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "human" {
            return Ok(PatchOrigin::Human);
        }
        Role::ALL
            .into_iter()
            .find(|r| r.as_str() == s)
            .map(PatchOrigin::Agent)
            .ok_or_else(|| format!("unknown patch origin {s:?}"))
    }
}

impl Serialize for PatchOrigin {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for PatchOrigin {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for PatchOrigin {
    fn schema_name() -> Cow<'static, str> {
        "PatchOrigin".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        let names: Vec<&'static str> = Role::ALL
            .into_iter()
            .map(Role::as_str)
            .chain(["human"])
            .collect();
        json_schema!({
            "description": "Who submitted a patch: an agent role or `human`.",
            "type": "string",
            "enum": names,
        })
    }
}

// ---- outcomes --------------------------------------------------------------------------------

/// An accepted patch: the new document plus everything the op log needs.
#[derive(Debug, Clone, PartialEq)]
pub struct AcceptedPatch {
    /// The normalized document at `revision_after`.
    pub document: CadDocument2,
    /// Validation of `document` (`completed = false`); contains only D-class issues.
    pub validation: Validation,
    /// Revision of the input document.
    pub revision_before: u64,
    /// `revision_before + 1`.
    pub revision_after: u64,
    /// `r<revision_after>`, the `provenance.patch_id` of components created by this patch.
    pub patch_id: String,
    /// Components created (and stamped) by this patch, in document order.
    pub created: Vec<Id>,
    /// SHA-256 of the canonical patch JSON (§1.4).
    pub patch_sha256: String,
    /// SHA-256 of the canonical JSON of the input document.
    pub doc_sha256_before: String,
    /// SHA-256 of the canonical JSON of `document`.
    pub doc_sha256_after: String,
    /// Reverse delta: `revert.apply(&document)` is byte-identical (canonically) to the input.
    pub revert: Revert,
}

/// A rejected patch. The input document is unchanged.
#[derive(Debug, Clone, PartialEq)]
pub struct RejectedPatch {
    /// Why.
    pub reason: RejectReason,
    /// The P-class issues behind a validation rejection (empty for other reasons).
    pub issues: Vec<ValidationIssue>,
}

/// Result of [`apply_patch`].
#[derive(Debug, Clone, PartialEq)]
pub enum PatchOutcome {
    /// Accepted (boxed: it carries a whole document and its validation).
    Accepted(Box<AcceptedPatch>),
    /// Rejected.
    Rejected(RejectedPatch),
}

impl PatchOutcome {
    /// `true` when accepted.
    pub fn is_accepted(&self) -> bool {
        matches!(self, PatchOutcome::Accepted(_))
    }

    /// The accepted patch.
    pub fn accepted(&self) -> Option<&AcceptedPatch> {
        match self {
            PatchOutcome::Accepted(a) => Some(a),
            PatchOutcome::Rejected(_) => None,
        }
    }

    /// The rejection.
    pub fn rejected(&self) -> Option<&RejectedPatch> {
        match self {
            PatchOutcome::Accepted(_) => None,
            PatchOutcome::Rejected(r) => Some(r),
        }
    }

    /// The reject reason.
    pub fn reason(&self) -> Option<&RejectReason> {
        self.rejected().map(|r| &r.reason)
    }

    /// The new document, if accepted.
    pub fn document(&self) -> Option<&CadDocument2> {
        self.accepted().map(|a| &a.document)
    }

    /// Accepted: all issues of the validation report (D-class); rejected: the attached
    /// P-class issues.
    pub fn issues(&self) -> &[ValidationIssue] {
        match self {
            PatchOutcome::Accepted(a) => &a.validation.report.issues,
            PatchOutcome::Rejected(r) => &r.issues,
        }
    }
}

// ---- apply -----------------------------------------------------------------------------------

fn rejected(reason: RejectReason) -> RejectedPatch {
    RejectedPatch {
        reason,
        issues: Vec::new(),
    }
}

/// `op_index` from a schema-violation path like `/ops/3/component/shape/diameter`.
fn op_index_of(path: &str) -> Option<usize> {
    path.strip_prefix("/ops/")?.split('/').next()?.parse().ok()
}

fn check_patch_schema(patch: &Patch, role: Option<Role>) -> Result<(), RejectReason> {
    let violations = patch.check_schema();
    if let Some(first) = violations.first() {
        let mut reason = RejectReason::new(
            RejectCode::SchemaInvalid,
            violations
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; "),
        );
        reason.op_index = op_index_of(&first.path);
        return Err(reason);
    }
    // §8.1: `resolves` lists validation issue codes; every entry must be a known §9.2 code
    // (applies to every origin). The message never echoes the offending text.
    if let Some(j) = patch
        .resolves
        .iter()
        .position(|c| c.parse::<Code>().is_err())
    {
        return Err(RejectReason::new(
            RejectCode::SchemaInvalid,
            format!("resolves[{j}] is not a known validation issue code (§9.2)"),
        ));
    }
    if let Some(role) = role {
        for (i, op) in patch.ops.iter().enumerate() {
            if role_allows_op(role, op) {
                continue;
            }
            let message = match op {
                PatchOp::AddComponent { component }
                    if role_ops(role).contains(&"add_component") =>
                {
                    format!(
                        "op {i} (add_component): {} {}{} is not in the {role} ComponentInput subset",
                        component.id,
                        component.kind().as_str(),
                        component
                            .role()
                            .map(|r| format!(
                                " role {}",
                                serde_json::to_value(r)
                                    .ok()
                                    .and_then(|v| v.as_str().map(str::to_owned))
                                    .unwrap_or_default()
                            ))
                            .unwrap_or_default(),
                    )
                }
                _ => format!(
                    "op {i} ({}) is not in the {role} patch schema",
                    op.op_name()
                ),
            };
            return Err(RejectReason::new(RejectCode::SchemaInvalid, message).at(i));
        }
    }
    Ok(())
}

/// Applies `patch` submitted by `origin` to `doc` (spec §8.6). Pure; see the module docs for
/// the exact steps and reason precedence.
pub fn apply_patch(doc: &CadDocument2, patch: &Patch, origin: PatchOrigin) -> PatchOutcome {
    match apply_inner(doc, patch, origin) {
        Ok(a) => PatchOutcome::Accepted(Box::new(a)),
        Err(r) => PatchOutcome::Rejected(r),
    }
}

fn apply_inner(
    doc: &CadDocument2,
    patch: &Patch,
    origin: PatchOrigin,
) -> Result<AcceptedPatch, RejectedPatch> {
    let role = origin.role();

    // 1. identity + schema
    if let Some(r) = role {
        if r != patch.agent {
            return Err(rejected(RejectReason::new(
                RejectCode::PermissionDenied,
                format!(
                    "patch.agent {} does not match the submitting role {r}",
                    patch.agent
                ),
            )));
        }
    }
    check_patch_schema(patch, role).map_err(rejected)?;

    // 2. revision
    if patch.base_revision != doc.revision {
        return Err(rejected(RejectReason::new(
            RejectCode::StaleRevision,
            format!(
                "base_revision {} does not match the current revision {}",
                patch.base_revision, doc.revision
            ),
        )));
    }

    // 3. §8.7 annotation-only removal (agents)
    if role.is_some() && !patch.has_design_op() {
        if let Some(i) = patch
            .ops
            .iter()
            .position(|op| matches!(op, PatchOp::RemoveAssumption { .. }))
        {
            return Err(rejected(
                RejectReason::new(
                    RejectCode::AnnotationOnlyRemoval,
                    format!(
                        "op {i} (remove_assumption): a patch that removes assumptions must contain at least one design op"
                    ),
                )
                .at(i),
            ));
        }
    }

    // 3 + 4 in one pass: permission / §8.7 checks see the state left by the earlier ops; the
    // first step-3 failure wins over any step-4 failure.
    let mut work = doc.clone();
    let mut created = apply::Created::new();
    let mut op_failure: Option<RejectReason> = None;
    // Codes of the pre-patch validation report; computed only when a repair unlock needs it.
    let mut pre_codes: Option<BTreeSet<String>> = None;
    for (i, op) in patch.ops.iter().enumerate() {
        if let Some(role) = role {
            if let Some(class) = op_class(&work, patch, op) {
                let unlocked = class != OpClass::SetParameterRepair
                    || permits(role, OpClass::SetParameter)
                    || {
                        let present = pre_codes.get_or_insert_with(|| {
                            validate::validate(
                                doc,
                                &ValidateOptions {
                                    completed: false,
                                    submitter: None,
                                },
                            )
                            .report
                            .issues
                            .iter()
                            .map(|i| i.code.clone())
                            .collect()
                        });
                        patch.resolves.iter().any(|c| present.contains(c.as_str()))
                    };
                if !unlocked {
                    let mut reason = RejectReason::new(
                        RejectCode::PermissionDenied,
                        format!(
                            "op {i} ({}): a {role} repair patch must resolve at least one issue present in the current validation report (§8.5)",
                            op.op_name()
                        ),
                    )
                    .at(i);
                    reason.target = op_target(op);
                    return Err(rejected(reason));
                }
                if !permits(role, class) {
                    let mut reason = RejectReason::new(
                        RejectCode::PermissionDenied,
                        format!(
                            "op {i} ({}): {role} may not perform {class:?} operations (§8.5)",
                            op.op_name()
                        ),
                    )
                    .at(i);
                    reason.target = op_target(op);
                    return Err(rejected(reason));
                }
            }
        }
        if let PatchOp::RemoveAssumption { text } = op {
            if apply::find_assumption(&work, text.as_str()).is_none() {
                return Err(rejected(apply::assumption_not_found(i)));
            }
        }
        if let Err(e) = apply::apply_op(&mut work, i, op, &mut created) {
            op_failure.get_or_insert(e);
        }
    }
    if let Some(e) = op_failure {
        return Err(rejected(e));
    }

    // 5. normalize
    let Some(revision_after) = doc.revision.checked_add(1) else {
        return Err(rejected(RejectReason::new(
            RejectCode::SchemaInvalid,
            format!(
                "revision {} cannot be incremented: the revision counter would overflow",
                doc.revision
            ),
        )));
    };
    let patch_id = format!("r{revision_after}");
    work.revision = revision_after;
    let binding_issues = recompute_bindings(&mut work);
    work.root = validate::compute_root(&work);
    let provenance = Provenance {
        agent: origin.provenance_agent(),
        patch_id: Some(patch_id.clone()),
    };
    let mut stamped = Vec::new();
    for c in &mut work.components {
        if created.contains(&c.id) {
            c.provenance = Some(provenance.clone());
            stamped.push(c.id.clone());
        }
    }
    let violations = work.check_schema();
    if !violations.is_empty() {
        let blocking_binding: Vec<ValidationIssue> = binding_issues
            .into_iter()
            .filter(|i| i.code().is_some_and(Code::is_patch_blocking))
            .map(|i| i.for_submitter(role))
            .collect();
        return Err(match blocking_binding.first() {
            Some(first) => issue_rejection(first, blocking_binding.clone()),
            None => rejected(RejectReason::new(
                RejectCode::SchemaInvalid,
                format!(
                    "the patched document violates the schema: {}",
                    violations
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("; ")
                ),
            )),
        });
    }

    // 6. validate
    let validation = validate::validate(
        &work,
        &ValidateOptions {
            completed: false,
            submitter: role,
        },
    );
    let blocking: Vec<ValidationIssue> = blocking_issues(&validation.report)
        .into_iter()
        .cloned()
        .collect();
    if let Some(first) = blocking.first() {
        return Err(issue_rejection(first, blocking.clone()));
    }
    check_standards(doc, &work).map_err(rejected)?;

    // 7. accept
    let doc_sha256_before = canonical_sha256(doc);
    let doc_sha256_after = canonical_sha256(&work);
    let revert = Revert::between(doc, &work);
    Ok(AcceptedPatch {
        validation,
        revision_before: doc.revision,
        revision_after,
        patch_id,
        created: stamped,
        patch_sha256: patch_sha256(patch),
        doc_sha256_before,
        doc_sha256_after,
        revert,
        document: work,
    })
}

fn issue_rejection(first: &ValidationIssue, issues: Vec<ValidationIssue>) -> RejectedPatch {
    // `schema_invalid` issues (e.g. the assembly depth cap, emitted by the validator's schema
    // layer) and any code string this crate does not know map to the `schema_invalid` reject
    // reason; the issue stays attached. Never panics.
    let code: ReasonCode = first
        .code()
        .map_or(RejectCode::SchemaInvalid.into(), ReasonCode::from);
    let mut reason = RejectReason::new(code, first.message.clone());
    reason.target = first.target.clone();
    RejectedPatch { reason, issues }
}

fn op_target(op: &PatchOp) -> Option<String> {
    match op {
        PatchOp::AddComponent { component } => Some(component.id.to_string()),
        PatchOp::RemoveComponent { id } | PatchOp::RemoveConstraint { id } => Some(id.to_string()),
        PatchOp::SetField { target, .. }
        | PatchOp::SetBinding { target, .. }
        | PatchOp::RemoveBinding { target } => Some(target.to_string()),
        PatchOp::AddConstraint { constraint } => Some(constraint.id.to_string()),
        PatchOp::SetParameter { parameter } => Some(parameter.name.to_string()),
        PatchOp::AddPart { part } => Some(part.id.to_string()),
        PatchOp::SetBody { part_id, .. } => Some(part_id.to_string()),
        PatchOp::AddAssembly { assembly } => Some(assembly.id.to_string()),
        PatchOp::AddAssumption { .. }
        | PatchOp::RemoveAssumption { .. }
        | PatchOp::AddNote { .. } => None,
    }
}

/// §8.6 step 6 standards re-check over hole Empties that are new or whose canonical form
/// changed (directly or through a binding).
fn check_standards(before: &CadDocument2, after: &CadDocument2) -> Result<(), RejectReason> {
    for c in &after.components {
        let Some(hole) = c.as_hole() else { continue };
        let changed = before
            .component(c.id.as_str())
            .is_none_or(|old| to_canonical_string(old) != to_canonical_string(c));
        if !changed {
            continue;
        }
        if let Some((kind, nominal, expected)) = standards::mismatch(hole) {
            let kind = serde_json::to_value(kind)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default();
            return Err(RejectReason::new(
                RejectCode::StandardsMismatch,
                format!(
                    "{}: M{nominal} {kind} hole diameter must be {expected} (got {})",
                    c.id, hole.diameter
                ),
            )
            .with_target(c.id.as_str()));
        }
    }
    Ok(())
}

/// `patch_sha256`: lower-case hex SHA-256 of the canonical patch JSON (§1.4).
pub fn patch_sha256(patch: &Patch) -> String {
    canonical_sha256(patch)
}

/// Undoes an accepted patch: applies its reverse delta to the post-patch document. The result
/// is canonically byte-identical to the pre-patch document, including its `revision`.
pub fn undo(after: &CadDocument2, revert: &Revert) -> Result<CadDocument2, RevertError> {
    revert.apply(after)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reason_codes_round_trip() {
        for r in RejectCode::ALL {
            let c = ReasonCode::from(r);
            assert_eq!(c.as_str().parse::<ReasonCode>(), Ok(c));
            assert_eq!(serde_json::to_value(c).unwrap(), r.as_str());
        }
        let c: ReasonCode = serde_json::from_str("\"binding_cycle\"").unwrap();
        assert_eq!(c, Code::BindingCycle);
        assert!("thin_wall".parse::<ReasonCode>().is_err());
        assert!("nope".parse::<ReasonCode>().is_err());
    }

    #[test]
    fn origins_round_trip() {
        for o in Role::ALL
            .into_iter()
            .map(PatchOrigin::Agent)
            .chain([PatchOrigin::Human])
        {
            let j = serde_json::to_string(&o).unwrap();
            assert_eq!(serde_json::from_str::<PatchOrigin>(&j).unwrap(), o);
        }
        assert_eq!(
            PatchOrigin::Human.provenance_agent(),
            ProvenanceAgent::Human
        );
        assert!("migration".parse::<PatchOrigin>().is_err());
    }

    #[test]
    fn op_index_from_path() {
        assert_eq!(op_index_of("/ops/3/component/shape"), Some(3));
        assert_eq!(op_index_of("/ops/12"), Some(12));
        assert_eq!(op_index_of("/resolves"), None);
    }
}
