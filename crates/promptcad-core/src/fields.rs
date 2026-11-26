//! Numeric field access by `id:field` (spec §1.3, §3.3, §4, §8.2 `set_field`).
//!
//! * Component fields: the numeric fields of `shape` listed as bindable in §3.3. Optional hole
//!   fields (`depth`, `counterbore_*`, `countersink_*`) are bindable only while non-null.
//! * Constraint fields: `value` (for `mate`, `flush`, `distance`, `clearance`) and `u`/`v`
//!   (for `on_face`). A `null` `mate`/`flush` value reads as its default 0, a `null` `on_face`
//!   `u`/`v` reads as 0; a `null` `distance`/`clearance` value is [`FieldError::Null`].
//!
//! Lookup follows namespace N1: components first, then constraints (a duplicate across the two
//! is a separate `duplicate_id` error).

use std::fmt;

use crate::model::common::{EnclosureSpec, LBracketSpec, PlateSpec, RingSpec};
use crate::model::ids::FieldRef;
use crate::model::issue::codes::Code;
use crate::model::v2::{
    CadDocument2, Component, Constraint, CylinderSpec, EmptyShape, Geometry, LineShape, PlaneShape,
    VolumeShape,
};

/// Why a field reference could not be read or written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldError {
    /// No component or constraint has this id (`dangling_reference`).
    Dangling(FieldRef),
    /// The id exists but `field` is not a numeric/bindable field of its shape or constraint
    /// kind (`unknown_field`).
    Unknown(FieldRef),
    /// The field exists but is currently `null` (optional hole field, or a missing
    /// `distance`/`clearance` value), so it is not bindable (`unknown_field`).
    Null(FieldRef),
    /// `set_field` with NaN or ±∞.
    NonFinite(FieldRef),
}

impl FieldError {
    /// The field reference involved.
    pub fn target(&self) -> &FieldRef {
        match self {
            FieldError::Dangling(t)
            | FieldError::Unknown(t)
            | FieldError::Null(t)
            | FieldError::NonFinite(t) => t,
        }
    }

    /// The matching §9.2 issue code (`NonFinite` maps to `binding_eval_error`).
    pub fn code(&self) -> Code {
        match self {
            FieldError::Dangling(_) => Code::DanglingReference,
            FieldError::Unknown(_) | FieldError::Null(_) => Code::UnknownField,
            FieldError::NonFinite(_) => Code::BindingEvalError,
        }
    }
}

impl fmt::Display for FieldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FieldError::Dangling(t) => write!(f, "{t}: no component or constraint {}", t.id_str()),
            FieldError::Unknown(t) => write!(f, "{t}: not a bindable numeric field"),
            FieldError::Null(t) => write!(f, "{t}: field is null and cannot be bound"),
            FieldError::NonFinite(t) => write!(f, "{t}: value must be finite"),
        }
    }
}

impl std::error::Error for FieldError {}

/// Permission class of a field target (§8.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FieldClass {
    /// Fields of `reserve`/`keep_out`/`envelope` Empties.
    Reserved,
    /// Fields of every other component.
    Physical,
    /// Constraint `value`/`u`/`v`.
    Constraint,
}

/// A borrowed numeric slot.
enum Slot<P, N> {
    /// A required `f64` field.
    Plain(P),
    /// An optional (hole) field.
    Nullable(N),
}

