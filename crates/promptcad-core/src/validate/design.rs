//! Design rules of the "空" and "零件間" groups (§9.2) on solved world geometry.
//!
//! World solids are built from the solver output ([`WorldView`]) as unions of
//! [`Piece`]s (see [`super::solid`]). Shortcut Volumes, Empty `box`/`cylinder`, `extrude` of a
//! `rect`/`disc`/circle profile and hole cutters are exact; `extrude` of other profiles,
//! `revolve` and countersink cones are over-approximated by their bounding primitive, which
//! can only hide issues, never create them.
//!
//! Body semantics are sequential (§6.1): the material contributed by `add` number *i* is that
//! Volume minus every later `cut`, clipped by every later `intersect` (clips by a shape that is
//! not a single exact primitive use its bounding box, an over-approximation).

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::model::geom::{Axis, AxisDir, TOL};
use crate::model::issue::codes::Code;
use crate::model::issue::ValidationIssue;
use crate::model::v2::LineShape;
use crate::model::v2::{
    BodyOpKind, CadDocument2, Component, EmptyRole, EmptyShape, Geometry, Part, PlaneShape,
    VolumeShape,
};
use crate::model::Id;
use crate::solver::{fmt_num, Aabb, Pose, WorldView};

use super::solid::{overlap, projection_covers, radial_axes, Overlap, Piece, Prim, Shape};

// =============================================================================================
// world solids
// =============================================================================================

enum Local {
    Box([f64; 3], [f64; 3]),
    /// Cylinder along local +z through the local origin.
    Cyl(f64, f64, f64),
}

fn world(pose: &Pose, l: &Local) -> Prim {
    match *l {
        Local::Box(lo, hi) => {
            let (a, b) = (pose.apply(lo), pose.apply(hi));
            Prim::Box(Aabb {
                min: std::array::from_fn(|i| a[i].min(b[i])),
                max: std::array::from_fn(|i| a[i].max(b[i])),
            })
        }
        Local::Cyl(r, lo, hi) => {
            let axis = pose.orientation.z_axis().index();
            let (a, b) = (pose.apply([0.0, 0.0, lo]), pose.apply([0.0, 0.0, hi]));
            let [p, q] = radial_axes(axis);
            Prim::Cyl {
                axis,
                c: [a[p], a[q]],
                r,
                lo: a[axis].min(b[axis]),
                hi: a[axis].max(b[axis]),
            }
        }
    }
}

fn circle_diameter(c: &Component) -> Option<f64> {
    match c.as_line()? {
        LineShape::Circle(c) => Some(c.diameter),
        _ => None,
    }
}

fn region_box(b: Aabb) -> Prim {
    Prim::Box(b)
}

/// Local pieces (positive, negatives) of a shortcut Volume or an Empty box/cylinder.
fn local_pieces(c: &Component) -> Option<Vec<(Local, Vec<Local>)>> {
    let bx = |l: f64, w: f64, z0: f64, z1: f64| {
        Local::Box([-l / 2.0, -w / 2.0, z0], [l / 2.0, w / 2.0, z1])
    };
    Some(match &c.geometry {
        Geometry::Volume { shape } => match shape {
            VolumeShape::Plate(s) => vec![(bx(s.length, s.width, 0.0, s.thickness), vec![])],
            VolumeShape::Cylinder(s) => vec![(Local::Cyl(s.diameter / 2.0, 0.0, s.height), vec![])],
            VolumeShape::Ring(s) => vec![(
                Local::Cyl(s.outer_diameter / 2.0, 0.0, s.height),
                vec![Local::Cyl(s.inner_diameter / 2.0, 0.0, s.height)],
            )],
            VolumeShape::LBracket(s) => {
                let (w, d, t, h) = (s.width, s.depth, s.thickness, s.vertical_height);
                vec![
                    (bx(w, d, 0.0, t), vec![]),
                    (
                        Local::Box([-w / 2.0, -d / 2.0, t], [w / 2.0, -d / 2.0 + t, t + h]),
                        vec![],
                    ),
                ]
            }
            VolumeShape::Enclosure(s) => {
                let (l, w, h, t) = (s.length, s.width, s.height, s.wall_thickness);
                let (x0, x1, y0, y1) = (-l / 2.0, l / 2.0, -w / 2.0, w / 2.0);
                vec![
                    (bx(l, w, 0.0, t), vec![]),
                    (Local::Box([x0, y0, t], [x0 + t, y1, h]), vec![]),
                    (Local::Box([x1 - t, y0, t], [x1, y1, h]), vec![]),
                    (Local::Box([x0 + t, y0, t], [x1 - t, y0 + t, h]), vec![]),
                    (Local::Box([x0 + t, y1 - t, t], [x1 - t, y1, h]), vec![]),
                ]
            }
            _ => return None,
        },
        Geometry::Empty { shape, .. } => match shape {
            EmptyShape::Box(s) => vec![(bx(s.length, s.width, 0.0, s.height), vec![])],
            EmptyShape::Cylinder(s) => vec![(Local::Cyl(s.diameter / 2.0, 0.0, s.height), vec![])],
            _ => return None,
        },
        _ => return None,
    })
}

