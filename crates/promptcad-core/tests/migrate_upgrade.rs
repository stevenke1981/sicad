//! Tests for `migrate::upgrade_1_to_2` / `normalize_v1` (spec §10.1, §10.3).

use promptcad_core::canonical::to_canonical_json;
use promptcad_core::migrate::{extents_v1, normalize_v1, upgrade_1_to_2};
use promptcad_core::model::common::{
    ChamferFeature, EdgeSelector, FilletFeature, HoleType, Material,
};
use promptcad_core::model::geom::{Axis, Frame};
use promptcad_core::model::v1::{BaseFeature, HoleFeature};
use promptcad_core::model::v2::{
    BodyOpKind, CadDocument2, ConstraintKind, EmptyRole, EmptyShape, Geometry, ProvenanceAgent,
};
use promptcad_core::model::CadDocument1;
use proptest::prelude::*;

const FIXED_EXAMPLE: &str = include_str!("../../../examples/generated/plate-four-holes/spec.json");

fn example() -> CadDocument1 {
    CadDocument1::from_json_str(FIXED_EXAMPLE).expect("example parses")
}

fn base_json(kind: &str) -> serde_json::Value {
    match kind {
        "plate" => serde_json::json!({"kind":"plate","length":120,"width":60,"thickness":10}),
        "cylinder" => serde_json::json!({"kind":"cylinder","diameter":40,"height":25}),
        "ring" => {
            serde_json::json!({"kind":"ring","outer_diameter":50,"inner_diameter":30,"height":12})
        }
        "l_bracket" => {
            serde_json::json!({"kind":"l_bracket","width":30,"depth":20,"vertical_height":40,"thickness":5})
        }
        "enclosure" => {
            serde_json::json!({"kind":"enclosure","length":100,"width":80,"height":30,"wall_thickness":3})
        }
        _ => unreachable!(),
    }
}

fn doc_with(kind: &str, holes: serde_json::Value) -> CadDocument1 {
    let v = serde_json::json!({
        "source_prompt": "x",
        "base": base_json(kind),
        "holes": holes,
        "planner": {"planner": "rule"}
    });
    CadDocument1::from_json_str(&v.to_string()).expect("valid doc")
}

fn assert_valid(d: &CadDocument2) {
    assert_eq!(d.check_schema(), vec![]);
    assert_eq!(d.check_completed(), vec![]);
}

fn frame_origin(d: &CadDocument2, kid: &str) -> [f64; 3] {
    let k = d.constraint(kid).unwrap_or_else(|| panic!("missing {kid}"));
    assert_eq!(k.kind, ConstraintKind::Fixed);
    let f: &Frame = k.frame.as_ref().unwrap();
    f.origin
}

fn hole_shape<'a>(d: &'a CadDocument2, cid: &str) -> &'a promptcad_core::model::v2::HoleSpec {
    match &d.component(cid).unwrap().geometry {
        Geometry::Empty {
            role: EmptyRole::Cut,
            shape: EmptyShape::Hole(h),
        } => h,
        other => panic!("not a cut hole: {other:?}"),
    }
}

