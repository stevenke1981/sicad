//! Types shared verbatim by CadDocument 1.0 and 2.0 (spec §3.3 "五種捷徑形狀與 1.0 … 完全相同",
//! §6.2 "fillets／chamfers 的型別與 1.0 相同", §2 `planner` "同 1.0").
//!
//! Field names, defaults and validation rules mirror `app/models/cad.py`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::schema::Checker;

/// Declares a single-value string enum used as a literal `kind` tag inside a struct
/// (Pydantic `Literal["hole"] = "hole"`).
macro_rules! literal_tag {
    ($(#[$meta:meta])* $name:ident, $lit:literal) => {
        $(#[$meta])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema,
        )]
        pub enum $name {
            #[doc = concat!("`\"", $lit, "\"`")]
            #[default]
            #[serde(rename = $lit)]
            Value,
        }
    };
}
pub(crate) use literal_tag;

literal_tag!(
    /// The unit literal `"mm"`.
    Unit,
    "mm"
);
literal_tag!(
    /// `kind: "fillet"`.
    FilletTag,
    "fillet"
);
literal_tag!(
    /// `kind: "chamfer"`.
    ChamferTag,
    "chamfer"
);

/// Part material (1.0 `Material`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Material {
    /// `aluminum`
    Aluminum,
    /// `steel`
    Steel,
    /// `stainless_steel`
    StainlessSteel,
    /// `plastic`
    Plastic,
    /// `wood`
    Wood,
    /// `other`
    Other,
}

/// Hole subtype (1.0 `HoleType`); default `through`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HoleType {
    /// `through` (default)
    #[default]
    Through,
    /// `blind` — requires `depth`
    Blind,
    /// `clearance`
    Clearance,
    /// `tapped`
    Tapped,
    /// `counterbore` — requires `counterbore_diameter` and `counterbore_depth`
    Counterbore,
    /// `countersink` — requires `countersink_diameter` and `countersink_angle`
    Countersink,
}

/// Edge selector for fillets and chamfers; default `vertical`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EdgeSelector {
    /// `all`
    All,
    /// `vertical` (default)
    #[default]
    Vertical,
    /// `top`
    Top,
    /// `bottom`
    Bottom,
}

/// `{kind: "fillet", radius, selector}` (1.0 `FilletFeature`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FilletFeature {
    /// Always `"fillet"`.
    #[serde(default)]
    pub kind: FilletTag,
    /// `Dim`.
    #[schemars(schema_with = "crate::model::schema::dim")]
    pub radius: f64,
    /// Edge selector.
    #[serde(default)]
    pub selector: EdgeSelector,
}

/// `{kind: "chamfer", distance, selector}` (1.0 `ChamferFeature`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChamferFeature {
    /// Always `"chamfer"`.
    #[serde(default)]
    pub kind: ChamferTag,
    /// `Dim`.
    #[schemars(schema_with = "crate::model::schema::dim")]
    pub distance: f64,
    /// Edge selector.
    #[serde(default)]
    pub selector: EdgeSelector,
}

fn default_confidence() -> f64 {
    1.0
}

/// `{planner, confidence, review_required}` (1.0 `PlannerMetadata`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlannerMetadata {
    /// Planner name, 1–64 chars.
    #[schemars(length(min = 1, max = 64))]
    pub planner: String,
    /// `0 ≤ x ≤ 1`, default 1.
    #[serde(default = "default_confidence")]
    #[schemars(schema_with = "crate::model::schema::confidence")]
    pub confidence: f64,
    /// Planner's own review flag, default `false`.
    #[serde(default)]
    pub review_required: bool,
}

// ---- shortcut shapes (1.0 `*Base` minus `kind`; the enum tag carries `kind`) ----------------

/// `plate`: `length`, `width`, `thickness` (all `Dim`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlateSpec {
    /// Along local X.
    #[schemars(schema_with = "crate::model::schema::dim")]
    pub length: f64,
    /// Along local Y.
    #[schemars(schema_with = "crate::model::schema::dim")]
    pub width: f64,
    /// Along local Z.
    #[schemars(schema_with = "crate::model::schema::dim")]
    pub thickness: f64,
}

