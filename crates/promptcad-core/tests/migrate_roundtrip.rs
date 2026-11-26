//! Round-trip and partiality tests for `migrate::downgrade_2_to_1` (spec §10.2, §10.3).

use promptcad_core::canonical::{parse_example, to_canonical_string};
use promptcad_core::migrate::{
    downgrade_2_to_1, downgrade_2_to_1_with, normalize_v1, upgrade_1_to_2,
};
use promptcad_core::model::common::{
    ChamferFeature, EdgeSelector, FilletFeature, HoleType, Material,
};
use promptcad_core::model::geom::{Axis, AxisDir, Frame};
use promptcad_core::model::v1::{BaseFeature, HoleFeature};
use promptcad_core::model::v2::{
    BodyOp, BodyOpKind, CadDocument2, Component, Constraint, ConstraintKind, DotShape, Geometry,
    VolumeShape,
};
use promptcad_core::model::{CadDocument1, Id};
use promptcad_core::solver::solve;
use proptest::prelude::*;

const FIXED_EXAMPLE: &str = include_str!("../../../examples/generated/plate-four-holes/spec.json");
const PLATE_CAD2: &str = include_str!("../../../examples/cad2/plate-four-holes.cad2.json");
const ENCLOSURE_CAD2: &str = include_str!("../../../examples/cad2/enclosure-with-lid.cad2.json");

fn id(s: &str) -> Id {
    Id::new(s).unwrap()
}

fn doc1(holes: serde_json::Value) -> CadDocument1 {
    let v = serde_json::json!({
        "source_prompt": "x",
        "base": {"kind":"plate","length":120,"width":60,"thickness":10},
        "holes": holes,
        "planner": {"planner": "rule"}
    });
    CadDocument1::from_json_str(&v.to_string()).expect("valid doc")
}

fn two() -> CadDocument2 {
    upgrade_1_to_2(&doc1(serde_json::json!([
        {"x": 5, "y": 6, "diameter": 3},
        {"x": -7, "y": 2, "diameter": 4, "hole_type": "blind", "depth": 2}
    ])))
}

fn fixed_k(kid: &str, a: &str, frame: Frame) -> Constraint {
    Constraint {
        id: id(kid),
        kind: ConstraintKind::Fixed,
        a: a.parse().unwrap(),
        b: None,
        value: None,
        u: None,
        v: None,
        frame: Some(frame),
    }
}

fn constraint_mut<'a>(d: &'a mut CadDocument2, kid: &str) -> &'a mut Constraint {
    d.constraints.iter_mut().find(|k| k.id == kid).unwrap()
}

fn volume2(cid: &str) -> Component {
    let mut c = two().component("v_base").unwrap().clone();
    c.id = id(cid);
    c
}

#[test]
fn fixed_example_round_trips_exactly() {
    let d1 = CadDocument1::from_json_str(FIXED_EXAMPLE).unwrap();
    let d2 = upgrade_1_to_2(&d1);
    let back = downgrade_2_to_1(&d2).expect("downgradable");
    assert_eq!(back, d1);
    assert_eq!(to_canonical_string(&back), to_canonical_string(&d1));
    // the `_with` variant agrees with the solving one
    assert_eq!(downgrade_2_to_1_with(&d2, &solve(&d2)), Some(d1));
}

#[test]
fn cad2_plate_with_on_face_holes_downgrades() {
    let (d2, _) = parse_example(PLATE_CAD2).unwrap();
    let d1 = downgrade_2_to_1(&d2).expect("downgradable");
    assert!(matches!(&d1.base, BaseFeature::Plate(p) if p.length == 120.0 && p.thickness == 10.0));
    let xyz: Vec<_> = d1.holes.iter().map(|h| (h.x, h.y, h.z)).collect();
    assert_eq!(
        xyz,
        [
            (-50.0, -20.0, 0.0),
            (50.0, -20.0, 0.0),
            (50.0, 20.0, 0.0),
            (-50.0, 20.0, 0.0)
        ]
    );
    assert!(d1
        .holes
        .iter()
        .all(|h| h.hole_type == HoleType::Clearance && h.axis == Axis::Z));
    assert!(d1.check_schema().is_empty());
}