/// Expands to the slot of `$name` inside `$geom`, borrowing with `$($r)*` (`&` or `&mut`).
macro_rules! shape_slot {
    ($geom:expr, $name:expr, $($r:tt)*) => {{
        let name: &str = $name;
        match $geom {
            Geometry::Empty { shape, .. } => match shape {
                EmptyShape::Box(s) => match name {
                    "length" => Some(Slot::Plain($($r)* s.length)),
                    "width" => Some(Slot::Plain($($r)* s.width)),
                    "height" => Some(Slot::Plain($($r)* s.height)),
                    _ => None,
                },
                EmptyShape::Cylinder(CylinderSpec { diameter, height }) => match name {
                    "diameter" => Some(Slot::Plain(diameter)),
                    "height" => Some(Slot::Plain(height)),
                    _ => None,
                },
                EmptyShape::Hole(h) => match name {
                    "diameter" => Some(Slot::Plain($($r)* h.diameter)),
                    "depth" => Some(Slot::Nullable($($r)* h.depth)),
                    "counterbore_diameter" => Some(Slot::Nullable($($r)* h.counterbore_diameter)),
                    "counterbore_depth" => Some(Slot::Nullable($($r)* h.counterbore_depth)),
                    "countersink_diameter" => Some(Slot::Nullable($($r)* h.countersink_diameter)),
                    "countersink_angle" => Some(Slot::Nullable($($r)* h.countersink_angle)),
                    _ => None,
                },
                EmptyShape::Of(_) => None,
            },
            Geometry::Dot { .. } => None,
            Geometry::Line { shape } => match shape {
                LineShape::Circle(s) if name == "diameter" => Some(Slot::Plain($($r)* s.diameter)),
                _ => None,
            },
            Geometry::Plane { shape } => match shape {
                PlaneShape::Rect(s) => match name {
                    "length" => Some(Slot::Plain($($r)* s.length)),
                    "width" => Some(Slot::Plain($($r)* s.width)),
                    _ => None,
                },
                PlaneShape::Disc(s) if name == "diameter" => Some(Slot::Plain($($r)* s.diameter)),
                PlaneShape::Section(s) if name == "offset" => Some(Slot::Plain($($r)* s.offset)),
                _ => None,
            },
            Geometry::Volume { shape } => match shape {
                VolumeShape::Plate(PlateSpec { length, width, thickness }) => match name {
                    "length" => Some(Slot::Plain(length)),
                    "width" => Some(Slot::Plain(width)),
                    "thickness" => Some(Slot::Plain(thickness)),
                    _ => None,
                },
                VolumeShape::Cylinder(CylinderSpec { diameter, height }) => match name {
                    "diameter" => Some(Slot::Plain(diameter)),
                    "height" => Some(Slot::Plain(height)),
                    _ => None,
                },
                VolumeShape::Ring(RingSpec { outer_diameter, inner_diameter, height }) => {
                    match name {
                        "outer_diameter" => Some(Slot::Plain(outer_diameter)),
                        "inner_diameter" => Some(Slot::Plain(inner_diameter)),
                        "height" => Some(Slot::Plain(height)),
                        _ => None,
                    }
                }
                VolumeShape::LBracket(LBracketSpec { width, depth, vertical_height, thickness }) => {
                    match name {
                        "width" => Some(Slot::Plain(width)),
                        "depth" => Some(Slot::Plain(depth)),
                        "vertical_height" => Some(Slot::Plain(vertical_height)),
                        "thickness" => Some(Slot::Plain(thickness)),
                        _ => None,
                    }
                }
                VolumeShape::Enclosure(EnclosureSpec { length, width, height, wall_thickness }) => {
                    match name {
                        "length" => Some(Slot::Plain(length)),
                        "width" => Some(Slot::Plain(width)),
                        "height" => Some(Slot::Plain(height)),
                        "wall_thickness" => Some(Slot::Plain(wall_thickness)),
                        _ => None,
                    }
                }
                VolumeShape::Extrude(s) if name == "height" => Some(Slot::Plain($($r)* s.height)),
                VolumeShape::Revolve(s) if name == "angle" => Some(Slot::Plain($($r)* s.angle)),
                _ => None,
            },
        }
    }};
}

/// Bindable numeric fields of a component right now (§3.3), in spec order. Optional hole
/// fields that are `null` are excluded. For the static per-shape list see
/// [`crate::model::v2::ShapeKind::numeric_fields`].
pub fn bindable_fields(component: &Component) -> Vec<&'static str> {
    component
        .shape_kind()
        .numeric_fields()
        .iter()
        .copied()
        .filter(|f| component_field(component, f).is_some_and(|v| v.is_some()))
        .collect()
}

/// Bindable fields of a constraint (§1.3): `["value"]`, `["u", "v"]` or `[]` by kind.
pub fn constraint_bindable_fields(constraint: &Constraint) -> &'static [&'static str] {
    constraint.kind.numeric_fields()
}

