//! CadDocument 1.0, mirroring `app/models/cad.py` exactly (field names, order, defaults and
//! model validators), so existing 1.0 `spec.json` files parse and round-trip.
//!
//! Validators that Pydantic runs at parse time are exposed as [`CadDocument1::check_schema`];
//! [`CadDocument1::from_json_str`] runs both.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::common::{hole_subtype_error, literal_tag};
pub use super::common::{
    ChamferFeature, CylinderSpec, EdgeSelector, EnclosureSpec, FilletFeature, HoleType,
    LBracketSpec, Material, PlannerMetadata, PlateSpec, RingSpec, Unit,
};
use super::geom::Axis;
use super::schema::{Checker, LoadError, SchemaViolation};

/// List caps of the 1.0 model.
pub mod limits {
    /// `holes` max length.
    pub const HOLES: usize = 64;
    /// `fillets` / `chamfers` max length.
    pub const EDGE_FEATURES: usize = 8;
    /// `assumptions` / `notes` max length.
    pub const NOTES: usize = 32;
    /// `name` max length.
    pub const NAME: usize = 80;
    /// `source_prompt` max length.
    pub const SOURCE_PROMPT: usize = 20_000;
    /// `thread` max length.
    pub const THREAD: usize = 32;
}

/// `schema_version: "1.0"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
pub enum SchemaVersion1 {
    /// `"1.0"`
    #[default]
    #[serde(rename = "1.0")]
    V1_0,
}

literal_tag!(
    /// `kind: "hole"` of a 1.0 hole.
    HoleTag,
    "hole"
);

/// 1.0 `BaseFeature`, discriminated by `kind`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BaseFeature {
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
}

fn is_zero_default() -> f64 {
    0.0
}

/// 1.0 `HoleFeature`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HoleFeature {
    /// Always `"hole"`.
    #[serde(default)]
    pub kind: HoleTag,
    /// `Coord`, default 0.
    #[serde(default = "is_zero_default")]
    #[schemars(schema_with = "crate::model::schema::coord_default0")]
    pub x: f64,
    /// `Coord`, default 0.
    #[serde(default = "is_zero_default")]
    #[schemars(schema_with = "crate::model::schema::coord_default0")]
    pub y: f64,
    /// `Coord`, default 0.
    #[serde(default = "is_zero_default")]
    #[schemars(schema_with = "crate::model::schema::coord_default0")]
    pub z: f64,
    /// Hole axis, default `z`.
    #[serde(default)]
    pub axis: Axis,
    /// `Dim`.
    #[schemars(schema_with = "crate::model::schema::dim")]
    pub diameter: f64,
    /// Default `through`.
    #[serde(default)]
    pub hole_type: HoleType,
    /// `Dim | null`.
    #[serde(default)]
    #[schemars(schema_with = "crate::model::schema::opt_dim")]
    pub depth: Option<f64>,
    /// Free text, ≤ 32 chars.
    #[serde(default)]
    #[schemars(length(max = 32))]
    pub thread: Option<String>,
    /// `Dim | null`.
    #[serde(default)]
    #[schemars(schema_with = "crate::model::schema::opt_dim")]
    pub counterbore_diameter: Option<f64>,
    /// `Dim | null`.
    #[serde(default)]
    #[schemars(schema_with = "crate::model::schema::opt_dim")]
    pub counterbore_depth: Option<f64>,
    /// `Dim | null`.
    #[serde(default)]
    #[schemars(schema_with = "crate::model::schema::opt_dim")]
    pub countersink_diameter: Option<f64>,
    /// `0 < x < 180 | null`.
    #[serde(default)]
    #[schemars(schema_with = "crate::model::schema::opt_countersink_angle")]
    pub countersink_angle: Option<f64>,
}

fn default_name_v1() -> String {
    "promptcad-part".to_owned()
}

/// CadDocument 1.0 (`app/models/cad.py::CadDocument`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "CadDocument1")]
pub struct CadDocument1 {
    /// `"1.0"`.
    #[serde(default)]
    pub schema_version: SchemaVersion1,
    /// 1–80 chars, default `"promptcad-part"`.
    #[serde(default = "default_name_v1")]
    #[schemars(length(min = 1, max = 80))]
    pub name: String,
    /// 1–20000 chars.
    #[schemars(length(min = 1, max = 20000))]
    pub source_prompt: String,
    /// `"mm"`.
    #[serde(default)]
    pub unit: Unit,
    /// Default `null`.
    #[serde(default)]
    pub material: Option<Material>,
    /// The single base solid.
    pub base: BaseFeature,
    /// ≤ 64.
    #[serde(default)]
    #[schemars(length(max = 64))]
    pub holes: Vec<HoleFeature>,
    /// ≤ 8.
    #[serde(default)]
    #[schemars(length(max = 8))]
    pub fillets: Vec<FilletFeature>,
    /// ≤ 8.
    #[serde(default)]
    #[schemars(length(max = 8))]
    pub chamfers: Vec<ChamferFeature>,
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

impl BaseFeature {
    /// The 1.0 `kind` string.
    pub fn kind_str(&self) -> &'static str {
        match self {
            BaseFeature::Plate(_) => "plate",
            BaseFeature::Cylinder(_) => "cylinder",
            BaseFeature::Ring(_) => "ring",
            BaseFeature::LBracket(_) => "l_bracket",
            BaseFeature::Enclosure(_) => "enclosure",
        }
    }

