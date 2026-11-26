//! CadDocument 2.0 type tree (spec `docs/CAD_DSL_2.md` §2–§6).
//!
//! Serialization is canonical (§1.4): every field is written (including `null` and empty
//! lists) in the order of the spec tables. Struct field order below therefore follows the spec
//! tables and must not be reordered. The only conditional key is a component's `role`, which
//! exists exactly on `kind = "empty"` components.
//!
//! Range/length/list-cap and cross-field shape rules that serde cannot express are checked by
//! [`CadDocument2::check_schema`]; [`CadDocument2::from_json_str`] parses and checks.

use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::common::hole_subtype_error;
pub use super::common::{
    ChamferFeature, CylinderSpec, EdgeSelector, EnclosureSpec, FilletFeature, HoleType,
    LBracketSpec, Material, PlannerMetadata, PlateSpec, RingSpec, Unit,
};
use super::geom::{Axis, Frame};
use super::ids::{FieldRef, Id, PortRef};
use super::schema::{Checker, LoadError, SchemaViolation};

/// Size limits of CadDocument 2.0 (spec §2–§6).
pub mod limits {
    /// `name` (document, part, assembly) max chars.
    pub const NAME: usize = 80;
    /// `source_prompt` max chars.
    pub const SOURCE_PROMPT: usize = 20_000;
    /// `parameters` max length.
    pub const PARAMETERS: usize = 64;
    /// `bindings` max length.
    pub const BINDINGS: usize = 128;
    /// `components` max length (min 1 only for completed documents).
    pub const COMPONENTS: usize = 256;
    /// `constraints` max length.
    pub const CONSTRAINTS: usize = 512;
    /// `parts` max length (min 1 only for completed documents).
    pub const PARTS: usize = 32;
    /// `assemblies` max length.
    pub const ASSEMBLIES: usize = 16;
    /// Maximum assembly nesting depth (graph rule, checked by the validator, not the schema).
    pub const ASSEMBLY_DEPTH: usize = 4;
    /// `assumptions` / `notes` max length.
    pub const NOTES: usize = 32;
    /// Component `label` max chars.
    pub const LABEL: usize = 80;
    /// `provenance.patch_id` max chars.
    pub const PATCH_ID: usize = 64;
    /// Parameter `note` max chars.
    pub const PARAMETER_NOTE: usize = 120;
    /// Binding `expr` max chars (min 1).
    pub const EXPR: usize = 160;
    /// Polyline `points` min length.
    pub const POLYLINE_MIN_POINTS: usize = 2;
    /// Polyline `points` max length.
    pub const POLYLINE_MAX_POINTS: usize = 64;
    /// Profile `inners` max length.
    pub const PROFILE_INNERS: usize = 16;
    /// Part `quantity` max.
    pub const QUANTITY: u32 = 999;
    /// Part `body` max length (min 1).
    pub const BODY: usize = 128;
    /// Part `fillets` / `chamfers` max length.
    pub const EDGE_FEATURES: usize = 8;
    /// Assembly `children` max length (min 1).
    pub const CHILDREN: usize = 32;
    /// `thread` max chars (implied by the 2.0 thread pattern; same as 1.0).
    pub const THREAD: usize = 32;
}

/// `schema_version: "2.0"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
pub enum SchemaVersion2 {
    /// `"2.0"`
    #[default]
    #[serde(rename = "2.0")]
    V2_0,
}

/// Returns `true` when `s` matches `^M\d{1,3}(\.\d)?(x\d(\.\d{1,2})?)?$` (spec §3.3).
pub fn is_valid_thread(s: &str) -> bool {
    let b = s.as_bytes();
    let digit = |i: usize| b.get(i).is_some_and(u8::is_ascii_digit);
    if b.first() != Some(&b'M') {
        return false;
    }
    let mut i = 1;
    while i < b.len() && i < 4 && digit(i) {
        i += 1;
    }
    if i == 1 {
        return false;
    }
    if b.get(i) == Some(&b'.') {
        if !digit(i + 1) {
            return false;
        }
        i += 2;
    }
    if b.get(i) == Some(&b'x') {
        if !digit(i + 1) {
            return false;
        }
        i += 2;
        if b.get(i) == Some(&b'.') {
            if !digit(i + 1) {
                return false;
            }
            i += 2;
            if digit(i) {
                i += 1;
            }
        }
    }
    i == b.len()
}

// =============================================================================================
// §3.1 components
// =============================================================================================

/// Component `kind` (§3.1).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ComponentKind {
    /// Negative/space element; carries a `role`.
    Empty,
    /// Point.
    Dot,
    /// Curve (always derived).
    Line,
    /// Planar region.
    Plane,
    /// Solid.
    Volume,
}

impl ComponentKind {
    /// The JSON string.
    pub fn as_str(self) -> &'static str {
        match self {
            ComponentKind::Empty => "empty",
            ComponentKind::Dot => "dot",
            ComponentKind::Line => "line",
            ComponentKind::Plane => "plane",
            ComponentKind::Volume => "volume",
        }
    }
}

/// Empty `role` (§3.1), required on and exclusive to `kind = "empty"`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum EmptyRole {
    /// Subtracted from a part body.
    Cut,
    /// Space reserved for something (PCB, connector, …).
    Reserve,
    /// Space no solid may enter.
    KeepOut,
    /// Bounding envelope; may be `intersect`ed by several parts.
    Envelope,
}

impl EmptyRole {
    /// `reserve`, `keep_out`, `envelope` are the "reserved class" of §8.4.
    pub fn is_reserved_class(self) -> bool {
        !matches!(self, EmptyRole::Cut)
    }
}

/// Who created a component (§3.1 `provenance.agent`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProvenanceAgent {
    /// Space planner role.
    SpacePlanner,
    /// Component smith role.
    ComponentSmith,
    /// Joiner role.
    Joiner,
    /// Assembler role.
    Assembler,
    /// A human edit (stamped by the coordinator).
    Human,
    /// Created by `upgrade_1_to_2`.
    Migration,
}

/// `{agent, patch_id}`, stamped by the coordinator (§3.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    /// Creating agent.
    pub agent: ProvenanceAgent,
    /// `r<revision_after>` of the creating patch; `null` for `migration`. ≤ 64 chars.
    #[schemars(length(max = 64))]
    pub patch_id: Option<String>,
}

// ---- §3.3 shapes ----------------------------------------------------------------------------

/// Empty `box`: `length`, `width`, `height` (all `Dim`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BoxSpec {
    /// Along local X.
    #[schemars(schema_with = "crate::model::schema::dim")]
    pub length: f64,
    /// Along local Y.
    #[schemars(schema_with = "crate::model::schema::dim")]
    pub width: f64,
    /// Along local Z.
    #[schemars(schema_with = "crate::model::schema::dim")]
    pub height: f64,
}

/// Empty `hole` (§3.3): 1.0 `HoleFeature` without `x`/`y`/`z`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HoleSpec {
    /// Local hole axis, default `z`.
    #[serde(default)]
    pub axis: Axis,
    /// `Dim`.
    #[schemars(schema_with = "crate::model::schema::dim")]
    pub diameter: f64,
    /// Default `through`.
    #[serde(default)]
    pub hole_type: HoleType,
    /// `Dim | null`; required for `blind`.
    #[schemars(schema_with = "crate::model::schema::opt_dim")]
    pub depth: Option<f64>,
    /// `^M\d{1,3}(\.\d)?(x\d(\.\d{1,2})?)?$ | null`.
    #[schemars(regex(pattern = r"^M\d{1,3}(\.\d)?(x\d(\.\d{1,2})?)?$"))]
    pub thread: Option<String>,
    /// `Dim | null`.
    #[schemars(schema_with = "crate::model::schema::opt_dim")]
    pub counterbore_diameter: Option<f64>,
    /// `Dim | null`.
    #[schemars(schema_with = "crate::model::schema::opt_dim")]
    pub counterbore_depth: Option<f64>,
    /// `Dim | null`.
    #[schemars(schema_with = "crate::model::schema::opt_dim")]
    pub countersink_diameter: Option<f64>,
    /// `0 < x < 180 | null`.
    #[schemars(schema_with = "crate::model::schema::opt_countersink_angle")]
    pub countersink_angle: Option<f64>,
}

