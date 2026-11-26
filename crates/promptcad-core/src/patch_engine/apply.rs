//! Per-op semantics (spec §8.2) applied to a working copy of the document.
//!
//! Every function validates first and mutates only on success, so a failed op leaves the
//! working copy as it was before that op (the caller discards the copy anyway).

use std::collections::BTreeSet;

use super::{ReasonCode, RejectReason};
use crate::bindings::evaluate_binding;
use crate::expr::Expr;
use crate::fields::{self, check_bindable, FieldError};
use crate::model::ids::{FieldRef, Id};
use crate::model::issue::codes::{Code, RejectCode};
use crate::model::patch::PatchOp;
use crate::model::v2::{limits, Binding, CadDocument2};

/// Ids of components created by the patch being applied (stamped with provenance at the end).
pub(crate) type Created = BTreeSet<Id>;

fn reject(
    code: impl Into<ReasonCode>,
    op_index: usize,
    target: Option<String>,
    message: String,
) -> RejectReason {
    RejectReason {
        code: code.into(),
        op_index: Some(op_index),
        target,
        message,
    }
}

fn in_n1(doc: &CadDocument2, id: &str) -> bool {
    doc.component(id).is_some() || doc.constraint(id).is_some()
}

fn in_n2(doc: &CadDocument2, id: &str) -> bool {
    doc.part(id).is_some() || doc.assembly(id).is_some()
}

fn cap(len: usize, max: usize, list: &str, i: usize, op: &PatchOp) -> Result<(), RejectReason> {
    if len >= max {
        Err(reject(
            RejectCode::SchemaInvalid,
            i,
            None,
            format!(
                "op {i} ({}): /{list} would exceed its limit of {max}",
                op.op_name()
            ),
        ))
    } else {
        Ok(())
    }
}

/// Field-reference ids used by binding expressions (unparsable expressions contribute none;
/// the validator reports them).
fn binding_expr_refs(doc: &CadDocument2) -> Vec<(FieldRef, FieldRef)> {
    doc.bindings
        .iter()
        .filter_map(|b| Expr::parse(&b.expr).ok().map(|e| (b.target.clone(), e)))
        .flat_map(|(t, e)| e.refs().into_iter().map(move |r| (t.clone(), r)))
        .collect()
}

/// After bindings targeting `id` were dropped: who still references `id`? (§8.2
/// `remove_component`: "still referenced by other constraints, `shape`, `body`".)
fn referrers(doc: &CadDocument2, id: &str) -> Vec<String> {
    let mut out = Vec::new();
    for k in &doc.constraints {
        if k.ports().any(|p| p.id_str() == id) {
            out.push(k.id.to_string());
        }
    }
    for c in &doc.components {
        if c.id.as_str() != id
            && c.geometry
                .shape_refs()
                .iter()
                .any(|(_, r)| r.as_str() == id)
        {
            out.push(c.id.to_string());
        }
    }
    for p in &doc.parts {
        if p.body.iter().any(|b| b.component.as_str() == id) {
            out.push(p.id.to_string());
        }
    }
    for (target, r) in binding_expr_refs(doc) {
        if r.id_str() == id {
            out.push(target.to_string());
        }
    }
    out.dedup();
    out
}

fn field_error(e: FieldError, i: usize, op: &PatchOp) -> RejectReason {
    let target = Some(e.target().to_string());
    let name = op.op_name();
    match e {
        FieldError::Dangling(t) => reject(
            RejectCode::UnknownTarget,
            i,
            target,
            format!("op {i} ({name}): no component or constraint {}", t.id_str()),
        ),
        FieldError::Unknown(t) => reject(
            Code::UnknownField,
            i,
            target,
            format!("op {i} ({name}): {t} is not a bindable numeric field"),
        ),
        FieldError::Null(t) => reject(
            Code::UnknownField,
            i,
            target,
            format!("op {i} ({name}): {t} is null and cannot be set or bound"),
        ),
        FieldError::NonFinite(t) => reject(
            RejectCode::SchemaInvalid,
            i,
            target,
            format!("op {i} ({name}): value for {t} must be finite"),
        ),
    }
}