#[test]
fn fixed_example_upgrades() {
    let d1 = example();
    let d2 = upgrade_1_to_2(&d1);
    assert_valid(&d2);

    assert_eq!(d2.revision, 0);
    assert_eq!(d2.root.as_ref().unwrap().as_str(), "p_main");
    assert_eq!(d2.name, d1.name);
    assert_eq!(d2.source_prompt, d1.source_prompt);
    assert_eq!(d2.notes, d1.notes);
    assert_eq!(d2.assumptions, d1.assumptions);
    assert_eq!(d2.planner, d1.planner);
    assert!(d2.parameters.is_empty() && d2.bindings.is_empty() && d2.assemblies.is_empty());

    let n = d1.holes.len();
    assert_eq!(n, 4);
    let comp_ids: Vec<_> = d2.components.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(
        comp_ids,
        ["v_base", "d_h1", "e_h1", "d_h2", "e_h2", "d_h3", "e_h3", "d_h4", "e_h4"]
    );
    let k_ids: Vec<_> = d2.constraints.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(
        k_ids,
        ["k_base", "k_dh1", "k_eh1", "k_dh2", "k_eh2", "k_dh3", "k_eh3", "k_dh4", "k_eh4"]
    );
    assert!(d2.components.iter().all(|c| {
        let p = c.provenance.as_ref().unwrap();
        p.agent == ProvenanceAgent::Migration && p.patch_id.is_none()
    }));

    // base
    assert_eq!(frame_origin(&d2, "k_base"), [0.0, 0.0, 0.0]);
    assert_eq!(d2.constraint("k_base").unwrap().a.as_str(), "v_base.origin");
    // holes: z-axis holes sit on the top face (z = thickness)
    for (i, h) in d1.holes.iter().enumerate() {
        let k = format!("k_dh{}", i + 1);
        assert_eq!(frame_origin(&d2, &k), [h.x, h.y, 10.0]);
        assert_eq!(
            d2.constraint(&k).unwrap().a.as_str(),
            format!("d_h{}.point", i + 1)
        );
        let c = d2.constraint(&format!("k_eh{}", i + 1)).unwrap();
        assert_eq!(c.kind, ConstraintKind::Coincident);
        assert_eq!(c.a.as_str(), format!("e_h{}.entry", i + 1));
        assert_eq!(
            c.b.as_ref().unwrap().as_str(),
            format!("d_h{}.point", i + 1)
        );
        let hs = hole_shape(&d2, &format!("e_h{}", i + 1));
        assert_eq!(hs.diameter, h.diameter);
        assert_eq!(hs.hole_type, h.hole_type);
        assert_eq!(hs.thread, h.thread);
    }

    // part
    let p = &d2.parts[0];
    assert_eq!(p.id.as_str(), "p_main");
    assert_eq!(p.name, d1.name);
    assert_eq!(p.material, d1.material);
    assert_eq!(p.quantity, 1);
    assert_eq!(p.fillets, d1.fillets);
    assert_eq!(p.chamfers, d1.chamfers);
    assert_eq!(p.body.len(), 1 + n);
    assert_eq!(p.body[0].op, BodyOpKind::Add);
    assert_eq!(p.body[0].component.as_str(), "v_base");
    for (i, op) in p.body[1..].iter().enumerate() {
        assert_eq!(op.op, BodyOpKind::Cut);
        assert_eq!(op.component.as_str(), format!("e_h{}", i + 1));
    }
}

#[test]
fn canonical_serialization_is_deterministic_and_round_trips() {
    let d1 = example();
    let a = to_canonical_json(&upgrade_1_to_2(&d1));
    let b = to_canonical_json(&upgrade_1_to_2(&d1.clone()));
    assert_eq!(a, b);
    let back = CadDocument2::from_json_str(&a).expect("canonical output parses");
    assert_eq!(back, upgrade_1_to_2(&d1));
    assert_eq!(to_canonical_json(&back), a);
}

#[test]
fn extents_per_base_kind() {
    let cases = [
        ("plate", [60.0, 30.0, 10.0]),
        ("cylinder", [20.0, 20.0, 25.0]),
        ("ring", [25.0, 25.0, 12.0]),
        ("l_bracket", [15.0, 10.0, 45.0]),
        ("enclosure", [50.0, 40.0, 30.0]),
    ];
    for (kind, ext) in cases {
        let d1 = doc_with(kind, serde_json::json!([]));
        assert_eq!(extents_v1(&d1.base), ext, "{kind}");
        let holes = serde_json::json!([
            {"x": 1.5, "y": -2.5, "z": 3.5, "axis": "z", "diameter": 3},
            {"x": 1.5, "y": -2.5, "z": 3.5, "axis": "x", "diameter": 3},
            {"x": 1.5, "y": -2.5, "z": 3.5, "axis": "y", "diameter": 3}
        ]);
        let d1 = doc_with(kind, holes);
        let d2 = upgrade_1_to_2(&d1);
        assert_valid(&d2);
        assert_eq!(frame_origin(&d2, "k_dh1"), [1.5, -2.5, ext[2]], "{kind} z");
        assert_eq!(frame_origin(&d2, "k_dh2"), [ext[0], -2.5, 3.5], "{kind} x");
        assert_eq!(frame_origin(&d2, "k_dh3"), [1.5, ext[1], 3.5], "{kind} y");
        assert_eq!(hole_shape(&d2, "e_h2").axis, Axis::X);
        assert_eq!(hole_shape(&d2, "e_h3").axis, Axis::Y);
    }
}

fn with_threads(threads: &[Option<&str>], notes: Vec<String>) -> CadDocument1 {
    let holes: Vec<_> = threads
        .iter()
        .map(|t| serde_json::json!({"diameter": 5, "thread": t}))
        .collect();
    let mut d = doc_with("plate", serde_json::Value::Array(holes));
    d.notes = notes;
    d
}