impl HoleSpec {
    /// 1.0 `validate_subtype` rule violation.
    pub fn subtype_error(&self) -> Option<&'static str> {
        hole_subtype_error(
            self.hole_type,
            self.diameter,
            self.depth,
            self.counterbore_diameter,
            self.counterbore_depth,
            self.countersink_diameter,
            self.countersink_angle,
        )
    }

    /// `max(diameter, counterbore_diameter, countersink_diameter)` (§3.4 hole `region`).
    pub fn max_diameter(&self) -> f64 {
        [self.counterbore_diameter, self.countersink_diameter]
            .into_iter()
            .flatten()
            .fold(self.diameter, f64::max)
    }
}

/// Empty `of`: wraps a Volume (`source`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OfSpec {
    /// Volume `id`.
    pub source: Id,
}

/// Empty shapes, discriminated by `shape.kind`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EmptyShape {
    /// `box`
    Box(BoxSpec),
    /// `cylinder`
    Cylinder(CylinderSpec),
    /// `hole`
    Hole(HoleSpec),
    /// `of`
    Of(OfSpec),
}

/// Dot shapes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DotShape {
    /// `point` (no fields).
    Point {},
}

/// Line `segment`: `start`, `end` (Dot ids).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SegmentSpec {
    /// Start Dot.
    pub start: Id,
    /// End Dot.
    pub end: Id,
}

/// Line `polyline`: `points` (2–64 Dot ids), `closed`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PolylineSpec {
    /// Dot ids, 2–64.
    #[schemars(length(min = 2, max = 64))]
    pub points: Vec<Id>,
    /// Whether the polyline is closed.
    pub closed: bool,
}

/// Line `arc`: `start`, `through`, `end` (Dot ids).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArcSpec {
    /// Start Dot.
    pub start: Id,
    /// A Dot the arc passes through.
    pub through: Id,
    /// End Dot.
    pub end: Id,
}

/// Line `circle`: `center` (Dot id), `diameter`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CircleSpec {
    /// Center Dot.
    pub center: Id,
    /// `Dim`.
    #[schemars(schema_with = "crate::model::schema::dim")]
    pub diameter: f64,
}

/// Line shapes (all derived components).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LineShape {
    /// `segment`
    Segment(SegmentSpec),
    /// `polyline`
    Polyline(PolylineSpec),
    /// `arc`
    Arc(ArcSpec),
    /// `circle`
    Circle(CircleSpec),
}

/// Plane `rect`: `length`, `width`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RectSpec {
    /// Along local X.
    #[schemars(schema_with = "crate::model::schema::dim")]
    pub length: f64,
    /// Along local Y.
    #[schemars(schema_with = "crate::model::schema::dim")]
    pub width: f64,
}

/// Plane `disc`: `diameter`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiscSpec {
    /// `Dim`.
    #[schemars(schema_with = "crate::model::schema::dim")]
    pub diameter: f64,
}

/// Plane `profile`: `outer` (closed Line id), `inners` (closed Line ids, ≤ 16).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileSpec {
    /// Outer closed Line.
    pub outer: Id,
    /// Inner closed Lines (holes in the profile), ≤ 16.
    #[serde(default)]
    #[schemars(length(max = 16))]
    pub inners: Vec<Id>,
}

/// Plane `section`: `source` (Volume id), `offset` (`Coord`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SectionSpec {
    /// Volume id.
    pub source: Id,
    /// Local Z of the horizontal section (`Coord`).
    #[schemars(schema_with = "crate::model::schema::coord")]
    pub offset: f64,
}

/// Plane shapes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PlaneShape {
    /// `rect`
    Rect(RectSpec),
    /// `disc`
    Disc(DiscSpec),
    /// `profile`
    Profile(ProfileSpec),
    /// `section`
    Section(SectionSpec),
}

/// Volume `extrude`: `profile` (Plane id), `height`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExtrudeSpec {
    /// Plane id.
    pub profile: Id,
    /// `Dim`.
    #[schemars(schema_with = "crate::model::schema::dim")]
    pub height: f64,
}

/// Volume `revolve`: `profile` (Plane id), `axis` (`segment` Line id), `angle` (`0 < a ≤ 360`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RevolveSpec {
    /// Plane id.
    pub profile: Id,
    /// `segment` Line id.
    pub axis: Id,
    /// Degrees, `0 < angle ≤ 360`.
    #[schemars(schema_with = "crate::model::schema::revolve_angle")]
    pub angle: f64,
}

/// Volume shapes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum VolumeShape {
    /// `plate`
    Plate(PlateSpec),
    /// `cylinder`
    Cylinder(CylinderSpec),
    /// `ring`
    Ring(RingSpec),
    /// `l_bracket`
    LBracket(LBracketSpec),
    /// `enclosure`
    Enclosure(EnclosureSpec),
    /// `extrude`
    Extrude(ExtrudeSpec),
    /// `revolve`
    Revolve(RevolveSpec),
}

/// Flat `(component kind, shape kind)` discriminant — handy for per-shape tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ShapeKind {
    /// Empty `box`
    EmptyBox,
    /// Empty `cylinder`
    EmptyCylinder,
    /// Empty `hole`
    EmptyHole,
    /// Empty `of`
    EmptyOf,
    /// Dot `point`
    DotPoint,
    /// Line `segment`
    LineSegment,
    /// Line `polyline`
    LinePolyline,
    /// Line `arc`
    LineArc,
    /// Line `circle`
    LineCircle,
    /// Plane `rect`
    PlaneRect,
    /// Plane `disc`
    PlaneDisc,
    /// Plane `profile`
    PlaneProfile,
    /// Plane `section`
    PlaneSection,
    /// Volume `plate`
    VolumePlate,
    /// Volume `cylinder`
    VolumeCylinder,
    /// Volume `ring`
    VolumeRing,
    /// Volume `l_bracket`
    VolumeLBracket,
    /// Volume `enclosure`
    VolumeEnclosure,
    /// Volume `extrude`
    VolumeExtrude,
    /// Volume `revolve`
    VolumeRevolve,
}

impl ShapeKind {
    /// All 20 shape kinds in spec §3.3 table order.
    pub const ALL: [ShapeKind; 20] = [
        ShapeKind::EmptyBox,
        ShapeKind::EmptyCylinder,
        ShapeKind::EmptyHole,
        ShapeKind::EmptyOf,
        ShapeKind::DotPoint,
        ShapeKind::LineSegment,
        ShapeKind::LinePolyline,
        ShapeKind::LineArc,
        ShapeKind::LineCircle,
        ShapeKind::PlaneRect,
        ShapeKind::PlaneDisc,
        ShapeKind::PlaneProfile,
        ShapeKind::PlaneSection,
        ShapeKind::VolumePlate,
        ShapeKind::VolumeCylinder,
        ShapeKind::VolumeRing,
        ShapeKind::VolumeLBracket,
        ShapeKind::VolumeEnclosure,
        ShapeKind::VolumeExtrude,
        ShapeKind::VolumeRevolve,
    ];

    /// The component `kind`.
    pub fn component_kind(self) -> ComponentKind {
        use ShapeKind::*;
        match self {
            EmptyBox | EmptyCylinder | EmptyHole | EmptyOf => ComponentKind::Empty,
            DotPoint => ComponentKind::Dot,
            LineSegment | LinePolyline | LineArc | LineCircle => ComponentKind::Line,
            PlaneRect | PlaneDisc | PlaneProfile | PlaneSection => ComponentKind::Plane,
            _ => ComponentKind::Volume,
        }
    }

