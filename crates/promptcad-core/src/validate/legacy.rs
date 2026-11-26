//! Rules carried over from 1.0 `app/services/validator.py` (`DesignValidator`), re-expressed
//! per component / part id with the same logic and thresholds.
//!
//! * Volume rules (`thin_wall`, `thin_bracket`, `extreme_aspect_ratio`) run on every Volume
//!   in document order; `extreme_aspect_ratio` uses the 1.0 `_dimensions()` of shortcut
//!   Volumes and the local bounding box of derived ones.
//! * Hole rules run per part, on the `cut` hole Empties of its `body` in body order (1.0 hole
//!   order for upgraded documents). A hole is evaluated in the frame of its *host*: the first
//!   `add` Volume whose world region contains the hole axis laterally (fallback `body[0]`);
//!   the entry point and cut direction come from the solver and are expressed in the host's
//!   frame, so an upgraded 1.0 hole reproduces `hole.x/y/z` and `hole.axis` exactly. For a
//!   shortcut host the 1.0 formulas are used verbatim; for a derived host (`extrude`,
//!   `revolve`) its local bounding box stands in for the 1.0 base box (conservative: the box
//!   is a superset, so outside/too-deep verdicts are certain, some inside cases are missed).
//! * Edge rules (`fillet_too_large`, `chamfer_too_large`) use the 1.0 `_dimensions()` of
//!   `body[0]` when it is the only `add` and a shortcut Volume, otherwise the part's bounding
//!   box in `body[0]`'s frame (a superset, so never a false error).
//! * `assumption` (info) per document assumption; the message cites only its index (§9.1).

use crate::model::geom::{vsub, Axis, AxisDir, Vec3, TOL};
use crate::model::issue::codes::Code;
use crate::model::issue::ValidationIssue;
use crate::model::v2::{
    BodyOpKind, CadDocument2, Component, EdgeSelector, EmptyRole, HoleSpec, HoleType, Part,
    VolumeShape,
};
use crate::solver::{fmt_num, Pose, WorldView};

#[derive(Clone, Copy)]
enum Kind {
    Plate,
    Enclosure { wall: f64 },
    LBracket { width: f64, thickness: f64 },
    Cylinder,
    Ring { inner_d: f64 },
    Derived,
}

/// The 1.0 "base" a hole is checked against.
struct Base {
    kind: Kind,
    /// 1.0 `_dimensions()` (or the local bounding-box size).
    dims: [f64; 3],
    /// Local center used by 1.0 `_axis_plane()` (or the local bounding-box center).
    center: [f64; 3],
    pose: Option<Pose>,
}

/// 1.0 `_dimensions()` of a shortcut Volume.
fn shortcut_dims(c: &Component) -> Option<(Kind, [f64; 3])> {
    Some(match c.as_volume()? {
        VolumeShape::Plate(s) => (Kind::Plate, [s.length, s.width, s.thickness]),
        VolumeShape::Cylinder(s) => (Kind::Cylinder, [s.diameter, s.diameter, s.height]),
        VolumeShape::Ring(s) => (
            Kind::Ring {
                inner_d: s.inner_diameter,
            },
            [s.outer_diameter, s.outer_diameter, s.height],
        ),
        VolumeShape::LBracket(s) => (
            Kind::LBracket {
                width: s.width,
                thickness: s.thickness,
            },
            [s.width, s.depth, s.vertical_height + s.thickness],
        ),
        VolumeShape::Enclosure(s) => (
            Kind::Enclosure {
                wall: s.wall_thickness,
            },
            [s.length, s.width, s.height],
        ),
        _ => return None,
    })
}