#[test]
fn multi_part_example_is_none() {
    let (d2, _) = parse_example(ENCLOSURE_CAD2).unwrap();
    assert!(d2.parts.len() > 1);
    assert_eq!(downgrade_2_to_1(&d2), None);
}

#[test]
fn axis_x_and_y_holes_round_trip() {
    let d1 = doc1(serde_json::json!([
        {"x": 0, "y": 5, "z": 3, "axis": "x", "diameter": 3},
        {"x": -4, "y": 0, "z": 7, "axis": "y", "diameter": 2},
        {"x": 1, "y": 2, "axis": "z", "diameter": 2}
    ]));
    let n = normalize_v1(&d1);
    let back = downgrade_2_to_1(&upgrade_1_to_2(&n)).unwrap();
    assert_eq!(back, n);
    let h = &back.holes;
    assert_eq!((h[0].x, h[0].y, h[0].z), (0.0, 5.0, 3.0));
    assert_eq!((h[1].x, h[1].y, h[1].z), (-4.0, 0.0, 7.0));
}

#[test]
fn upgraded_document_is_downgradable_baseline() {
    assert!(downgrade_2_to_1(&two()).is_some());
}

#[test]
fn lossy_cases_still_downgrade() {
    let mut d = two();
    d.components.push(Component {
        id: id("d_helper"),
        geometry: Geometry::Dot {
            shape: DotShape::Point {},
        },
        label: Some("helper".into()),
        provenance: None,
    });
    d.constraints.push(fixed_k(
        "k_helper",
        "d_helper.point",
        Frame::at([1.0, 2.0, 3.0]),
    ));
    d.parts[0].quantity = 4;
    let back = downgrade_2_to_1(&d).expect("lossy but downgradable");
    assert_eq!(back.holes.len(), 2);
}

#[test]
fn holes_follow_cut_order() {
    let mut d = two();
    d.parts[0].body.swap(1, 2);
    let back = downgrade_2_to_1(&d).unwrap();
    assert_eq!(back.holes[0].x, -7.0);
    assert_eq!(back.holes[1].x, 5.0);
}

#[test]
fn none_when_two_parts() {
    let mut d = two();
    let mut p = d.parts[0].clone();
    p.id = id("p_other");
    d.parts.push(p);
    assert_eq!(downgrade_2_to_1(&d), None);
}

#[test]
fn none_when_volume_rotated() {
    let mut d = two();
    constraint_mut(&mut d, "k_base").frame = Some(Frame {
        origin: [0.0, 0.0, 0.0],
        x_axis: AxisDir::PosY,
        z_axis: AxisDir::PosZ,
    });
    assert_eq!(downgrade_2_to_1(&d), None);
}

#[test]
fn none_when_volume_off_origin() {
    let mut d = two();
    constraint_mut(&mut d, "k_base").frame = Some(Frame::at([1.0, 0.0, 0.0]));
    assert_eq!(downgrade_2_to_1(&d), None);
}

#[test]
fn none_when_hole_enters_from_side_face() {
    let mut d = two();
    let k = constraint_mut(&mut d, "k_dh1");
    k.kind = ConstraintKind::OnFace;
    k.frame = None;
    k.b = Some("v_base.x_pos".parse().unwrap());
    k.u = Some(0.0);
    k.v = Some(0.0);
    assert_eq!(downgrade_2_to_1(&d), None);
}

#[test]
fn hole_on_top_face_via_on_face_downgrades() {
    let mut d = two();
    let k = constraint_mut(&mut d, "k_dh1");
    k.kind = ConstraintKind::OnFace;
    k.frame = None;
    k.b = Some("v_base.top".parse().unwrap());
    k.u = Some(11.0);
    k.v = Some(-3.0);
    let back = downgrade_2_to_1(&d).unwrap();
    let h = &back.holes[0];
    assert_eq!((h.x, h.y, h.z), (11.0, -3.0, 0.0));
}

