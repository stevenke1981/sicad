//! §5.5 translation fixpoint (steps 2–5; step 1 `no_fixed_root` and step 6 checks are in
//! [`super::solve`]).
//!
//! Every non-check constraint becomes single-axis equations `X_a,k − X_b,k = c`, where `X` is
//! a port's anchor coordinate (point/axis position, face center). Equations are *built*
//! lazily: a constraint whose `b` directions depend on not-yet-known translations (derived
//! `profile`/`extrude` frames, `segment` axes) is retried on every scan and only joins the
//! propagation once those directions are computable.

use crate::model::geom::{Axis, TOL};
use crate::model::issue::codes::Code;
use crate::model::issue::ValidationIssue;
use crate::model::resolved::ResolvedPort;
use crate::model::v2::ConstraintKind;
use crate::ports::ConstraintPorts;

use super::world::{anchor_coord, World};
use super::{fmt_num, join_ids};

#[derive(Debug, Clone, Copy)]
enum Term<'d> {
    Port(usize, &'d str),
    Const(f64),
}

#[derive(Debug, Clone)]
struct Equation<'d> {
    axis: usize,
    a: Term<'d>,
    b: Term<'d>,
    c: f64,
    used: bool,
}

enum Slot<'d> {
    /// Never produces equations (check-only, invalid ports, unusable geometry, root `fixed`).
    Skip,
    /// Not buildable yet.
    Pending,
    /// Built.
    Built(Vec<Equation<'d>>),
}

pub(crate) struct TranslationResult {
    /// Axes defaulted to 0 per component (§5.5 step 4).
    pub defaulted: Vec<Vec<Axis>>,
    /// `under_constrained` warnings in defaulting order.
    pub under: Vec<ValidationIssue>,
    /// `redundant_constraint` / `conflicting_constraints` in constraint order.
    pub residual: Vec<ValidationIssue>,
}

const AXIS_NAMES: [&str; 3] = ["x", "y", "z"];

struct State<'w, 'd> {
    world: &'w mut World<'d>,
    known: Vec<[bool; 3]>,
    assigner: Vec<[Option<usize>; 3]>,
}

