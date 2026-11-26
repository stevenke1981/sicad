//! Frozen port tables and local port geometry (spec §3.4), plus the §5.3 port compatibility
//! rules as reusable checks.
//!
//! Ports are derived from `shape.kind`; agents can only reference them. This module answers
//! three questions without solving anything:
//!
//! * **Which ports exist and what type are they?** [`port_table`], [`port_names`],
//!   [`shape_port_type`], and the document-aware [`port_type`] (which resolves Empty `of`
//!   through its source Volume and distinguishes a dangling component id from an unknown port
//!   name, see [`PortError`]).
//! * **Is a constraint's port usage legal?** [`compatible_kinds`], [`is_compatible`],
//!   [`check_fixed_target`] and [`check_constraint_ports`], whose errors map to the §9.2
//!   codes `dangling_reference`, `unknown_port`, `port_type_mismatch` and
//!   `invalid_fixed_target` (see [`ConstraintPortError::to_issue`]).
//! * **Where is a port in component-local coordinates?** [`local_port`] for independent
//!   components, [`transform_port`] to move it into the world with an axis-aligned pose.
//!   World geometry of derived components (Lines, `profile`, `section`, `extrude`, `revolve`,
//!   `of`) depends on solved source positions and lives in [`crate::solver`].
//!
//! Local geometry is returned as a [`ResolvedPort`] (the same shape as `resolved.json`
//! entries), expressed in the component's own frame.

use std::fmt;

use crate::model::geom::{vadd, AxisDir, Orientation, Vec3};
use crate::model::ids::{Id, PortRef};
use crate::model::issue::codes::Code;
use crate::model::issue::ValidationIssue;
use crate::model::resolved::{PortType, ResolvedPort};
use crate::model::v2::{
    CadDocument2, Component, ConstraintKind, EmptyShape, Geometry, PlaneShape, VolumeShape,
};
use crate::model::{Constraint, ShapeKind};

use PortType::{Axis as A, Face as F, Point as P, Region as R};

// =============================================================================================
// port tables (§3.4)
// =============================================================================================