/// `cylinder`: `diameter`, `height` (Volume and Empty cylinders share this).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CylinderSpec {
    /// Diameter.
    #[schemars(schema_with = "crate::model::schema::dim")]
    pub diameter: f64,
    /// Along local Z.
    #[schemars(schema_with = "crate::model::schema::dim")]
    pub height: f64,
}

/// `ring`: `outer_diameter`, `inner_diameter`, `height`; `inner < outer`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RingSpec {
    /// Outer diameter.
    #[schemars(schema_with = "crate::model::schema::dim")]
    pub outer_diameter: f64,
    /// Inner (bore) diameter, must be smaller than `outer_diameter`.
    #[schemars(schema_with = "crate::model::schema::dim")]
    pub inner_diameter: f64,
    /// Along local Z.
    #[schemars(schema_with = "crate::model::schema::dim")]
    pub height: f64,
}

/// `l_bracket`: `width`, `depth`, `vertical_height`, `thickness`;
/// `thickness < min(depth, vertical_height)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LBracketSpec {
    /// Along local X.
    #[schemars(schema_with = "crate::model::schema::dim")]
    pub width: f64,
    /// Along local Y.
    #[schemars(schema_with = "crate::model::schema::dim")]
    pub depth: f64,
    /// Height of the upright above the base plate.
    #[schemars(schema_with = "crate::model::schema::dim")]
    pub vertical_height: f64,
    /// Plate thickness.
    #[schemars(schema_with = "crate::model::schema::dim")]
    pub thickness: f64,
}

/// `enclosure`: `length`, `width`, `height`, `wall_thickness`;
/// `2·wall < min(length, width)` and `wall < height`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnclosureSpec {
    /// Along local X.
    #[schemars(schema_with = "crate::model::schema::dim")]
    pub length: f64,
    /// Along local Y.
    #[schemars(schema_with = "crate::model::schema::dim")]
    pub width: f64,
    /// Along local Z.
    #[schemars(schema_with = "crate::model::schema::dim")]
    pub height: f64,
    /// Wall and floor thickness.
    #[schemars(schema_with = "crate::model::schema::dim")]
    pub wall_thickness: f64,
}

impl PlateSpec {
    /// Cross-field rule violation (none for plates).
    pub fn rule_error(&self) -> Option<&'static str> {
        None
    }
}

impl CylinderSpec {
    /// Cross-field rule violation (none for cylinders).
    pub fn rule_error(&self) -> Option<&'static str> {
        None
    }
}

impl RingSpec {
    /// 1.0 `RingBase.inner_must_be_smaller`.
    pub fn rule_error(&self) -> Option<&'static str> {
        (self.inner_diameter >= self.outer_diameter)
            .then_some("inner_diameter must be smaller than outer_diameter")
    }
}

impl LBracketSpec {
    /// 1.0 `LBracketBase.thickness_must_fit`.
    pub fn rule_error(&self) -> Option<&'static str> {
        (self.thickness >= self.depth.min(self.vertical_height))
            .then_some("thickness must be smaller than depth and vertical_height")
    }
}

impl EnclosureSpec {
    /// 1.0 `EnclosureBase.wall_must_fit`.
    pub fn rule_error(&self) -> Option<&'static str> {
        if self.wall_thickness * 2.0 >= self.length.min(self.width) {
            return Some("wall_thickness is too large for enclosure length/width");
        }
        if self.wall_thickness >= self.height {
            return Some("wall_thickness must be smaller than enclosure height");
        }
        None
    }
}

/// 1.0 `HoleFeature.validate_subtype`, shared by 1.0 holes and 2.0 `hole` Empties.
#[allow(clippy::too_many_arguments)]
pub fn hole_subtype_error(
    hole_type: HoleType,
    diameter: f64,
    depth: Option<f64>,
    counterbore_diameter: Option<f64>,
    counterbore_depth: Option<f64>,
    countersink_diameter: Option<f64>,
    countersink_angle: Option<f64>,
) -> Option<&'static str> {
    match hole_type {
        HoleType::Blind if depth.is_none() => Some("blind hole requires depth"),
        HoleType::Counterbore => match (counterbore_diameter, counterbore_depth) {
            (Some(d), Some(_)) if d <= diameter => {
                Some("counterbore_diameter must exceed hole diameter")
            }
            (Some(_), Some(_)) => None,
            _ => Some("counterbore requires counterbore_diameter and counterbore_depth"),
        },
        HoleType::Countersink => match (countersink_diameter, countersink_angle) {
            (Some(d), Some(_)) if d <= diameter => {
                Some("countersink_diameter must exceed hole diameter")
            }
            (Some(_), Some(_)) => None,
            _ => Some("countersink requires countersink_diameter and countersink_angle"),
        },
        _ => None,
    }
}

