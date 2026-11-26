//! Axis-aligned geometry primitives (spec §1.1, §5.1, §5.4).
//!
//! Orientations are restricted to the 24 axis-aligned rotations; directions are [`AxisDir`]s.

use std::fmt;
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Upper bound of every `Dim` (`0 < x ≤ MAX_DIMENSION_MM`) and of `|Coord|`.
pub const MAX_DIMENSION_MM: f64 = 100_000.0;
/// Comparison tolerance in mm (redundancy, `expected_resolved`, binding cache checks).
pub const TOL: f64 = 1e-6;

/// A point or vector in mm.
pub type Vec3 = [f64; 3];

/// The zero vector.
pub const ZERO: Vec3 = [0.0, 0.0, 0.0];

/// `a + b`.
pub fn vadd(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// `a − b`.
pub fn vsub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// `k · a`.
pub fn vscale(a: Vec3, k: f64) -> Vec3 {
    [a[0] * k, a[1] * k, a[2] * k]
}

/// `−a`.
pub fn vneg(a: Vec3) -> Vec3 {
    [-a[0], -a[1], -a[2]]
}

/// Dot product.
pub fn vdot(a: Vec3, b: Vec3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Cross product.
pub fn vcross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// Component-wise comparison within `tol`.
pub fn vapprox_eq(a: Vec3, b: Vec3, tol: f64) -> bool {
    (0..3).all(|i| (a[i] - b[i]).abs() <= tol)
}

/// Returns `true` when `|a − b| ≤ tol`.
pub fn approx_eq(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

// ---------------------------------------------------------------------------------------------

/// A world/local coordinate axis, serialized as `"x"`, `"y"`, `"z"`.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Axis {
    /// X axis.
    X,
    /// Y axis.
    Y,
    /// Z axis (default, matching 1.0 `HoleFeature.axis`).
    #[default]
    Z,
}

impl Axis {
    /// `[X, Y, Z]`.
    pub const ALL: [Axis; 3] = [Axis::X, Axis::Y, Axis::Z];

    /// 0, 1 or 2.
    pub fn index(self) -> usize {
        match self {
            Axis::X => 0,
            Axis::Y => 1,
            Axis::Z => 2,
        }
    }

    /// Inverse of [`Axis::index`].
    pub fn from_index(i: usize) -> Option<Axis> {
        Axis::ALL.get(i).copied()
    }

    /// The positive direction along this axis.
    pub fn positive(self) -> AxisDir {
        match self {
            Axis::X => AxisDir::PosX,
            Axis::Y => AxisDir::PosY,
            Axis::Z => AxisDir::PosZ,
        }
    }

    /// The negative direction along this axis.
    pub fn negative(self) -> AxisDir {
        self.positive().neg()
    }

    /// `"x"`, `"y"` or `"z"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Axis::X => "x",
            Axis::Y => "y",
            Axis::Z => "z",
        }
    }
}

impl fmt::Display for Axis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Axis {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "x" => Ok(Axis::X),
            "y" => Ok(Axis::Y),
            "z" => Ok(Axis::Z),
            _ => Err(format!("invalid axis {s:?}")),
        }
    }
}

// ---------------------------------------------------------------------------------------------

/// A signed axis direction, serialized as `"+x"`, `"-x"`, … `"-z"` (spec §1.1).
///
/// Declaration order (`+x, -x, +y, -y, +z, -z`) is the canonical order used by §5.4.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub enum AxisDir {
    /// `+x`
    #[serde(rename = "+x")]
    PosX,
    /// `-x`
    #[serde(rename = "-x")]
    NegX,
    /// `+y`
    #[serde(rename = "+y")]
    PosY,
    /// `-y`
    #[serde(rename = "-y")]
    NegY,
    /// `+z`
    #[serde(rename = "+z")]
    PosZ,
    /// `-z`
    #[serde(rename = "-z")]
    NegZ,
}

