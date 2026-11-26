//! Orientation and fixpoint translation solver, hole directions and cutters, `resolved.json`
//! (spec §5.4, §5.5, §5.6, §7).
//!
//! [`solve`] is pure and deterministic: the result depends only on the document content and
//! order, and serializing the [`Resolved`] of the same document twice gives identical bytes.
//! It uses the cached numeric field values in the document (bindings are evaluated
//! elsewhere) and never panics on invalid input: constraints whose ports do not resolve or do
//! not fit §5.3 are skipped (the validator reports them).
//!
//! # Pipeline
//!
//! 1. §5.5 step 1: independent components but no `fixed` at all → only `no_fixed_root`.
//! 2. §5.4 steps 1–6 (orientations), see `orient.rs`.
//! 3. §5.5 steps 2–5 (translations, `under_constrained`, `redundant_constraint`,
//!    `conflicting_constraints`), see `translate.rs`.
//!    If step 2 had to defer a position-dependent direction (Dot `profile` frames,
//!    `segment.axis`, a hole whose `body[0]` is such an `extrude`), steps 1–6 are re-run on the
//!    solved positions with those directions known, and the translations re-solved when an
//!    orientation changed (at most `MAX_ORIENTATION_ROUNDS` rounds).
//! 4. §7.1 hole directions (`hole_direction_conflict`, `hole_axis_mismatch`).
//! 5. §5.4 step 7: every orientation requirement (and every `fixed` frame) re-checked on the
//!    final world directions (`orientation_unsatisfiable`). It runs after translation so that
//!    position-dependent directions (derived `profile`/`extrude` frames, `segment` axes) can
//!    be checked too; constraints touching a component already reported by step 5 are not
//!    reported again.
//! 6. §5.5 step 6: `inside` / `clearance` on resolved `region` AABBs.
//!
//! `Resolved.issues` order: §5.4 (step 5, then step 7), `under_constrained`,
//! redundant/conflicting, `inside`/`clearance` violations, §7.1 hole issues.
//!
//! # Derived components (§3.2, §3.4)
//!
//! Derived components never own unknowns. Their world geometry is computed from sources:
//! `of` = its source Volume; `section` = the source frame shifted to local `z = offset` and
//! centered on the source region's XY center, extent = region XY size; `profile` = closed
//! `polyline` (points on one axis-aligned plane, normal from the counter-clockwise rule,
//! center = bounding-box center, orientation = first canonical orientation whose `z_axis` is
//! the normal) or `circle` (normal `+z`); `extrude` = box table over
//! `extent_u × extent_v × height` in the profile frame; `revolve` = `origin` at the axis
//! segment start and a full-revolution bounding `region` (conservative for `angle < 360`);
//! Lines from their Dots (`arc.center` = circumcenter, `circle.axis` = `+z`). In
//! `Resolved.components` a derived component's entry is its frame (Lines: identity at their
//! first point). For §5.5 knowledge a derived port is known on axis `k` when all its source
//! independents are known on `k`; ports whose coordinates are not axis-separable
//! (`extrude` ports, `arc.center`, `revolve.region`, `section` of an `extrude`) need all
//! three axes of every source. Orientation knowledge is static: `of`, `section`, and `extrude`
//! of a `rect`/`disc`/`section` (any chain of them) carry the orientation of the chain's
//! independent root and drive §5.4 step 5 directly (as if they were that root), Lines and
//! `revolve` are identity (`circle.axis` = `+z`), and `profile`-based frames plus
//! `segment.axis` are position-dependent (second pass, see the pipeline). Derived geometry is
//! memoized per component and evaluated without recursion, so derivation chains of any length
//! (up to the 256-component cap) cost linear time; see `world.rs`.
//!
//! # Limits (spec §13 leaves these open)
//!
//! * `arc` has no plane/normal port; `circle` is assumed to lie in a `+z`-normal plane.
//! * `revolve` exposes only `origin` and `region`.
//! * A hole follows its part's `body[0]` orientation (§5.4 step 3) whether `body[0]` is
//!   independent or derived; a hole with its **own** `fixed` keeps that frame instead (§5.4
//!   step 1 comes first and is unconditional; 1.0 upgrades fix a Dot and attach the hole with
//!   `coincident`, so step 3 applies to them).
//! * `inside` / `clearance` use AABBs, not exact shapes.
//! * `inside` / `clearance` whose `a` or `b` region does not resolve (e.g. the `region` of a
//!   hole no part cuts, which spans to the owning part's bounding box, or of a derived
//!   component with a broken source) produce **no** issue: §9.2 has no code for an
//!   unevaluable check, and the unresolved reference itself is reported by the validator when
//!   it is a structural error.