impl<'d> State<'_, 'd> {
    fn independent(&self, t: Term<'d>) -> Option<usize> {
        match t {
            Term::Port(ci, _) if self.world.comp(ci).is_independent() => Some(ci),
            _ => None,
        }
    }

    fn fully_known(&self, ci: usize) -> bool {
        let srcs = self.world.sources(ci);
        !srcs.is_empty() && srcs.iter().all(|s| self.known[*s].iter().all(|k| *k))
    }

    fn term_known(&self, t: Term<'d>, axis: usize) -> bool {
        match t {
            Term::Const(_) => true,
            Term::Port(ci, port) => {
                if self.world.comp(ci).is_independent() {
                    return self.known[ci][axis];
                }
                if self.world.port(ci, port).is_none() {
                    return false;
                }
                let srcs = self.world.sources(ci);
                if srcs.is_empty() {
                    return false;
                }
                if self.world.anchor_separable(ci, port) {
                    srcs.iter().all(|s| self.known[*s][axis])
                } else {
                    srcs.iter().all(|s| self.known[*s].iter().all(|k| *k))
                }
            }
        }
    }

    fn value(&self, t: Term<'d>, axis: usize) -> Option<f64> {
        match t {
            Term::Const(c) => Some(c),
            Term::Port(ci, port) => anchor_coord(&self.world.port(ci, port)?, axis),
        }
    }

    /// Applies the §5.5 step-3 rule to one equation; `true` when it assigned a translation.
    fn try_assign(&mut self, eq: &mut Equation<'d>, ki: usize) -> bool {
        let (ia, ib) = (self.independent(eq.a), self.independent(eq.b));
        if ia.is_some() && ia == ib {
            return false; // both ends on the same component: a constant check (step 5)
        }
        let axis = eq.axis;
        let (ka, kb) = (self.term_known(eq.a, axis), self.term_known(eq.b, axis));
        let (unknown, known_term, unknown_term, sign) = match (ka, kb, ia, ib) {
            // X_b = X_a − c
            (true, false, _, Some(u)) => (u, eq.a, eq.b, -1.0),
            // X_a = X_b + c
            (false, true, Some(u), _) => (u, eq.b, eq.a, 1.0),
            _ => return false,
        };
        let (Some(x_known), Some(x_unknown)) =
            (self.value(known_term, axis), self.value(unknown_term, axis))
        else {
            return false;
        };
        let local = x_unknown - self.world.trans(unknown)[axis];
        self.world
            .set_trans_axis(unknown, axis, x_known + sign * eq.c - local);
        self.known[unknown][axis] = true;
        self.assigner[unknown][axis] = Some(ki);
        eq.used = true;
        true
    }

    /// Builds the equations of a non-`fixed` constraint.
    fn build(&self, ki: usize) -> Slot<'d> {
        let doc = self.world.doc;
        let k = &doc.constraints[ki];
        let Some(b) = k.b.as_ref() else {
            return Slot::Skip;
        };
        let (Some(ai), Some(bi)) = (
            self.world.lookup(k.a.id_str()),
            self.world.lookup(b.id_str()),
        ) else {
            return Slot::Skip;
        };
        let (ta, tb) = (Term::Port(ai, k.a.port()), Term::Port(bi, b.port()));
        if !self.world.dirs_static(bi, b.port()) && !self.fully_known(bi) {
            return Slot::Pending;
        }
        let Some(pb) = self.world.port(bi, b.port()) else {
            return if self.fully_known(bi) {
                Slot::Skip
            } else {
                Slot::Pending
            };
        };
        let eq = |axis: usize, c: f64| Equation {
            axis,
            a: ta,
            b: tb,
            c,
            used: false,
        };
        let eqs = match (k.kind, &pb) {
            (ConstraintKind::Coincident, _) => (0..3).map(|i| eq(i, 0.0)).collect(),
            (ConstraintKind::OnFace, ResolvedPort::Face { u, v, .. }) => {
                let (su, sv) = (
                    k.effective_u().unwrap_or(0.0),
                    k.effective_v().unwrap_or(0.0),
                );
                let (uu, vv) = (u.unit(), v.unit());
                (0..3).map(|i| eq(i, su * uu[i] + sv * vv[i])).collect()
            }
            (ConstraintKind::Coaxial, ResolvedPort::Axis { direction, .. }) => (0..3)
                .filter(|i| *i != direction.index())
                .map(|i| eq(i, 0.0))
                .collect(),
            (
                ConstraintKind::Mate | ConstraintKind::Flush | ConstraintKind::Distance,
                ResolvedPort::Face { normal, .. },
            ) => {
                let value = k.effective_value().unwrap_or(0.0);
                vec![eq(normal.index(), value * normal.sign())]
            }
            _ => return Slot::Skip,
        };
        Slot::Built(eqs)
    }

    fn assigners(&self, t: Term<'d>, axis: usize, exclude: usize, out: &mut Vec<usize>) {
        if let Term::Port(ci, _) = t {
            for &s in self.world.sources(ci) {
                if let Some(k) = self.assigner[s][axis] {
                    if k != exclude && !out.contains(&k) {
                        out.push(k);
                    }
                }
            }
        }
    }
}

