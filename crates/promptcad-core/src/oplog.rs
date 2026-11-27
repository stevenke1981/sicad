//! Append-only op log (`oplog.jsonl`, ADR 0005 item 5; spec §8.6 step 7) and a [`Session`]
//! that owns the current document plus its log.
//!
//! Every submitted patch is logged, accepted or rejected, with who submitted it, the patch as
//! submitted and its `patch_sha256`, the revisions before/after, the validation outcome (issue
//! codes and severities), document hashes before/after, and — for accepted patches — the
//! reverse delta that undo applies. The library never reads a clock: timestamps are optional,
//! caller-supplied strings.
//!
//! **Undo semantics.** Undo is itself a new log entry (`kind: "undo"`, `undoes: <seq>`). It
//! restores the pre-image of the most recent not-yet-undone accepted patch exactly, *including
//! its `revision`* (so the canonical JSON is byte-identical to the pre-patch document, ADR 0005
//! acceptance criterion). Revision numbers can therefore repeat across the log; log entries
//! are identified by `seq`, and `doc_sha256_*` pin the exact states. Repeated undos walk back
//! through the accepted patches (stack discipline); a patch accepted after an undo is pushed
//! on the same stack. There is no redo (re-submit the patch instead).
//!
//! **JSON Lines.** [`OpLog::to_jsonl`] writes one compact JSON object per entry, keys in
//! struct order, each line terminated by `\n`; output is a pure function of the entries.

use std::fmt;

use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};

use crate::canonical::canonical_sha256;
use crate::model::issue::{Severity, ValidationIssue};
use crate::model::patch::Patch;
use crate::model::v2::{CadDocument2, SchemaVersion2};
use crate::patch_engine::{
    apply_patch, patch_sha256, PatchOrigin, PatchOutcome, RejectReason, Revert,
};

/// What an entry records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    /// A submitted patch (accepted or rejected).
    Patch,
    /// An undo of an earlier accepted patch.
    Undo,
}

/// `{code, severity}` of one validation issue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueSummary {
    /// Issue code.
    pub code: String,
    /// Severity.
    pub severity: Severity,
}

impl From<&ValidationIssue> for IssueSummary {
    fn from(i: &ValidationIssue) -> Self {
        IssueSummary {
            code: i.code.clone(),
            severity: i.severity,
        }
    }
}

fn submitted_patch_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "type": ["object", "null"],
        "additionalProperties": true,
        "description": "The patch as submitted: structurally a Patch (schemas/cad2/patch.schema.json) that deserialized, but not necessarily within its range / length constraints (a `schema_invalid` rejection is logged as submitted); null for undo entries."
    })
}

/// One line of `oplog.jsonl`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpLogEntry {
    /// 0-based position in the log.
    pub seq: u64,
    /// Document schema version the entry was produced under (`"2.0"`).
    pub schema_version: SchemaVersion2,
    /// `patch` or `undo`.
    pub kind: EntryKind,
    /// Who: `"human"` or the agent role.
    pub origin: PatchOrigin,
    /// Caller-supplied timestamp (the library never reads a clock).
    pub timestamp: Option<String>,
    /// The patch as submitted (`null` for undo entries).
    ///
    /// Its JSON Schema is a plain object: a patch rejected as `schema_invalid` is logged as
    /// submitted and need not satisfy `patch.schema.json`'s range / length constraints.
    #[schemars(schema_with = "submitted_patch_schema")]
    pub patch: Option<Patch>,
    /// SHA-256 of the canonical patch JSON (`null` for undo entries).
    pub patch_sha256: Option<String>,
    /// For undo entries: `seq` of the undone patch entry.
    pub undoes: Option<u64>,
    /// Document revision when the entry was made (for patches: the revision the patch was
    /// checked against, which differs from `patch.base_revision` exactly when it was
    /// rejected as `stale_revision`).
    pub base_revision: u64,
    /// Revision after the entry; `null` when rejected.
    pub revision_after: Option<u64>,
    /// Whether the document changed.
    pub accepted: bool,
    /// Reject reason (`null` when accepted).
    pub reject: Option<RejectReason>,
    /// Issues of the outcome: the full validation report when accepted, the attached P-class
    /// issues when rejected.
    pub issues: Vec<IssueSummary>,
    /// SHA-256 of the canonical document before the entry.
    pub doc_sha256_before: String,
    /// SHA-256 of the canonical document after the entry (`null` when rejected).
    pub doc_sha256_after: Option<String>,
    /// Reverse delta of an accepted patch (what undo applies); `null` otherwise.
    pub revert: Option<Revert>,
}