mod holes;
mod orient;
mod translate;
mod world;

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::model::geom::{AxisDir, Orientation, Vec3, TOL};
use crate::model::issue::codes::Code;
use crate::model::issue::ValidationIssue;
use crate::model::resolved::{PortType, Resolved, ResolvedComponent, ResolvedHole, ResolvedPort};
use crate::model::v2::{CadDocument2, ConstraintKind, EmptyRole};
use crate::model::PortRef;
use crate::ports::{check_constraint_ports, frame_port_ref, principal_dir, ConstraintPorts};

use world::World;

// =============================================================================================
// public types
// =============================================================================================

/// An axis-aligned world placement: `world = orientation · local + origin`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    /// One of the 24 axis-aligned orientations.
    pub orientation: Orientation,
    /// World translation.
    pub origin: Vec3,
}

impl Pose {
    /// Maps a local point to the world.
    pub fn apply(&self, local: Vec3) -> Vec3 {
        crate::model::geom::vadd(self.orientation.apply(local), self.origin)
    }
}

/// An axis-aligned bounding box.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Aabb {
    /// Minimum corner.
    pub min: Vec3,
    /// Maximum corner.
    pub max: Vec3,
}

impl Aabb {
    /// Smallest box containing both.
    pub fn union(&self, other: &Aabb) -> Aabb {
        Aabb {
            min: std::array::from_fn(|i| self.min[i].min(other.min[i])),
            max: std::array::from_fn(|i| self.max[i].max(other.max[i])),
        }
    }

    /// `max − min`.
    pub fn size(&self) -> Vec3 {
        std::array::from_fn(|i| self.max[i] - self.min[i])
    }

    /// Box center.
    pub fn center(&self) -> Vec3 {
        std::array::from_fn(|i| (self.min[i] + self.max[i]) / 2.0)
    }

    /// Whether `other` lies inside `self` (within `tol`).
    pub fn contains(&self, other: &Aabb, tol: f64) -> bool {
        (0..3).all(|i| other.min[i] >= self.min[i] - tol && other.max[i] <= self.max[i] + tol)
    }

    /// Whether the interiors overlap by more than `tol` on every axis.
    pub fn intersects(&self, other: &Aabb, tol: f64) -> bool {
        (0..3).all(|i| self.min[i] < other.max[i] - tol && other.min[i] < self.max[i] - tol)
    }

    /// Euclidean distance between the boxes (0 when they touch or overlap).
    pub fn distance(&self, other: &Aabb) -> f64 {
        (0..3)
            .map(|i| {
                let gap = (other.min[i] - self.max[i])
                    .max(self.min[i] - other.max[i])
                    .max(0.0);
                gap * gap
            })
            .sum::<f64>()
            .sqrt()
    }
}

/// A cylindrical cutter from `start` to `end` (§7.2).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Cutter {
    /// Start point (entry side).
    pub start: Vec3,
    /// End point (along the cut direction).
    pub end: Vec3,
    /// Cylinder diameter.
    pub diameter: f64,
}