    /// The `shape.kind` string.
    pub fn shape_name(self) -> &'static str {
        use ShapeKind::*;
        match self {
            EmptyBox => "box",
            EmptyCylinder | VolumeCylinder => "cylinder",
            EmptyHole => "hole",
            EmptyOf => "of",
            DotPoint => "point",
            LineSegment => "segment",
            LinePolyline => "polyline",
            LineArc => "arc",
            LineCircle => "circle",
            PlaneRect => "rect",
            PlaneDisc => "disc",
            PlaneProfile => "profile",
            PlaneSection => "section",
            VolumePlate => "plate",
            VolumeRing => "ring",
            VolumeLBracket => "l_bracket",
            VolumeEnclosure => "enclosure",
            VolumeExtrude => "extrude",
            VolumeRevolve => "revolve",
        }
    }

    /// Independent components (§3.2) own 3 translation unknowns plus an orientation; derived
    /// components are fully determined by their sources.
    pub fn is_independent(self) -> bool {
        use ShapeKind::*;
        matches!(
            self,
            DotPoint
                | PlaneRect
                | PlaneDisc
                | VolumePlate
                | VolumeCylinder
                | VolumeRing
                | VolumeLBracket
                | VolumeEnclosure
                | EmptyBox
                | EmptyCylinder
                | EmptyHole
        )
    }

    /// The frame port of an independent component (§5.3: `origin` for box/cylinder-like
    /// shapes, `point` for Dots, `entry` for holes, `center` for `rect`/`disc`); `None` for
    /// derived components.
    pub fn frame_port(self) -> Option<&'static str> {
        use ShapeKind::*;
        match self {
            DotPoint => Some("point"),
            EmptyHole => Some("entry"),
            PlaneRect | PlaneDisc => Some("center"),
            VolumePlate | VolumeCylinder | VolumeRing | VolumeLBracket | VolumeEnclosure
            | EmptyBox | EmptyCylinder => Some("origin"),
            _ => None,
        }
    }

    /// Every numeric shape field that *can* be bound (§3.3 "可綁定欄位"), in spec order.
    ///
    /// For holes the optional fields are only bindable while non-null; use
    /// [`crate::fields::bindable_fields`] for the per-component answer.
    pub fn numeric_fields(self) -> &'static [&'static str] {
        use ShapeKind::*;
        match self {
            EmptyBox => &["length", "width", "height"],
            EmptyCylinder | VolumeCylinder => &["diameter", "height"],
            EmptyHole => &[
                "diameter",
                "depth",
                "counterbore_diameter",
                "counterbore_depth",
                "countersink_diameter",
                "countersink_angle",
            ],
            EmptyOf | DotPoint | LineSegment | LinePolyline | LineArc | PlaneProfile => &[],
            LineCircle | PlaneDisc => &["diameter"],
            PlaneRect => &["length", "width"],
            PlaneSection => &["offset"],
            VolumePlate => &["length", "width", "thickness"],
            VolumeRing => &["outer_diameter", "inner_diameter", "height"],
            VolumeLBracket => &["width", "depth", "vertical_height", "thickness"],
            VolumeEnclosure => &["length", "width", "height", "wall_thickness"],
            VolumeExtrude => &["height"],
            VolumeRevolve => &["angle"],
        }
    }
}

/// The kind-specific part of a component: `kind`, `role` (Empty only) and `shape`.
#[derive(Debug, Clone, PartialEq)]
pub enum Geometry {
    /// `kind: "empty"` with its required `role`.
    Empty {
        /// Empty role.
        role: EmptyRole,
        /// Empty shape.
        shape: EmptyShape,
    },
    /// `kind: "dot"`.
    Dot {
        /// Dot shape.
        shape: DotShape,
    },
    /// `kind: "line"`.
    Line {
        /// Line shape.
        shape: LineShape,
    },
    /// `kind: "plane"`.
    Plane {
        /// Plane shape.
        shape: PlaneShape,
    },
    /// `kind: "volume"`.
    Volume {
        /// Volume shape.
        shape: VolumeShape,
    },
}

impl Geometry {
    /// The component `kind`.
    pub fn kind(&self) -> ComponentKind {
        match self {
            Geometry::Empty { .. } => ComponentKind::Empty,
            Geometry::Dot { .. } => ComponentKind::Dot,
            Geometry::Line { .. } => ComponentKind::Line,
            Geometry::Plane { .. } => ComponentKind::Plane,
            Geometry::Volume { .. } => ComponentKind::Volume,
        }
    }

    /// The Empty role, `None` for non-Empties.
    pub fn role(&self) -> Option<EmptyRole> {
        match self {
            Geometry::Empty { role, .. } => Some(*role),
            _ => None,
        }
    }

    /// The flat shape discriminant.
    pub fn shape_kind(&self) -> ShapeKind {
        match self {
            Geometry::Empty { shape, .. } => match shape {
                EmptyShape::Box(_) => ShapeKind::EmptyBox,
                EmptyShape::Cylinder(_) => ShapeKind::EmptyCylinder,
                EmptyShape::Hole(_) => ShapeKind::EmptyHole,
                EmptyShape::Of(_) => ShapeKind::EmptyOf,
            },
            Geometry::Dot { shape } => match shape {
                DotShape::Point {} => ShapeKind::DotPoint,
            },
            Geometry::Line { shape } => match shape {
                LineShape::Segment(_) => ShapeKind::LineSegment,
                LineShape::Polyline(_) => ShapeKind::LinePolyline,
                LineShape::Arc(_) => ShapeKind::LineArc,
                LineShape::Circle(_) => ShapeKind::LineCircle,
            },
            Geometry::Plane { shape } => match shape {
                PlaneShape::Rect(_) => ShapeKind::PlaneRect,
                PlaneShape::Disc(_) => ShapeKind::PlaneDisc,
                PlaneShape::Profile(_) => ShapeKind::PlaneProfile,
                PlaneShape::Section(_) => ShapeKind::PlaneSection,
            },
            Geometry::Volume { shape } => match shape {
                VolumeShape::Plate(_) => ShapeKind::VolumePlate,
                VolumeShape::Cylinder(_) => ShapeKind::VolumeCylinder,
                VolumeShape::Ring(_) => ShapeKind::VolumeRing,
                VolumeShape::LBracket(_) => ShapeKind::VolumeLBracket,
                VolumeShape::Enclosure(_) => ShapeKind::VolumeEnclosure,
                VolumeShape::Extrude(_) => ShapeKind::VolumeExtrude,
                VolumeShape::Revolve(_) => ShapeKind::VolumeRevolve,
            },
        }
    }

    /// Ids referenced from inside `shape` (promotion sources), as `(field name, id)` in field
    /// order; list fields repeat the field name.
    pub fn shape_refs(&self) -> Vec<(&'static str, &Id)> {
        match self {
            Geometry::Empty {
                shape: EmptyShape::Of(s),
                ..
            } => vec![("source", &s.source)],
            Geometry::Line { shape } => match shape {
                LineShape::Segment(s) => vec![("start", &s.start), ("end", &s.end)],
                LineShape::Polyline(s) => s.points.iter().map(|p| ("points", p)).collect(),
                LineShape::Arc(s) => {
                    vec![
                        ("start", &s.start),
                        ("through", &s.through),
                        ("end", &s.end),
                    ]
                }
                LineShape::Circle(s) => vec![("center", &s.center)],
            },
            Geometry::Plane { shape } => match shape {
                PlaneShape::Profile(s) => std::iter::once(("outer", &s.outer))
                    .chain(s.inners.iter().map(|i| ("inners", i)))
                    .collect(),
                PlaneShape::Section(s) => vec![("source", &s.source)],
                PlaneShape::Rect(_) | PlaneShape::Disc(_) => vec![],
            },
            Geometry::Volume { shape } => match shape {
                VolumeShape::Extrude(s) => vec![("profile", &s.profile)],
                VolumeShape::Revolve(s) => vec![("profile", &s.profile), ("axis", &s.axis)],
                _ => vec![],
            },
            Geometry::Empty { .. } | Geometry::Dot { .. } => vec![],
        }
    }
}

/// A component (§3.1): `{id, kind, role?, shape, label, provenance}`.
///
/// `kind`, `role` and `shape` live in [`Component::geometry`]. Serde is hand-written so the
/// canonical key order `id, kind, role, shape, label, provenance` is preserved (an internally
/// tagged enum would emit `kind` first).
#[derive(Debug, Clone, PartialEq)]
pub struct Component {
    /// N1 identifier.
    pub id: Id,
    /// `kind`, `role` (Empty only) and `shape`.
    pub geometry: Geometry,
    /// Human-only label, ≤ 80 chars; never fed back to agents.
    pub label: Option<String>,
    /// Creation stamp; set by the coordinator.
    pub provenance: Option<Provenance>,
}

