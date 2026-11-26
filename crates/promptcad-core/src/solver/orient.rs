//! §5.4 orientation determination (steps 1–6; step 7 runs in [`super::solve`] once every port
//! direction, including position-dependent ones, can be evaluated).
//!
//! # Directions during the orientation phase
//!
//! Every requirement end gets a symbolic direction ([`Sym`]):
//!
//! * an independent component: `orientation(root) · local`, where `root` is the component
//!   itself, or for a hole without its own `fixed`, the orientation source of its part's
//!   `body[0]` (step 3);
//! * a derived component (step 4) follows its [`OrientSrc`]: `of`, `section`, and `extrude` of
//!   a `rect` / `disc` / `section` (any chain of them) carry the orientation of the chain's
//!   independent root, so `extrude(rect).top` is `orientation(rect) · +z` and drives step 5
//!   exactly like the `top` of a plate; Lines and `revolve` are identity (`circle.axis` =
//!   `+z`);
//! * position-dependent directions (`profile` from Dots and everything derived from it,
//!   `segment.axis`) are *deferred*: they do not drive the first pass of step 5.
//!
//! When anything was deferred, [`super::solve`] runs a second pass after the translations are
//! known: deferred directions are then evaluated on the solved positions (they depend only on
//! Dot positions, never on orientations) and step 5 is re-run from scratch with them as fixed
//! world directions. If that changes any orientation the translations are re-solved (bounded
//! number of rounds). Step 7 always re-checks every requirement on the final world directions,
//! so a requirement that could not drive step 5 is still checked against the true geometry.
//!
//! # Holes (step 3)
//!
//! A hole follows the orientation of its owning part's `body[0]` whether that Volume is
//! independent or derived (no independence condition in §5.4 step 3): for an `extrude` of a
//! `rect` the hole follows the rect. When `body[0]`'s orientation is position-dependent
//! (`extrude` of a Dot `profile`) the hole is deferred like any positional direction and takes
//! `body[0]`'s solved orientation in the second pass.
//!
//! A hole with its **own** `fixed` keeps the fixed frame (step 1 precedes step 3 and is
//! unconditional: "a component with `fixed`: orientation = `frame`"). Holes are normally
//! positioned through `coincident` with a fixed Dot (that is what `upgrade_1_to_2` emits), in
//! which case step 3 applies.
//!
//! # Limits
//!
//! The coaxial equations of a deferred hole's `axis` are built in the first translation pass
//! with the provisional identity direction; they are rebuilt when the second pass changes the
//! hole's orientation.

use crate::model::geom::{AxisDir, Orientation};
use crate::model::issue::codes::Code;
use crate::model::issue::ValidationIssue;
use crate::model::v2::{ConstraintKind, EmptyShape, Geometry, LineShape, PlaneShape, VolumeShape};
use crate::model::ShapeKind;
use crate::ports::{
    box_port, has_orientation_requirement, local_port, plane_port, principal_dir, ConstraintPorts,
};

use super::world::{OrientSrc, World};
use super::{join_ids, orientation_requirement_holds};

/// A constraint carrying an orientation requirement, with both ends resolved to components.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Requirement<'d> {
    pub k: usize,
    pub kind: ConstraintKind,
    pub a: (usize, &'d str),
    pub b: (usize, &'d str),
}

/// A port direction during the orientation phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sym {
    /// `orientation(root) · local`.
    Rel(usize, AxisDir),
    /// A fixed world direction.
    Abs(AxisDir),
    /// Depends on translations: ignored by the first pass of step 5, evaluated on the solved
    /// positions in the second pass.
    Deferred,
    /// Does not resolve: ignored by step 5.
    Unknown,
}

pub(crate) struct OrientResult {
    /// `fixed` constraint (index) that determined each component's orientation.
    pub fixed_by: Vec<Option<usize>>,
    /// Components for which step 5 found no orientation (they get identity).
    pub failed: Vec<bool>,
    /// Step-5 `orientation_unsatisfiable` issues.
    pub issues: Vec<ValidationIssue>,
    /// Some direction (or hole orientation) was deferred to the positional pass.
    pub deferred: bool,
}

/// All orientation-requirement constraints with valid ports, in document order.
pub(crate) fn requirements<'d>(
    world: &World<'d>,
    checked: &[Option<ConstraintPorts>],
) -> Vec<Requirement<'d>> {
    let doc = world.doc;
    let mut out = Vec::new();
    for (ki, k) in doc.constraints.iter().enumerate() {
        let (Some(ports), Some(b)) = (checked[ki], k.b.as_ref()) else {
            continue;
        };
        let Some(bt) = ports.b else { continue };
        if !has_orientation_requirement(k.kind, ports.a, bt) {
            continue;
        }
        let (Some(ai), Some(bi)) = (world.lookup(k.a.id_str()), world.lookup(b.id_str())) else {
            continue;
        };
        out.push(Requirement {
            k: ki,
            kind: k.kind,
            a: (ai, k.a.port()),
            b: (bi, b.port()),
        });
    }
    out
}