/// World solids of components and parts.
pub(super) struct Geo<'a> {
    pub doc: &'a CadDocument2,
    pub view: &'a WorldView<'a>,
}

impl<'a> Geo<'a> {
    /// World pieces of a component (Volumes, Empties); `None` when it has no solved geometry.
    pub fn pieces(&self, id: &str) -> Option<Vec<Piece>> {
        self.pieces_at(id, 0)
    }

    fn pieces_at(&self, id: &str, depth: usize) -> Option<Vec<Piece>> {
        if depth > 16 {
            return None;
        }
        let c = self.doc.component(id)?;
        if c.is_independent() && c.as_hole().is_none() {
            let pose = self.view.pose(id)?;
            let locals = local_pieces(c)?;
            return Some(
                locals
                    .iter()
                    .map(|(p, negs)| Piece {
                        pos: Shape::exact(world(&pose, p)),
                        negs: negs.iter().map(|n| Shape::exact(world(&pose, n))).collect(),
                    })
                    .collect(),
            );
        }
        match &c.geometry {
            Geometry::Empty {
                shape: EmptyShape::Hole(_),
                ..
            } => {
                let cut = self.view.hole_cutters(id)?;
                let cyl = |start: [f64; 3], end: [f64; 3], d: f64| {
                    let axis = cut.direction.index();
                    let [p, q] = radial_axes(axis);
                    Prim::Cyl {
                        axis,
                        c: [start[p], start[q]],
                        r: d / 2.0,
                        lo: start[axis].min(end[axis]),
                        hi: start[axis].max(end[axis]),
                    }
                };
                let mut out = vec![Piece::solid(Shape::exact(cyl(
                    cut.main.start,
                    cut.main.end,
                    cut.main.diameter,
                )))];
                if let Some(cb) = cut.counterbore {
                    out.push(Piece::solid(Shape::exact(cyl(
                        cb.start,
                        cb.end,
                        cb.diameter,
                    ))));
                }
                if let Some(cs) = cut.countersink {
                    out.push(Piece::solid(Shape::over(cyl(
                        cs.start,
                        cs.end,
                        cs.outer_diameter,
                    ))));
                }
                Some(out)
            }
            Geometry::Empty {
                shape: EmptyShape::Of(of),
                ..
            } => {
                let src = self.doc.component(of.source.as_str())?;
                src.as_volume()?;
                self.pieces_at(of.source.as_str(), depth + 1)
            }
            Geometry::Volume {
                shape: VolumeShape::Extrude(e),
            } => {
                let region = self.view.region(id)?;
                let pose = self.view.pose(id)?;
                let profile = self.doc.component(e.profile.as_str())?;
                let circle_r = match profile.as_plane()? {
                    PlaneShape::Disc(d) => Some(d.diameter / 2.0),
                    PlaneShape::Profile(p) if p.inners.is_empty() => self
                        .doc
                        .component(p.outer.as_str())
                        .and_then(circle_diameter)
                        .map(|d| d / 2.0),
                    _ => None,
                };
                let shape = match profile.as_plane()? {
                    PlaneShape::Rect(_) => Shape::exact(region_box(region)),
                    _ => match circle_r {
                        Some(r) => {
                            let axis = pose.orientation.z_axis().index();
                            let [p, q] = radial_axes(axis);
                            Shape::exact(Prim::Cyl {
                                axis,
                                c: [pose.origin[p], pose.origin[q]],
                                r,
                                lo: region.min[axis],
                                hi: region.max[axis],
                            })
                        }
                        None => Shape::over(region_box(region)),
                    },
                };
                Some(vec![Piece::solid(shape)])
            }
            Geometry::Volume {
                shape: VolumeShape::Revolve(_),
            } => Some(vec![Piece::solid(Shape::over(region_box(
                self.view.region(id)?,
            )))]),
            _ => None,
        }
    }