/// Steps 2–5 of §5.5. Writes solved translations into `world.trans`.
pub(crate) fn solve_translations(
    world: &mut World<'_>,
    checked: &[Option<ConstraintPorts>],
) -> TranslationResult {
    let doc = world.doc;
    let n = doc.components.len();
    let m = doc.constraints.len();
    for ci in 0..n {
        world.set_trans(ci, [0.0; 3]);
    }
    let mut st = State {
        world,
        known: vec![[false; 3]; n],
        assigner: vec![[None; 3]; n],
    };
    let mut slots: Vec<Slot<'_>> = Vec::with_capacity(m);

    // 2. roots
    for (ki, k) in doc.constraints.iter().enumerate() {
        let usable = checked[ki].is_some();
        if k.kind != ConstraintKind::Fixed {
            slots.push(if usable && !k.kind.is_check_only() {
                Slot::Pending
            } else {
                Slot::Skip
            });
            continue;
        }
        let (true, Some(frame), Some(ci)) =
            (usable, k.frame.as_ref(), st.world.lookup(k.a.id_str()))
        else {
            slots.push(Slot::Skip);
            continue;
        };
        if st.known[ci].iter().all(|x| !x) {
            st.world.set_trans(ci, frame.origin);
            st.known[ci] = [true; 3];
            st.assigner[ci] = [Some(ki); 3];
            slots.push(Slot::Skip);
        } else {
            let eqs = (0..3)
                .map(|i| Equation {
                    axis: i,
                    a: Term::Port(ci, k.a.port()),
                    b: Term::Const(frame.origin[i]),
                    c: 0.0,
                    used: false,
                })
                .collect();
            slots.push(Slot::Built(eqs));
        }
    }

    let mut defaulted: Vec<Vec<Axis>> = vec![Vec::new(); n];
    let mut under = Vec::new();
    loop {
        // 3. propagate in document order until a full scan assigns nothing
        loop {
            let mut changed = false;
            for (ki, slot) in slots.iter_mut().enumerate() {
                if matches!(slot, Slot::Pending) {
                    *slot = st.build(ki);
                }
                if let Slot::Built(eqs) = slot {
                    for eq in eqs.iter_mut().filter(|e| !e.used) {
                        changed |= st.try_assign(eq, ki);
                    }
                }
            }
            if !changed {
                break;
            }
        }
        // 4. default the first under-constrained independent component, then re-propagate
        let pending = (0..n).find(|&ci| {
            st.world.is_canonical(ci)
                && st.world.comp(ci).is_independent()
                && st.known[ci].iter().any(|k| !k)
        });
        let Some(ci) = pending else { break };
        for axis in 0..3 {
            if !st.known[ci][axis] {
                st.world.set_trans_axis(ci, axis, 0.0);
                st.known[ci][axis] = true;
                defaulted[ci].push(Axis::ALL[axis]);
            }
        }
        let id = st.world.comp(ci).id.to_string();
        let axes: Vec<&str> = defaulted[ci].iter().map(|a| a.as_str()).collect();
        under.push(
            ValidationIssue::new(
                Code::UnderConstrained,
                format!(
                    "component {id} is under-constrained on axes {}; defaulted to 0",
                    axes.join(", ")
                ),
            )
            .with_target(id),
        );
    }

    // 5. redundant / conflicting
    let mut residual = Vec::new();
    for (ki, slot) in slots.iter().enumerate() {
        let Slot::Built(eqs) = slot else { continue };
        let mut unused = Vec::new();
        for eq in eqs.iter().filter(|e| !e.used) {
            if let (Some(xa), Some(xb)) = (st.value(eq.a, eq.axis), st.value(eq.b, eq.axis)) {
                unused.push((eq, (xa - xb - eq.c).abs()));
            }
        }
        if unused.is_empty() {
            continue;
        }
        let id = doc.constraints[ki].id.to_string();
        if unused.iter().all(|(_, r)| *r <= TOL) {
            let axes: Vec<&str> = unused.iter().map(|(e, _)| AXIS_NAMES[e.axis]).collect();
            residual.push(
                ValidationIssue::new(
                    Code::RedundantConstraint,
                    format!("constraint {id} is redundant on axes {}", axes.join(", ")),
                )
                .with_target(id),
            );
        } else {
            let mut related = Vec::new();
            let mut parts = Vec::new();
            for (eq, r) in unused.iter().filter(|(_, r)| *r > TOL) {
                st.assigners(eq.a, eq.axis, ki, &mut related);
                st.assigners(eq.b, eq.axis, ki, &mut related);
                parts.push(format!(
                    "axis {} residual {} mm",
                    AXIS_NAMES[eq.axis],
                    fmt_num(*r)
                ));
            }
            let related: Vec<String> = related
                .iter()
                .map(|&k| doc.constraints[k].id.to_string())
                .collect();
            residual.push(
                ValidationIssue::new(
                    Code::ConflictingConstraints,
                    format!(
                        "constraint {id} conflicts with {}: {}",
                        if related.is_empty() {
                            "the solved positions".to_owned()
                        } else {
                            join_ids(&related)
                        },
                        parts.join("; ")
                    ),
                )
                .with_target(id)
                .with_related(related),
            );
        }
    }

    TranslationResult {
        defaulted,
        under,
        residual,
    }
}