/// The owning part's `body[0]` of a hole without its own `fixed` (the component whose
/// orientation the hole takes, §5.4 step 3).
fn follows_body0(world: &World<'_>, fixed_by: &[Option<usize>], ci: usize) -> Option<usize> {
    if fixed_by[ci].is_some() || !world.is_hole(ci) {
        return None;
    }
    world
        .hole_body0(ci)
        .filter(|&b| b != ci && !world.is_hole(b))
}

/// The independent component whose orientation an independent component's orientation equals:
/// itself, or for a hole without its own `fixed`, the independent orientation root of its
/// part's `body[0]` (§5.4 step 3; `body[0]` may be derived, e.g. an `extrude` of a `rect`).
pub(crate) fn root(world: &World<'_>, fixed_by: &[Option<usize>], ci: usize) -> usize {
    match follows_body0(world, fixed_by, ci).map(|b| world.orient_src(b)) {
        Some(OrientSrc::Of(r)) if r != ci && !world.is_hole(r) => r,
        _ => ci,
    }
}

/// A hole whose `body[0]` orientation depends on translations (`extrude` of a `profile`).
pub(crate) fn hole_deferred(world: &World<'_>, fixed_by: &[Option<usize>], ci: usize) -> bool {
    follows_body0(world, fixed_by, ci).is_some_and(|b| world.orient_src(b) == OrientSrc::Positional)
}

/// A position-dependent direction: deferred in the first pass, read from the (solved) world
/// in the second.
fn positional(world: &World<'_>, ci: usize, port: &str, eval: bool) -> Sym {
    if !eval {
        return Sym::Deferred;
    }
    match world.port(ci, port).as_ref().and_then(principal_dir) {
        Some(d) => Sym::Abs(d),
        None => Sym::Unknown,
    }
}

/// Symbolic direction of `ci.port`; `eval` = evaluate deferred directions on the world.
fn sym_dir(
    world: &World<'_>,
    fixed_by: &[Option<usize>],
    ci: usize,
    port: &str,
    eval: bool,
) -> Sym {
    let comp = world.comp(ci);
    if comp.is_independent() {
        let Some(l) = local_port(comp, port).as_ref().and_then(principal_dir) else {
            return Sym::Unknown;
        };
        if hole_deferred(world, fixed_by, ci) && !eval {
            return Sym::Deferred;
        }
        return Sym::Rel(root(world, fixed_by, ci), l);
    }
    let local = match &comp.geometry {
        Geometry::Empty {
            shape: EmptyShape::Of(of),
            ..
        } => {
            // the source is a Volume (independent, `extrude` or `revolve`): one level
            return match world.lookup(of.source.as_str()) {
                Some(si) if si != ci && world.comp(si).as_volume().is_some() => {
                    sym_dir(world, fixed_by, si, port, eval)
                }
                _ => Sym::Unknown,
            };
        }
        Geometry::Line {
            shape: LineShape::Circle(_),
        } if port == "axis" => return Sym::Abs(AxisDir::PosZ),
        Geometry::Line {
            shape: LineShape::Segment(_),
        } if port == "axis" => return positional(world, ci, port, eval),
        Geometry::Plane {
            shape: PlaneShape::Profile(_) | PlaneShape::Section(_),
        } => plane_port([1.0, 1.0], port),
        Geometry::Volume {
            shape: VolumeShape::Extrude(_),
        } => box_port(1.0, 1.0, 1.0, port),
        _ => None,
    };
    let Some(l) = local.as_ref().and_then(principal_dir) else {
        return Sym::Unknown;
    };
    match world.orient_src(ci) {
        OrientSrc::Of(r) => Sym::Rel(root(world, fixed_by, r), l),
        OrientSrc::Identity => Sym::Abs(l),
        OrientSrc::Positional => positional(world, ci, port, eval),
        OrientSrc::Broken => Sym::Unknown,
    }
}

fn resolve(sym: Sym, orient: &[Option<Orientation>]) -> Option<AxisDir> {
    match sym {
        Sym::Rel(r, l) => orient[r].map(|o| o.apply_dir(l)),
        Sym::Abs(d) => Some(d),
        Sym::Deferred | Sym::Unknown => None,
    }
}