impl AxisDir {
    /// `+x, -x, +y, -y, +z, -z` (the §5.4 `x_axis` enumeration order).
    pub const ALL: [AxisDir; 6] = [
        AxisDir::PosX,
        AxisDir::NegX,
        AxisDir::PosY,
        AxisDir::NegY,
        AxisDir::PosZ,
        AxisDir::NegZ,
    ];

    /// The §5.4 `z_axis` enumeration order: `+z, -z, +x, -x, +y, -y`.
    pub const Z_ORDER: [AxisDir; 6] = [
        AxisDir::PosZ,
        AxisDir::NegZ,
        AxisDir::PosX,
        AxisDir::NegX,
        AxisDir::PosY,
        AxisDir::NegY,
    ];

    /// Builds a direction from an axis and a sign (`positive = true` → `+`).
    pub fn new(axis: Axis, positive: bool) -> AxisDir {
        if positive {
            axis.positive()
        } else {
            axis.negative()
        }
    }

    /// The unsigned axis.
    pub fn axis(self) -> Axis {
        match self {
            AxisDir::PosX | AxisDir::NegX => Axis::X,
            AxisDir::PosY | AxisDir::NegY => Axis::Y,
            AxisDir::PosZ | AxisDir::NegZ => Axis::Z,
        }
    }

    /// Index of the axis (0, 1, 2).
    pub fn index(self) -> usize {
        self.axis().index()
    }

    /// `+1.0` or `-1.0`.
    pub fn sign(self) -> f64 {
        if self.is_positive() {
            1.0
        } else {
            -1.0
        }
    }

    /// `true` for `+x`, `+y`, `+z`.
    pub fn is_positive(self) -> bool {
        matches!(self, AxisDir::PosX | AxisDir::PosY | AxisDir::PosZ)
    }

    /// The opposite direction.
    #[allow(clippy::should_implement_trait)]
    pub fn neg(self) -> AxisDir {
        match self {
            AxisDir::PosX => AxisDir::NegX,
            AxisDir::NegX => AxisDir::PosX,
            AxisDir::PosY => AxisDir::NegY,
            AxisDir::NegY => AxisDir::PosY,
            AxisDir::PosZ => AxisDir::NegZ,
            AxisDir::NegZ => AxisDir::PosZ,
        }
    }

    /// The unit vector.
    pub fn unit(self) -> Vec3 {
        let mut v = ZERO;
        v[self.index()] = self.sign();
        v
    }

    /// `self × other`; `None` when the two are parallel (cross product is zero).
    pub fn cross(self, other: AxisDir) -> Option<AxisDir> {
        AxisDir::from_vec(vcross(self.unit(), other.unit()))
    }

    /// Dot product of the unit vectors (−1, 0 or 1).
    pub fn dot(self, other: AxisDir) -> f64 {
        vdot(self.unit(), other.unit())
    }

    /// `true` when both lie on the same axis (equal or opposite).
    pub fn is_parallel(self, other: AxisDir) -> bool {
        self.axis() == other.axis()
    }

    /// `true` when the two directions are perpendicular.
    pub fn is_perpendicular(self, other: AxisDir) -> bool {
        self.axis() != other.axis()
    }

    /// Converts an (approximately) axis-aligned unit vector back to a direction.
    pub fn from_vec(v: Vec3) -> Option<AxisDir> {
        let mut found = None;
        for (i, c) in v.iter().enumerate() {
            if (c.abs() - 1.0).abs() <= TOL {
                if found.is_some() {
                    return None;
                }
                found = Some(AxisDir::new(Axis::ALL[i], *c > 0.0));
            } else if c.abs() > TOL {
                return None;
            }
        }
        found
    }

    /// `"+x"` … `"-z"`.
    pub fn as_str(self) -> &'static str {
        match self {
            AxisDir::PosX => "+x",
            AxisDir::NegX => "-x",
            AxisDir::PosY => "+y",
            AxisDir::NegY => "-y",
            AxisDir::PosZ => "+z",
            AxisDir::NegZ => "-z",
        }
    }
}