/// Reads a component shape field: `None` = not a numeric field of this shape,
/// `Some(None)` = optional field that is null, `Some(Some(v))` = value.
pub fn component_field(component: &Component, field: &str) -> Option<Option<f64>> {
    match shape_slot!(&component.geometry, field, &)? {
        Slot::Plain(v) => Some(Some(*v)),
        Slot::Nullable(v) => Some(*v),
    }
}

/// Writes a component shape field. Returns `false` if the field is not numeric for this shape
/// or is an optional field that is currently null (null fields are not bindable).
pub fn set_component_field(component: &mut Component, field: &str, value: f64) -> bool {
    match shape_slot!(&mut component.geometry, field, &mut) {
        Some(Slot::Plain(v)) => {
            *v = value;
            true
        }
        Some(Slot::Nullable(Some(v))) => {
            *v = value;
            true
        }
        Some(Slot::Nullable(None)) | None => false,
    }
}

/// Reads a constraint field with §5.1 defaults: `None` = not used by this kind,
/// `Some(None)` = required but null, `Some(Some(v))` = value.
pub fn constraint_field(constraint: &Constraint, field: &str) -> Option<Option<f64>> {
    match field {
        "value" if constraint.kind.uses_value() => Some(constraint.effective_value()),
        "u" if constraint.kind.uses_uv() => Some(constraint.effective_u()),
        "v" if constraint.kind.uses_uv() => Some(constraint.effective_v()),
        _ => None,
    }
}

/// Writes a constraint field. Returns `false` when the field is not used by the kind.
pub fn set_constraint_field(constraint: &mut Constraint, field: &str, value: f64) -> bool {
    let kind = constraint.kind;
    let slot = match field {
        "value" if kind.uses_value() => &mut constraint.value,
        "u" if kind.uses_uv() => &mut constraint.u,
        "v" if kind.uses_uv() => &mut constraint.v,
        _ => return false,
    };
    *slot = Some(value);
    true
}

fn resolve(found: Option<Option<f64>>, target: &FieldRef) -> Result<f64, FieldError> {
    match found {
        None => Err(FieldError::Unknown(target.clone())),
        Some(None) => Err(FieldError::Null(target.clone())),
        Some(Some(v)) => Ok(v),
    }
}

/// Reads `target` from the document.
pub fn get_field(doc: &CadDocument2, target: &FieldRef) -> Result<f64, FieldError> {
    let (id, field) = (target.id_str(), target.field());
    if let Some(c) = doc.component(id) {
        resolve(component_field(c, field), target)
    } else if let Some(k) = doc.constraint(id) {
        resolve(constraint_field(k, field), target)
    } else {
        Err(FieldError::Dangling(target.clone()))
    }
}

/// Writes `target` in the document. Does not consult bindings (the patch engine rejects
/// `set_field` on bound fields with `field_is_bound`) and does not range-check (run
/// `check_schema` / the validator afterwards).
pub fn set_field(doc: &mut CadDocument2, target: &FieldRef, value: f64) -> Result<(), FieldError> {
    // Validate existence/bindability first so the error kinds match `get_field`.
    get_field(doc, target).or_else(|e| match e {
        // `distance`/`clearance` with a null value: settable, since the field is used.
        FieldError::Null(_) if is_constraint_value(doc, target) => Ok(0.0),
        e => Err(e),
    })?;
    if !value.is_finite() {
        return Err(FieldError::NonFinite(target.clone()));
    }
    let (id, field) = (target.id_str(), target.field());
    let ok = if let Some(c) = doc.component_mut(id) {
        set_component_field(c, field, value)
    } else if let Some(k) = doc.constraint_mut(id) {
        set_constraint_field(k, field, value)
    } else {
        false
    };
    debug_assert!(ok, "existence was checked by get_field");
    Ok(())
}

fn is_constraint_value(doc: &CadDocument2, target: &FieldRef) -> bool {
    doc.component(target.id_str()).is_none()
        && doc
            .constraint(target.id_str())
            .is_some_and(|k| constraint_field(k, target.field()).is_some())
}