/// Local bounding box of a component from its world region and pose.
fn local_box(view: &WorldView<'_>, id: &str) -> Option<(Vec3, Vec3, Pose)> {
    let pose = view.pose(id)?;
    let r = view.region(id)?;
    let (a, b) = (
        pose.orientation.apply_inverse(vsub(r.min, pose.origin)),
        pose.orientation.apply_inverse(vsub(r.max, pose.origin)),
    );
    Some((
        std::array::from_fn(|i| a[i].min(b[i])),
        std::array::from_fn(|i| a[i].max(b[i])),
        pose,
    ))
}

fn base_of(view: Option<&WorldView<'_>>, c: &Component) -> Option<Base> {
    let pose = view.and_then(|v| v.pose(c.id.as_str()));
    if let Some((kind, dims)) = shortcut_dims(c) {
        return Some(Base {
            kind,
            dims,
            center: [0.0, 0.0, dims[2] / 2.0],
            pose,
        });
    }
    c.as_volume()?;
    let (lo, hi, pose) = local_box(view?, c.id.as_str())?;
    Some(Base {
        kind: Kind::Derived,
        dims: std::array::from_fn(|i| hi[i] - lo[i]),
        center: std::array::from_fn(|i| (lo[i] + hi[i]) / 2.0),
        pose: Some(pose),
    })
}

fn plane_coords(p: Vec3, axis: Axis) -> (f64, f64) {
    match axis {
        Axis::Z => (p[0], p[1]),
        Axis::X => (p[1], p[2]),
        Axis::Y => (p[0], p[2]),
    }
}

/// All carried-over 1.0 rules, in the order documented in the module docs: Volume rules,
/// then per part (document order) hole rules (per hole: thread, boundary, depth), then
/// overlapping pairs, then fillets and chamfers; then thread rules of cut holes owned by no
/// part; then assumptions.
pub(super) fn check(doc: &CadDocument2, view: Option<&WorldView<'_>>) -> Vec<ValidationIssue> {
    let mut out = Vec::new();
    for (i, c) in doc.components.iter().enumerate() {
        if doc.component_index(c.id.as_str()) == Some(i) && c.as_volume().is_some() {
            volume_rules(view, c, &mut out);
        }
    }
    for part in &doc.parts {
        part_rules(doc, view, part, &mut out);
    }
    for (i, c) in doc.components.iter().enumerate() {
        if doc.component_index(c.id.as_str()) != Some(i) || c.role() != Some(EmptyRole::Cut) {
            continue;
        }
        let Some(h) = c.as_hole() else { continue };
        let owned = doc.parts.iter().any(|p| {
            p.body
                .iter()
                .any(|op| op.op == BodyOpKind::Cut && op.component == c.id)
        });
        if !owned {
            thread_rule(c, h, &mut out);
        }
    }
    for i in 0..doc.assumptions.len() {
        out.push(ValidationIssue::new(
            Code::Assumption,
            format!("assumption {} is recorded; review required", i + 1),
        ));
    }
    out
}

fn volume_rules(view: Option<&WorldView<'_>>, c: &Component, out: &mut Vec<ValidationIssue>) {
    match c.as_volume() {
        Some(VolumeShape::Enclosure(s)) if s.wall_thickness < 0.8 => out.push(
            ValidationIssue::new(
                Code::ThinWall,
                format!(
                    "enclosure {}: wall thickness {} mm is thin; review for material and process",
                    c.id,
                    fmt_num(s.wall_thickness)
                ),
            )
            .with_target(c.id.as_str()),
        ),
        Some(VolumeShape::LBracket(s)) if s.thickness < 2.0 => out.push(
            ValidationIssue::new(
                Code::ThinBracket,
                format!(
                    "l_bracket {}: thickness {} mm is below 2 mm; load capacity may be \
                     insufficient",
                    c.id,
                    fmt_num(s.thickness)
                ),
            )
            .with_target(c.id.as_str()),
        ),
        _ => {}
    }
    let dims = match shortcut_dims(c) {
        Some((_, d)) => Some(d),
        None => view
            .and_then(|v| local_box(v, c.id.as_str()))
            .map(|(lo, hi, _)| std::array::from_fn(|i| hi[i] - lo[i])),
    };
    if let Some(d) = dims {
        let (mx, mn) = (
            d.iter().copied().fold(f64::MIN, f64::max),
            d.iter().copied().fold(f64::MAX, f64::min),
        );
        if mn > 0.0 && mx / mn > 1000.0 {
            out.push(
                ValidationIssue::new(
                    Code::ExtremeAspectRatio,
                    format!(
                        "volume {}: dimension ratio {} exceeds 1000; check the units",
                        c.id,
                        fmt_num(mx / mn)
                    ),
                )
                .with_target(c.id.as_str()),
            );
        }
    }
}