    /// Solids of a part's body.
    pub fn part(&self, part: &Part) -> PartGeo {
        let mut g = PartGeo {
            adds: Vec::new(),
            cuts: Vec::new(),
            clips: Vec::new(),
            complete: true,
        };
        for (i, op) in part.body.iter().enumerate() {
            let Some(pieces) = self.pieces(op.component.as_str()) else {
                g.complete = false;
                continue;
            };
            match op.op {
                BodyOpKind::Add => g.adds.push((i, op.component.clone(), pieces)),
                BodyOpKind::Cut => g.cuts.push((i, op.component.clone(), pieces)),
                BodyOpKind::Intersect => {
                    let clip = match pieces.as_slice() {
                        [one] => one.as_clip(),
                        _ => Shape::over(Prim::Box(union_aabb(pieces.iter().map(|p| p.pos)))),
                    };
                    g.clips.push((i, clip));
                }
            }
        }
        g
    }
}

/// Solved solids of one part.
pub(super) struct PartGeo {
    /// `(body index, component, pieces)` of `add` ops.
    pub adds: Vec<(usize, Id, Vec<Piece>)>,
    /// `(body index, component, pieces)` of `cut` ops.
    pub cuts: Vec<(usize, Id, Vec<Piece>)>,
    /// `(body index, clip)` of `intersect` ops.
    pub clips: Vec<(usize, Shape)>,
    /// Every body component has solved geometry.
    pub complete: bool,
}

/// One material piece of a part: `add` piece with the later cuts and clips.
pub(super) struct Material<'g> {
    pub component: &'g Id,
    pub piece: &'g Piece,
    pub clips: Vec<Shape>,
    pub cuts: Vec<Shape>,
}

impl Material<'_> {
    /// `[piece, clips…, extra…]` positives and `[own negs, cuts…, extra…]` negatives.
    fn query(&self, extra_pos: &[Shape], extra_neg: &[Shape]) -> Overlap {
        let mut pos = vec![self.piece.pos];
        pos.extend(self.clips.iter().copied());
        pos.extend(extra_pos.iter().copied());
        let mut negs = self.piece.negs.clone();
        negs.extend(self.cuts.iter().copied());
        negs.extend(extra_neg.iter().copied());
        overlap(&pos, &negs)
    }

    fn aabb(&self) -> Aabb {
        self.clips.iter().fold(self.piece.pos.prim.aabb(), |a, c| {
            intersect_aabb(&a, &c.prim.aabb())
        })
    }
}

impl PartGeo {
    pub fn materials(&self) -> Vec<Material<'_>> {
        let mut out = Vec::new();
        for (i, id, pieces) in &self.adds {
            let clips: Vec<Shape> = self
                .clips
                .iter()
                .filter(|(j, _)| j > i)
                .map(|(_, s)| *s)
                .collect();
            let cuts: Vec<Shape> = self
                .cuts
                .iter()
                .filter(|(j, _, _)| j > i)
                .flat_map(|(_, _, ps)| ps.iter().map(Piece::as_neg))
                .collect();
            for piece in pieces {
                out.push(Material {
                    component: id,
                    piece,
                    clips: clips.clone(),
                    cuts: cuts.clone(),
                });
            }
        }
        out
    }
}