    pub(crate) fn check(&self, c: &mut Checker, path: &str) {
        match self {
            BaseFeature::Plate(s) => s.check(c, path),
            BaseFeature::Cylinder(s) => s.check(c, path),
            BaseFeature::Ring(s) => s.check(c, path),
            BaseFeature::LBracket(s) => s.check(c, path),
            BaseFeature::Enclosure(s) => s.check(c, path),
        }
    }
}

impl HoleFeature {
    /// 1.0 `validate_subtype`.
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

    pub(crate) fn check(&self, c: &mut Checker, path: &str) {
        c.coord(&format!("{path}/x"), self.x);
        c.coord(&format!("{path}/y"), self.y);
        c.coord(&format!("{path}/z"), self.z);
        c.dim(&format!("{path}/diameter"), self.diameter);
        c.opt_dim(&format!("{path}/depth"), self.depth);
        c.opt_text(
            &format!("{path}/thread"),
            self.thread.as_deref(),
            limits::THREAD,
        );
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

impl CadDocument1 {
    /// Pydantic-equivalent field constraints and model validators.
    pub fn check_schema(&self) -> Vec<SchemaViolation> {
        let mut c = Checker::new();
        c.text("/name", &self.name, 1, limits::NAME);
        c.text(
            "/source_prompt",
            &self.source_prompt,
            1,
            limits::SOURCE_PROMPT,
        );
        self.base.check(&mut c, "/base");
        c.list("/holes", self.holes.len(), 0, limits::HOLES);
        for (i, h) in self.holes.iter().enumerate() {
            h.check(&mut c, &format!("/holes/{i}"));
        }
        c.list("/fillets", self.fillets.len(), 0, limits::EDGE_FEATURES);
        for (i, f) in self.fillets.iter().enumerate() {
            f.check(&mut c, &format!("/fillets/{i}"));
        }
        c.list("/chamfers", self.chamfers.len(), 0, limits::EDGE_FEATURES);
        for (i, f) in self.chamfers.iter().enumerate() {
            f.check(&mut c, &format!("/chamfers/{i}"));
        }
        c.list("/assumptions", self.assumptions.len(), 0, limits::NOTES);
        c.list("/notes", self.notes.len(), 0, limits::NOTES);
        self.planner.check(&mut c, "/planner");
        c.finish()
    }

    /// Parses JSON and runs [`CadDocument1::check_schema`].
    pub fn from_json_str(s: &str) -> Result<Self, LoadError> {
        let doc: Self = serde_json::from_str(s)?;
        let v = doc.check_schema();
        if v.is_empty() {
            Ok(doc)
        } else {
            Err(LoadError::Schema(v))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"{
        "source_prompt": "plate",
        "base": {"kind": "plate", "length": 10, "width": 5, "thickness": 1},
        "holes": [{"diameter": 2}],
        "planner": {"planner": "rule"}
    }"#;

    #[test]
    fn defaults_match_pydantic() {
        let d = CadDocument1::from_json_str(MINIMAL).unwrap();
        assert_eq!(d.name, "promptcad-part");
        assert_eq!(d.material, None);
        let h = &d.holes[0];
        assert_eq!((h.x, h.y, h.z), (0.0, 0.0, 0.0));
        assert_eq!(h.axis, Axis::Z);
        assert_eq!(h.hole_type, HoleType::Through);
        assert_eq!(d.planner.confidence, 1.0);
        assert!(!d.planner.review_required);
        let v = serde_json::to_value(&d).unwrap();
        assert_eq!(v["schema_version"], "1.0");
        assert_eq!(v["unit"], "mm");
        assert_eq!(v["holes"][0]["kind"], "hole");
        assert_eq!(v["holes"][0]["depth"], serde_json::Value::Null);
    }

    #[test]
    fn validators() {
        let mut d: CadDocument1 = serde_json::from_str(MINIMAL).unwrap();
        d.holes[0].hole_type = HoleType::Blind;
        d.base = BaseFeature::Ring(RingSpec {
            outer_diameter: 5.0,
            inner_diameter: 6.0,
            height: 1.0,
        });
        d.planner.confidence = 1.5;
        let paths: Vec<_> = d.check_schema().into_iter().map(|v| v.path).collect();
        assert_eq!(paths, ["/base", "/holes/0", "/planner/confidence"]);
    }

    #[test]
    fn rejects_unknown_and_wrong_version() {
        assert!(serde_json::from_str::<CadDocument1>(
            &MINIMAL.replace("\"source_prompt\"", "\"bogus\": 1, \"source_prompt\"")
        )
        .is_err());
        assert!(serde_json::from_str::<CadDocument1>(&MINIMAL.replace(
            "\"source_prompt\"",
            "\"schema_version\": \"2.0\", \"source_prompt\""
        ))
        .is_err());
    }
}