/// [`Component`] without `provenance` — the payload of `add_component` (§8.2).
#[derive(Debug, Clone, PartialEq)]
pub struct ComponentInput {
    /// N1 identifier.
    pub id: Id,
    /// `kind`, `role` (Empty only) and `shape`.
    pub geometry: Geometry,
    /// Human-only label, ≤ 80 chars.
    pub label: Option<String>,
}

macro_rules! component_accessors {
    ($ty:ty) => {
        impl $ty {
            /// The component `kind`.
            pub fn kind(&self) -> ComponentKind {
                self.geometry.kind()
            }

            /// The Empty role, `None` for non-Empties.
            pub fn role(&self) -> Option<EmptyRole> {
                self.geometry.role()
            }

            /// The flat shape discriminant.
            pub fn shape_kind(&self) -> ShapeKind {
                self.geometry.shape_kind()
            }

            /// Independent vs derived (§3.2).
            pub fn is_independent(&self) -> bool {
                self.shape_kind().is_independent()
            }

            /// The Volume shape, if this is a Volume.
            pub fn as_volume(&self) -> Option<&VolumeShape> {
                match &self.geometry {
                    Geometry::Volume { shape } => Some(shape),
                    _ => None,
                }
            }

            /// `(role, shape)` if this is an Empty.
            pub fn as_empty(&self) -> Option<(EmptyRole, &EmptyShape)> {
                match &self.geometry {
                    Geometry::Empty { role, shape } => Some((*role, shape)),
                    _ => None,
                }
            }

            /// The hole spec if this is an Empty `hole` (any role).
            pub fn as_hole(&self) -> Option<&HoleSpec> {
                match &self.geometry {
                    Geometry::Empty {
                        shape: EmptyShape::Hole(h),
                        ..
                    } => Some(h),
                    _ => None,
                }
            }

            /// The Line shape, if this is a Line.
            pub fn as_line(&self) -> Option<&LineShape> {
                match &self.geometry {
                    Geometry::Line { shape } => Some(shape),
                    _ => None,
                }
            }

            /// The Plane shape, if this is a Plane.
            pub fn as_plane(&self) -> Option<&PlaneShape> {
                match &self.geometry {
                    Geometry::Plane { shape } => Some(shape),
                    _ => None,
                }
            }
        }
    };
}

component_accessors!(Component);
component_accessors!(ComponentInput);

impl ComponentInput {
    /// Attaches a provenance stamp.
    pub fn into_component(self, provenance: Option<Provenance>) -> Component {
        Component {
            id: self.id,
            geometry: self.geometry,
            label: self.label,
            provenance,
        }
    }
}

impl From<ComponentInput> for Component {
    fn from(c: ComponentInput) -> Self {
        c.into_component(None)
    }
}

impl From<Component> for ComponentInput {
    fn from(c: Component) -> Self {
        ComponentInput {
            id: c.id,
            geometry: c.geometry,
            label: c.label,
        }
    }
}

impl Component {
    /// Copy without provenance.
    pub fn to_input(&self) -> ComponentInput {
        self.clone().into()
    }
}

fn serialize_component<S: Serializer>(
    s: S,
    name: &'static str,
    id: &Id,
    geometry: &Geometry,
    label: &Option<String>,
    provenance: Option<&Option<Provenance>>,
) -> Result<S::Ok, S::Error> {
    let len = 4 + usize::from(geometry.role().is_some()) + usize::from(provenance.is_some());
    let mut st = s.serialize_struct(name, len)?;
    st.serialize_field("id", id)?;
    st.serialize_field("kind", &geometry.kind())?;
    match geometry {
        Geometry::Empty { role, shape } => {
            st.serialize_field("role", role)?;
            st.serialize_field("shape", shape)?;
        }
        Geometry::Dot { shape } => st.serialize_field("shape", shape)?,
        Geometry::Line { shape } => st.serialize_field("shape", shape)?,
        Geometry::Plane { shape } => st.serialize_field("shape", shape)?,
        Geometry::Volume { shape } => st.serialize_field("shape", shape)?,
    }
    st.serialize_field("label", label)?;
    if let Some(p) = provenance {
        st.serialize_field("provenance", p)?;
    }
    st.end()
}

impl Serialize for Component {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        serialize_component(
            s,
            "Component",
            &self.id,
            &self.geometry,
            &self.label,
            Some(&self.provenance),
        )
    }
}

impl Serialize for ComponentInput {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        serialize_component(
            s,
            "ComponentInput",
            &self.id,
            &self.geometry,
            &self.label,
            None,
        )
    }
}

/// Wire form of [`Component`] used for deserialization and JSON Schema.
#[derive(Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(rename = "Component")]
enum ComponentRepr {
    Empty {
        id: Id,
        role: EmptyRole,
        shape: EmptyShape,
        #[serde(default)]
        #[schemars(length(max = 80))]
        label: Option<String>,
        #[serde(default)]
        provenance: Option<Provenance>,
    },
    Dot {
        id: Id,
        shape: DotShape,
        #[serde(default)]
        #[schemars(length(max = 80))]
        label: Option<String>,
        #[serde(default)]
        provenance: Option<Provenance>,
    },
    Line {
        id: Id,
        shape: LineShape,
        #[serde(default)]
        #[schemars(length(max = 80))]
        label: Option<String>,
        #[serde(default)]
        provenance: Option<Provenance>,
    },
    Plane {
        id: Id,
        shape: PlaneShape,
        #[serde(default)]
        #[schemars(length(max = 80))]
        label: Option<String>,
        #[serde(default)]
        provenance: Option<Provenance>,
    },
    Volume {
        id: Id,
        shape: VolumeShape,
        #[serde(default)]
        #[schemars(length(max = 80))]
        label: Option<String>,
        #[serde(default)]
        provenance: Option<Provenance>,
    },
}

/// Wire form of [`ComponentInput`] (no `provenance`).
#[derive(Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(rename = "ComponentInput")]
enum ComponentInputRepr {
    Empty {
        id: Id,
        role: EmptyRole,
        shape: EmptyShape,
        #[serde(default)]
        #[schemars(length(max = 80))]
        label: Option<String>,
    },
    Dot {
        id: Id,
        shape: DotShape,
        #[serde(default)]
        #[schemars(length(max = 80))]
        label: Option<String>,
    },
    Line {
        id: Id,
        shape: LineShape,
        #[serde(default)]
        #[schemars(length(max = 80))]
        label: Option<String>,
    },
    Plane {
        id: Id,
        shape: PlaneShape,
        #[serde(default)]
        #[schemars(length(max = 80))]
        label: Option<String>,
    },
    Volume {
        id: Id,
        shape: VolumeShape,
        #[serde(default)]
        #[schemars(length(max = 80))]
        label: Option<String>,
    },
}

impl From<ComponentRepr> for Component {
    fn from(r: ComponentRepr) -> Self {
        let (id, geometry, label, provenance) = match r {
            ComponentRepr::Empty {
                id,
                role,
                shape,
                label,
                provenance,
            } => (id, Geometry::Empty { role, shape }, label, provenance),
            ComponentRepr::Dot {
                id,
                shape,
                label,
                provenance,
            } => (id, Geometry::Dot { shape }, label, provenance),
            ComponentRepr::Line {
                id,
                shape,
                label,
                provenance,
            } => (id, Geometry::Line { shape }, label, provenance),
            ComponentRepr::Plane {
                id,
                shape,
                label,
                provenance,
            } => (id, Geometry::Plane { shape }, label, provenance),
            ComponentRepr::Volume {
                id,
                shape,
                label,
                provenance,
            } => (id, Geometry::Volume { shape }, label, provenance),
        };
        Component {
            id,
            geometry,
            label,
            provenance,
        }
    }
}