fn thread_rule(c: &Component, h: &HoleSpec, out: &mut Vec<ValidationIssue>) {
    if h.thread.is_some() {
        out.push(
            ValidationIssue::new(
                Code::ThreadAnnotationOnly,
                format!(
                    "hole {}: the thread is an annotation only; it is modelled as a plain \
                     cylinder",
                    c.id
                ),
            )
            .with_target(c.id.as_str()),
        );
    } else if h.hole_type == HoleType::Tapped {
        out.push(
            ValidationIssue::new(
                Code::TappedHoleWithoutThread,
                format!("hole {}: tapped hole without a thread specification", c.id),
            )
            .with_target(c.id.as_str()),
        );
    }
}

/// A placed hole: entry (world) and cut direction (world).
struct Placed<'d> {
    c: &'d Component,
    h: &'d HoleSpec,
    at: Option<(Vec3, AxisDir)>,
}

fn part_rules(
    doc: &CadDocument2,
    view: Option<&WorldView<'_>>,
    part: &Part,
    out: &mut Vec<ValidationIssue>,
) {
    let holes: Vec<Placed<'_>> = part
        .body
        .iter()
        .filter(|op| op.op == BodyOpKind::Cut)
        .filter_map(|op| doc.component(op.component.as_str()))
        .filter(|c| c.role() == Some(EmptyRole::Cut))
        .filter_map(|c| {
            let h = c.as_hole()?;
            let at = view
                .and_then(|v| v.hole_cutters(c.id.as_str()))
                .map(|cut| (cut.entry, cut.direction));
            Some(Placed { c, h, at })
        })
        .collect();
    let adds: Vec<&Component> = part
        .body
        .iter()
        .filter(|op| op.op == BodyOpKind::Add)
        .filter_map(|op| doc.component(op.component.as_str()))
        .filter(|c| c.as_volume().is_some())
        .collect();
    let body0 = part
        .body
        .first()
        .and_then(|op| doc.component(op.component.as_str()))
        .filter(|c| c.as_volume().is_some());

    for ph in &holes {
        thread_rule(ph.c, ph.h, out);
        // host
        let host = match (ph.at, view) {
            (Some((entry, dir)), Some(v)) => {
                let [p, q] = super::solid::radial_axes(dir.index());
                adds.iter()
                    .copied()
                    .find(|a| {
                        v.region(a.id.as_str()).is_some_and(|r| {
                            [p, q]
                                .iter()
                                .all(|&k| entry[k] >= r.min[k] - TOL && entry[k] <= r.max[k] + TOL)
                        })
                    })
                    .or(body0)
            }
            _ => body0,
        };
        let Some(host) = host else { continue };
        let Some(base) = base_of(view, host) else {
            continue;
        };
        // local entry / axis
        let local = match (ph.at, base.pose) {
            (Some((entry, dir)), Some(pose)) => Some((
                pose.orientation.apply_inverse(vsub(entry, pose.origin)),
                pose.orientation.apply_dir_inverse(dir).axis(),
            )),
            _ => None,
        };
        if let Some((e, axis)) = local {
            boundary_rule(ph, &base, e, axis, out);
        }
        let axis = local.map_or(ph.h.axis, |(_, a)| a);
        depth_rule(ph, &base, axis, out);
    }

    // overlapping pairs (same world cut axis)
    for (i, a) in holes.iter().enumerate() {
        for b in &holes[i + 1..] {
            let (Some((ea, da)), Some((eb, db))) = (a.at, b.at) else {
                continue;
            };
            if da.axis() != db.axis() {
                continue;
            }
            let (a1, a2) = plane_coords(ea, da.axis());
            let (b1, b2) = plane_coords(eb, db.axis());
            let distance = (a1 - b1).hypot(a2 - b2);
            let required = (a.h.max_diameter() + b.h.max_diameter()) / 2.0;
            if distance < required {
                out.push(
                    ValidationIssue::new(
                        Code::OverlappingHoles,
                        format!(
                            "holes {} and {} overlap: center distance {} mm < {} mm",
                            a.c.id,
                            b.c.id,
                            fmt_num(distance),
                            fmt_num(required)
                        ),
                    )
                    .with_target(a.c.id.as_str())
                    .with_related([b.c.id.as_str(), part.id.as_str()]),
                );
            }
        }
    }

    // fillets / chamfers
    let dims = edge_dims(view, part, &adds, body0);
    if let Some([dx, dy, dz]) = dims {
        let safe = |sel: EdgeSelector| {
            if sel == EdgeSelector::Vertical {
                dx.min(dy) / 2.0
            } else {
                dx.min(dy).min(dz) / 2.0
            }
        };
        for (i, f) in part.fillets.iter().enumerate() {
            let s = safe(f.selector);
            if f.radius >= s {
                out.push(
                    ValidationIssue::new(
                        Code::FilletTooLarge,
                        format!(
                            "part {}: fillet {} radius {} mm is not below half the available \
                             edge size ({} mm)",
                            part.id,
                            i + 1,
                            fmt_num(f.radius),
                            fmt_num(s)
                        ),
                    )
                    .with_target(part.id.as_str()),
                );
            }
        }
        for (i, c) in part.chamfers.iter().enumerate() {
            let s = safe(c.selector);
            if c.distance >= s {
                out.push(
                    ValidationIssue::new(
                        Code::ChamferTooLarge,
                        format!(
                            "part {}: chamfer {} distance {} mm is not below half the available \
                             edge size ({} mm)",
                            part.id,
                            i + 1,
                            fmt_num(c.distance),
                            fmt_num(s)
                        ),
                    )
                    .with_target(part.id.as_str()),
                );
            }
        }
    }
}