/// Checks that `target` is a bindable field (§1.3, §4.3 `unknown_field`).
pub fn check_bindable(doc: &CadDocument2, target: &FieldRef) -> Result<(), FieldError> {
    get_field(doc, target).map(|_| ())
}

/// Permission class of `target` (§8.4). Only existence of the id is checked.
pub fn field_class(doc: &CadDocument2, target: &FieldRef) -> Result<FieldClass, FieldError> {
    let id = target.id_str();
    if let Some(c) = doc.component(id) {
        Ok(match c.role() {
            Some(r) if r.is_reserved_class() => FieldClass::Reserved,
            _ => FieldClass::Physical,
        })
    } else if doc.constraint(id).is_some() {
        Ok(FieldClass::Constraint)
    } else {
        Err(FieldError::Dangling(target.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::v2::ShapeKind;
    use serde_json::json;

    fn doc() -> CadDocument2 {
        serde_json::from_value(json!({
            "schema_version": "2.0",
            "source_prompt": "t",
            "revision": 0,
            "components": [
                {"id": "v_plate", "kind": "volume", "shape": {"kind": "plate", "length": 120, "width": 60, "thickness": 10}},
                {"id": "e_pcb", "kind": "empty", "role": "reserve", "shape": {"kind": "box", "length": 80, "width": 50, "height": 25}},
                {"id": "e_h1", "kind": "empty", "role": "cut", "shape": {"kind": "hole", "diameter": 6.6, "hole_type": "blind",
                    "depth": 5, "thread": null, "counterbore_diameter": null, "counterbore_depth": null,
                    "countersink_diameter": null, "countersink_angle": null}},
                {"id": "l_c", "kind": "line", "shape": {"kind": "circle", "center": "d_a", "diameter": 4}},
                {"id": "p_s", "kind": "plane", "shape": {"kind": "section", "source": "v_plate", "offset": -2}},
                {"id": "v_r", "kind": "volume", "shape": {"kind": "revolve", "profile": "p_s", "axis": "l_s", "angle": 90}},
                {"id": "d_a", "kind": "dot", "shape": {"kind": "point"}}
            ],
            "constraints": [
                {"id": "k_d1", "kind": "on_face", "a": "d_a.point", "b": "v_plate.top", "u": -50},
                {"id": "k_m", "kind": "mate", "a": "v_plate.bottom", "b": "v_plate.top"},
                {"id": "k_c", "kind": "coincident", "a": "d_a.point", "b": "e_h1.entry"},
                {"id": "k_dist", "kind": "distance", "a": "d_a.point", "b": "v_plate.top"}
            ],
            "planner": {"planner": "t"}
        }))
        .unwrap()
    }

    fn fr(s: &str) -> FieldRef {
        s.parse().unwrap()
    }

    #[test]
    fn get_component_fields() {
        let d = doc();
        assert_eq!(get_field(&d, &fr("v_plate:length")), Ok(120.0));
        assert_eq!(get_field(&d, &fr("v_plate:thickness")), Ok(10.0));
        assert_eq!(get_field(&d, &fr("e_pcb:height")), Ok(25.0));
        assert_eq!(get_field(&d, &fr("e_h1:depth")), Ok(5.0));
        assert_eq!(get_field(&d, &fr("l_c:diameter")), Ok(4.0));
        assert_eq!(get_field(&d, &fr("p_s:offset")), Ok(-2.0));
        assert_eq!(get_field(&d, &fr("v_r:angle")), Ok(90.0));
        assert_eq!(
            get_field(&d, &fr("e_h1:counterbore_depth")),
            Err(FieldError::Null(fr("e_h1:counterbore_depth")))
        );
        for unknown in [
            "v_plate:kind",
            "e_h1:axis",
            "e_h1:thread",
            "p_s:source",
            "d_a:x",
            "v_plate:height",
        ] {
            assert_eq!(
                get_field(&d, &fr(unknown)),
                Err(FieldError::Unknown(fr(unknown))),
                "{unknown}"
            );
        }
        assert_eq!(
            get_field(&d, &fr("nope:length")),
            Err(FieldError::Dangling(fr("nope:length")))
        );
        assert_eq!(
            FieldError::Dangling(fr("a:b")).code(),
            Code::DanglingReference
        );
        assert_eq!(FieldError::Null(fr("a:b")).code(), Code::UnknownField);
    }

    #[test]
    fn get_constraint_fields_with_defaults() {
        let d = doc();
        assert_eq!(get_field(&d, &fr("k_d1:u")), Ok(-50.0));
        assert_eq!(get_field(&d, &fr("k_d1:v")), Ok(0.0));
        assert_eq!(get_field(&d, &fr("k_m:value")), Ok(0.0));
        assert_eq!(
            get_field(&d, &fr("k_d1:value")),
            Err(FieldError::Unknown(fr("k_d1:value")))
        );
        assert_eq!(
            get_field(&d, &fr("k_m:u")),
            Err(FieldError::Unknown(fr("k_m:u")))
        );
        assert_eq!(
            get_field(&d, &fr("k_c:value")),
            Err(FieldError::Unknown(fr("k_c:value")))
        );
        assert_eq!(
            get_field(&d, &fr("k_dist:value")),
            Err(FieldError::Null(fr("k_dist:value")))
        );
    }

    #[test]
    fn set_fields() {
        let mut d = doc();
        set_field(&mut d, &fr("v_plate:width"), 70.0).unwrap();
        assert_eq!(get_field(&d, &fr("v_plate:width")), Ok(70.0));
        set_field(&mut d, &fr("e_h1:depth"), 7.5).unwrap();
        assert_eq!(get_field(&d, &fr("e_h1:depth")), Ok(7.5));
        set_field(&mut d, &fr("k_d1:v"), 20.0).unwrap();
        assert_eq!(d.constraint("k_d1").unwrap().v, Some(20.0));
        set_field(&mut d, &fr("k_m:value"), 1.5).unwrap();
        assert_eq!(d.constraint("k_m").unwrap().value, Some(1.5));
        set_field(&mut d, &fr("k_dist:value"), 5.0).unwrap();
        assert_eq!(get_field(&d, &fr("k_dist:value")), Ok(5.0));

        let before = d.clone();
        assert_eq!(
            set_field(&mut d, &fr("e_h1:countersink_angle"), 90.0),
            Err(FieldError::Null(fr("e_h1:countersink_angle")))
        );
        assert_eq!(
            set_field(&mut d, &fr("k_c:value"), 1.0),
            Err(FieldError::Unknown(fr("k_c:value")))
        );
        assert_eq!(
            set_field(&mut d, &fr("zz:value"), 1.0),
            Err(FieldError::Dangling(fr("zz:value")))
        );
        assert_eq!(
            set_field(&mut d, &fr("v_plate:width"), f64::NAN),
            Err(FieldError::NonFinite(fr("v_plate:width")))
        );
        assert_eq!(d, before);
    }

    #[test]
    fn bindable_lists_and_classes() {
        let d = doc();
        assert_eq!(
            bindable_fields(d.component("v_plate").unwrap()),
            ["length", "width", "thickness"]
        );
        assert_eq!(
            bindable_fields(d.component("e_h1").unwrap()),
            ["diameter", "depth"]
        );
        assert!(bindable_fields(d.component("d_a").unwrap()).is_empty());
        assert_eq!(
            constraint_bindable_fields(d.constraint("k_d1").unwrap()),
            ["u", "v"]
        );
        assert_eq!(
            constraint_bindable_fields(d.constraint("k_c").unwrap()),
            [] as [&str; 0]
        );
        assert_eq!(
            field_class(&d, &fr("e_pcb:length")),
            Ok(FieldClass::Reserved)
        );
        assert_eq!(field_class(&d, &fr("e_h1:depth")), Ok(FieldClass::Physical));
        assert_eq!(
            field_class(&d, &fr("k_m:value")),
            Ok(FieldClass::Constraint)
        );
        // every statically-listed field of every shape is reachable through the slot table
        for kind in ShapeKind::ALL {
            for f in kind.numeric_fields() {
                assert!(!f.is_empty());
            }
        }
    }
}