impl From<ComponentInputRepr> for ComponentInput {
    fn from(r: ComponentInputRepr) -> Self {
        let (id, geometry, label) = match r {
            ComponentInputRepr::Empty {
                id,
                role,
                shape,
                label,
            } => (id, Geometry::Empty { role, shape }, label),
            ComponentInputRepr::Dot { id, shape, label } => (id, Geometry::Dot { shape }, label),
            ComponentInputRepr::Line { id, shape, label } => (id, Geometry::Line { shape }, label),
            ComponentInputRepr::Plane { id, shape, label } => {
                (id, Geometry::Plane { shape }, label)
            }
            ComponentInputRepr::Volume { id, shape, label } => {
                (id, Geometry::Volume { shape }, label)
            }
        };
        ComponentInput {
            id,
            geometry,
            label,
        }
    }
}

impl<'de> Deserialize<'de> for Component {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        ComponentRepr::deserialize(d).map(Into::into)
    }
}

impl<'de> Deserialize<'de> for ComponentInput {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        ComponentInputRepr::deserialize(d).map(Into::into)
    }
}

impl JsonSchema for Component {
    fn schema_name() -> Cow<'static, str> {
        "Component".into()
    }
    fn json_schema(g: &mut SchemaGenerator) -> Schema {
        ComponentRepr::json_schema(g)
    }
}

impl JsonSchema for ComponentInput {
    fn schema_name() -> Cow<'static, str> {
        "ComponentInput".into()
    }
    fn json_schema(g: &mut SchemaGenerator) -> Schema {
        ComponentInputRepr::json_schema(g)
    }
}

// =============================================================================================
// §4 parameters and bindings
// =============================================================================================

/// A named parameter (§4.1). `name` is in namespace N3 and may not be `min` / `max`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Parameter {
    /// Parameter name (N3).
    pub name: Id,
    /// Current value.
    pub value: f64,
    /// Optional lower bound (`parameter_out_of_range` is a validator code).
    #[serde(default)]
    pub min: Option<f64>,
    /// Optional upper bound.
    #[serde(default)]
    pub max: Option<f64>,
    /// ≤ 120 chars.
    #[serde(default)]
    #[schemars(length(max = 120))]
    pub note: Option<String>,
}

/// Reserved words that may not be parameter names (§1.2 N3).
pub const RESERVED_PARAMETER_NAMES: [&str; 2] = ["min", "max"];

/// `{target, expr}` (§4.2). The bound field keeps its cached numeric value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    /// `id:field` being driven.
    pub target: FieldRef,
    /// Expression text, 1–160 chars (closed grammar §4.3, parsed by [`crate::expr`]).
    #[schemars(length(min = 1, max = 160))]
    pub expr: String,
}

// =============================================================================================
// §5 constraints
// =============================================================================================

/// Constraint `kind` (§5.1).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ConstraintKind {
    /// Root a frame port at `frame`.
    Fixed,
    /// point–point.
    Coincident,
    /// point–face at `(u, v)`.
    OnFace,
    /// axis–axis.
    Coaxial,
    /// face–face, opposite normals, gap `value ≥ 0`.
    Mate,
    /// face–face, same normal, offset `value`.
    Flush,
    /// point–face or face–face signed distance along `n_b`.
    Distance,
    /// region–region containment (check only).
    Inside,
    /// region–region minimum gap ≥ `value` (check only).
    Clearance,
}

impl ConstraintKind {
    /// All kinds in spec order.
    pub const ALL: [ConstraintKind; 9] = [
        ConstraintKind::Fixed,
        ConstraintKind::Coincident,
        ConstraintKind::OnFace,
        ConstraintKind::Coaxial,
        ConstraintKind::Mate,
        ConstraintKind::Flush,
        ConstraintKind::Distance,
        ConstraintKind::Inside,
        ConstraintKind::Clearance,
    ];

    /// The JSON string.
    pub fn as_str(self) -> &'static str {
        match self {
            ConstraintKind::Fixed => "fixed",
            ConstraintKind::Coincident => "coincident",
            ConstraintKind::OnFace => "on_face",
            ConstraintKind::Coaxial => "coaxial",
            ConstraintKind::Mate => "mate",
            ConstraintKind::Flush => "flush",
            ConstraintKind::Distance => "distance",
            ConstraintKind::Inside => "inside",
            ConstraintKind::Clearance => "clearance",
        }
    }

    /// Whether `value` is used (`mate`, `flush`, `distance`, `clearance`).
    pub fn uses_value(self) -> bool {
        matches!(
            self,
            ConstraintKind::Mate
                | ConstraintKind::Flush
                | ConstraintKind::Distance
                | ConstraintKind::Clearance
        )
    }

    /// Default for a `null` `value`: `Some(0.0)` for `mate`/`flush`, `None` when `value` is
    /// required (`distance`, `clearance`) or unused.
    pub fn default_value(self) -> Option<f64> {
        matches!(self, ConstraintKind::Mate | ConstraintKind::Flush).then_some(0.0)
    }

    /// Whether `u`/`v` are used (`on_face` only, default 0).
    pub fn uses_uv(self) -> bool {
        matches!(self, ConstraintKind::OnFace)
    }

    /// Whether the kind imposes an orientation requirement (§5.4 step 5; face–face
    /// `distance` only — the caller must check the port types for `distance`).
    pub fn may_constrain_orientation(self) -> bool {
        matches!(
            self,
            ConstraintKind::Coaxial
                | ConstraintKind::Mate
                | ConstraintKind::Flush
                | ConstraintKind::Distance
        )
    }

    /// `inside` / `clearance`: checked after solving, never assign positions.
    pub fn is_check_only(self) -> bool {
        matches!(self, ConstraintKind::Inside | ConstraintKind::Clearance)
    }

    /// Bindable / `set_field`-able constraint fields of this kind (§1.3).
    pub fn numeric_fields(self) -> &'static [&'static str] {
        if self.uses_value() {
            &["value"]
        } else if self.uses_uv() {
            &["u", "v"]
        } else {
            &[]
        }
    }
}

/// A constraint (§5.1): `{id, kind, a, b, value, u, v, frame}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Constraint {
    /// N1 identifier.
    pub id: Id,
    /// Constraint kind.
    pub kind: ConstraintKind,
    /// First port (for `fixed`: the frame port).
    pub a: PortRef,
    /// Second port; `null` exactly for `fixed`.
    #[serde(default)]
    pub b: Option<PortRef>,
    /// `Coord | null` — see [`ConstraintKind::uses_value`].
    #[serde(default)]
    #[schemars(schema_with = "crate::model::schema::opt_coord")]
    pub value: Option<f64>,
    /// `Coord | null` — `on_face` only.
    #[serde(default)]
    #[schemars(schema_with = "crate::model::schema::opt_coord")]
    pub u: Option<f64>,
    /// `Coord | null` — `on_face` only.
    #[serde(default)]
    #[schemars(schema_with = "crate::model::schema::opt_coord")]
    pub v: Option<f64>,
    /// `fixed` only (required there).
    #[serde(default)]
    pub frame: Option<Frame>,
}

impl Constraint {
    /// `value` with the `mate`/`flush` default 0 applied.
    pub fn effective_value(&self) -> Option<f64> {
        if self.kind.uses_value() {
            self.value.or(self.kind.default_value())
        } else {
            None
        }
    }

    /// `u` with the `on_face` default 0 applied.
    pub fn effective_u(&self) -> Option<f64> {
        self.kind.uses_uv().then(|| self.u.unwrap_or(0.0))
    }

    /// `v` with the `on_face` default 0 applied.
    pub fn effective_v(&self) -> Option<f64> {
        self.kind.uses_uv().then(|| self.v.unwrap_or(0.0))
    }

    /// The ports referenced (`a`, then `b` when present).
    pub fn ports(&self) -> impl Iterator<Item = &PortRef> {
        std::iter::once(&self.a).chain(self.b.as_ref())
    }
}

// =============================================================================================
// §6 parts and assemblies
// =============================================================================================

/// Body boolean operation (§6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BodyOpKind {
    /// Union with a Volume.
    Add,
    /// Subtract a `cut` Empty.
    Cut,
    /// Intersect with a Volume or `envelope` Empty.
    Intersect,
}