/// One port-table row: `(port name, port type)`.
pub type PortEntry = (&'static str, PortType);

/// Box-like shapes: Volume `plate`, `enclosure`, `l_bracket`, `extrude`, Empty `box`.
pub const BOX_PORTS: &[PortEntry] = &[
    ("origin", P),
    ("center", P),
    ("axis_z", A),
    ("top", F),
    ("bottom", F),
    ("x_pos", F),
    ("x_neg", F),
    ("y_pos", F),
    ("y_neg", F),
    ("region", R),
];

/// Volume `enclosure`: box ports plus `cavity_floor`, `cavity`.
pub const ENCLOSURE_PORTS: &[PortEntry] = &[
    ("origin", P),
    ("center", P),
    ("axis_z", A),
    ("top", F),
    ("bottom", F),
    ("x_pos", F),
    ("x_neg", F),
    ("y_pos", F),
    ("y_neg", F),
    ("region", R),
    ("cavity_floor", F),
    ("cavity", R),
];

/// Volume `l_bracket`: box ports (of the bounding box) plus `base_top`, `upright_front`.
pub const L_BRACKET_PORTS: &[PortEntry] = &[
    ("origin", P),
    ("center", P),
    ("axis_z", A),
    ("top", F),
    ("bottom", F),
    ("x_pos", F),
    ("x_neg", F),
    ("y_pos", F),
    ("y_neg", F),
    ("region", R),
    ("base_top", F),
    ("upright_front", F),
];

/// Cylinder-like shapes: Volume `cylinder`, Empty `cylinder`.
pub const CYLINDER_PORTS: &[PortEntry] = &[
    ("origin", P),
    ("center", P),
    ("axis", A),
    ("top", F),
    ("bottom", F),
    ("region", R),
];

/// Volume `ring`: cylinder ports plus `bore`.
pub const RING_PORTS: &[PortEntry] = &[
    ("origin", P),
    ("center", P),
    ("axis", A),
    ("top", F),
    ("bottom", F),
    ("region", R),
    ("bore", R),
];

/// Empty `hole`.
pub const HOLE_PORTS: &[PortEntry] = &[("entry", P), ("axis", A), ("region", R)];

/// Dot `point`.
pub const DOT_PORTS: &[PortEntry] = &[("point", P)];

/// Plane `rect`, `disc`, `profile`, `section`.
pub const PLANE_PORTS: &[PortEntry] = &[("center", P), ("normal", A), ("face", F)];

/// Line `segment`.
pub const SEGMENT_PORTS: &[PortEntry] = &[("start", P), ("end", P), ("mid", P), ("axis", A)];

/// Line `polyline`.
pub const POLYLINE_PORTS: &[PortEntry] = &[("start", P), ("end", P)];

/// Line `arc`.
pub const ARC_PORTS: &[PortEntry] = &[("start", P), ("end", P), ("center", P)];

/// Line `circle`.
pub const CIRCLE_PORTS: &[PortEntry] = &[("center", P), ("axis", A)];

/// Volume `revolve` (§3.4: only `origin` and `region`; the full table is deferred, §13).
pub const REVOLVE_PORTS: &[PortEntry] = &[("origin", P), ("region", R)];

/// The static port table of a shape kind, in spec table order.
///
/// Empty `of` has no table of its own (it uses its source Volume's table); this returns an
/// empty slice for it — use [`component_port_table`] to resolve it.
pub fn port_table(shape: ShapeKind) -> &'static [PortEntry] {
    use ShapeKind::*;
    match shape {
        VolumePlate | EmptyBox | VolumeExtrude => BOX_PORTS,
        VolumeEnclosure => ENCLOSURE_PORTS,
        VolumeLBracket => L_BRACKET_PORTS,
        VolumeCylinder | EmptyCylinder => CYLINDER_PORTS,
        VolumeRing => RING_PORTS,
        EmptyHole => HOLE_PORTS,
        EmptyOf => &[],
        DotPoint => DOT_PORTS,
        PlaneRect | PlaneDisc | PlaneProfile | PlaneSection => PLANE_PORTS,
        LineSegment => SEGMENT_PORTS,
        LinePolyline => POLYLINE_PORTS,
        LineArc => ARC_PORTS,
        LineCircle => CIRCLE_PORTS,
        VolumeRevolve => REVOLVE_PORTS,
    }
}

/// Port names of a shape kind, in table order (empty for Empty `of`, see [`port_table`]).
pub fn port_names(shape: ShapeKind) -> Vec<&'static str> {
    port_table(shape).iter().map(|(n, _)| *n).collect()
}

/// Type of `port` on a shape kind; `None` when the shape has no such port (or is `of`).
pub fn shape_port_type(shape: ShapeKind, port: &str) -> Option<PortType> {
    port_table(shape)
        .iter()
        .find(|(n, _)| *n == port)
        .map(|(_, t)| *t)
}

/// The frame port of a shape (§5.3): `origin` for box/cylinder-like shapes, `point` for Dots,
/// `entry` for holes, `center` for `rect`/`disc`; `None` for derived components.
pub fn frame_port(shape: ShapeKind) -> Option<&'static str> {
    shape.frame_port()
}

/// The frame port of `component` as a [`PortRef`] (`None` for derived components).
pub fn frame_port_ref(component: &Component) -> Option<PortRef> {
    let name = frame_port(component.shape_kind())?;
    let port = Id::new(name).ok()?;
    Some(component.id.port(&port))
}

// =============================================================================================
// document-aware lookup
// =============================================================================================

/// Why a port reference does not resolve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortError {
    /// No component has the referenced id (`dangling_reference`).
    DanglingReference {
        /// The offending reference.
        port: PortRef,
    },
    /// The component exists but its shape has no such port (`unknown_port`).
    UnknownPort {
        /// The offending reference.
        port: PortRef,
        /// The component's shape.
        shape: ShapeKind,
    },
    /// The component is an Empty `of` whose `source` is missing or not a Volume, so its port
    /// table is undefined (`dangling_reference`; the validator also reports the shape
    /// reference itself, as `dangling_reference` or `invalid_promotion`).
    UnresolvedSource {
        /// The offending reference.
        port: PortRef,
        /// The `of` component's `source`.
        source: Id,
    },
}