fn union_aabb(shapes: impl Iterator<Item = Shape>) -> Aabb {
    shapes
        .map(|s| s.prim.aabb())
        .reduce(|a, b| a.union(&b))
        .unwrap_or(Aabb {
            min: [0.0; 3],
            max: [0.0; 3],
        })
}

fn intersect_aabb(a: &Aabb, b: &Aabb) -> Aabb {
    Aabb {
        min: std::array::from_fn(|i| a.min[i].max(b.min[i])),
        max: std::array::from_fn(|i| a.max[i].min(b.max[i]).max(a.min[i].max(b.min[i]))),
    }
}

// =============================================================================================
// rules
// =============================================================================================

/// Canonical (first-occurrence) Empties with `role`, in document order.
fn empties(doc: &CadDocument2, role: EmptyRole) -> Vec<&Component> {
    doc.components
        .iter()
        .enumerate()
        .filter(|(i, c)| doc.component_index(c.id.as_str()) == Some(*i) && c.role() == Some(role))
        .map(|(_, c)| c)
        .collect()
}

/// The "空" and "零件間" groups, in this order: `reserve_violated`, `keep_out_violated`,
/// `envelope_exceeded`, `empty_cut_no_effect`, `empty_cut_removes_all`, `reserve_overlap`,
/// `reserve_unreachable`, `part_interference`, `floating_part`.
pub(super) fn check(geo: &Geo<'_>) -> Vec<ValidationIssue> {
    let doc = geo.doc;
    let parts: Vec<(&Part, PartGeo)> = doc.parts.iter().map(|p| (p, geo.part(p))).collect();
    let mut out = Vec::new();

    // ---- reserve_violated / keep_out_violated ----
    for (role, code, name) in [
        (EmptyRole::Reserve, Code::ReserveViolated, "reserve"),
        (EmptyRole::KeepOut, Code::KeepOutViolated, "keep_out"),
    ] {
        for e in empties(doc, role) {
            let Some(zone) = geo.pieces(e.id.as_str()) else {
                continue;
            };
            for (part, pg) in &parts {
                if !pg.complete {
                    continue;
                }
                'part: for m in pg.materials() {
                    for z in &zone {
                        if m.query(&[z.pos], &z.negs) == Overlap::Yes {
                            let (depth, dir) = intrusion(&m.aabb(), &z.pos.prim.aabb());
                            out.push(
                                ValidationIssue::new(
                                    code,
                                    format!(
                                        "{name} {} is intruded by part {} (component {}): {} mm \
                                         along {dir}",
                                        e.id,
                                        part.id,
                                        m.component,
                                        fmt_num(depth)
                                    ),
                                )
                                .with_target(e.id.as_str())
                                .with_related([part.id.as_str(), m.component.as_str()]),
                            );
                            break 'part;
                        }
                    }
                }
            }
        }
    }

    // ---- envelope_exceeded ----
    for e in empties(doc, EmptyRole::Envelope) {
        let Some(env) = geo.pieces(e.id.as_str()) else {
            continue;
        };
        let env_negs: Vec<Shape> = env.iter().map(Piece::as_neg).collect();
        let env_box = union_aabb(env.iter().map(|p| p.pos));
        for (part, pg) in &parts {
            if !pg.complete {
                continue;
            }
            for m in pg.materials() {
                if m.query(&[], &env_negs) == Overlap::Yes {
                    let (amount, dir) = protrusion(&m.aabb(), &env_box, &env);
                    out.push(
                        ValidationIssue::new(
                            Code::EnvelopeExceeded,
                            format!(
                                "part {} (component {}) extends {} mm beyond envelope {} {dir}",
                                part.id,
                                m.component,
                                fmt_num(amount),
                                e.id
                            ),
                        )
                        .with_target(e.id.as_str())
                        .with_related([part.id.as_str(), m.component.as_str()]),
                    );
                    break;
                }
            }
        }
    }

    // ---- empty_cut_no_effect ----
    for (part, pg) in &parts {
        if !pg.complete {
            continue;
        }
        for (ci, cid, cut) in &pg.cuts {
            let before: Vec<&Piece> = pg
                .adds
                .iter()
                .filter(|(i, _, _)| i < ci)
                .flat_map(|(_, _, ps)| ps.iter())
                .collect();
            let all_no = before.iter().all(|a| {
                cut.iter().all(|c| {
                    let mut negs = a.negs.clone();
                    negs.extend(c.negs.iter().copied());
                    overlap(&[a.pos, c.pos], &negs) == Overlap::No
                })
            });
            if all_no {
                out.push(
                    ValidationIssue::new(
                        Code::EmptyCutNoEffect,
                        format!(
                            "cut {cid} does not intersect the material of part {}",
                            part.id
                        ),
                    )
                    .with_target(cid.as_str())
                    .with_related([part.id.as_str()]),
                );
            }
        }
    }

    // ---- empty_cut_removes_all ----
    for (part, pg) in &parts {
        if !pg.complete || pg.cuts.is_empty() || pg.adds.is_empty() {
            continue;
        }
        let mats = pg.materials();
        let without_cuts = mats.iter().any(|m| {
            let mut pos = vec![m.piece.pos];
            pos.extend(m.clips.iter().copied());
            overlap(&pos, &m.piece.negs) == Overlap::Yes
        });
        if without_cuts && mats.iter().all(|m| m.query(&[], &[]) == Overlap::No) {
            let cuts: Vec<&str> = pg.cuts.iter().map(|(_, id, _)| id.as_str()).collect();
            out.push(
                ValidationIssue::new(
                    Code::EmptyCutRemovesAll,
                    format!(
                        "part {} has no material left after cutting {}",
                        part.id,
                        cuts.join(", ")
                    ),
                )
                .with_target(part.id.as_str())
                .with_related(cuts),
            );
        }
    }

    // ---- reserve_overlap ----
    let reserves: Vec<(&Component, Option<Vec<Piece>>)> = empties(doc, EmptyRole::Reserve)
        .into_iter()
        .map(|e| (e, geo.pieces(e.id.as_str())))
        .collect();
    for (i, (a, pa)) in reserves.iter().enumerate() {
        for (b, pb) in &reserves[i + 1..] {
            let (Some(pa), Some(pb)) = (pa, pb) else {
                continue;
            };
            let hit = pa.iter().any(|x| {
                pb.iter().any(|y| {
                    let mut negs = x.negs.clone();
                    negs.extend(y.negs.iter().copied());
                    overlap(&[x.pos, y.pos], &negs) == Overlap::Yes
                })
            });
            if hit {
                out.push(
                    ValidationIssue::new(
                        Code::ReserveOverlap,
                        format!("reserve {} overlaps reserve {}", a.id, b.id),
                    )
                    .with_target(a.id.as_str())
                    .with_related([b.id.as_str()]),
                );
            }
        }
    }

    // ---- reserve_unreachable ----
    for (r, pr) in &reserves {
        let Some(pr) = pr else {
            continue;
        };
        let rb = union_aabb(pr.iter().map(|p| p.pos));
        let mut blockers: Vec<&str> = Vec::new();
        let mut all = true;
        for axis in Axis::ALL {
            for positive in [true, false] {
                match blocker(&parts, &rb, axis.index(), positive) {
                    Some(p) => {
                        if !blockers.contains(&p) {
                            blockers.push(p);
                        }
                    }
                    None => all = false,
                }
            }
        }
        if all {
            let order: Vec<&str> = doc
                .parts
                .iter()
                .map(|p| p.id.as_str())
                .filter(|p| blockers.contains(p))
                .collect();
            out.push(
                ValidationIssue::new(
                    Code::ReserveUnreachable,
                    format!(
                        "reserve {} is enclosed on all six sides by {} with no opening",
                        r.id,
                        order.join(", ")
                    ),
                )
                .with_target(r.id.as_str())
                .with_related(order),
            );
        }
    }

    // ---- part_interference ----
    for (i, (pa, ga)) in parts.iter().enumerate() {
        if !ga.complete {
            continue;
        }
        let ma = ga.materials();
        for (pb, gb) in &parts[i + 1..] {
            if !gb.complete || pa.id == pb.id {
                continue;
            }
            let mb = gb.materials();
            let hit = ma.iter().find_map(|x| {
                mb.iter().find_map(|y| {
                    let mut pos = vec![y.piece.pos];
                    pos.extend(y.clips.iter().copied());
                    let mut negs = y.piece.negs.clone();
                    negs.extend(y.cuts.iter().copied());
                    (x.query(&pos, &negs) == Overlap::Yes).then_some((x.component, y.component))
                })
            });
            if let Some((ca, cb)) = hit {
                out.push(
                    ValidationIssue::new(
                        Code::PartInterference,
                        format!(
                            "part {} (component {ca}) intersects part {} (component {cb})",
                            pa.id, pb.id
                        ),
                    )
                    .with_target(pa.id.as_str())
                    .with_related([pb.id.as_str()]),
                );
            }
        }
    }

    // ---- floating_part ----
    out.extend(floating_parts(doc));
    out
}