#[test]
fn none_when_body_is_not_shortcut_volume_plus_hole_cuts() {
    let mut d = two();
    d.parts[0].body[0].component = id("d_h1");
    assert_eq!(downgrade_2_to_1(&d), None);
    let mut d = two();
    d.parts[0].body[1].component = id("v_base"); // cut of a non-hole
    assert_eq!(downgrade_2_to_1(&d), None);
}

#[test]
fn none_when_extrude_or_revolve_volume() {
    let (mut d, _) = parse_example(PLATE_CAD2).unwrap();
    let other = parse_example(ENCLOSURE_CAD2).unwrap().0;
    let shape = other.components.iter().find_map(|c| match &c.geometry {
        Geometry::Volume {
            shape: s @ (VolumeShape::Extrude(_) | VolumeShape::Revolve(_)),
        } => Some(s.clone()),
        _ => None,
    });
    // The enclosure example may not contain one; the test then only checks the baseline.
    assert!(downgrade_2_to_1(&d).is_some());
    if let Some(s) = shape {
        for c in &mut d.components {
            if let Geometry::Volume { shape } = &mut c.geometry {
                *shape = s.clone();
            }
        }
        assert_eq!(downgrade_2_to_1(&d), None);
    }
}

#[test]
fn none_when_extra_add() {
    let mut d = two();
    d.components.push(volume2("v_two"));
    d.constraints.push(fixed_k(
        "k_two",
        "v_two.origin",
        Frame::at([500.0, 0.0, 0.0]),
    ));
    d.parts[0].body.push(BodyOp {
        op: BodyOpKind::Add,
        component: id("v_two"),
    });
    assert_eq!(downgrade_2_to_1(&d), None);
}

#[test]
fn none_when_intersect() {
    let mut d = two();
    d.components.push(volume2("v_two"));
    d.constraints
        .push(fixed_k("k_two", "v_two.origin", Frame::at([0.0, 0.0, 0.0])));
    d.parts[0].body.push(BodyOp {
        op: BodyOpKind::Intersect,
        component: id("v_two"),
    });
    assert_eq!(downgrade_2_to_1(&d), None);
}

#[test]
fn none_on_solver_error() {
    let mut d = two();
    d.constraints.retain(|k| k.kind != ConstraintKind::Fixed); // no fixed root
    assert!(solve(&d)
        .issues
        .iter()
        .any(|i| i.severity == promptcad_core::model::Severity::Error));
    assert_eq!(downgrade_2_to_1(&d), None);
}

// ---- property tests --------------------------------------------------------------------------

fn dim() -> impl Strategy<Value = f64> {
    (1u32..4000).prop_map(|v| f64::from(v) / 8.0)
}

fn coord() -> impl Strategy<Value = f64> {
    (-4000i32..4000).prop_map(|v| f64::from(v) / 8.0)
}

fn frac() -> impl Strategy<Value = f64> {
    (1u32..9).prop_map(|v| f64::from(v) / 10.0)
}

fn base_strategy() -> impl Strategy<Value = BaseFeature> {
    use promptcad_core::model::common::{
        CylinderSpec, EnclosureSpec, LBracketSpec, PlateSpec, RingSpec,
    };
    prop_oneof![
        (dim(), dim(), dim()).prop_map(|(length, width, thickness)| BaseFeature::Plate(
            PlateSpec {
                length,
                width,
                thickness
            }
        )),
        (dim(), dim()).prop_map(|(diameter, height)| BaseFeature::Cylinder(CylinderSpec {
            diameter,
            height
        })),
        (dim(), frac(), dim()).prop_map(|(o, f, height)| BaseFeature::Ring(RingSpec {
            outer_diameter: o,
            inner_diameter: o * f,
            height
        })),
        (dim(), dim(), dim(), frac()).prop_map(|(width, depth, vh, f)| BaseFeature::LBracket(
            LBracketSpec {
                width,
                depth,
                vertical_height: vh,
                thickness: depth.min(vh) * f
            }
        )),
        (dim(), dim(), dim(), frac(), frac()).prop_map(|(length, width, height, f, g)| {
            BaseFeature::Enclosure(EnclosureSpec {
                length,
                width,
                height,
                wall_thickness: (length.min(width) / 2.0 * f).min(height * g),
            })
        }),
    ]
}

