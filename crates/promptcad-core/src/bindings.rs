//! Parameter and binding checks, and binding evaluation in dependency order (spec §4, §4.4).
//!
//! * [`check_bindings`] reports every §9.2 "parameters and bindings" problem of a document.
//! * [`recompute_bindings`] is the §4.4 normalizer step: it evaluates all bindings in
//!   topological order of their dependencies and overwrites the cached field values.
//! * [`evaluate_binding`] evaluates one binding against the document's current values.
//!
//! A binding `i` depends on binding `j` when `i`'s expression references the field that `j`
//! drives. Unbound referenced fields contribute their current value. Evaluation order is
//! deterministic: Kahn's algorithm that always takes the lowest binding index first.
//!
//! Failure policy: a binding with a structural, evaluation or range error is skipped and its
//! field keeps its current value; bindings that (transitively) depend on a skipped binding are
//! skipped silently, since the root cause carries the issue.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::expr::{EvalError, Expr, ParseError, Resolver};
use crate::fields::{check_bindable, get_field, set_field, FieldError};
use crate::model::geom::{approx_eq, TOL};
use crate::model::ids::FieldRef;
use crate::model::issue::codes::Code;
use crate::model::issue::ValidationIssue;
use crate::model::schema::Checker;
use crate::model::v2::{Binding, CadDocument2, ConstraintKind, RESERVED_PARAMETER_NAMES};

/// `true` if `target` is driven by a binding.
pub fn is_bound(doc: &CadDocument2, target: &FieldRef) -> bool {
    doc.binding(target).is_some()
}

/// Evaluates `binding` against the document's *current* field values (bound fields are read
/// from their cache, so this does not follow dependency chains; use [`recompute_bindings`]
/// for that). Does not range-check the result.
///
/// Errors: `binding_parse_error`, `binding_unknown_name`, `unknown_field` /
/// `dangling_reference` (a referenced field is missing or not bindable) and
/// `binding_eval_error`. The issue's `target` is the binding target.
pub fn evaluate_binding(doc: &CadDocument2, binding: &Binding) -> Result<f64, ValidationIssue> {
    let expr = Expr::parse(&binding.expr).map_err(|e| parse_issue(&binding.target, e))?;
    eval_in(doc, &binding.target, &expr)
}

/// All parameter and binding problems of `doc`, including cached values that differ from the
/// recomputed ones by more than 1e-6 (§4.4 step 1). Order: parameter issues, then binding
/// issues in processing order, then cache mismatches.
///
/// The cache-mismatch issue uses code `binding_eval_error` (the frozen table has no dedicated
/// code); use [`check_bindings_ignoring_cache`] to skip it.
pub fn check_bindings(doc: &CadDocument2) -> Vec<ValidationIssue> {
    let mut issues = check_parameters(doc);
    let plan = Plan::new(doc);
    issues.extend(plan.issues.iter().cloned());
    for v in &plan.values {
        if let Ok(cached) = get_field(doc, &v.target) {
            if !approx_eq(cached, v.value, TOL) {
                issues.push(
                    ValidationIssue::new(
                        Code::BindingEvalError,
                        format!(
                            "{}: cached value {cached} differs from evaluated value {}",
                            v.target, v.value
                        ),
                    )
                    .with_target(v.target.as_str()),
                );
            }
        }
    }
    issues
}

/// Like [`check_bindings`] but without the cache-mismatch check; this is what to run on a
/// document whose bindings are about to be recomputed.
pub fn check_bindings_ignoring_cache(doc: &CadDocument2) -> Vec<ValidationIssue> {
    let mut issues = check_parameters(doc);
    issues.extend(Plan::new(doc).issues);
    issues
}