fn require_field_owner(
    doc: &CadDocument2,
    target: &FieldRef,
    i: usize,
    op: &PatchOp,
) -> Result<(), RejectReason> {
    if in_n1(doc, target.id_str()) {
        Ok(())
    } else {
        Err(field_error(FieldError::Dangling(target.clone()), i, op))
    }
}

/// Applies op `i` to `doc`. On error `doc` is unchanged.
pub(crate) fn apply_op(
    doc: &mut CadDocument2,
    i: usize,
    op: &PatchOp,
    created: &mut Created,
) -> Result<(), RejectReason> {
    let name = op.op_name();
    match op {
        PatchOp::AddComponent { component } => {
            let id = component.id.as_str();
            if in_n1(doc, id) {
                return Err(reject(
                    Code::DuplicateId,
                    i,
                    Some(id.to_owned()),
                    format!("op {i} ({name}): id {id} already exists in namespace N1"),
                ));
            }
            cap(
                doc.components.len(),
                limits::COMPONENTS,
                "components",
                i,
                op,
            )?;
            doc.components.push(component.clone().into_component(None));
            created.insert(component.id.clone());
        }
        PatchOp::RemoveComponent { id } => {
            let Some(index) = doc.component_index(id.as_str()) else {
                return Err(reject(
                    RejectCode::UnknownTarget,
                    i,
                    Some(id.to_string()),
                    format!("op {i} ({name}): no component {id}"),
                ));
            };
            let mut next = doc.clone();
            next.bindings.retain(|b| b.target.id_str() != id.as_str());
            next.components.remove(index);
            let refs = referrers(&next, id.as_str());
            if !refs.is_empty() {
                return Err(reject(
                    Code::DanglingReference,
                    i,
                    Some(id.to_string()),
                    format!(
                        "op {i} ({name}): {id} is still referenced by {}",
                        refs.join(", ")
                    ),
                ));
            }
            *doc = next;
            created.remove(id);
        }
        PatchOp::SetField { target, value } => {
            require_field_owner(doc, target, i, op)?;
            if doc.binding(target).is_some() {
                return Err(reject(
                    RejectCode::FieldIsBound,
                    i,
                    Some(target.to_string()),
                    format!("op {i} ({name}): {target} is bound; remove_binding first"),
                ));
            }
            fields::set_field(doc, target, *value).map_err(|e| field_error(e, i, op))?;
        }
        PatchOp::SetBinding { target, expr } => {
            require_field_owner(doc, target, i, op)?;
            check_bindable(doc, target).map_err(|e| field_error(e, i, op))?;
            let binding = Binding {
                target: target.clone(),
                expr: expr.as_str().to_owned(),
            };
            match doc.bindings.iter().position(|b| &b.target == target) {
                Some(pos) => doc.bindings[pos] = binding.clone(),
                None => {
                    cap(doc.bindings.len(), limits::BINDINGS, "bindings", i, op)?;
                    doc.bindings.push(binding.clone());
                }
            }
            // §8.2: evaluated immediately and written to the cache. An expression that cannot
            // be evaluated yet keeps the old cache; normalization recomputes every binding and
            // the validator reports parse / name / eval / cycle problems as P-class issues.
            if let Ok(v) = evaluate_binding(doc, &binding) {
                let _ = fields::set_field(doc, target, v);
            }
        }
        PatchOp::RemoveBinding { target } => {
            let Some(pos) = doc.bindings.iter().position(|b| &b.target == target) else {
                return Err(reject(
                    RejectCode::UnknownTarget,
                    i,
                    Some(target.to_string()),
                    format!("op {i} ({name}): {target} has no binding"),
                ));
            };
            // The field keeps its last cached value (§4.4 step 4).
            doc.bindings.remove(pos);
        }
        PatchOp::AddConstraint { constraint } => {
            let id = constraint.id.as_str();
            if in_n1(doc, id) {
                return Err(reject(
                    Code::DuplicateId,
                    i,
                    Some(id.to_owned()),
                    format!("op {i} ({name}): id {id} already exists in namespace N1"),
                ));
            }
            cap(
                doc.constraints.len(),
                limits::CONSTRAINTS,
                "constraints",
                i,
                op,
            )?;
            doc.constraints.push(constraint.clone());
        }
        PatchOp::RemoveConstraint { id } => {
            let Some(index) = doc.constraints.iter().position(|k| k.id == *id) else {
                return Err(reject(
                    RejectCode::UnknownTarget,
                    i,
                    Some(id.to_string()),
                    format!("op {i} ({name}): no constraint {id}"),
                ));
            };
            let mut next = doc.clone();
            next.bindings.retain(|b| b.target.id_str() != id.as_str());
            next.constraints.remove(index);
            let refs = referrers(&next, id.as_str());
            if !refs.is_empty() {
                return Err(reject(
                    Code::DanglingReference,
                    i,
                    Some(id.to_string()),
                    format!(
                        "op {i} ({name}): {id} is still referenced by {}",
                        refs.join(", ")
                    ),
                ));
            }
            *doc = next;
        }
        PatchOp::SetParameter { parameter } => {
            match doc.parameters.iter().position(|p| p.name == parameter.name) {
                Some(pos) => doc.parameters[pos] = parameter.clone(),
                None => {
                    cap(
                        doc.parameters.len(),
                        limits::PARAMETERS,
                        "parameters",
                        i,
                        op,
                    )?;
                    doc.parameters.push(parameter.clone());
                }
            }
        }
        PatchOp::AddPart { part } => {
            let id = part.id.as_str();
            if in_n2(doc, id) {
                return Err(reject(
                    Code::DuplicateId,
                    i,
                    Some(id.to_owned()),
                    format!("op {i} ({name}): id {id} already exists in namespace N2"),
                ));
            }
            cap(doc.parts.len(), limits::PARTS, "parts", i, op)?;
            doc.parts.push(part.clone());
        }
        PatchOp::SetBody { part_id, body } => {
            let Some(part) = doc.parts.iter_mut().find(|p| p.id == *part_id) else {
                return Err(reject(
                    RejectCode::UnknownTarget,
                    i,
                    Some(part_id.to_string()),
                    format!("op {i} ({name}): no part {part_id}"),
                ));
            };
            part.body = body.clone();
        }
        PatchOp::AddAssembly { assembly } => {
            let id = assembly.id.as_str();
            if in_n2(doc, id) {
                return Err(reject(
                    Code::DuplicateId,
                    i,
                    Some(id.to_owned()),
                    format!("op {i} ({name}): id {id} already exists in namespace N2"),
                ));
            }
            cap(
                doc.assemblies.len(),
                limits::ASSEMBLIES,
                "assemblies",
                i,
                op,
            )?;
            doc.assemblies.push(assembly.clone());
        }
        PatchOp::AddAssumption { text } => {
            cap(doc.assumptions.len(), limits::NOTES, "assumptions", i, op)?;
            doc.assumptions.push(text.as_str().to_owned());
        }
        PatchOp::RemoveAssumption { text } => {
            let Some(pos) = find_assumption(doc, text.as_str()) else {
                return Err(assumption_not_found(i));
            };
            doc.assumptions.remove(pos);
        }
        PatchOp::AddNote { text } => {
            cap(doc.notes.len(), limits::NOTES, "notes", i, op)?;
            doc.notes.push(text.as_str().to_owned());
        }
    }
    Ok(())
}

/// Index of the first assumption exactly equal to `text`.
pub(crate) fn find_assumption(doc: &CadDocument2, text: &str) -> Option<usize> {
    doc.assumptions.iter().position(|a| a == text)
}

/// `assumption_not_found` for op `i`. The message never echoes the free text (§9.1).
pub(crate) fn assumption_not_found(i: usize) -> RejectReason {
    reject(
        RejectCode::AssumptionNotFound,
        i,
        None,
        format!("op {i} (remove_assumption): no identical assumption in the document"),
    )
}