fn thread_strategy() -> impl Strategy<Value = Option<String>> {
    prop_oneof![
        Just(None),
        prop::sample::select(vec![
            "M6",
            " m6x1 ",
            "M8X1.25",
            "M6 tapped",
            "bolt",
            "",
            "M1000",
            "m10.5",
            "  ",
        ])
        .prop_map(|s| Some(s.to_owned())),
    ]
}

fn hole_strategy() -> impl Strategy<Value = HoleFeature> {
    let head = (
        coord(),
        coord(),
        coord(),
        prop::sample::select(vec![Axis::X, Axis::Y, Axis::Z]),
        dim(),
        thread_strategy(),
    );
    let ty = (
        prop::sample::select(vec![
            HoleType::Through,
            HoleType::Blind,
            HoleType::Clearance,
            HoleType::Tapped,
            HoleType::Counterbore,
            HoleType::Countersink,
        ]),
        dim(),
        dim(),
        dim(),
        dim(),
        (1u32..179).prop_map(f64::from),
        any::<bool>(),
    );
    (head, ty).prop_map(
        |((x, y, z, axis, diameter, thread), (ht, a, b, c, d, ang, extra))| {
            let mut h: HoleFeature = serde_json::from_value(
                serde_json::json!({"x": x, "y": y, "z": z, "axis": axis, "diameter": diameter}),
            )
            .unwrap();
            h.hole_type = ht;
            h.thread = thread;
            match ht {
                HoleType::Blind => h.depth = Some(a),
                HoleType::Counterbore => {
                    h.counterbore_diameter = Some(diameter + b);
                    h.counterbore_depth = Some(c);
                }
                HoleType::Countersink => {
                    h.countersink_diameter = Some(diameter + d);
                    h.countersink_angle = Some(ang);
                }
                _ => {
                    if extra {
                        h.depth = Some(a);
                    }
                }
            }
            h
        },
    )
}

fn edge_selector() -> impl Strategy<Value = EdgeSelector> {
    prop::sample::select(vec![
        EdgeSelector::All,
        EdgeSelector::Vertical,
        EdgeSelector::Top,
        EdgeSelector::Bottom,
    ])
}

fn doc_strategy() -> impl Strategy<Value = CadDocument1> {
    let parts = (
        base_strategy(),
        prop::collection::vec(hole_strategy(), 0..=8),
        prop::collection::vec((dim(), edge_selector()), 0..=3),
        prop::collection::vec((dim(), edge_selector()), 0..=3),
        prop::option::of(prop::sample::select(vec![
            Material::Aluminum,
            Material::Steel,
            Material::StainlessSteel,
            Material::Plastic,
            Material::Wood,
            Material::Other,
        ])),
        prop::collection::vec("[a-z ]{0,12}", 0..=32),
        prop::collection::vec("[a-z ]{0,12}", 0..=32),
    );
    parts.prop_map(|(base, holes, fil, cha, material, assumptions, notes)| {
        let mut d = CadDocument1::from_json_str(
            r#"{"source_prompt":"p","base":{"kind":"plate","length":1,"width":1,"thickness":1},
                "planner":{"planner":"rule"}}"#,
        )
        .unwrap();
        d.base = base;
        d.holes = holes;
        d.fillets = fil
            .into_iter()
            .map(|(radius, selector)| FilletFeature {
                radius,
                selector,
                ..serde_json::from_str(r#"{"radius":1}"#).unwrap()
            })
            .collect();
        d.chamfers = cha
            .into_iter()
            .map(|(distance, selector)| ChamferFeature {
                distance,
                selector,
                ..serde_json::from_str(r#"{"distance":1}"#).unwrap()
            })
            .collect();
        d.material = material;
        d.assumptions = assumptions;
        d.notes = notes;
        d
    })
}

proptest! {
    #[test]
    fn round_trip_on_normalized_documents(d in doc_strategy()) {
        let n = normalize_v1(&d);
        let up = upgrade_1_to_2(&n);
        // the thread note must not be appended a second time
        prop_assert_eq!(&up.notes, &n.notes);
        prop_assert_eq!(downgrade_2_to_1(&up), Some(n));
    }
}