/// A conical cutter (countersink) from `start` (outer diameter) to `end` (inner diameter).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ConeCutter {
    /// Apex-side start at the entry (diameter `outer_diameter`); no back-off (§7.2).
    pub start: Vec3,
    /// End along the cut direction (diameter `inner_diameter`).
    pub end: Vec3,
    /// `countersink_diameter`.
    pub outer_diameter: f64,
    /// Hole `diameter`.
    pub inner_diameter: f64,
}

/// Every cutter of one hole (§7.2): the main hole plus optional counterbore / countersink.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HoleCutters {
    /// Inward cut direction (§7.1).
    pub direction: AxisDir,
    /// World entry point.
    pub entry: Vec3,
    /// Main hole: blind = entry − 0.1 → entry + depth + 0.1; other types span the owning
    /// part's bounding box ± 1.0 mm along the direction, laterally at the entry.
    pub main: Cutter,
    /// `counterbore`: entry − 0.1 → + `counterbore_depth + 0.2`.
    pub counterbore: Option<Cutter>,
    /// `countersink`: cone from the entry, height
    /// `(countersink_diameter − diameter) / (2·max(0.01, tan(angle/2)))`.
    pub countersink: Option<ConeCutter>,
}

// =============================================================================================
// solve
// =============================================================================================

/// Upper bound on orientation ↔ translation rounds when some direction depends on positions
/// (see `orient.rs`); every round after the first runs only if the previous one changed an
/// orientation.
const MAX_ORIENTATION_ROUNDS: usize = 3;

/// Whether two world directions satisfy the §5.4 orientation requirement of `kind`
/// (`coaxial`/`distance`: parallel; `mate`: opposite; `flush`: equal; other kinds: always).
pub fn orientation_requirement_holds(kind: ConstraintKind, a: AxisDir, b: AxisDir) -> bool {
    match kind {
        ConstraintKind::Coaxial | ConstraintKind::Distance => a.is_parallel(b),
        ConstraintKind::Mate => a == b.neg(),
        ConstraintKind::Flush => a == b,
        _ => true,
    }
}