/// Evaluates every binding in dependency order and writes the results into the bound fields
/// (the §4.4 normalizer). Bindings with errors leave their field untouched; the returned
/// issues are the binding issues of [`check_bindings`] (no parameter or cache issues).
pub fn recompute_bindings(doc: &mut CadDocument2) -> Vec<ValidationIssue> {
    let plan = Plan::new(doc);
    for v in &plan.values {
        // Cannot fail: the plan only holds targets that passed `check_bindable`.
        let _ = set_field(doc, &v.target, v.value);
    }
    plan.issues
}

// ---- parameters -----------------------------------------------------------------------------

fn check_parameters(doc: &CadDocument2) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    let mut seen = BTreeSet::new();
    for p in &doc.parameters {
        let name = p.name.as_str();
        if !seen.insert(name) {
            issues.push(
                ValidationIssue::new(
                    Code::DuplicateId,
                    format!("{name}: duplicate parameter name"),
                )
                .with_target(name),
            );
        }
        if RESERVED_PARAMETER_NAMES.contains(&name) {
            // No dedicated code exists; the name can never be resolved in an expression.
            issues.push(
                ValidationIssue::new(
                    Code::BindingUnknownName,
                    format!("{name}: min and max are reserved and cannot be parameter names"),
                )
                .with_target(name),
            );
        }
        let below = p.min.is_some_and(|m| p.value < m);
        let above = p.max.is_some_and(|m| p.value > m);
        if below || above {
            issues.push(
                ValidationIssue::new(
                    Code::ParameterOutOfRange,
                    format!(
                        "{name}: value {} is outside [{}, {}]",
                        p.value,
                        p.min.map_or("-inf".to_owned(), |m| m.to_string()),
                        p.max.map_or("inf".to_owned(), |m| m.to_string()),
                    ),
                )
                .with_target(name),
            );
        }
    }
    issues
}

// ---- evaluation helpers ---------------------------------------------------------------------

struct DocResolver<'a>(&'a CadDocument2);

impl Resolver for DocResolver<'_> {
    fn param(&self, name: &str) -> Option<f64> {
        self.0.parameter(name).map(|p| p.value)
    }
    fn field(&self, r: &FieldRef) -> Option<f64> {
        get_field(self.0, r).ok()
    }
}

fn parse_issue(target: &FieldRef, e: ParseError) -> ValidationIssue {
    ValidationIssue::new(Code::BindingParseError, format!("{target}: {e}"))
        .with_target(target.as_str())
}

fn field_issue(target: &FieldRef, e: &FieldError) -> ValidationIssue {
    ValidationIssue::new(e.code(), format!("{target}: expression reference {e}"))
        .with_target(target.as_str())
        .with_related([e.target().id_str().to_owned()])
}

fn eval_in(doc: &CadDocument2, target: &FieldRef, expr: &Expr) -> Result<f64, ValidationIssue> {
    match expr.eval(&DocResolver(doc)) {
        // Normalise -0.0 so caches never serialize as "-0.0".
        Ok(v) => Ok(if v == 0.0 { 0.0 } else { v }),
        Err(EvalError::UnknownParam(p)) => Err(ValidationIssue::new(
            Code::BindingUnknownName,
            format!("{target}: unknown parameter {p}"),
        )
        .with_target(target.as_str())
        .with_related([p.to_string()])),
        Err(EvalError::UnknownField(r)) => Err(match check_bindable(doc, &r) {
            Err(e) => field_issue(target, &e),
            Ok(()) => ValidationIssue::new(
                Code::UnknownField,
                format!("{target}: cannot read field {r}"),
            )
            .with_target(target.as_str())
            .with_related([r.id_str().to_owned()]),
        }),
        Err(e @ (EvalError::DivisionByZero | EvalError::NonFinite)) => Err(ValidationIssue::new(
            Code::BindingEvalError,
            format!("{target}: {e}"),
        )
        .with_target(target.as_str())),
    }
}