fn edge_dims(
    view: Option<&WorldView<'_>>,
    part: &Part,
    adds: &[&Component],
    body0: Option<&Component>,
) -> Option<[f64; 3]> {
    let body0 = body0?;
    if adds.len() == 1 {
        if let Some((_, d)) = shortcut_dims(body0) {
            return Some(d);
        }
    }
    let v = view?;
    let bb = v.part_bbox(part.id.as_str())?;
    let size = bb.size();
    Some(match v.pose(body0.id.as_str()) {
        Some(pose) => {
            std::array::from_fn(|i| size[pose.orientation.axis_dir(Axis::ALL[i]).index()])
        }
        None => size,
    })
}

fn boundary_rule(
    ph: &Placed<'_>,
    base: &Base,
    e: Vec3,
    axis: Axis,
    out: &mut Vec<ValidationIssue>,
) {
    let id = ph.c.id.as_str();
    let eff = ph.h.max_diameter();
    let radius = eff / 2.0;
    let clearance;
    let round = matches!(base.kind, Kind::Cylinder | Kind::Ring { .. });
    if axis == Axis::Z && round {
        let center_radius = e[0].hypot(e[1]);
        let outer_radius = base.dims[0] / 2.0;
        clearance = outer_radius - center_radius - radius;
        if clearance < 0.0 {
            out.push(
                ValidationIssue::new(
                    Code::HoleOutsidePart,
                    format!("hole {id} extends beyond the circular outline"),
                )
                .with_target(id),
            );
            return;
        }
        if let Kind::Ring { inner_d } = base.kind {
            let inner_radius = inner_d / 2.0;
            if center_radius + radius <= inner_radius {
                out.push(
                    ValidationIssue::new(
                        Code::HoleInInnerVoid,
                        format!("hole {id} lies entirely inside the ring bore"),
                    )
                    .with_target(id),
                );
            } else if center_radius - radius < inner_radius {
                out.push(
                    ValidationIssue::new(
                        Code::HoleIntersectsInnerVoid,
                        format!("hole {id} intersects the ring bore and opens into it"),
                    )
                    .with_target(id),
                );
            }
        }
    } else {
        let [dx, dy, dz] = base.dims;
        let [cx, cy, cz] = base.center;
        let (span_a, span_b, center_a, center_b) = match axis {
            Axis::Z => (dx, dy, cx, cy),
            Axis::X => (dy, dz, cy, cz),
            Axis::Y => (dx, dz, cx, cz),
        };
        let (point_a, point_b) = plane_coords(e, axis);
        let edge_a = span_a / 2.0 - (point_a - center_a).abs() - radius;
        let edge_b = span_b / 2.0 - (point_b - center_b).abs() - radius;
        clearance = edge_a.min(edge_b);
        if clearance < 0.0 {
            out.push(
                ValidationIssue::new(
                    Code::HoleOutsidePart,
                    format!(
                        "hole {id} extends beyond the part outline in the plane normal to {}",
                        axis.as_str().to_uppercase()
                    ),
                )
                .with_target(id),
            );
            return;
        }
    }
    if clearance < 1.0f64.max(eff * 0.25) {
        out.push(
            ValidationIssue::new(
                Code::LowEdgeClearance,
                format!(
                    "hole {id}: minimum edge clearance is only {:.2} mm",
                    clearance + 0.0
                ),
            )
            .with_target(id),
        );
    }
}