impl std::ops::Neg for AxisDir {
    type Output = AxisDir;
    fn neg(self) -> AxisDir {
        AxisDir::neg(self)
    }
}

impl fmt::Display for AxisDir {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for AxisDir {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        AxisDir::ALL
            .into_iter()
            .find(|d| d.as_str() == s)
            .ok_or_else(|| format!("invalid axis direction {s:?}"))
    }
}

// ---------------------------------------------------------------------------------------------

/// One of the 24 axis-aligned orientations, as `(x_axis, z_axis)` (spec §5.4).
///
/// The rotation matrix has columns `x_axis`, `y_axis = z_axis × x_axis`, `z_axis`; it maps
/// component-local coordinates to world directions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Orientation {
    x_axis: AxisDir,
    z_axis: AxisDir,
}

impl Default for Orientation {
    fn default() -> Self {
        Orientation::IDENTITY
    }
}

impl Orientation {
    /// The identity orientation `(+x, +z)`.
    pub const IDENTITY: Orientation = Orientation {
        x_axis: AxisDir::PosX,
        z_axis: AxisDir::PosZ,
    };

    /// Builds an orientation; `None` when `x_axis` is not perpendicular to `z_axis`.
    pub fn new(x_axis: AxisDir, z_axis: AxisDir) -> Option<Orientation> {
        x_axis
            .is_perpendicular(z_axis)
            .then_some(Orientation { x_axis, z_axis })
    }

    /// The 24 orientations in the canonical §5.4 order: `z_axis` in `+z, -z, +x, -x, +y, -y`,
    /// and under each `z_axis`, `x_axis` in `+x, -x, +y, -y, +z, -z` skipping non-perpendicular
    /// ones. The first entry is [`Orientation::IDENTITY`].
    pub fn all() -> [Orientation; 24] {
        let mut out = [Orientation::IDENTITY; 24];
        let mut n = 0;
        for z in AxisDir::Z_ORDER {
            for x in AxisDir::ALL {
                if let Some(o) = Orientation::new(x, z) {
                    out[n] = o;
                    n += 1;
                }
            }
        }
        debug_assert_eq!(n, 24);
        out
    }

    /// Position of this orientation in [`Orientation::all`].
    pub fn canonical_index(self) -> usize {
        Orientation::all()
            .iter()
            .position(|o| *o == self)
            .expect("every Orientation is one of the 24")
    }

    /// The local X axis expressed in world directions.
    pub fn x_axis(self) -> AxisDir {
        self.x_axis
    }

    /// The local Z axis expressed in world directions.
    pub fn z_axis(self) -> AxisDir {
        self.z_axis
    }

    /// The local Y axis: `z_axis × x_axis`.
    pub fn y_axis(self) -> AxisDir {
        self.z_axis
            .cross(self.x_axis)
            .expect("x_axis ⟂ z_axis is an Orientation invariant")
    }

    /// The local axis `axis` expressed in world directions.
    pub fn axis_dir(self, axis: Axis) -> AxisDir {
        match axis {
            Axis::X => self.x_axis,
            Axis::Y => self.y_axis(),
            Axis::Z => self.z_axis,
        }
    }

    /// `R · local`.
    pub fn apply(self, local: Vec3) -> Vec3 {
        let x = vscale(self.x_axis.unit(), local[0]);
        let y = vscale(self.y_axis().unit(), local[1]);
        let z = vscale(self.z_axis.unit(), local[2]);
        vadd(vadd(x, y), z)
    }

    /// `Rᵀ · world` (world vector back to local coordinates).
    pub fn apply_inverse(self, world: Vec3) -> Vec3 {
        [
            vdot(self.x_axis.unit(), world),
            vdot(self.y_axis().unit(), world),
            vdot(self.z_axis.unit(), world),
        ]
    }

    /// Rotates a local direction into a world direction.
    pub fn apply_dir(self, local: AxisDir) -> AxisDir {
        let d = self.axis_dir(local.axis());
        if local.is_positive() {
            d
        } else {
            d.neg()
        }
    }