/// Steps 1–6 of §5.4. Writes the final orientation of every independent component into the
/// world.
///
/// `eval_deferred = false` is the first pass (position-dependent directions are ignored);
/// `true` is the second pass, run on a world whose translations are solved: position-dependent
/// directions are read from it, and holes following a position-dependent `body[0]` take its
/// solved orientation.
pub(crate) fn solve_orientations(
    world: &mut World<'_>,
    checked: &[Option<ConstraintPorts>],
    eval_deferred: bool,
) -> OrientResult {
    let doc = world.doc;
    let n = doc.components.len();
    let mut orient: Vec<Option<Orientation>> = vec![None; n];
    let mut fixed_by: Vec<Option<usize>> = vec![None; n];
    let mut failed = vec![false; n];
    let mut issues = Vec::new();
    let mut deferred = false;

    // 1. fixed → frame
    for (ki, k) in doc.constraints.iter().enumerate() {
        if k.kind != ConstraintKind::Fixed || checked[ki].is_none() {
            continue;
        }
        let (Some(frame), Some(ci)) = (k.frame.as_ref(), world.lookup(k.a.id_str())) else {
            continue;
        };
        if orient[ci].is_none() {
            if let Some(o) = frame.orientation() {
                orient[ci] = Some(o);
                fixed_by[ci] = Some(ki);
            }
        }
    }
    for (ci, o) in orient.iter_mut().enumerate() {
        if o.is_some() || !world.comp(ci).is_independent() {
            continue;
        }
        // 2. Dots are always identity
        if world.comp(ci).shape_kind() == ShapeKind::DotPoint {
            *o = Some(Orientation::IDENTITY);
        }
        // 3. holes outside any part (or whose body[0] is identity-oriented / unresolvable):
        //    identity; holes following an independent orientation root copy it through
        //    `root` (step 6); holes following a position-dependent body[0] are deferred
        if world.is_hole(ci) {
            if hole_deferred(world, &fixed_by, ci) {
                deferred = true;
                if eval_deferred {
                    *o = Some(
                        world
                            .hole_body0(ci)
                            .and_then(|b| world.pose(b))
                            .map_or(Orientation::IDENTITY, |p| p.orientation),
                    );
                }
            } else if root(world, &fixed_by, ci) == ci {
                *o = Some(Orientation::IDENTITY);
            }
        }
    }
    // 4. derived components follow their sources (`sym_dir`, `World::orient_src`).

    // 5. propagate through orientation requirements
    let reqs = requirements(world, checked);
    let syms: Vec<(Sym, Sym)> = reqs
        .iter()
        .map(|r| {
            (
                sym_dir(world, &fixed_by, r.a.0, r.a.1, eval_deferred),
                sym_dir(world, &fixed_by, r.b.0, r.b.1, eval_deferred),
            )
        })
        .collect();
    deferred |= syms
        .iter()
        .any(|&(a, b)| a == Sym::Deferred || b == Sym::Deferred);
    loop {
        let mut changed = false;
        for &(sa, sb) in &syms {
            let target = match (resolve(sa, &orient), resolve(sb, &orient), sa, sb) {
                (Some(_), None, _, Sym::Rel(c, _)) | (None, Some(_), Sym::Rel(c, _), _) => c,
                _ => continue,
            };
            if orient[target].is_some() || failed[target] {
                continue;
            }
            // every requirement between `target` and an end with a known direction
            let mut involved = Vec::new();
            let mut checks = Vec::new();
            for (r, &(xa, xb)) in reqs.iter().zip(&syms) {
                match (xa, xb) {
                    (Sym::Rel(x, _), Sym::Rel(y, _)) if x == target && y == target => {}
                    (Sym::Rel(x, la), other) if x == target => {
                        if let Some(db) = resolve(other, &orient) {
                            checks.push((r.kind, true, la, db));
                            involved.push(r.k);
                        }
                    }
                    (other, Sym::Rel(x, lb)) if x == target => {
                        if let Some(da) = resolve(other, &orient) {
                            checks.push((r.kind, false, lb, da));
                            involved.push(r.k);
                        }
                    }
                    _ => {}
                }
            }
            let found = Orientation::all().into_iter().find(|o| {
                checks.iter().all(|&(kind, target_is_a, local, known)| {
                    let mine = o.apply_dir(local);
                    if target_is_a {
                        orientation_requirement_holds(kind, mine, known)
                    } else {
                        orientation_requirement_holds(kind, known, mine)
                    }
                })
            });
            match found {
                Some(o) => {
                    orient[target] = Some(o);
                    changed = true;
                }
                None => {
                    failed[target] = true;
                    involved.dedup();
                    let ids: Vec<String> = involved
                        .iter()
                        .map(|&k| doc.constraints[k].id.to_string())
                        .collect();
                    let cid = world.comp(target).id.to_string();
                    issues.push(
                        ValidationIssue::new(
                            Code::OrientationUnsatisfiable,
                            format!(
                                "no axis-aligned orientation of {cid} satisfies the orientation \
                                 requirements of {}",
                                join_ids(&ids)
                            ),
                        )
                        .with_target(cid)
                        .with_related(ids),
                    );
                }
            }
        }
        if !changed {
            break;
        }
    }

    // 6. everything else: identity; holes then copy their root
    for (ci, o) in orient.iter_mut().enumerate() {
        if o.is_none() && !world.is_hole(ci) {
            *o = Some(Orientation::IDENTITY);
        }
    }
    for ci in 0..n {
        if orient[ci].is_none() {
            let r = root(world, &fixed_by, ci);
            orient[ci] = Some(orient[r].unwrap_or(Orientation::IDENTITY));
        }
    }
    for (ci, o) in orient.into_iter().enumerate() {
        world.set_orient(ci, o.unwrap_or(Orientation::IDENTITY));
    }
    OrientResult {
        fixed_by,
        failed,
        issues,
        deferred,
    }
}