/// Range rule of the field itself (`Dim`, `Coord`, revolve angle, countersink angle, mate gap).
fn field_range_violation(doc: &CadDocument2, target: &FieldRef, v: f64) -> Option<String> {
    let mut c = Checker::new();
    let field = target.field();
    if doc.component(target.id_str()).is_some() {
        match field {
            "offset" => c.coord("", v),
            "angle" => c.left_open_range("", v, 0.0, 360.0),
            "countersink_angle" => c.open_range("", v, 0.0, 180.0),
            _ => c.dim("", v),
        }
    } else if let Some(k) = doc.constraint(target.id_str()) {
        c.coord("", v);
        if k.kind == ConstraintKind::Mate && field == "value" && v < 0.0 {
            c.err("", format!("mate gap must be >= 0 (got {v})"));
        }
    }
    c.finish().into_iter().next().map(|x| x.message)
}

/// Cross-field shape rule (ring, l_bracket, enclosure, hole subtype) of component `id`.
fn shape_rule_violation(doc: &CadDocument2, id: &str) -> Option<String> {
    let comp = doc.component(id)?;
    let mut c = Checker::new();
    comp.check(&mut c, "");
    c.finish()
        .into_iter()
        .find(|v| v.path == "/shape")
        .map(|v| v.message)
}

// ---- the plan -------------------------------------------------------------------------------

struct Applied {
    target: FieldRef,
    value: f64,
}

/// Result of evaluating all bindings on a scratch copy of the document.
struct Plan {
    issues: Vec<ValidationIssue>,
    /// Successfully evaluated, range-checked values in evaluation order.
    values: Vec<Applied>,
}