impl OpLogEntry {
    /// The reject code string, if rejected.
    pub fn reject_code(&self) -> Option<&'static str> {
        self.reject.as_ref().map(|r| r.code.as_str())
    }

    /// Builds the entry for `outcome` of `patch` submitted by `origin` against `before`.
    /// `seq` is assigned by [`OpLog::append`].
    pub fn for_patch(
        before: &CadDocument2,
        patch: &Patch,
        origin: PatchOrigin,
        outcome: &PatchOutcome,
        timestamp: Option<String>,
    ) -> Self {
        let (accepted, sha_before, sha_after, revision_after, revert, reject, sha) = match outcome {
            PatchOutcome::Accepted(a) => (
                true,
                a.doc_sha256_before.clone(),
                Some(a.doc_sha256_after.clone()),
                Some(a.revision_after),
                Some(a.revert.clone()),
                None,
                a.patch_sha256.clone(),
            ),
            PatchOutcome::Rejected(r) => (
                false,
                canonical_sha256(before),
                None,
                None,
                None,
                Some(r.reason.clone()),
                patch_sha256(patch),
            ),
        };
        OpLogEntry {
            seq: 0,
            schema_version: SchemaVersion2::V2_0,
            kind: EntryKind::Patch,
            origin,
            timestamp,
            patch: Some(patch.clone()),
            patch_sha256: Some(sha),
            undoes: None,
            base_revision: before.revision,
            revision_after,
            accepted,
            reject,
            issues: outcome.issues().iter().map(IssueSummary::from).collect(),
            doc_sha256_before: sha_before,
            doc_sha256_after: sha_after,
            revert,
        }
    }
}

/// Error reading or extending an op log.
#[derive(Debug)]
pub enum OpLogError {
    /// A JSONL line failed to parse (1-based line number).
    Parse {
        /// 1-based line number.
        line: usize,
        /// Parser error.
        error: serde_json::Error,
    },
    /// `seq` values must be 0, 1, 2, … in order.
    Sequence {
        /// Expected seq.
        expected: u64,
        /// Found seq.
        found: u64,
    },
}

impl fmt::Display for OpLogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpLogError::Parse { line, error } => write!(f, "oplog line {line}: {error}"),
            OpLogError::Sequence { expected, found } => {
                write!(
                    f,
                    "oplog seq out of order: expected {expected}, found {found}"
                )
            }
        }
    }
}

impl std::error::Error for OpLogError {}

/// Append-only sequence of [`OpLogEntry`]s.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OpLog {
    entries: Vec<OpLogEntry>,
}

impl OpLog {
    /// An empty log.
    pub fn new() -> Self {
        Self::default()
    }