impl PortError {
    /// The §9.2 code.
    pub fn code(&self) -> Code {
        match self {
            PortError::DanglingReference { .. } | PortError::UnresolvedSource { .. } => {
                Code::DanglingReference
            }
            PortError::UnknownPort { .. } => Code::UnknownPort,
        }
    }

    /// The offending reference.
    pub fn port(&self) -> &PortRef {
        match self {
            PortError::DanglingReference { port }
            | PortError::UnknownPort { port, .. }
            | PortError::UnresolvedSource { port, .. } => port,
        }
    }
}

impl fmt::Display for PortError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PortError::DanglingReference { port } => {
                write!(
                    f,
                    "port {port} references unknown component {}",
                    port.id_str()
                )
            }
            PortError::UnknownPort { port, shape } => write!(
                f,
                "port {} is not in the port table of shape {}",
                port,
                shape.shape_name()
            ),
            PortError::UnresolvedSource { port, source } => write!(
                f,
                "port {port} cannot be resolved: of source {source} is not a Volume"
            ),
        }
    }
}

impl std::error::Error for PortError {}

/// The first component with `id` (document order).
fn find<'d>(doc: &'d CadDocument2, id: &str) -> Option<&'d Component> {
    doc.component(id)
}

/// The port table of a component; Empty `of` resolves to its source Volume's table.
///
/// `port` is only used to build the error value.
pub fn component_port_table(
    doc: &CadDocument2,
    component: &Component,
    port: &PortRef,
) -> Result<&'static [PortEntry], PortError> {
    match &component.geometry {
        Geometry::Empty {
            shape: EmptyShape::Of(of),
            ..
        } => match find(doc, of.source.as_str()) {
            // an `of` source must be a Volume, and a Volume is never itself an `of`
            Some(src) if src.as_volume().is_some() => Ok(port_table(src.shape_kind())),
            _ => Err(PortError::UnresolvedSource {
                port: port.clone(),
                source: of.source.clone(),
            }),
        },
        _ => Ok(port_table(component.shape_kind())),
    }
}

/// Port names of a document component (resolving Empty `of`); empty when unresolvable.
pub fn component_port_names(doc: &CadDocument2, component: &Component) -> Vec<&'static str> {
    let dummy = frame_dummy(component);
    component_port_table(doc, component, &dummy)
        .map(|t| t.iter().map(|(n, _)| *n).collect())
        .unwrap_or_default()
}

fn frame_dummy(component: &Component) -> PortRef {
    // any syntactically valid ref works; it is only carried in the error value
    component.id.port(&component.id)
}

/// Resolves the type of a port reference against the document (§3.4, §9.2).
pub fn port_type(doc: &CadDocument2, port: &PortRef) -> Result<PortType, PortError> {
    let component = find(doc, port.id_str())
        .ok_or_else(|| PortError::DanglingReference { port: port.clone() })?;
    let table = component_port_table(doc, component, port)?;
    table
        .iter()
        .find(|(n, _)| *n == port.port())
        .map(|(_, t)| *t)
        .ok_or_else(|| PortError::UnknownPort {
            port: port.clone(),
            shape: component.shape_kind(),
        })
}

// =============================================================================================
// §5.3 compatibility
// =============================================================================================

/// Constraint kinds allowed between an `a` port of type `a` and a `b` port of type `b`
/// (§5.3 matrix; `fixed` is never listed — it has no `b`).
pub fn compatible_kinds(a: PortType, b: PortType) -> &'static [ConstraintKind] {
    use ConstraintKind::*;
    match (a, b) {
        (P, P) => &[Coincident],
        (P, F) => &[OnFace, Distance],
        (A, A) => &[Coaxial],
        (F, F) => &[Mate, Flush, Distance],
        (R, R) => &[Inside, Clearance],
        _ => &[],
    }
}

/// Whether `kind` is allowed between ports of types `a` and `b` (§5.3).
pub fn is_compatible(kind: ConstraintKind, a: PortType, b: PortType) -> bool {
    compatible_kinds(a, b).contains(&kind)
}

/// Whether a constraint with these port types carries a §5.4 orientation requirement
/// (`coaxial`, `mate`, `flush`, face–face `distance`).
pub fn has_orientation_requirement(kind: ConstraintKind, a: PortType, b: PortType) -> bool {
    match kind {
        ConstraintKind::Coaxial | ConstraintKind::Mate | ConstraintKind::Flush => true,
        ConstraintKind::Distance => a == F && b == F,
        _ => false,
    }
}