    /// Rotates a world direction back into local coordinates.
    pub fn apply_dir_inverse(self, world: AxisDir) -> AxisDir {
        AxisDir::from_vec(self.apply_inverse(world.unit()))
            .expect("rotation of an axis direction is an axis direction")
    }
}

// ---------------------------------------------------------------------------------------------

/// `fixed` constraint frame: `{origin, x_axis, z_axis}` with `x_axis ⟂ z_axis` (spec §5.1).
///
/// Perpendicularity is enforced on deserialize; [`crate::model::v2::CadDocument2::check_schema`]
/// re-checks it (and the `Coord` range of `origin`) for values built in code.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "FrameRepr")]
pub struct Frame {
    /// World origin of the component frame (each coordinate a `Coord`).
    pub origin: Vec3,
    /// World direction of the component's local X axis.
    pub x_axis: AxisDir,
    /// World direction of the component's local Z axis.
    pub z_axis: AxisDir,
}

impl Frame {
    /// The orientation part; `None` when `x_axis` is not perpendicular to `z_axis`.
    pub fn orientation(&self) -> Option<Orientation> {
        Orientation::new(self.x_axis, self.z_axis)
    }

    /// `y_axis = z_axis × x_axis`; `None` when the axes are parallel.
    pub fn y_axis(&self) -> Option<AxisDir> {
        self.z_axis.cross(self.x_axis)
    }

    /// Identity frame at `origin`.
    pub fn at(origin: Vec3) -> Frame {
        Frame {
            origin,
            x_axis: AxisDir::PosX,
            z_axis: AxisDir::PosZ,
        }
    }
}

/// Wire form of [`Frame`] (also its JSON Schema).
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "Frame")]
struct FrameRepr {
    #[schemars(schema_with = "crate::model::schema::coord3")]
    origin: Vec3,
    x_axis: AxisDir,
    z_axis: AxisDir,
}

impl TryFrom<FrameRepr> for Frame {
    type Error = String;
    fn try_from(r: FrameRepr) -> Result<Self, Self::Error> {
        if r.x_axis.is_parallel(r.z_axis) {
            return Err(format!(
                "frame x_axis {} must be perpendicular to z_axis {}",
                r.x_axis, r.z_axis
            ));
        }
        Ok(Frame {
            origin: r.origin,
            x_axis: r.x_axis,
            z_axis: r.z_axis,
        })
    }
}