/// Solves a document (§5.4, §5.5, §7) into `resolved.json` content.
pub fn solve(doc: &CadDocument2) -> Resolved {
    let mut resolved = Resolved::new(doc.revision);
    let mut world = World::new(doc);
    let n = doc.components.len();

    let any_independent =
        (0..n).any(|ci| world.is_canonical(ci) && world.comp(ci).is_independent());
    if any_independent
        && !doc
            .constraints
            .iter()
            .any(|k| k.kind == ConstraintKind::Fixed)
    {
        resolved.issues.push(ValidationIssue::new(
            Code::NoFixedRoot,
            "no fixed constraint roots the document; positions are not solved",
        ));
        return resolved;
    }

    let checked: Vec<Option<ConstraintPorts>> = doc
        .constraints
        .iter()
        .map(|k| check_constraint_ports(doc, k).ok())
        .collect();

    let mut orient = orient::solve_orientations(&mut world, &checked, false);
    let mut trans = translate::solve_translations(&mut world, &checked);
    if orient.deferred {
        // second pass: position-dependent directions evaluated on the solved positions
        for _ in 0..MAX_ORIENTATION_ROUNDS {
            let before = world.orients().to_vec();
            orient = orient::solve_orientations(&mut world, &checked, true);
            if world.orients() == before.as_slice() {
                break;
            }
            trans = translate::solve_translations(&mut world, &checked);
        }
    }

    // §7.1
    let mut hole_issues = Vec::new();
    for ci in 0..n {
        if !world.is_canonical(ci) {
            continue;
        }
        let Some(hd) = holes::hole_direction(&world, ci) else {
            continue;
        };
        world.hole_dirs[ci] = Some(hd.dir);
        if world.comp(ci).role() != Some(EmptyRole::Cut) {
            continue;
        }
        let id = world.comp(ci).id.to_string();
        if hd.conflict {
            let related: Vec<String> = hd
                .candidates
                .iter()
                .map(|(k, _)| doc.constraints[*k].id.to_string())
                .collect();
            let normals: Vec<&str> = hd.candidates.iter().map(|(_, n)| n.as_str()).collect();
            hole_issues.push(
                ValidationIssue::new(
                    Code::HoleDirectionConflict,
                    format!(
                        "hole {id} entry lies on faces with different normals ({}) via {}",
                        normals.join(", "),
                        join_ids(&related)
                    ),
                )
                .with_target(id.clone())
                .with_related(related),
            );
        }
        if hd.mismatch {
            let axis = world.hole_axis_world(ci).map_or("?", |a| a.axis().as_str());
            hole_issues.push(
                ValidationIssue::new(
                    Code::HoleAxisMismatch,
                    format!(
                        "hole {id} cut direction {} is not parallel to its world axis {axis}",
                        hd.dir
                    ),
                )
                .with_target(id),
            );
        }
    }

    // §5.4 step 7
    let mut orientation_issues = orient.issues;
    let root_failed = |ci: usize| -> bool {
        world
            .sources(ci)
            .iter()
            .any(|&s| orient.failed[orient::root(&world, &orient.fixed_by, s)])
    };
    for (ki, k) in doc.constraints.iter().enumerate() {
        if k.kind != ConstraintKind::Fixed || checked[ki].is_none() {
            continue;
        }
        let (Some(frame), Some(ci)) = (k.frame.as_ref(), world.lookup(k.a.id_str())) else {
            continue;
        };
        if orient.fixed_by[ci] != Some(ki) && frame.orientation() != Some(world.orient(ci)) {
            let cid = world.comp(ci).id.to_string();
            orientation_issues.push(
                ValidationIssue::new(
                    Code::OrientationUnsatisfiable,
                    format!(
                        "fixed {} requires {cid} at (x_axis {}, z_axis {}) but it is at \
                         (x_axis {}, z_axis {})",
                        k.id,
                        frame.x_axis,
                        frame.z_axis,
                        world.orient(ci).x_axis(),
                        world.orient(ci).z_axis()
                    ),
                )
                .with_target(k.id.to_string())
                .with_related([cid]),
            );
        }
    }
    for r in orient::requirements(&world, &checked) {
        if root_failed(r.a.0) || root_failed(r.b.0) {
            continue;
        }
        let da = world.port(r.a.0, r.a.1).as_ref().and_then(principal_dir);
        let db = world.port(r.b.0, r.b.1).as_ref().and_then(principal_dir);
        let (Some(da), Some(db)) = (da, db) else {
            continue;
        };
        if orientation_requirement_holds(r.kind, da, db) {
            continue;
        }
        let k = &doc.constraints[r.k];
        let need = match r.kind {
            ConstraintKind::Mate => "opposite",
            ConstraintKind::Flush => "equal",
            _ => "parallel",
        };
        let mut related = vec![world.comp(r.a.0).id.to_string()];
        let bid = world.comp(r.b.0).id.to_string();
        if !related.contains(&bid) {
            related.push(bid);
        }
        orientation_issues.push(
            ValidationIssue::new(
                Code::OrientationUnsatisfiable,
                format!(
                    "{} {} requires {need} directions but {} is {da} and {} is {db}",
                    k.kind.as_str(),
                    k.id,
                    k.a,
                    k.b.as_ref().map_or("?", |b| b.as_str())
                ),
            )
            .with_target(k.id.to_string())
            .with_related(related),
        );
    }

    // §5.5 step 6
    let check_issues = check_constraints(&world, &checked);

    // ---- assemble resolved.json -------------------------------------------------------------
    resolved.issues.extend(orientation_issues);
    resolved.issues.extend(trans.under);
    resolved.issues.extend(trans.residual);
    resolved.issues.extend(check_issues);
    resolved.issues.extend(hole_issues);

    for (ci, defaulted) in trans.defaulted.into_iter().enumerate() {
        if !world.is_canonical(ci) {
            continue;
        }
        let comp = world.comp(ci);
        if let Some(pose) = world.pose(ci) {
            resolved.components.insert(
                comp.id.clone(),
                ResolvedComponent::new(clean3(pose.origin), pose.orientation, defaulted),
            );
        }
        if let Some(fp) = frame_port_ref(comp) {
            if let Some(p) = world.port_ref(&fp) {
                resolved.ports.insert(fp, clean_port(p));
            }
        }
        if comp.role() == Some(EmptyRole::Cut) {
            if let Some(c) = world.hole_cutters(ci) {
                resolved.holes.insert(
                    comp.id.clone(),
                    ResolvedHole {
                        direction: c.direction,
                        cutter_start: clean3(c.main.start),
                        cutter_end: clean3(c.main.end),
                        diameter: c.main.diameter,
                    },
                );
            }
        }
    }
    for k in &doc.constraints {
        for p in k.ports() {
            if resolved.ports.contains_key(p) {
                continue;
            }
            if let Some(port) = world.port_ref(p) {
                resolved.ports.insert(p.clone(), clean_port(port));
            }
        }
    }
    resolved
}