/// Why a constraint's ports are not usable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConstraintPortError {
    /// `a` or `b` does not resolve.
    Port(PortError),
    /// A non-`fixed` constraint without `b` (a schema error; reported here as
    /// `port_type_mismatch` so that callers never have to unwrap `b`).
    MissingB,
    /// The port types are not allowed for this kind (§5.3, `port_type_mismatch`).
    TypeMismatch {
        /// Constraint kind.
        kind: ConstraintKind,
        /// Type of `a`.
        a: PortType,
        /// Type of `b`.
        b: PortType,
    },
    /// `fixed.a` is not the frame port of an independent component (`invalid_fixed_target`).
    InvalidFixedTarget {
        /// The `a` reference.
        port: PortRef,
    },
}

impl ConstraintPortError {
    /// The §9.2 code.
    pub fn code(&self) -> Code {
        match self {
            ConstraintPortError::Port(e) => e.code(),
            ConstraintPortError::MissingB | ConstraintPortError::TypeMismatch { .. } => {
                Code::PortTypeMismatch
            }
            ConstraintPortError::InvalidFixedTarget { .. } => Code::InvalidFixedTarget,
        }
    }

    /// An analytic issue for constraint `constraint_id` (owner left to the caller via
    /// [`ValidationIssue::for_submitter`]).
    pub fn to_issue(&self, constraint_id: &str) -> ValidationIssue {
        let issue =
            ValidationIssue::new(self.code(), format!("constraint {constraint_id}: {self}"))
                .with_target(constraint_id);
        match self {
            ConstraintPortError::Port(e) => issue.with_related([e.port().id_str()]),
            ConstraintPortError::InvalidFixedTarget { port } => issue.with_related([port.id_str()]),
            _ => issue,
        }
    }
}

fn type_name(t: PortType) -> &'static str {
    match t {
        P => "point",
        A => "axis",
        F => "face",
        R => "region",
    }
}

impl fmt::Display for ConstraintPortError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConstraintPortError::Port(e) => e.fmt(f),
            ConstraintPortError::MissingB => f.write_str("b is required"),
            ConstraintPortError::TypeMismatch { kind, a, b } => write!(
                f,
                "{} does not accept {}-{} ports",
                kind.as_str(),
                type_name(*a),
                type_name(*b)
            ),
            ConstraintPortError::InvalidFixedTarget { port } => write!(
                f,
                "fixed target {port} is not the frame port of an independent component"
            ),
        }
    }
}

impl std::error::Error for ConstraintPortError {}

/// Checks that `port` is the frame port of an independent component (§5.3 `fixed.a`).
pub fn check_fixed_target(doc: &CadDocument2, port: &PortRef) -> Result<(), ConstraintPortError> {
    port_type(doc, port).map_err(ConstraintPortError::Port)?;
    let component = find(doc, port.id_str()).ok_or_else(|| {
        ConstraintPortError::Port(PortError::DanglingReference { port: port.clone() })
    })?;
    if component.is_independent() && frame_port(component.shape_kind()) == Some(port.port()) {
        Ok(())
    } else {
        Err(ConstraintPortError::InvalidFixedTarget { port: port.clone() })
    }
}

/// Resolved port types of a constraint that passed [`check_constraint_ports`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConstraintPorts {
    /// Type of `a`.
    pub a: PortType,
    /// Type of `b` (`None` exactly for `fixed`).
    pub b: Option<PortType>,
}