/// `{op, component}` (§6.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BodyOp {
    /// Boolean op.
    pub op: BodyOpKind,
    /// Component id.
    pub component: Id,
}

/// Part source (§6.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PartSource {
    /// Manufactured.
    Made,
    /// Bought (BOM only, no manufacturing outputs).
    Purchased,
}

/// A part (§6.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Part {
    /// N2 identifier.
    pub id: Id,
    /// 1–80 chars.
    #[schemars(length(min = 1, max = 80))]
    pub name: String,
    /// Material or `null`.
    #[serde(default)]
    pub material: Option<Material>,
    /// 1–999.
    #[schemars(range(min = 1, max = 999))]
    pub quantity: u32,
    /// `made` / `purchased`.
    pub source: PartSource,
    /// Boolean recipe, 1–128 ops, first must be `add`.
    #[schemars(length(min = 1, max = 128))]
    pub body: Vec<BodyOp>,
    /// ≤ 8.
    #[serde(default)]
    #[schemars(length(max = 8))]
    pub fillets: Vec<FilletFeature>,
    /// ≤ 8.
    #[serde(default)]
    #[schemars(length(max = 8))]
    pub chamfers: Vec<ChamferFeature>,
}

/// An assembly (§6.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Assembly {
    /// N2 identifier.
    pub id: Id,
    /// 1–80 chars.
    #[schemars(length(min = 1, max = 80))]
    pub name: String,
    /// 1–32 N2 ids (parts or assemblies).
    #[schemars(length(min = 1, max = 32))]
    pub children: Vec<Id>,
}

// =============================================================================================
// §2 document
// =============================================================================================

fn default_name_v2() -> String {
    "promptcad-design".to_owned()
}

/// CadDocument 2.0 (§2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "CadDocument2")]
pub struct CadDocument2 {
    /// `"2.0"`.
    pub schema_version: SchemaVersion2,
    /// 1–80 chars, default `"promptcad-design"`.
    #[serde(default = "default_name_v2")]
    #[schemars(length(min = 1, max = 80))]
    pub name: String,
    /// 1–20000 chars.
    #[schemars(length(min = 1, max = 20000))]
    pub source_prompt: String,
    /// `"mm"`.
    #[serde(default)]
    pub unit: Unit,
    /// Incremented by the coordinator on every accepted patch.
    pub revision: u64,
    /// ≤ 64.
    #[serde(default)]
    #[schemars(length(max = 64))]
    pub parameters: Vec<Parameter>,
    /// ≤ 128.
    #[serde(default)]
    #[schemars(length(max = 128))]
    pub bindings: Vec<Binding>,
    /// ≤ 256 (≥ 1 only for completed documents).
    #[serde(default)]
    #[schemars(length(max = 256))]
    pub components: Vec<Component>,
    /// ≤ 512.
    #[serde(default)]
    #[schemars(length(max = 512))]
    pub constraints: Vec<Constraint>,
    /// ≤ 32 (≥ 1 only for completed documents).
    #[serde(default)]
    #[schemars(length(max = 32))]
    pub parts: Vec<Part>,
    /// ≤ 16.
    #[serde(default)]
    #[schemars(length(max = 16))]
    pub assemblies: Vec<Assembly>,
    /// Root part or assembly, maintained by the coordinator (§6.3). `null` while the document
    /// has no parts yet (mid-loop revisions); required for completed documents.
    #[serde(default)]
    pub root: Option<Id>,
    /// ≤ 32.
    #[serde(default)]
    #[schemars(length(max = 32))]
    pub assumptions: Vec<String>,
    /// ≤ 32.
    #[serde(default)]
    #[schemars(length(max = 32))]
    pub notes: Vec<String>,
    /// Planner metadata.
    pub planner: PlannerMetadata,
}

// ---- lookups --------------------------------------------------------------------------------

impl CadDocument2 {
    /// Component by id.
    pub fn component(&self, id: &str) -> Option<&Component> {
        self.components.iter().find(|c| c.id.as_str() == id)
    }

    /// Mutable component by id.
    pub fn component_mut(&mut self, id: &str) -> Option<&mut Component> {
        self.components.iter_mut().find(|c| c.id.as_str() == id)
    }

    /// Index of a component in document order.
    pub fn component_index(&self, id: &str) -> Option<usize> {
        self.components.iter().position(|c| c.id.as_str() == id)
    }

    /// Constraint by id.
    pub fn constraint(&self, id: &str) -> Option<&Constraint> {
        self.constraints.iter().find(|c| c.id.as_str() == id)
    }

    /// Mutable constraint by id.
    pub fn constraint_mut(&mut self, id: &str) -> Option<&mut Constraint> {
        self.constraints.iter_mut().find(|c| c.id.as_str() == id)
    }

    /// Part by id.
    pub fn part(&self, id: &str) -> Option<&Part> {
        self.parts.iter().find(|p| p.id.as_str() == id)
    }

    /// Assembly by id.
    pub fn assembly(&self, id: &str) -> Option<&Assembly> {
        self.assemblies.iter().find(|a| a.id.as_str() == id)
    }

    /// Parameter by name.
    pub fn parameter(&self, name: &str) -> Option<&Parameter> {
        self.parameters.iter().find(|p| p.name.as_str() == name)
    }

    /// Binding by target.
    pub fn binding(&self, target: &FieldRef) -> Option<&Binding> {
        self.bindings.iter().find(|b| &b.target == target)
    }

    /// Parses JSON (no example keys allowed) and runs [`CadDocument2::check_schema`].
    pub fn from_json_str(s: &str) -> Result<Self, LoadError> {
        let doc: Self = serde_json::from_str(s)?;
        doc.into_checked()
    }

    /// Like [`CadDocument2::from_json_str`] from an already-parsed JSON value.
    pub fn from_value(v: serde_json::Value) -> Result<Self, LoadError> {
        let doc: Self = serde_json::from_value(v)?;
        doc.into_checked()
    }

    fn into_checked(self) -> Result<Self, LoadError> {
        let v = self.check_schema();
        if v.is_empty() {
            Ok(self)
        } else {
            Err(LoadError::Schema(v))
        }
    }
}

// ---- schema checks --------------------------------------------------------------------------

impl HoleSpec {
    pub(crate) fn check(&self, c: &mut Checker, path: &str) {
        c.dim(&format!("{path}/diameter"), self.diameter);
        c.opt_dim(&format!("{path}/depth"), self.depth);
        if let Some(t) = &self.thread {
            if !is_valid_thread(t) {
                c.err(
                    format!("{path}/thread"),
                    "must match ^M\\d{1,3}(\\.\\d)?(x\\d(\\.\\d{1,2})?)?$",
                );
            }
        }
        c.opt_dim(
            &format!("{path}/counterbore_diameter"),
            self.counterbore_diameter,
        );
        c.opt_dim(&format!("{path}/counterbore_depth"), self.counterbore_depth);
        c.opt_dim(
            &format!("{path}/countersink_diameter"),
            self.countersink_diameter,
        );
        if let Some(a) = self.countersink_angle {
            c.open_range(&format!("{path}/countersink_angle"), a, 0.0, 180.0);
        }
        if let Some(msg) = self.subtype_error() {
            c.err(path, msg);
        }
    }
}