/// §5.5 step 6: `inside` and `clearance` on world `region` AABBs.
fn check_constraints(
    world: &World<'_>,
    checked: &[Option<ConstraintPorts>],
) -> Vec<ValidationIssue> {
    let mut out = Vec::new();
    for (ki, k) in world.doc.constraints.iter().enumerate() {
        if !k.kind.is_check_only() {
            continue;
        }
        let (Some(ports), Some(b)) = (checked[ki], k.b.as_ref()) else {
            continue;
        };
        if ports.a != PortType::Region || ports.b != Some(PortType::Region) {
            continue;
        }
        let (
            Some(ResolvedPort::Region {
                min: amin,
                max: amax,
            }),
            Some(ResolvedPort::Region {
                min: bmin,
                max: bmax,
            }),
        ) = (world.port_ref(&k.a), world.port_ref(b))
        else {
            continue;
        };
        let (ra, rb) = (
            Aabb {
                min: amin,
                max: amax,
            },
            Aabb {
                min: bmin,
                max: bmax,
            },
        );
        let mut related = vec![k.a.id_str().to_owned()];
        if b.id_str() != k.a.id_str() {
            related.push(b.id_str().to_owned());
        }
        match k.kind {
            ConstraintKind::Inside => {
                // worst protrusion of a outside b, with its direction
                let mut worst = (0.0, AxisDir::PosX);
                for (i, axis) in crate::model::geom::Axis::ALL.iter().enumerate() {
                    let below = rb.min[i] - ra.min[i];
                    let above = ra.max[i] - rb.max[i];
                    if below > worst.0 {
                        worst = (below, axis.negative());
                    }
                    if above > worst.0 {
                        worst = (above, axis.positive());
                    }
                }
                if worst.0 > TOL {
                    out.push(
                        ValidationIssue::new(
                            Code::InsideViolated,
                            format!(
                                "inside {}: {} extends {} mm beyond {} along {}",
                                k.id,
                                k.a,
                                fmt_num(worst.0),
                                b,
                                worst.1
                            ),
                        )
                        .with_target(k.id.to_string())
                        .with_related(related),
                    );
                }
            }
            ConstraintKind::Clearance => {
                let required = k.effective_value().unwrap_or(0.0);
                let actual = ra.distance(&rb);
                if actual < required - TOL {
                    out.push(
                        ValidationIssue::new(
                            Code::ClearanceViolated,
                            format!(
                                "clearance {}: {} and {} are {} mm apart, required {} mm",
                                k.id,
                                k.a,
                                b,
                                fmt_num(actual),
                                fmt_num(required)
                            ),
                        )
                        .with_target(k.id.to_string())
                        .with_related(related),
                    );
                }
            }
            _ => {}
        }
    }
    out
}

// =============================================================================================
// helpers for later waves
// =============================================================================================