/// All §5.3 / reference checks for one constraint's ports: `a`/`b` resolve
/// (`dangling_reference`, `unknown_port`), `fixed.a` is a frame port
/// (`invalid_fixed_target`), and the pair is allowed for the kind (`port_type_mismatch`).
///
/// Errors are returned in order: `a`, then `b`, then the pair check (only when both resolve).
/// For `fixed`, `b` is ignored (a non-null `b` is a schema error).
pub fn check_constraint_ports(
    doc: &CadDocument2,
    constraint: &Constraint,
) -> Result<ConstraintPorts, Vec<ConstraintPortError>> {
    if constraint.kind == ConstraintKind::Fixed {
        let a = port_type(doc, &constraint.a).map_err(|e| vec![ConstraintPortError::Port(e)])?;
        check_fixed_target(doc, &constraint.a).map_err(|e| vec![e])?;
        return Ok(ConstraintPorts { a, b: None });
    }
    let mut errors = Vec::new();
    let a = port_type(doc, &constraint.a)
        .map_err(|e| errors.push(ConstraintPortError::Port(e)))
        .ok();
    let b = match &constraint.b {
        None => {
            errors.push(ConstraintPortError::MissingB);
            None
        }
        Some(b) => port_type(doc, b)
            .map_err(|e| errors.push(ConstraintPortError::Port(e)))
            .ok(),
    };
    if let (Some(a), Some(b)) = (a, b) {
        if !is_compatible(constraint.kind, a, b) {
            errors.push(ConstraintPortError::TypeMismatch {
                kind: constraint.kind,
                a,
                b,
            });
        }
    }
    match (a, b) {
        (Some(a), Some(b)) if errors.is_empty() => Ok(ConstraintPorts { a, b: Some(b) }),
        _ => Err(errors),
    }
}

// =============================================================================================
// local geometry (§3.4)
// =============================================================================================

const PX: AxisDir = AxisDir::PosX;
const NX: AxisDir = AxisDir::NegX;
const PY: AxisDir = AxisDir::PosY;
const NY: AxisDir = AxisDir::NegY;
const PZ: AxisDir = AxisDir::PosZ;
const NZ: AxisDir = AxisDir::NegZ;

fn point(position: Vec3) -> ResolvedPort {
    ResolvedPort::Point { position }
}

fn axis(position: Vec3, direction: AxisDir) -> ResolvedPort {
    ResolvedPort::Axis {
        position,
        direction,
    }
}

fn face(origin: Vec3, normal: AxisDir, u: AxisDir, v: AxisDir, extent: [f64; 2]) -> ResolvedPort {
    ResolvedPort::Face {
        origin,
        normal,
        u,
        v,
        extent,
    }
}

fn region(min: Vec3, max: Vec3) -> ResolvedPort {
    ResolvedPort::Region { min, max }
}

/// Local box-port geometry for `L × W × H` (origin at the bottom-face center), §3.4 table.
pub fn box_port(l: f64, w: f64, h: f64, port: &str) -> Option<ResolvedPort> {
    Some(match port {
        "origin" => point([0.0, 0.0, 0.0]),
        "center" => point([0.0, 0.0, h / 2.0]),
        "axis_z" => axis([0.0, 0.0, 0.0], PZ),
        "top" => face([0.0, 0.0, h], PZ, PX, PY, [l, w]),
        "bottom" => face([0.0, 0.0, 0.0], NZ, PX, NY, [l, w]),
        "x_pos" => face([l / 2.0, 0.0, h / 2.0], PX, PY, PZ, [w, h]),
        "x_neg" => face([-l / 2.0, 0.0, h / 2.0], NX, NY, PZ, [w, h]),
        "y_pos" => face([0.0, w / 2.0, h / 2.0], PY, NX, PZ, [l, h]),
        "y_neg" => face([0.0, -w / 2.0, h / 2.0], NY, PX, PZ, [l, h]),
        "region" => region([-l / 2.0, -w / 2.0, 0.0], [l / 2.0, w / 2.0, h]),
        _ => return None,
    })
}

/// Local cylinder-port geometry for diameter `d`, height `h`, §3.4 table.
pub fn cylinder_port(d: f64, h: f64, port: &str) -> Option<ResolvedPort> {
    Some(match port {
        "origin" => point([0.0, 0.0, 0.0]),
        "center" => point([0.0, 0.0, h / 2.0]),
        "axis" => axis([0.0, 0.0, 0.0], PZ),
        "top" => face([0.0, 0.0, h], PZ, PX, PY, [d, d]),
        "bottom" => face([0.0, 0.0, 0.0], NZ, PX, NY, [d, d]),
        "region" => region([-d / 2.0, -d / 2.0, 0.0], [d / 2.0, d / 2.0, h]),
        _ => return None,
    })
}