impl Plan {
    fn new(doc: &CadDocument2) -> Plan {
        let mut issues = Vec::new();
        let n = doc.bindings.len();

        // 1. Structure: duplicates, parse, names, targets, references.
        let mut first_of: HashMap<&FieldRef, usize> = HashMap::new();
        let mut exprs: Vec<Option<Expr>> = vec![None; n];
        for (i, b) in doc.bindings.iter().enumerate() {
            if first_of.contains_key(&b.target) {
                issues.push(
                    ValidationIssue::new(
                        Code::DuplicateBinding,
                        format!("{}: more than one binding for this target", b.target),
                    )
                    .with_target(b.target.as_str()),
                );
                continue;
            }
            first_of.insert(&b.target, i);
            let expr = match Expr::parse(&b.expr) {
                Ok(e) => e,
                Err(e) => {
                    issues.push(parse_issue(&b.target, e));
                    continue;
                }
            };
            let mut ok = true;
            for p in expr.params() {
                if doc.parameter(p.as_str()).is_none() {
                    ok = false;
                    issues.push(
                        ValidationIssue::new(
                            Code::BindingUnknownName,
                            format!("{}: unknown parameter {p}", b.target),
                        )
                        .with_target(b.target.as_str())
                        .with_related([p.to_string()]),
                    );
                }
            }
            if let Err(e) = check_bindable(doc, &b.target) {
                ok = false;
                issues.push(
                    ValidationIssue::new(e.code(), e.to_string()).with_target(b.target.as_str()),
                );
            }
            for r in expr.refs() {
                if let Err(e) = check_bindable(doc, &r) {
                    ok = false;
                    issues.push(field_issue(&b.target, &e));
                }
            }
            if ok {
                exprs[i] = Some(expr);
            }
        }

        // 2. Dependency graph over structurally valid bindings.
        let valid: Vec<usize> = (0..n).filter(|&i| exprs[i].is_some()).collect();
        let by_target: HashMap<&FieldRef, usize> = valid
            .iter()
            .map(|&i| (&doc.bindings[i].target, i))
            .collect();
        let mut deps: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
        for &i in &valid {
            let set = exprs[i]
                .as_ref()
                .map(|e| e.refs())
                .unwrap_or_default()
                .iter()
                .filter_map(|r| by_target.get(r).copied())
                .collect();
            deps.insert(i, set);
        }

        // 3. Kahn, lowest index first.
        let mut pending: BTreeMap<usize, usize> = deps.iter().map(|(&i, d)| (i, d.len())).collect();
        let mut ready: BTreeSet<usize> = pending
            .iter()
            .filter(|(_, &c)| c == 0)
            .map(|(&i, _)| i)
            .collect();
        let mut order = Vec::new();
        while let Some(i) = ready.pop_first() {
            order.push(i);
            for (&j, d) in &deps {
                if d.contains(&i) {
                    if let Some(c) = pending.get_mut(&j) {
                        *c -= 1;
                        if *c == 0 {
                            ready.insert(j);
                        }
                    }
                }
            }
        }
        let ordered: BTreeSet<usize> = order.iter().copied().collect();
        let mut stuck: BTreeSet<usize> = valid
            .iter()
            .copied()
            .filter(|i| !ordered.contains(i))
            .collect();
        // Prune bindings that merely depend on a cycle: what remains lies on cycles.
        let mut on_cycle = stuck.clone();
        loop {
            let drop: Vec<usize> = on_cycle
                .iter()
                .copied()
                .filter(|i| !on_cycle.iter().any(|j| deps[j].contains(i)))
                .collect();
            if drop.is_empty() {
                break;
            }
            for i in drop {
                on_cycle.remove(&i);
            }
        }
        for &i in &on_cycle {
            let target = &doc.bindings[i].target;
            let others = deps[&i]
                .iter()
                .filter(|j| on_cycle.contains(j))
                .map(|&j| doc.bindings[j].target.to_string());
            issues.push(
                ValidationIssue::new(
                    Code::BindingCycle,
                    format!("{target}: binding is part of a dependency cycle"),
                )
                .with_target(target.as_str())
                .with_related(others),
            );
        }

        // 4. Evaluate on a scratch copy.
        let mut work = doc.clone();
        let mut failed: BTreeSet<usize> = std::mem::take(&mut stuck);
        let mut values: Vec<Applied> = Vec::new();
        for i in order {
            if deps[&i].iter().any(|j| failed.contains(j)) {
                failed.insert(i);
                continue;
            }
            let b = &doc.bindings[i];
            let Some(expr) = exprs[i].as_ref() else {
                continue;
            };
            let value = match eval_in(&work, &b.target, expr) {
                Ok(v) => v,
                Err(issue) => {
                    issues.push(issue);
                    failed.insert(i);
                    continue;
                }
            };
            if let Some(msg) = field_range_violation(&work, &b.target, value) {
                issues.push(
                    ValidationIssue::new(Code::BindingOutOfRange, format!("{}: {msg}", b.target))
                        .with_target(b.target.as_str()),
                );
                failed.insert(i);
                continue;
            }
            if let Err(e) = set_field(&mut work, &b.target, value) {
                issues.push(
                    ValidationIssue::new(e.code(), e.to_string()).with_target(b.target.as_str()),
                );
                failed.insert(i);
                continue;
            }
            values.push(Applied {
                target: b.target.clone(),
                value,
            });
        }

        // 5. Cross-field shape rules, once all bindings are applied.
        let mut seen: Vec<String> = Vec::new();
        for v in &values {
            let id = v.target.id_str();
            if !seen.iter().any(|s| s == id) {
                seen.push(id.to_owned());
            }
        }
        let mut rejected: Vec<String> = Vec::new();
        for id in seen {
            if let Some(msg) = shape_rule_violation(&work, &id) {
                let on_comp: Vec<&FieldRef> = values
                    .iter()
                    .map(|v| &v.target)
                    .filter(|t| t.id_str() == id.as_str())
                    .collect();
                issues.push(
                    ValidationIssue::new(Code::BindingOutOfRange, format!("{}: {msg}", on_comp[0]))
                        .with_target(on_comp[0].as_str())
                        .with_related(on_comp[1..].iter().map(|t| t.to_string())),
                );
                rejected.push(id.clone());
            }
        }
        values.retain(|v| !rejected.iter().any(|r| r == v.target.id_str()));

        Plan { issues, values }
    }
}