impl Geometry {
    /// Shape-level rules for this geometry; `path` points at the component.
    pub(crate) fn check(&self, c: &mut Checker, path: &str) {
        let sp = format!("{path}/shape");
        let f = |name: &str| format!("{sp}/{name}");
        match self {
            Geometry::Empty { shape, .. } => match shape {
                EmptyShape::Box(s) => {
                    c.dim(&f("length"), s.length);
                    c.dim(&f("width"), s.width);
                    c.dim(&f("height"), s.height);
                }
                EmptyShape::Cylinder(s) => s.check(c, &sp),
                EmptyShape::Hole(s) => s.check(c, &sp),
                EmptyShape::Of(_) => {}
            },
            Geometry::Dot { .. } => {}
            Geometry::Line { shape } => match shape {
                LineShape::Polyline(s) => c.list(
                    &f("points"),
                    s.points.len(),
                    limits::POLYLINE_MIN_POINTS,
                    limits::POLYLINE_MAX_POINTS,
                ),
                LineShape::Circle(s) => c.dim(&f("diameter"), s.diameter),
                LineShape::Segment(_) | LineShape::Arc(_) => {}
            },
            Geometry::Plane { shape } => match shape {
                PlaneShape::Rect(s) => {
                    c.dim(&f("length"), s.length);
                    c.dim(&f("width"), s.width);
                }
                PlaneShape::Disc(s) => c.dim(&f("diameter"), s.diameter),
                PlaneShape::Profile(s) => {
                    c.list(&f("inners"), s.inners.len(), 0, limits::PROFILE_INNERS)
                }
                PlaneShape::Section(s) => c.coord(&f("offset"), s.offset),
            },
            Geometry::Volume { shape } => match shape {
                VolumeShape::Plate(s) => s.check(c, &sp),
                VolumeShape::Cylinder(s) => s.check(c, &sp),
                VolumeShape::Ring(s) => s.check(c, &sp),
                VolumeShape::LBracket(s) => s.check(c, &sp),
                VolumeShape::Enclosure(s) => s.check(c, &sp),
                VolumeShape::Extrude(s) => c.dim(&f("height"), s.height),
                VolumeShape::Revolve(s) => c.left_open_range(&f("angle"), s.angle, 0.0, 360.0),
            },
        }
    }
}

impl Component {
    pub(crate) fn check(&self, c: &mut Checker, path: &str) {
        self.geometry.check(c, path);
        c.opt_text(
            &format!("{path}/label"),
            self.label.as_deref(),
            limits::LABEL,
        );
        if let Some(p) = &self.provenance {
            c.opt_text(
                &format!("{path}/provenance/patch_id"),
                p.patch_id.as_deref(),
                limits::PATCH_ID,
            );
        }
    }
}

impl ComponentInput {
    pub(crate) fn check(&self, c: &mut Checker, path: &str) {
        self.geometry.check(c, path);
        c.opt_text(
            &format!("{path}/label"),
            self.label.as_deref(),
            limits::LABEL,
        );
    }
}

impl Parameter {
    pub(crate) fn check(&self, c: &mut Checker, path: &str) {
        if RESERVED_PARAMETER_NAMES.contains(&self.name.as_str()) {
            c.err(
                format!("{path}/name"),
                "min and max are reserved and cannot be parameter names",
            );
        }
        c.float(&format!("{path}/value"), self.value);
        c.opt_float(&format!("{path}/min"), self.min);
        c.opt_float(&format!("{path}/max"), self.max);
        c.opt_text(
            &format!("{path}/note"),
            self.note.as_deref(),
            limits::PARAMETER_NOTE,
        );
    }
}

impl Binding {
    pub(crate) fn check(&self, c: &mut Checker, path: &str) {
        c.text(&format!("{path}/expr"), &self.expr, 1, limits::EXPR);
    }
}

impl Constraint {
    pub(crate) fn check(&self, c: &mut Checker, path: &str) {
        let p = |name: &str| format!("{path}/{name}");
        let kind = self.kind;
        if kind == ConstraintKind::Fixed {
            if self.b.is_some() {
                c.err(p("b"), "must be null for fixed constraints");
            }
            match &self.frame {
                None => c.err(p("frame"), "is required for fixed constraints"),
                Some(fr) => {
                    for (i, v) in fr.origin.iter().enumerate() {
                        c.coord(&format!("{path}/frame/origin/{i}"), *v);
                    }
                    if fr.x_axis.is_parallel(fr.z_axis) {
                        c.err(p("frame"), "x_axis must be perpendicular to z_axis");
                    }
                }
            }
        } else {
            if self.b.is_none() {
                c.err(p("b"), "is required for non-fixed constraints");
            }
            if self.frame.is_some() {
                c.err(p("frame"), "must be null for non-fixed constraints");
            }
        }
        match self.value {
            Some(v) if kind.uses_value() => {
                c.coord(&p("value"), v);
                if kind == ConstraintKind::Mate && v < 0.0 {
                    c.err(p("value"), format!("mate gap must be >= 0 (got {v})"));
                }
            }
            Some(_) => c.err(
                p("value"),
                format!("must be null for {} constraints", kind.as_str()),
            ),
            None if kind.uses_value() && kind.default_value().is_none() => c.err(
                p("value"),
                format!("is required for {} constraints", kind.as_str()),
            ),
            None => {}
        }
        for (name, val) in [("u", self.u), ("v", self.v)] {
            if let Some(x) = val {
                if kind.uses_uv() {
                    c.coord(&p(name), x);
                } else {
                    c.err(
                        p(name),
                        format!("must be null for {} constraints", kind.as_str()),
                    );
                }
            }
        }
    }
}

/// Checks a `body` list (1–128 ops) at `path`.
pub(crate) fn check_body(c: &mut Checker, path: &str, body: &[BodyOp]) {
    c.list(path, body.len(), 1, limits::BODY);
}

impl Part {
    pub(crate) fn check(&self, c: &mut Checker, path: &str) {
        c.text(&format!("{path}/name"), &self.name, 1, limits::NAME);
        if !(1..=limits::QUANTITY).contains(&self.quantity) {
            c.err(
                format!("{path}/quantity"),
                format!(
                    "must satisfy 1 <= x <= {} (got {})",
                    limits::QUANTITY,
                    self.quantity
                ),
            );
        }
        check_body(c, &format!("{path}/body"), &self.body);
        c.list(
            &format!("{path}/fillets"),
            self.fillets.len(),
            0,
            limits::EDGE_FEATURES,
        );
        for (i, f) in self.fillets.iter().enumerate() {
            f.check(c, &format!("{path}/fillets/{i}"));
        }
        c.list(
            &format!("{path}/chamfers"),
            self.chamfers.len(),
            0,
            limits::EDGE_FEATURES,
        );
        for (i, f) in self.chamfers.iter().enumerate() {
            f.check(c, &format!("{path}/chamfers/{i}"));
        }
    }
}

impl Assembly {
    pub(crate) fn check(&self, c: &mut Checker, path: &str) {
        c.text(&format!("{path}/name"), &self.name, 1, limits::NAME);
        c.list(
            &format!("{path}/children"),
            self.children.len(),
            1,
            limits::CHILDREN,
        );
    }
}

impl CadDocument2 {
    /// Schema-level constraints serde cannot express: `Dim`/`Coord` ranges, string lengths,
    /// list caps, thread pattern, hole subtype rules, ring/l_bracket/enclosure validators,
    /// constraint field usage (`fixed` ⇒ `frame` and null `b`; `value`/`u`/`v` only where
    /// used; `mate` gap ≥ 0), reserved parameter names.
    ///
    /// Does **not** check the completed-document lower bounds (see
    /// [`CadDocument2::check_completed`]) nor any graph/reference rule (validator codes).
    pub fn check_schema(&self) -> Vec<SchemaViolation> {
        let mut c = Checker::new();
        c.text("/name", &self.name, 1, limits::NAME);
        c.text(
            "/source_prompt",
            &self.source_prompt,
            1,
            limits::SOURCE_PROMPT,
        );
        c.list("/parameters", self.parameters.len(), 0, limits::PARAMETERS);
        for (i, p) in self.parameters.iter().enumerate() {
            p.check(&mut c, &format!("/parameters/{i}"));
        }
        c.list("/bindings", self.bindings.len(), 0, limits::BINDINGS);
        for (i, b) in self.bindings.iter().enumerate() {
            b.check(&mut c, &format!("/bindings/{i}"));
        }
        c.list("/components", self.components.len(), 0, limits::COMPONENTS);
        for (i, comp) in self.components.iter().enumerate() {
            comp.check(&mut c, &format!("/components/{i}"));
        }
        c.list(
            "/constraints",
            self.constraints.len(),
            0,
            limits::CONSTRAINTS,
        );
        for (i, k) in self.constraints.iter().enumerate() {
            k.check(&mut c, &format!("/constraints/{i}"));
        }
        c.list("/parts", self.parts.len(), 0, limits::PARTS);
        for (i, p) in self.parts.iter().enumerate() {
            p.check(&mut c, &format!("/parts/{i}"));
        }
        c.list("/assemblies", self.assemblies.len(), 0, limits::ASSEMBLIES);
        for (i, a) in self.assemblies.iter().enumerate() {
            a.check(&mut c, &format!("/assemblies/{i}"));
        }
        c.list("/assumptions", self.assumptions.len(), 0, limits::NOTES);
        c.list("/notes", self.notes.len(), 0, limits::NOTES);
        self.planner.check(&mut c, "/planner");
        c.finish()
    }