/// Local plane-port geometry (`rect`, `disc`, `profile`, `section`) with face `extent`
/// `[along u, along v]`.
///
/// §3.4 gives no extent for plane faces; this uses `[length, width]` for `rect`,
/// `[diameter, diameter]` for `disc`, and the bounding-box extent for derived planes.
pub fn plane_port(extent: [f64; 2], port: &str) -> Option<ResolvedPort> {
    Some(match port {
        "center" => point([0.0, 0.0, 0.0]),
        "normal" => axis([0.0, 0.0, 0.0], PZ),
        "face" => face([0.0, 0.0, 0.0], PZ, PX, PY, extent),
        _ => return None,
    })
}

/// `(L, W, H)` used for the box-port table of a box-like independent component (§3.4;
/// `l_bracket` uses its bounding box `W × D × (h + t)`, like 1.0 `_extents()`).
pub fn box_dims(component: &Component) -> Option<(f64, f64, f64)> {
    match &component.geometry {
        Geometry::Volume { shape } => match shape {
            VolumeShape::Plate(s) => Some((s.length, s.width, s.thickness)),
            VolumeShape::Enclosure(s) => Some((s.length, s.width, s.height)),
            VolumeShape::LBracket(s) => Some((s.width, s.depth, s.vertical_height + s.thickness)),
            _ => None,
        },
        Geometry::Empty {
            shape: EmptyShape::Box(s),
            ..
        } => Some((s.length, s.width, s.height)),
        _ => None,
    }
}

/// `(D, H)` of a cylinder-like independent component (`ring` uses `outer_diameter`).
pub fn cylinder_dims(component: &Component) -> Option<(f64, f64)> {
    match &component.geometry {
        Geometry::Volume { shape } => match shape {
            VolumeShape::Cylinder(s) => Some((s.diameter, s.height)),
            VolumeShape::Ring(s) => Some((s.outer_diameter, s.height)),
            _ => None,
        },
        Geometry::Empty {
            shape: EmptyShape::Cylinder(s),
            ..
        } => Some((s.diameter, s.height)),
        _ => None,
    }
}

/// Local geometry of `port` on an **independent** component, in its own frame (§3.4).
///
/// Returns `None` for unknown ports, for derived components (their geometry is derived from
/// solved sources, see [`crate::solver`]), and for the hole `region` (it spans to the far
/// boundary of the owning part's bounding box). The hole `axis` is returned with its default
/// direction `−shape.axis` (§7.1 step 3); the solved direction may differ when the entry is
/// placed `on_face`.
pub fn local_port(component: &Component, port: &str) -> Option<ResolvedPort> {
    match &component.geometry {
        Geometry::Volume { shape } => match shape {
            VolumeShape::Plate(s) => box_port(s.length, s.width, s.thickness, port),
            VolumeShape::Enclosure(s) => {
                let (l, w, h, t) = (s.length, s.width, s.height, s.wall_thickness);
                match port {
                    "cavity_floor" => {
                        Some(face([0.0, 0.0, t], PZ, PX, PY, [l - 2.0 * t, w - 2.0 * t]))
                    }
                    "cavity" => Some(region(
                        [-l / 2.0 + t, -w / 2.0 + t, t],
                        [l / 2.0 - t, w / 2.0 - t, h],
                    )),
                    _ => box_port(l, w, h, port),
                }
            }
            VolumeShape::LBracket(s) => {
                let (w, d, h, t) = (s.width, s.depth, s.vertical_height, s.thickness);
                match port {
                    "base_top" => Some(face([0.0, t / 2.0, t], PZ, PX, PY, [w, d - t])),
                    "upright_front" => {
                        Some(face([0.0, -d / 2.0 + t, t + h / 2.0], PY, NX, PZ, [w, h]))
                    }
                    _ => box_port(w, d, h + t, port),
                }
            }
            VolumeShape::Cylinder(s) => cylinder_port(s.diameter, s.height, port),
            VolumeShape::Ring(s) => match port {
                "bore" => {
                    let r = s.inner_diameter / 2.0;
                    Some(region([-r, -r, 0.0], [r, r, s.height]))
                }
                _ => cylinder_port(s.outer_diameter, s.height, port),
            },
            VolumeShape::Extrude(_) | VolumeShape::Revolve(_) => None,
        },
        Geometry::Empty { shape, .. } => match shape {
            EmptyShape::Box(s) => box_port(s.length, s.width, s.height, port),
            EmptyShape::Cylinder(s) => cylinder_port(s.diameter, s.height, port),
            EmptyShape::Hole(h) => match port {
                "entry" => Some(point([0.0, 0.0, 0.0])),
                "axis" => Some(axis([0.0, 0.0, 0.0], h.axis.negative())),
                _ => None,
            },
            EmptyShape::Of(_) => None,
        },
        Geometry::Dot { .. } => (port == "point").then(|| point([0.0, 0.0, 0.0])),
        Geometry::Plane { shape } => match shape {
            PlaneShape::Rect(s) => plane_port([s.length, s.width], port),
            PlaneShape::Disc(s) => plane_port([s.diameter, s.diameter], port),
            PlaneShape::Profile(_) | PlaneShape::Section(_) => None,
        },
        Geometry::Line { .. } => None,
    }
}