#[test]
fn thread_normalization() {
    let d1 = with_threads(
        &[Some(" m6x1 "), Some("m8"), None, Some("M10X1.25")],
        vec![],
    );
    let d2 = upgrade_1_to_2(&d1);
    assert_valid(&d2);
    let t = |c| hole_shape(&d2, c).thread.clone();
    assert_eq!(t("e_h1").as_deref(), Some("M6x1"));
    assert_eq!(t("e_h2").as_deref(), Some("M8"));
    assert_eq!(t("e_h3"), None);
    assert_eq!(t("e_h4").as_deref(), Some("M10x1.25"));
    assert!(d2.notes.is_empty());
}

#[test]
fn invalid_thread_becomes_null_with_note() {
    let d1 = with_threads(&[Some("M6"), Some("M6 tapped")], vec!["keep".into()]);
    let d2 = upgrade_1_to_2(&d1);
    assert_valid(&d2);
    assert_eq!(hole_shape(&d2, "e_h2").thread, None);
    assert_eq!(d2.notes, ["keep", "1.0 thread 原值：e_h2=M6 tapped"]);
}

#[test]
fn multiple_bad_threads_merge_into_one_line() {
    let d1 = with_threads(
        &[Some("M6 tapped"), Some("M6"), Some("bolt"), Some("")],
        vec![],
    );
    let d2 = upgrade_1_to_2(&d1);
    assert_valid(&d2);
    assert_eq!(
        d2.notes,
        ["1.0 thread 原值：e_h1=M6 tapped；e_h3=bolt；e_h4="]
    );
}

#[test]
fn full_notes_append_to_last_entry() {
    let notes: Vec<String> = (0..32).map(|i| format!("n{i}")).collect();
    let d1 = with_threads(&[Some("junk")], notes.clone());
    let d2 = upgrade_1_to_2(&d1);
    assert_valid(&d2);
    assert_eq!(d2.notes.len(), 32);
    assert_eq!(&d2.notes[..31], &notes[..31]);
    assert_eq!(d2.notes[31], "n31；1.0 thread 原值：e_h1=junk");
}

#[test]
fn normalize_v1_is_idempotent_and_upgrade_consistent() {
    let mut d1 = with_threads(&[Some(" m6x1 "), Some("M6 tapped"), None], vec!["a".into()]);
    d1.holes[0].z = 7.0;
    d1.holes[1].axis = Axis::X;
    d1.holes[1].x = 9.0;
    d1.holes[2].axis = Axis::Y;
    d1.holes[2].y = 4.0;
    let n = normalize_v1(&d1);
    assert_eq!((n.holes[0].z, n.holes[1].x, n.holes[2].y), (0.0, 0.0, 0.0));
    assert_eq!(n.holes[0].thread.as_deref(), Some("M6x1"));
    assert_eq!(n.holes[1].thread, None);
    assert_eq!(n.notes, ["a", "1.0 thread 原值：e_h2=M6 tapped"]);
    assert_eq!(normalize_v1(&n), n);
    assert!(n.check_schema().is_empty());
    // positions of the non-along-axis coordinates are preserved
    assert_eq!(upgrade_1_to_2(&n).notes, upgrade_1_to_2(&d1).notes);
}

#[test]
fn fixed_example_is_already_normalized() {
    let d1 = example();
    assert_eq!(normalize_v1(&d1), d1);
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
    fn upgrade_is_total_and_valid(d1 in doc_strategy()) {
        prop_assert!(d1.check_schema().is_empty(), "generator bug: {:?}", d1.check_schema());
        let d2 = upgrade_1_to_2(&d1);
        prop_assert_eq!(d2.check_schema(), vec![]);
        prop_assert_eq!(d2.check_completed(), vec![]);
        prop_assert_eq!(d2.components.len(), 1 + 2 * d1.holes.len());
        prop_assert_eq!(d2.constraints.len(), 1 + 2 * d1.holes.len());
        prop_assert_eq!(d2.parts[0].body.len(), 1 + d1.holes.len());
        // deterministic and independent of normalization
        prop_assert_eq!(&d2, &upgrade_1_to_2(&normalize_v1(&d1)));
        let n = normalize_v1(&d1);
        prop_assert!(n.check_schema().is_empty());
        prop_assert_eq!(normalize_v1(&n), n);
        // canonical output parses back to the same document
        let back = CadDocument2::from_json_str(&to_canonical_json(&d2)).unwrap();
        prop_assert_eq!(back, d2);
    }
}