/// World geometry queries over a solved document (a [`Resolved`] from [`solve`], or one read
/// back from `resolved.json`). Any port can be resolved, not only those stored in
/// `Resolved.ports`; derived components are recomputed from their sources.
pub struct WorldView<'a> {
    world: World<'a>,
}

impl<'a> WorldView<'a> {
    /// Builds the view. Independent components missing from `resolved.components` have no
    /// geometry (their ports resolve to `None`). Hole directions come from `resolved.holes`,
    /// or are recomputed with §7.1 for holes not listed there.
    pub fn new(doc: &'a CadDocument2, resolved: &Resolved) -> Self {
        let mut world = World::new(doc);
        for (ci, comp) in doc.components.iter().enumerate() {
            if !comp.is_independent() {
                continue;
            }
            match resolved
                .components
                .get(&comp.id)
                .filter(|_| world.is_canonical(ci))
            {
                Some(rc) => {
                    world.set_orient(ci, rc.orientation().unwrap_or(Orientation::IDENTITY));
                    world.set_trans(ci, rc.origin);
                }
                None => world.set_present(ci, false),
            }
        }
        let dirs: Vec<Option<AxisDir>> = (0..doc.components.len())
            .map(|ci| {
                let comp = world.comp(ci);
                comp.as_hole()?;
                resolved
                    .holes
                    .get(&comp.id)
                    .map(|h| h.direction)
                    .or_else(|| holes::hole_direction(&world, ci).map(|h| h.dir))
            })
            .collect();
        world.hole_dirs = dirs;
        WorldView { world }
    }

    /// World geometry of any port reference.
    pub fn port(&self, port: &PortRef) -> Option<ResolvedPort> {
        self.world.port_ref(port).map(clean_port)
    }

    /// World geometry of `id.port`.
    pub fn component_port(&self, id: &str, port: &str) -> Option<ResolvedPort> {
        self.world
            .port(self.world.lookup(id)?, port)
            .map(clean_port)
    }

    /// World pose of a component (derived components: their derived frame).
    pub fn pose(&self, id: &str) -> Option<Pose> {
        self.world.pose(self.world.lookup(id)?)
    }

    /// World AABB of a component's `region` port (`None` if it has none).
    pub fn region(&self, id: &str) -> Option<Aabb> {
        self.world.region(self.world.lookup(id)?)
    }

    /// World bounding box of all `add` components of a part (§7.2).
    pub fn part_bbox(&self, part_id: &str) -> Option<Aabb> {
        let pi = self.world.doc.parts.iter().position(|p| p.id == part_id)?;
        self.world.part_bbox(pi)
    }

    /// The owning part of a hole: the first part whose `body` `cut`s it.
    pub fn hole_part(&self, id: &str) -> Option<&'a str> {
        let pi = self.world.hole_part[self.world.lookup(id)?]?;
        Some(self.world.doc.parts[pi].id.as_str())
    }

    /// Cut direction of a hole (§7.1).
    pub fn hole_direction(&self, id: &str) -> Option<AxisDir> {
        let ci = self.world.lookup(id)?;
        self.world.comp(ci).as_hole()?;
        Some(self.world.hole_dir(ci))
    }

    /// All cutters of a hole (§7.2).
    pub fn hole_cutters(&self, id: &str) -> Option<HoleCutters> {
        self.world.hole_cutters(self.world.lookup(id)?)
    }
}

/// World geometry of any port of a solved document (see [`WorldView`]).
pub fn world_port(doc: &CadDocument2, resolved: &Resolved, port: &PortRef) -> Option<ResolvedPort> {
    WorldView::new(doc, resolved).port(port)
}

/// World AABB of a component's `region` port.
pub fn component_region(doc: &CadDocument2, resolved: &Resolved, id: &str) -> Option<Aabb> {
    WorldView::new(doc, resolved).region(id)
}

/// World bounding box of a part's `add` components.
pub fn part_bbox(doc: &CadDocument2, resolved: &Resolved, part_id: &str) -> Option<Aabb> {
    WorldView::new(doc, resolved).part_bbox(part_id)
}