/// Moves a local port into the world: `R · local + t` for positions, `R · d` for directions;
/// regions are re-boxed (an axis-aligned rotation maps a box to a box).
pub fn transform_port(port: &ResolvedPort, orientation: Orientation, t: Vec3) -> ResolvedPort {
    let pos = |p: &Vec3| vadd(orientation.apply(*p), t);
    match port {
        ResolvedPort::Point { position } => point(pos(position)),
        ResolvedPort::Axis {
            position,
            direction,
        } => axis(pos(position), orientation.apply_dir(*direction)),
        ResolvedPort::Face {
            origin,
            normal,
            u,
            v,
            extent,
        } => face(
            pos(origin),
            orientation.apply_dir(*normal),
            orientation.apply_dir(*u),
            orientation.apply_dir(*v),
            *extent,
        ),
        ResolvedPort::Region { min, max } => {
            let (a, b) = (pos(min), pos(max));
            region(
                std::array::from_fn(|i| a[i].min(b[i])),
                std::array::from_fn(|i| a[i].max(b[i])),
            )
        }
    }
}

/// The anchor position used by translation equations: `position` of points and axes, `origin`
/// (center) of faces; `None` for regions.
pub fn anchor(port: &ResolvedPort) -> Option<Vec3> {
    match port {
        ResolvedPort::Point { position } | ResolvedPort::Axis { position, .. } => Some(*position),
        ResolvedPort::Face { origin, .. } => Some(*origin),
        ResolvedPort::Region { .. } => None,
    }
}