    /// Lower bounds that apply only to completed documents (§2, §8.6): `components ≥ 1`,
    /// `parts ≥ 1`, and `root` present (non-null). Whether `root` follows §6.3 is the
    /// validator's `invalid_root`.
    pub fn check_completed(&self) -> Vec<SchemaViolation> {
        let mut c = Checker::new();
        c.list("/components", self.components.len(), 1, limits::COMPONENTS);
        c.list("/parts", self.parts.len(), 1, limits::PARTS);
        if self.root.is_none() {
            c.err("/root", "is required for a completed document");
        }
        c.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn thread_pattern() {
        for ok in [
            "M6", "M6x1", "M8x1.25", "M10.5", "M100", "M6.5x1.5", "M6x1.2",
        ] {
            assert!(is_valid_thread(ok), "{ok}");
        }
        for bad in [
            "",
            "M",
            "m6",
            "M6 tapped",
            "M1000",
            "M6x",
            "M6x10",
            "M6.",
            "M6.55",
            "M6x1.",
            "M6x1.255",
            "M6X1",
            " M6",
        ] {
            assert!(!is_valid_thread(bad), "{bad}");
        }
    }

    #[test]
    fn component_serialization_order_and_role() {
        let j = json!({
            "id": "e_h1", "kind": "empty", "role": "cut",
            "shape": {"kind": "hole", "axis": "z", "diameter": 6.6, "hole_type": "clearance",
                      "depth": null, "thread": "M6", "counterbore_diameter": null,
                      "counterbore_depth": null, "countersink_diameter": null,
                      "countersink_angle": null},
            "label": null,
            "provenance": {"agent": "component_smith", "patch_id": "r2"}
        });
        let c: Component = serde_json::from_value(j).unwrap();
        assert_eq!(c.kind(), ComponentKind::Empty);
        assert_eq!(c.role(), Some(EmptyRole::Cut));
        assert_eq!(c.shape_kind(), ShapeKind::EmptyHole);
        assert!(c.is_independent());
        let s = serde_json::to_string(&c).unwrap();
        assert!(
            s.starts_with(
                r#"{"id":"e_h1","kind":"empty","role":"cut","shape":{"kind":"hole","axis":"z""#
            ),
            "{s}"
        );
        assert!(
            s.ends_with(
                r#""label":null,"provenance":{"agent":"component_smith","patch_id":"r2"}}"#
            ),
            "{s}"
        );

        let v: Component = serde_json::from_value(json!({
            "id": "v", "kind": "volume", "shape": {"kind": "l_bracket", "width": 1, "depth": 2,
            "vertical_height": 3, "thickness": 0.5}, "label": null, "provenance": null
        }))
        .unwrap();
        let s = serde_json::to_string(&v).unwrap();
        assert!(!s.contains("role"), "{s}");
        assert_eq!(v.shape_kind(), ShapeKind::VolumeLBracket);
    }

    #[test]
    fn component_rejects_bad_shapes() {
        let bad = [
            // role on a volume
            json!({"id":"v","kind":"volume","role":"cut","shape":{"kind":"plate","length":1,"width":1,"thickness":1}}),
            // missing role on an empty
            json!({"id":"e","kind":"empty","shape":{"kind":"box","length":1,"width":1,"height":1}}),
            // origin is not a component field
            json!({"id":"v","kind":"volume","origin":[0,0,0],"shape":{"kind":"plate","length":1,"width":1,"thickness":1}}),
            // shape kind of the wrong component kind
            json!({"id":"v","kind":"volume","shape":{"kind":"hole","diameter":1}}),
            // unknown shape field
            json!({"id":"d","kind":"dot","shape":{"kind":"point","x":1}}),
            // bad id
            json!({"id":"V","kind":"dot","shape":{"kind":"point"}}),
            // bad referenced id
            json!({"id":"l","kind":"line","shape":{"kind":"segment","start":"A","end":"b"}}),
        ];
        for j in bad {
            assert!(
                serde_json::from_value::<Component>(j.clone()).is_err(),
                "{j}"
            );
        }
        // ComponentInput rejects provenance
        assert!(serde_json::from_value::<ComponentInput>(
            json!({"id":"d","kind":"dot","shape":{"kind":"point"},"label":null,"provenance":null})
        )
        .is_err());
        let ci: ComponentInput = serde_json::from_value(
            json!({"id":"d","kind":"dot","shape":{"kind":"point"},"label":null}),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(&ci).unwrap(),
            json!({"id":"d","kind":"dot","shape":{"kind":"point"},"label":null})
        );
        let full = ci.clone().into_component(Some(Provenance {
            agent: ProvenanceAgent::Human,
            patch_id: None,
        }));
        assert_eq!(full.to_input(), ci);
    }

    #[test]
    fn shape_refs_listed() {
        let c: Component = serde_json::from_value(json!({
            "id": "p", "kind": "plane",
            "shape": {"kind": "profile", "outer": "l_out", "inners": ["l_a", "l_b"]}
        }))
        .unwrap();
        let refs: Vec<_> = c
            .geometry
            .shape_refs()
            .into_iter()
            .map(|(f, id)| format!("{f}={id}"))
            .collect();
        assert_eq!(refs, ["outer=l_out", "inners=l_a", "inners=l_b"]);
        assert!(!c.is_independent());
    }

    fn constraint(j: serde_json::Value) -> Vec<String> {
        let k: Constraint = serde_json::from_value(j).unwrap();
        let mut c = Checker::new();
        k.check(&mut c, "");
        c.finish().into_iter().map(|v| v.path).collect()
    }

    #[test]
    fn constraint_field_rules() {
        assert!(constraint(
            json!({"id":"k","kind":"fixed","a":"v.origin","b":null,"value":null,"u":null,"v":null,
            "frame":{"origin":[0,0,0],"x_axis":"+x","z_axis":"+z"}})
        )
        .is_empty());
        assert_eq!(
            constraint(json!({"id":"k","kind":"fixed","a":"v.origin","b":"w.origin"})),
            ["/b", "/frame"]
        );
        assert_eq!(
            constraint(json!({"id":"k","kind":"mate","a":"a.top","b":"b.bottom","value":-1})),
            ["/value"]
        );
        assert!(constraint(json!({"id":"k","kind":"mate","a":"a.top","b":"b.bottom"})).is_empty());
        assert_eq!(
            constraint(json!({"id":"k","kind":"distance","a":"a.top","b":"b.bottom"})),
            ["/value"]
        );
        assert_eq!(
            constraint(json!({"id":"k","kind":"coincident","a":"a.p","b":"b.p","value":1,"u":2})),
            ["/value", "/u"]
        );
        assert_eq!(
            constraint(json!({"id":"k","kind":"on_face","a":"a.p","b":"b.top","u":200000})),
            ["/u"]
        );
        let k: Constraint =
            serde_json::from_value(json!({"id":"k","kind":"on_face","a":"a.p","b":"b.top","u":3}))
                .unwrap();
        assert_eq!((k.effective_u(), k.effective_v()), (Some(3.0), Some(0.0)));
        assert_eq!(k.effective_value(), None);
        assert!(serde_json::from_value::<Constraint>(
            json!({"id":"k","kind":"fixed","a":"v.origin","frame":{"origin":[0,0,0],"x_axis":"+z","z_axis":"+z"}})
        )
        .is_err());
        assert!(serde_json::from_value::<Constraint>(
            json!({"id":"k","kind":"coincident","a":"v:origin","b":"w.point"})
        )
        .is_err());
    }
}