/// All cutters of a hole.
pub fn hole_cutters(doc: &CadDocument2, resolved: &Resolved, id: &str) -> Option<HoleCutters> {
    WorldView::new(doc, resolved).hole_cutters(id)
}

/// The resolved pose of a component as stored in `resolved.components`.
pub fn resolved_pose(resolved: &Resolved, id: &str) -> Option<Pose> {
    let rc = resolved.components.get(id)?;
    Some(Pose {
        orientation: rc.orientation()?,
        origin: rc.origin,
    })
}

/// All ports of `Resolved.ports` keyed for quick lookup by component id.
pub fn ports_of<'r>(resolved: &'r Resolved, id: &str) -> BTreeMap<&'r str, &'r ResolvedPort> {
    resolved
        .ports
        .iter()
        .filter(|(k, _)| k.id_str() == id)
        .map(|(k, v)| (k.port(), v))
        .collect()
}

// ---- formatting / cleaning ------------------------------------------------------------------

/// `-0.0` → `0.0` so that serialized output never contains negative zeros.
fn clean(x: f64) -> f64 {
    x + 0.0
}

fn clean3(v: Vec3) -> Vec3 {
    v.map(clean)
}

fn clean_port(p: ResolvedPort) -> ResolvedPort {
    match p {
        ResolvedPort::Point { position } => ResolvedPort::Point {
            position: clean3(position),
        },
        ResolvedPort::Axis {
            position,
            direction,
        } => ResolvedPort::Axis {
            position: clean3(position),
            direction,
        },
        ResolvedPort::Face {
            origin,
            normal,
            u,
            v,
            extent,
        } => ResolvedPort::Face {
            origin: clean3(origin),
            normal,
            u,
            v,
            extent: extent.map(clean),
        },
        ResolvedPort::Region { min, max } => ResolvedPort::Region {
            min: clean3(min),
            max: clean3(max),
        },
    }
}

/// A number for issue messages: at most 6 decimals, trailing zeros removed.
pub(crate) fn fmt_num(x: f64) -> String {
    let s = format!("{:.6}", clean(x));
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".to_owned()
    } else {
        s.to_owned()
    }
}

pub(crate) fn join_ids(ids: &[String]) -> String {
    ids.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aabb_and_formatting() {
        let a = Aabb {
            min: [0.0; 3],
            max: [1.0; 3],
        };
        let b = Aabb {
            min: [4.0, 5.0, 0.0],
            max: [5.0, 6.0, 1.0],
        };
        assert_eq!(a.distance(&b), 5.0);
        assert_eq!(a.distance(&a), 0.0);
        assert!(a.union(&b).contains(&b, 0.0));
        assert!(!a.intersects(&b, 0.0));
        assert_eq!(a.center(), [0.5; 3]);
        assert_eq!(fmt_num(1.0), "1");
        assert_eq!(fmt_num(-0.0), "0");
        assert_eq!(fmt_num(0.1 + 0.2), "0.3");
        assert_eq!(fmt_num(-2.5), "-2.5");
        assert_eq!(clean(-0.0).to_bits(), 0.0f64.to_bits());
    }

    #[test]
    fn requirement_rules() {
        use AxisDir::*;
        assert!(orientation_requirement_holds(
            ConstraintKind::Mate,
            NegZ,
            PosZ
        ));
        assert!(!orientation_requirement_holds(
            ConstraintKind::Mate,
            PosZ,
            PosZ
        ));
        assert!(orientation_requirement_holds(
            ConstraintKind::Flush,
            PosZ,
            PosZ
        ));
        assert!(orientation_requirement_holds(
            ConstraintKind::Coaxial,
            NegZ,
            PosZ
        ));
        assert!(!orientation_requirement_holds(
            ConstraintKind::Distance,
            PosX,
            PosZ
        ));
    }
}