/// The direction carrying orientation requirements: an axis's `direction`, a face's `normal`;
/// `None` for points and regions.
pub fn principal_dir(port: &ResolvedPort) -> Option<AxisDir> {
    match port {
        ResolvedPort::Axis { direction, .. } => Some(*direction),
        ResolvedPort::Face { normal, .. } => Some(*normal),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical::parse_example;

    fn plate_doc() -> CadDocument2 {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/cad2/plate-four-holes.cad2.json"
        );
        parse_example(&std::fs::read_to_string(path).unwrap())
            .unwrap()
            .0
    }

    fn pr(s: &str) -> PortRef {
        s.parse().unwrap()
    }

    #[test]
    fn tables_cover_every_shape_and_face_frames_are_right_handed() {
        for shape in ShapeKind::ALL {
            let names = port_names(shape);
            if shape == ShapeKind::EmptyOf {
                assert!(names.is_empty());
                continue;
            }
            assert!(!names.is_empty(), "{shape:?}");
            if let Some(fp) = frame_port(shape) {
                assert_eq!(
                    shape_port_type(shape, fp),
                    Some(PortType::Point),
                    "{shape:?}"
                );
            }
        }
        for name in port_names(ShapeKind::VolumeLBracket) {
            for port in [
                box_port(10.0, 20.0, 30.0, name),
                cylinder_port(10.0, 30.0, name),
                plane_port([1.0, 2.0], name),
            ]
            .into_iter()
            .flatten()
            {
                if let ResolvedPort::Face { normal, u, v, .. } = port {
                    assert_eq!(u.cross(v), Some(normal), "{name}");
                }
            }
        }
    }

    #[test]
    fn document_lookup_distinguishes_errors() {
        let doc = plate_doc();
        assert_eq!(port_type(&doc, &pr("v_plate.top")), Ok(PortType::Face));
        assert_eq!(port_type(&doc, &pr("e_h1.axis")), Ok(PortType::Axis));
        let e = port_type(&doc, &pr("v_nope.top")).unwrap_err();
        assert_eq!(e.code(), Code::DanglingReference);
        let e = port_type(&doc, &pr("v_plate.axis")).unwrap_err();
        assert_eq!(e.code(), Code::UnknownPort);
        assert!(e.to_string().contains("plate"));
        // constraint ids are not components
        assert_eq!(
            port_type(&doc, &pr("k_d1.top")).unwrap_err().code(),
            Code::DanglingReference
        );
    }

    #[test]
    fn constraint_checks() {
        let doc = plate_doc();
        for k in &doc.constraints {
            assert!(check_constraint_ports(&doc, k).is_ok(), "{}", k.id);
        }
        let mut k = doc.constraint("k_d1").unwrap().clone();
        k.b = Some(pr("e_h1.axis"));
        let errs = check_constraint_ports(&doc, &k).unwrap_err();
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].code(), Code::PortTypeMismatch);
        assert_eq!(errs[0].to_issue("k_d1").target.as_deref(), Some("k_d1"));
        k.a = pr("zz.point");
        k.b = Some(pr("v_plate.nope"));
        let codes: Vec<_> = check_constraint_ports(&doc, &k)
            .unwrap_err()
            .iter()
            .map(|e| e.code())
            .collect();
        assert_eq!(codes, [Code::DanglingReference, Code::UnknownPort]);

        let mut f = doc.constraint("k_root").unwrap().clone();
        f.a = pr("v_plate.top");
        assert_eq!(
            check_constraint_ports(&doc, &f).unwrap_err()[0].code(),
            Code::InvalidFixedTarget
        );
        assert!(check_fixed_target(&doc, &pr("e_h2.entry")).is_ok());
        assert!(check_fixed_target(&doc, &pr("d_h2.point")).is_ok());
        assert_eq!(
            check_fixed_target(&doc, &pr("e_h2.axis"))
                .unwrap_err()
                .code(),
            Code::InvalidFixedTarget
        );
        assert!(is_compatible(
            ConstraintKind::Distance,
            PortType::Point,
            PortType::Face
        ));
        assert!(!is_compatible(
            ConstraintKind::Distance,
            PortType::Face,
            PortType::Point
        ));
        assert!(has_orientation_requirement(
            ConstraintKind::Distance,
            PortType::Face,
            PortType::Face
        ));
        assert!(!has_orientation_requirement(
            ConstraintKind::Distance,
            PortType::Point,
            PortType::Face
        ));
    }

    #[test]
    fn local_geometry_and_transform() {
        let doc = plate_doc();
        let plate = doc.component("v_plate").unwrap();
        assert_eq!(
            local_port(plate, "x_neg"),
            Some(face([-60.0, 0.0, 5.0], NX, NY, PZ, [60.0, 10.0]))
        );
        let hole = doc.component("e_h1").unwrap();
        assert_eq!(local_port(hole, "axis"), Some(axis([0.0, 0.0, 0.0], NZ)));
        assert_eq!(local_port(hole, "region"), None);
        assert_eq!(frame_port_ref(hole).unwrap().as_str(), "e_h1.entry");
        // local Z → world +X, local X → world +Y
        let o = Orientation::new(PY, PX).unwrap();
        let top = local_port(plate, "top").unwrap();
        assert_eq!(
            transform_port(&top, o, [1.0, 2.0, 3.0]),
            face([11.0, 2.0, 3.0], PX, PY, PZ, [120.0, 60.0])
        );
        let reg = local_port(plate, "region").unwrap();
        assert_eq!(
            transform_port(&reg, o, [0.0, 0.0, 0.0]),
            region([0.0, -60.0, -30.0], [10.0, 60.0, 30.0])
        );
        assert_eq!(anchor(&reg), None);
        assert_eq!(principal_dir(&top), Some(PZ));
    }
}