impl JsonSchema for Frame {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Frame".into()
    }
    fn json_schema(g: &mut schemars::SchemaGenerator) -> schemars::Schema {
        FrameRepr::json_schema(g)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn o(x: &str, z: &str) -> Orientation {
        Orientation::new(x.parse().unwrap(), z.parse().unwrap()).unwrap()
    }

    #[test]
    fn axis_dir_serde_and_math() {
        assert_eq!(serde_json::to_string(&AxisDir::NegZ).unwrap(), "\"-z\"");
        assert_eq!(
            serde_json::from_str::<AxisDir>("\"+y\"").unwrap(),
            AxisDir::PosY
        );
        assert!(serde_json::from_str::<AxisDir>("\"z\"").is_err());
        assert_eq!(AxisDir::PosX.cross(AxisDir::PosY), Some(AxisDir::PosZ));
        assert_eq!(AxisDir::PosY.cross(AxisDir::PosX), Some(AxisDir::NegZ));
        assert_eq!(AxisDir::PosZ.cross(AxisDir::PosX), Some(AxisDir::PosY));
        assert_eq!(AxisDir::PosZ.cross(AxisDir::NegZ), None);
        assert_eq!(-AxisDir::PosX, AxisDir::NegX);
        assert_eq!(AxisDir::NegY.unit(), [0.0, -1.0, 0.0]);
        assert_eq!(AxisDir::NegY.index(), 1);
        assert_eq!(AxisDir::NegY.sign(), -1.0);
        assert_eq!(AxisDir::from_vec([0.0, 0.0, -1.0]), Some(AxisDir::NegZ));
        assert_eq!(AxisDir::from_vec([0.5, 0.0, -1.0]), None);
        for d in AxisDir::ALL {
            assert_eq!(d.as_str().parse::<AxisDir>().unwrap(), d);
        }
        assert_eq!(serde_json::to_string(&Axis::Y).unwrap(), "\"y\"");
    }

    #[test]
    fn canonical_orientation_order() {
        let all = Orientation::all();
        let expected = [
            ("+x", "+z"),
            ("-x", "+z"),
            ("+y", "+z"),
            ("-y", "+z"),
            ("+x", "-z"),
            ("-x", "-z"),
            ("+y", "-z"),
            ("-y", "-z"),
            ("+y", "+x"),
            ("-y", "+x"),
            ("+z", "+x"),
            ("-z", "+x"),
            ("+y", "-x"),
            ("-y", "-x"),
            ("+z", "-x"),
            ("-z", "-x"),
            ("+x", "+y"),
            ("-x", "+y"),
            ("+z", "+y"),
            ("-z", "+y"),
            ("+x", "-y"),
            ("-x", "-y"),
            ("+z", "-y"),
            ("-z", "-y"),
        ];
        for (i, (x, z)) in expected.iter().enumerate() {
            assert_eq!(all[i], o(x, z), "index {i}");
            assert_eq!(all[i].canonical_index(), i);
        }
        assert_eq!(all[0], Orientation::IDENTITY);
        // all distinct and proper rotations (right-handed: x × y = z)
        for (i, a) in all.iter().enumerate() {
            assert_eq!(a.x_axis().cross(a.y_axis()), Some(a.z_axis()));
            for b in &all[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn orientation_apply() {
        let id = Orientation::IDENTITY;
        assert_eq!(id.y_axis(), AxisDir::PosY);
        assert_eq!(id.apply([1.0, 2.0, 3.0]), [1.0, 2.0, 3.0]);
        // local Z → world +X, local X → world +Y; then local Y = z × x = +x × +y = +z
        let r = o("+y", "+x");
        assert_eq!(r.y_axis(), AxisDir::PosZ);
        assert_eq!(r.apply([1.0, 2.0, 3.0]), [3.0, 1.0, 2.0]);
        assert_eq!(r.apply_inverse(r.apply([1.0, 2.0, 3.0])), [1.0, 2.0, 3.0]);
        assert_eq!(r.apply_dir(AxisDir::NegZ), AxisDir::NegX);
        assert_eq!(r.apply_dir(AxisDir::PosY), AxisDir::PosZ);
        for orient in Orientation::all() {
            for d in AxisDir::ALL {
                assert_eq!(orient.apply_dir_inverse(orient.apply_dir(d)), d);
                assert_eq!(
                    AxisDir::from_vec(orient.apply(d.unit())),
                    Some(orient.apply_dir(d))
                );
            }
        }
        assert_eq!(Orientation::new(AxisDir::PosZ, AxisDir::NegZ), None);
    }

    #[test]
    fn frame_rejects_parallel_axes() {
        let ok: Frame =
            serde_json::from_str(r#"{"origin":[0,0,0],"x_axis":"+x","z_axis":"+z"}"#).unwrap();
        assert_eq!(ok.orientation(), Some(Orientation::IDENTITY));
        assert_eq!(ok.y_axis(), Some(AxisDir::PosY));
        assert!(
            serde_json::from_str::<Frame>(r#"{"origin":[0,0,0],"x_axis":"+z","z_axis":"-z"}"#)
                .is_err()
        );
        assert!(serde_json::from_str::<Frame>(
            r#"{"origin":[0,0,0],"x_axis":"+x","z_axis":"+z","y_axis":"+y"}"#
        )
        .is_err());
        assert_eq!(
            serde_json::to_value(ok).unwrap(),
            serde_json::json!({"origin":[0.0,0.0,0.0],"x_axis":"+x","z_axis":"+z"})
        );
    }
}