/// Intrusion depth and direction of `material` into `zone` (both AABBs): the smallest overlap
/// along an axis, directed from the material side into the zone.
fn intrusion(material: &Aabb, zone: &Aabb) -> (f64, AxisDir) {
    let ov = intersect_aabb(material, zone);
    let mut best = (f64::INFINITY, AxisDir::PosX);
    for axis in Axis::ALL {
        let k = axis.index();
        let len = ov.max[k] - ov.min[k];
        if len < best.0 {
            let from_positive = (ov.min[k] + ov.max[k]) > (zone.min[k] + zone.max[k]);
            best = (len, AxisDir::new(axis, !from_positive));
        }
    }
    best
}

/// How far `material` sticks out of an envelope (its AABB `env_box`; radial for a single
/// cylinder envelope). Returns the amount and a fixed-text direction phrase.
fn protrusion(material: &Aabb, env_box: &Aabb, env: &[Piece]) -> (f64, String) {
    let mut worst = (0.0, String::new());
    for axis in Axis::ALL {
        let k = axis.index();
        let above = material.max[k] - env_box.max[k];
        let below = env_box.min[k] - material.min[k];
        if above > worst.0 {
            worst = (above, format!("along {}", axis.positive()));
        }
        if below > worst.0 {
            worst = (below, format!("along {}", axis.negative()));
        }
    }
    if worst.0 <= TOL {
        if let [Piece {
            pos:
                Shape {
                    prim: Prim::Cyl { axis, c, r, .. },
                    ..
                },
            ..
        }] = env
        {
            let [p, q] = radial_axes(*axis);
            let far = |v: f64, a: f64, b: f64| (v - a).abs().max((b - v).abs());
            let d = far(c[0], material.min[p], material.max[p]).hypot(far(
                c[1],
                material.min[q],
                material.max[q],
            ));
            return ((d - r).max(0.0), "radially".to_owned());
        }
        return (worst.0.max(0.0), "outside its boundary".to_owned());
    }
    worst
}