fn depth_rule(ph: &Placed<'_>, base: &Base, axis: Axis, out: &mut Vec<ValidationIssue>) {
    let id = ph.c.id.as_str();
    let h = ph.h;
    let material = match base.kind {
        Kind::Enclosure { wall } => wall,
        Kind::LBracket { width, thickness } => {
            if axis == Axis::X {
                width
            } else {
                thickness
            }
        }
        _ => base.dims[axis.index()],
    };
    if let (HoleType::Blind, Some(d)) = (h.hole_type, h.depth) {
        if d >= material {
            out.push(
                ValidationIssue::new(
                    Code::BlindDepthExceedsMaterial,
                    format!(
                        "hole {id}: blind depth {} mm is not below the estimated material \
                         thickness {} mm",
                        fmt_num(d),
                        fmt_num(material)
                    ),
                )
                .with_target(id),
            );
        }
    }
    if let (HoleType::Counterbore, Some(d)) = (h.hole_type, h.counterbore_depth) {
        if d >= material {
            out.push(
                ValidationIssue::new(
                    Code::CounterboreDepthExceedsMaterial,
                    format!(
                        "hole {id}: counterbore depth {} mm is not below the estimated material \
                         thickness {} mm",
                        fmt_num(d),
                        fmt_num(material)
                    ),
                )
                .with_target(id),
            );
        }
    }
    if let (HoleType::Countersink, Some(cd), Some(angle)) =
        (h.hole_type, h.countersink_diameter, h.countersink_angle)
    {
        let depth = (cd - h.diameter) / (2.0 * (angle / 2.0).to_radians().tan().max(0.01));
        if depth >= material {
            out.push(
                ValidationIssue::new(
                    Code::CountersinkDepthExceedsMaterial,
                    format!(
                        "hole {id}: countersink depth {} mm is not below the estimated material \
                         thickness {} mm",
                        fmt_num(depth),
                        fmt_num(material)
                    ),
                )
                .with_target(id),
            );
        }
    }
}