    /// All entries in order.
    pub fn entries(&self) -> &[OpLogEntry] {
        &self.entries
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` when empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The last entry.
    pub fn last(&self) -> Option<&OpLogEntry> {
        self.entries.last()
    }

    /// Entry by `seq`.
    pub fn get(&self, seq: u64) -> Option<&OpLogEntry> {
        usize::try_from(seq).ok().and_then(|i| self.entries.get(i))
    }

    /// Appends `entry`, assigning the next `seq`; returns it.
    pub fn append(&mut self, mut entry: OpLogEntry) -> u64 {
        let seq = self.entries.len() as u64;
        entry.seq = seq;
        self.entries.push(entry);
        seq
    }

    /// JSON Lines: one compact JSON object per entry, each followed by `\n`.
    pub fn to_jsonl(&self) -> String {
        let mut out = String::new();
        for e in &self.entries {
            out.push_str(&serde_json::to_string(e).expect("log entries always serialize"));
            out.push('\n');
        }
        out
    }

    /// Parses JSON Lines (blank lines and a trailing `\r` are tolerated); `seq` must count up
    /// from 0.
    pub fn from_jsonl(text: &str) -> Result<Self, OpLogError> {
        let mut entries = Vec::new();
        for (n, line) in text.split('\n').enumerate() {
            let line = line.strip_suffix('\r').unwrap_or(line);
            if line.trim().is_empty() {
                continue;
            }
            let entry: OpLogEntry = serde_json::from_str(line)
                .map_err(|error| OpLogError::Parse { line: n + 1, error })?;
            let expected = entries.len() as u64;
            if entry.seq != expected {
                return Err(OpLogError::Sequence {
                    expected,
                    found: entry.seq,
                });
            }
            entries.push(entry);
        }
        Ok(OpLog { entries })
    }
}

/// Why an undo was not possible.
#[derive(Debug)]
pub enum UndoError {
    /// No accepted patch left to undo.
    NothingToUndo,
    /// The current document is not the post-image recorded for the patch being undone.
    DocumentMismatch {
        /// Seq of the patch entry.
        seq: u64,
    },
    /// The patch entry carries no reverse delta.
    MissingRevert {
        /// Seq of the patch entry.
        seq: u64,
    },
    /// The reverse delta did not produce a valid document.
    Revert(crate::patch_engine::RevertError),
}

impl fmt::Display for UndoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UndoError::NothingToUndo => write!(f, "nothing to undo"),
            UndoError::DocumentMismatch { seq } => write!(
                f,
                "the current document is not the result of oplog entry {seq}"
            ),
            UndoError::MissingRevert { seq } => {
                write!(f, "oplog entry {seq} has no reverse delta")
            }
            UndoError::Revert(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for UndoError {}

/// Why a replay did not reproduce the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayError {
    /// A patch entry has no patch.
    MissingPatch {
        /// Entry seq.
        seq: u64,
    },
    /// The recorded `patch_sha256` does not match the recorded patch.
    PatchHash {
        /// Entry seq.
        seq: u64,
    },
    /// The document hash or revision before the entry differs from the replayed state.
    StateBefore {
        /// Entry seq.
        seq: u64,
    },
    /// Re-applying the patch gave a different accept/reject decision or reject code.
    Outcome {
        /// Entry seq.
        seq: u64,
        /// What was recorded.
        recorded: String,
        /// What replay produced.
        replayed: String,
    },
    /// The resulting revision or document hash differs from the recorded one.
    StateAfter {
        /// Entry seq.
        seq: u64,
    },
    /// An undo entry does not undo the most recent not-yet-undone accepted patch.
    Undo {
        /// Entry seq.
        seq: u64,
    },
}

impl fmt::Display for ReplayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReplayError::MissingPatch { seq } => {
                write!(f, "entry {seq}: patch entry without patch")
            }
            ReplayError::PatchHash { seq } => write!(f, "entry {seq}: patch_sha256 mismatch"),
            ReplayError::StateBefore { seq } => {
                write!(f, "entry {seq}: document before the entry differs")
            }
            ReplayError::Outcome {
                seq,
                recorded,
                replayed,
            } => write!(
                f,
                "entry {seq}: recorded {recorded}, replay gave {replayed}"
            ),
            ReplayError::StateAfter { seq } => {
                write!(f, "entry {seq}: document after the entry differs")
            }
            ReplayError::Undo { seq } => write!(f, "entry {seq}: undo out of order"),
        }
    }
}

impl std::error::Error for ReplayError {}

/// Coordinator-side state: the current document, its op log and the undo stack.
#[derive(Debug, Clone)]
pub struct Session {
    document: CadDocument2,
    log: OpLog,
    /// `(seq, pre-image)` of accepted, not-yet-undone patch entries.
    undo_stack: Vec<(u64, CadDocument2)>,
    last: Option<PatchOutcome>,
}

impl Session {
    /// A session starting from `document` with an empty log.
    pub fn new(document: CadDocument2) -> Self {
        Session {
            document,
            log: OpLog::new(),
            undo_stack: Vec::new(),
            last: None,
        }
    }

    /// Rebuilds a session by replaying `log` from `initial` (see [`replay`]).
    pub fn resume(initial: CadDocument2, log: OpLog) -> Result<Self, ReplayError> {
        let mut s = Session::new(initial);
        for e in log.entries {
            s.replay_entry(e)?;
        }
        Ok(s)
    }

    /// The current document.
    pub fn document(&self) -> &CadDocument2 {
        &self.document
    }

    /// The op log.
    pub fn log(&self) -> &OpLog {
        &self.log
    }

    /// The outcome of the most recent [`Session::submit`].
    pub fn last_outcome(&self) -> Option<&PatchOutcome> {
        self.last.as_ref()
    }

    /// Number of accepted patches that can currently be undone.
    pub fn undo_depth(&self) -> usize {
        self.undo_stack.len()
    }

    /// Consumes the session.
    pub fn into_parts(self) -> (CadDocument2, OpLog) {
        (self.document, self.log)
    }

    /// Applies `patch`, logs the outcome (accepted or rejected) and, if accepted, makes the
    /// new document current.
    pub fn submit(&mut self, patch: &Patch, origin: PatchOrigin) -> &PatchOutcome {
        self.submit_at(patch, origin, None)
    }

    /// [`Session::submit`] with a caller-supplied timestamp for the log entry.
    pub fn submit_at(
        &mut self,
        patch: &Patch,
        origin: PatchOrigin,
        timestamp: Option<String>,
    ) -> &PatchOutcome {
        let outcome = apply_patch(&self.document, patch, origin);
        let entry = OpLogEntry::for_patch(&self.document, patch, origin, &outcome, timestamp);
        let seq = self.log.append(entry);
        if let PatchOutcome::Accepted(a) = &outcome {
            let before = std::mem::replace(&mut self.document, a.document.clone());
            self.undo_stack.push((seq, before));
        }
        self.last.insert(outcome)
    }

    /// Undoes the most recent not-yet-undone accepted patch, as a human request.
    pub fn undo(&mut self) -> Result<&OpLogEntry, UndoError> {
        self.undo_by(PatchOrigin::Human, None)
    }

    /// Undoes the most recent not-yet-undone accepted patch on behalf of `origin`, using the
    /// reverse delta recorded in the log, and logs an `undo` entry.
    pub fn undo_by(
        &mut self,
        origin: PatchOrigin,
        timestamp: Option<String>,
    ) -> Result<&OpLogEntry, UndoError> {
        let &(seq, _) = self.undo_stack.last().ok_or(UndoError::NothingToUndo)?;
        let entry = self
            .log
            .get(seq)
            .ok_or(UndoError::DocumentMismatch { seq })?;
        let sha_before = canonical_sha256(&self.document);
        if entry.doc_sha256_after.as_deref() != Some(sha_before.as_str()) {
            return Err(UndoError::DocumentMismatch { seq });
        }
        let revert = entry
            .revert
            .as_ref()
            .ok_or(UndoError::MissingRevert { seq })?;
        let restored = revert.apply(&self.document).map_err(UndoError::Revert)?;
        let sha_after = canonical_sha256(&restored);
        if sha_after != entry.doc_sha256_before {
            return Err(UndoError::DocumentMismatch { seq });
        }
        let undo = OpLogEntry {
            seq: 0,
            schema_version: SchemaVersion2::V2_0,
            kind: EntryKind::Undo,
            origin,
            timestamp,
            patch: None,
            patch_sha256: None,
            undoes: Some(seq),
            base_revision: self.document.revision,
            revision_after: Some(restored.revision),
            accepted: true,
            reject: None,
            issues: Vec::new(),
            doc_sha256_before: sha_before,
            doc_sha256_after: Some(sha_after),
            revert: None,
        };
        self.undo_stack.pop();
        self.document = restored;
        let new_seq = self.log.append(undo);
        self.log
            .get(new_seq)
            .ok_or(UndoError::DocumentMismatch { seq: new_seq })
    }

    fn replay_entry(&mut self, e: OpLogEntry) -> Result<(), ReplayError> {
        let seq = e.seq;
        let sha_before = canonical_sha256(&self.document);
        if e.doc_sha256_before != sha_before || e.base_revision != self.document.revision {
            return Err(ReplayError::StateBefore { seq });
        }
        match e.kind {
            EntryKind::Patch => {
                let patch = e.patch.as_ref().ok_or(ReplayError::MissingPatch { seq })?;
                if e.patch_sha256.as_deref() != Some(patch_sha256(patch).as_str()) {
                    return Err(ReplayError::PatchHash { seq });
                }
                let outcome = apply_patch(&self.document, patch, e.origin);
                let describe = |accepted: bool, code: Option<&str>| match (accepted, code) {
                    (true, _) => "accepted".to_owned(),
                    (false, c) => format!("rejected ({})", c.unwrap_or("?")),
                };
                let replayed = describe(
                    outcome.is_accepted(),
                    outcome.reason().map(|r| r.code.as_str()),
                );
                let recorded = describe(e.accepted, e.reject_code());
                if replayed != recorded {
                    return Err(ReplayError::Outcome {
                        seq,
                        recorded,
                        replayed,
                    });
                }
                if let PatchOutcome::Accepted(a) = &outcome {
                    if e.revision_after != Some(a.revision_after)
                        || e.doc_sha256_after.as_deref() != Some(a.doc_sha256_after.as_str())
                    {
                        return Err(ReplayError::StateAfter { seq });
                    }
                }
                let new_seq = self.log.append(e);
                debug_assert_eq!(new_seq, seq);
                if let PatchOutcome::Accepted(a) = &outcome {
                    let before = std::mem::replace(&mut self.document, a.document.clone());
                    self.undo_stack.push((seq, before));
                }
                self.last = Some(outcome);
            }
            EntryKind::Undo => {
                let Some((top, pre)) = self.undo_stack.pop() else {
                    return Err(ReplayError::Undo { seq });
                };
                if e.undoes != Some(top) {
                    return Err(ReplayError::Undo { seq });
                }
                if e.revision_after != Some(pre.revision)
                    || e.doc_sha256_after.as_deref() != Some(canonical_sha256(&pre).as_str())
                {
                    return Err(ReplayError::StateAfter { seq });
                }
                self.document = pre;
                self.log.append(e);
            }
        }
        Ok(())
    }
}

/// Re-applies every entry of `log` to `initial` — accepted and rejected patches through
/// [`apply_patch`], undos by restoring the replayed pre-image — and verifies that each entry
/// reproduces the recorded decision, reject code, revisions and document hashes. Returns the
/// final document.
pub fn replay(initial: &CadDocument2, log: &OpLog) -> Result<CadDocument2, ReplayError> {
    Session::resume(initial.clone(), log.clone()).map(|s| s.document)
}