/// A part whose material covers the cross-section of `rb` on the `positive`/negative side of
/// axis `k` (an exact piece without negatives, no later clip, no later cut near the covered
/// slab).
fn blocker<'p>(
    parts: &'p [(&Part, PartGeo)],
    rb: &Aabb,
    k: usize,
    positive: bool,
) -> Option<&'p str> {
    let [p, q] = radial_axes(k);
    let rect = [[rb.min[p], rb.max[p]], [rb.min[q], rb.max[q]]];
    for (part, pg) in parts {
        if !pg.complete {
            continue;
        }
        for m in pg.materials() {
            let s = m.piece.pos;
            if s.fit != super::solid::Fit::Exact || !m.piece.negs.is_empty() || !m.clips.is_empty()
            {
                continue;
            }
            let b = s.prim.aabb();
            let beyond = if positive {
                b.min[k] >= rb.max[k] - TOL
            } else {
                b.max[k] <= rb.min[k] + TOL
            };
            if !beyond || !projection_covers(&s.prim, k, rect) {
                continue;
            }
            // the covering slab must not be pierced by a later cut
            let mut slab = Aabb {
                min: rb.min,
                max: rb.max,
            };
            slab.min[k] = b.min[k];
            slab.max[k] = b.max[k];
            if m.cuts.iter().any(|c| c.prim.aabb().intersects(&slab, TOL)) {
                continue;
            }
            return Some(part.id.as_str());
        }
    }
    None
}

