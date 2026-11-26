//! Analytic validator (spec §9, ADR 0005): one [`ValidationReport`] with every §9.2 code the
//! analytic layer emits, plus the solver output.
//!
//! # Layers and gating
//!
//! [`validate`] runs these layers; the report lists their issues in this order:
//!
//! 0. **Schema.** [`CadDocument2::check_schema`] plus the assembly nesting-depth cap of §2
//!    (`≤ 4`). §9.2 has no code for schema violations, so each violation becomes an `error`
//!    issue whose `code` is the patch reject reason [`SCHEMA_INVALID`] (`"schema_invalid"`,
//!    §9.2 last paragraph; not a [`Code`](crate::model::Code), so `issue.code()` is `None`), `target` = the JSON
//!    pointer, `owner` = the submitter. A schema-invalid document is not a document the other
//!    layers can reason about (negative dimensions, missing `frame`, …): **validation stops
//!    here** and `resolved` is empty. Documents loaded with `from_json_str` can only hit the
//!    depth cap (it is not part of `check_schema`).
//! 1. **Structure and references** (P class, plus `orphan_component`), in this order:
//!    `duplicate_id` (N1, then N2); per component, its shape references
//!    (`dangling_reference`, `invalid_promotion`, `open_profile`); shape-graph
//!    `reference_cycle`; per constraint, its port checks (`dangling_reference`,
//!    `unknown_port`, `invalid_fixed_target`, `port_type_mismatch`); per part, its body
//!    (`body_first_not_add`, `dangling_reference`, `invalid_body_component`); body
//!    `multi_owner`; assembly children (`dangling_reference`, multi-parent `multi_owner`,
//!    `reference_cycle`); `root` (`dangling_reference`; `invalid_root` only with
//!    [`ValidateOptions::completed`], which also maps the `check_completed` rules to
//!    `invalid_root`); `orphan_component`. Finally `profile_not_axis_aligned`, which needs
//!    solved positions: it is computed after the solver (so not at all when the solver is
//!    gated) and placed at the end of this layer.
//! 2. **Parameters and bindings**: [`bindings::check_bindings`] (including `duplicate_id` for
//!    N3 and the cache check).
//! 3. **Solver**: the issues of [`solver::solve`] (`no_fixed_root`, orientation, `under_…`,
//!    `redundant_…`, `conflicting_…`, `inside_violated`, `clearance_violated`, hole
//!    direction/axis), in the solver's order. **Gating:** the solver is skipped (and
//!    `resolved` is `Resolved::new(revision)`, empty) when layer 1 has any error among the
//!    checks that feed positions — `duplicate_id`, shape-reference, shape-cycle, constraint
//!    port and body checks (`body_first_not_add`, `invalid_body_component`, body
//!    `multi_owner`). Assembly-tree, `root`/`invalid_root` and binding errors do not gate (the
//!    solver reads cached field values and ignores assemblies).
//! 4. **Design rules** (D class), only when the solver ran: the "空" and "零件間" groups
//!    (`reserve_violated`, `keep_out_violated`, `envelope_exceeded`, `empty_cut_no_effect`,
//!    `empty_cut_removes_all`, `reserve_overlap`, `reserve_unreachable`,
//!    `part_interference`, `floating_part`), then the rules carried over from 1.0 (Volume
//!    rules in document order; per part: per hole thread, boundary, depth rules, then
//!    `overlapping_holes`, then fillets and chamfers), then `assumption`. Checks that need
//!    geometry skip components without a solved placement (e.g. after `no_fixed_root`).
//!
//! Every P-class issue's `owner` is [`ValidateOptions::submitter`] (`None` for human/file
//! input); D-class owners are the fixed §9.2 roles. All messages embed only ids, numbers and
//! fixed template text (§9.1).
//!
//! Approximations (never a false issue; see the `solid` submodule): solids are axis-aligned boxes and
//! cylinders; `extrude` of a non-rect/disc/circle profile, `revolve` and countersink cones are
//! bounding primitives; undecidable overlaps (coincident curved walls) report nothing.

mod design;
mod legacy;
mod solid;
mod structure;

use crate::bindings;
use crate::model::{CadDocument2, Id, Resolved, Role, Severity, ValidationIssue, ValidationReport};
use crate::solver::{self, WorldView};

/// `code` of schema-violation issues (the §9.2 patch reject reason `schema_invalid`).
pub const SCHEMA_INVALID: &str = "schema_invalid";

/// Options for a validation run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ValidateOptions {
    /// Check the "completed document" rules (components ≥ 1, parts ≥ 1, root; spec §8.6).
    pub completed: bool,
    /// Role that submitted the change; becomes `owner` of P-class issues (None for human / file input).
    pub submitter: Option<Role>,
}

/// Result of a validation run.
#[derive(Debug, Clone, PartialEq)]
pub struct Validation {
    /// Analytic-layer report (all issues, incl. the solver's).
    pub report: ValidationReport,
    /// Solver output (`resolved.json`).
    pub resolved: Resolved,
}

/// Validates a document (see the module docs for layers, gating and issue order).
pub fn validate(doc: &CadDocument2, opts: &ValidateOptions) -> Validation {
    let finish = |issues: Vec<ValidationIssue>, resolved: Resolved| {
        let issues = issues
            .into_iter()
            .map(|i| {
                if i.code == SCHEMA_INVALID {
                    i.with_owner(opts.submitter)
                } else {
                    i.for_submitter(opts.submitter)
                }
            })
            .collect();
        Validation {
            report: ValidationReport::for_document(doc, issues),
            resolved,
        }
    };

    let schema = structure::schema_issues(doc);
    if !schema.is_empty() {
        return finish(schema, Resolved::new(doc.revision));
    }

    let st = structure::check(doc, opts.completed);
    let mut issues = st.issues;
    let binding_issues = bindings::check_bindings(doc);

    if st.blocks_solver {
        issues.extend(binding_issues);
        return finish(issues, Resolved::new(doc.revision));
    }

    let resolved = solver::solve(doc);
    let view = WorldView::new(doc, &resolved);
    issues.extend(structure::check_profiles(doc, &view));
    issues.extend(binding_issues);
    issues.extend(resolved.issues.iter().cloned());
    let geo = design::Geo { doc, view: &view };
    issues.extend(design::check(&geo));
    issues.extend(legacy::check(doc, Some(&view)));
    finish(issues, resolved)
}

/// Only the P-class (patch-blocking) issues; subset of `validate(..).report.issues`.
///
/// Includes [`SCHEMA_INVALID`] issues (a schema violation always rejects a patch).
pub fn blocking_issues(report: &ValidationReport) -> Vec<&ValidationIssue> {
    report
        .issues
        .iter()
        .filter(|i| {
            i.severity == Severity::Error
                && (i.code == SCHEMA_INVALID || i.code().is_some_and(|c| c.is_patch_blocking()))
        })
        .collect()
}

/// The root the coordinator would compute per spec §6.3 (None when it would be `invalid_root`).
///
/// With assemblies: the unique assembly that is no assembly's child; without: the only part.
pub fn compute_root(doc: &CadDocument2) -> Option<Id> {
    structure::compute_root(doc)
}