// ---- checks ---------------------------------------------------------------------------------

impl FilletFeature {
    pub(crate) fn check(&self, c: &mut Checker, path: &str) {
        c.dim(&format!("{path}/radius"), self.radius);
    }
}

impl ChamferFeature {
    pub(crate) fn check(&self, c: &mut Checker, path: &str) {
        c.dim(&format!("{path}/distance"), self.distance);
    }
}

impl PlannerMetadata {
    pub(crate) fn check(&self, c: &mut Checker, path: &str) {
        c.text(&format!("{path}/planner"), &self.planner, 1, 64);
        c.closed_range(&format!("{path}/confidence"), self.confidence, 0.0, 1.0);
    }
}

macro_rules! check_dims_and_rule {
    ($ty:ty, [$($field:ident),+]) => {
        impl $ty {
            pub(crate) fn check(&self, c: &mut Checker, path: &str) {
                $( c.dim(&format!("{path}/{}", stringify!($field)), self.$field); )+
                if let Some(msg) = self.rule_error() {
                    c.err(path, msg);
                }
            }
        }
    };
}

check_dims_and_rule!(PlateSpec, [length, width, thickness]);
check_dims_and_rule!(CylinderSpec, [diameter, height]);
check_dims_and_rule!(RingSpec, [outer_diameter, inner_diameter, height]);
check_dims_and_rule!(LBracketSpec, [width, depth, vertical_height, thickness]);
check_dims_and_rule!(EnclosureSpec, [length, width, height, wall_thickness]);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shape_rules() {
        let ring = RingSpec {
            outer_diameter: 10.0,
            inner_diameter: 10.0,
            height: 1.0,
        };
        assert!(ring.rule_error().is_some());
        let br = LBracketSpec {
            width: 10.0,
            depth: 5.0,
            vertical_height: 20.0,
            thickness: 5.0,
        };
        assert!(br.rule_error().is_some());
        let enc = EnclosureSpec {
            length: 100.0,
            width: 70.0,
            height: 33.0,
            wall_thickness: 2.0,
        };
        assert!(enc.rule_error().is_none());
        let enc_bad = EnclosureSpec {
            wall_thickness: 35.0,
            ..enc
        };
        assert!(enc_bad.rule_error().is_some());
    }

    #[test]
    fn hole_subtype() {
        use HoleType::*;
        assert!(hole_subtype_error(Blind, 5.0, None, None, None, None, None).is_some());
        assert!(hole_subtype_error(Blind, 5.0, Some(1.0), None, None, None, None).is_none());
        assert!(
            hole_subtype_error(Counterbore, 5.0, None, Some(5.0), Some(1.0), None, None).is_some()
        );
        assert!(hole_subtype_error(Counterbore, 5.0, None, Some(8.0), None, None, None).is_some());
        assert!(
            hole_subtype_error(Counterbore, 5.0, None, Some(8.0), Some(1.0), None, None).is_none()
        );
        assert!(
            hole_subtype_error(Countersink, 5.0, None, None, None, Some(9.0), Some(90.0)).is_none()
        );
        assert!(
            hole_subtype_error(Countersink, 5.0, None, None, None, Some(4.0), Some(90.0)).is_some()
        );
        assert!(hole_subtype_error(Through, 5.0, None, None, None, None, None).is_none());
    }

    #[test]
    fn fillet_defaults() {
        let f: FilletFeature = serde_json::from_str(r#"{"radius": 5}"#).unwrap();
        assert_eq!(f.selector, EdgeSelector::Vertical);
        assert_eq!(
            serde_json::to_value(&f).unwrap(),
            serde_json::json!({"kind": "fillet", "radius": 5.0, "selector": "vertical"})
        );
        assert!(serde_json::from_str::<FilletFeature>(r#"{"kind":"chamfer","radius":5}"#).is_err());
    }
}