/// `floating_part`: within each top-level assembly tree with ≥ 2 parts, a part none of whose
/// body components is linked — by constraints or shape references, possibly through
/// components that belong to no part (Dots, Lines, Planes, reserved Empties; `envelope`
/// Empties are not traversed) — to a body component of another part of the same tree.
fn floating_parts(doc: &CadDocument2) -> Vec<ValidationIssue> {
    let mut out = Vec::new();
    let index: BTreeMap<&str, usize> = {
        let mut m = BTreeMap::new();
        for (i, c) in doc.components.iter().enumerate() {
            m.entry(c.id.as_str()).or_insert(i);
        }
        m
    };
    let n = doc.components.len();
    let mut adj: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); n];
    let mut link = |a: &str, b: &str| {
        if let (Some(&x), Some(&y)) = (index.get(a), index.get(b)) {
            if x != y {
                adj[x].insert(y);
                adj[y].insert(x);
            }
        }
    };
    for k in &doc.constraints {
        if let Some(b) = &k.b {
            link(k.a.id_str(), b.id_str());
        }
    }
    for c in &doc.components {
        for (_, r) in c.geometry.shape_refs() {
            link(c.id.as_str(), r.as_str());
        }
    }
    // owner part (index into doc.parts) of each component; envelopes are shared, not owned
    let mut owner: Vec<Option<usize>> = vec![None; n];
    for (pi, p) in doc.parts.iter().enumerate() {
        for op in &p.body {
            if let Some(&ci) = index.get(op.component.as_str()) {
                if doc.components[ci].role() != Some(EmptyRole::Envelope) && owner[ci].is_none() {
                    owner[ci] = Some(pi);
                }
            }
        }
    }
    let children: BTreeSet<&str> = doc
        .assemblies
        .iter()
        .flat_map(|a| a.children.iter().map(Id::as_str))
        .collect();
    let mut reported = BTreeSet::new();
    for top in doc
        .assemblies
        .iter()
        .filter(|a| !children.contains(a.id.as_str()))
    {
        // parts of the tree
        let mut tree_parts: BTreeSet<usize> = BTreeSet::new();
        let mut seen = BTreeSet::new();
        let mut stack = vec![top.id.as_str()];
        while let Some(a) = stack.pop() {
            if !seen.insert(a) {
                continue;
            }
            if let Some(asm) = doc.assembly(a) {
                for ch in &asm.children {
                    if let Some(pi) = doc.parts.iter().position(|p| p.id == *ch) {
                        tree_parts.insert(pi);
                    } else {
                        stack.push(ch.as_str());
                    }
                }
            }
        }
        if tree_parts.len() < 2 {
            continue;
        }
        for &pi in &tree_parts {
            let starts: Vec<usize> = (0..n).filter(|&ci| owner[ci] == Some(pi)).collect();
            let mut visited: BTreeSet<usize> = starts.iter().copied().collect();
            let mut queue: VecDeque<usize> = starts.into_iter().collect();
            let mut joined = false;
            while let Some(ci) = queue.pop_front() {
                for &nb in &adj[ci] {
                    if !visited.insert(nb) {
                        continue;
                    }
                    if doc.components[nb].role() == Some(EmptyRole::Envelope) {
                        continue;
                    }
                    match owner[nb] {
                        Some(o) if o != pi => {
                            if tree_parts.contains(&o) {
                                joined = true;
                            }
                        }
                        _ => queue.push_back(nb),
                    }
                }
                if joined {
                    break;
                }
            }
            if !joined && reported.insert(pi) {
                let p = &doc.parts[pi];
                out.push(
                    ValidationIssue::new(
                        Code::FloatingPart,
                        format!(
                            "part {} is not joined to any other part of assembly {}",
                            p.id, top.id
                        ),
                    )
                    .with_target(p.id.as_str())
                    .with_related([top.id.as_str()]),
                );
            }
        }
    }
    out
}
