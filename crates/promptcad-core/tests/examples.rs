//! Golden-file tests over `examples/cad2/*.cad2.json` and the 1.0 `plate-four-holes` spec.

use std::collections::BTreeMap;
use std::path::PathBuf;

use promptcad_core::canonical::{
    normalize_numbers, parse_example, strip_example_keys, to_canonical_json,
};
use promptcad_core::fields::get_field;
use promptcad_core::model::resolved::{ResolvedComponent, ResolvedHole, ResolvedPort};
use promptcad_core::model::v2::{ConstraintKind, EmptyRole, VolumeShape};
use promptcad_core::model::{
    CadDocument1, CadDocument2, ComponentKind, FieldRef, Id, LoadError, PortRef, ShapeKind,
};
use serde_json::{json, Value};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

const CAD2_EXAMPLES: [&str; 2] = [
    "examples/cad2/plate-four-holes.cad2.json",
    "examples/cad2/enclosure-with-lid.cad2.json",
];

const TOP_LEVEL_KEYS: [&str; 15] = [
    "schema_version",
    "name",
    "source_prompt",
    "unit",
    "revision",
    "parameters",
    "bindings",
    "components",
    "constraints",
    "parts",
    "assemblies",
    "root",
    "assumptions",
    "notes",
    "planner",
];

/// Keys of a JSON object in textual order (serde_json's `Value` sorts keys, so scan the text).
fn top_level_key_order(text: &str) -> Vec<String> {
    let mut keys = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("  \"") {
            if let Some(end) = rest.find('"') {
                keys.push(rest[..end].to_owned());
            }
        }
    }
    keys
}

#[test]
fn cad2_examples_parse_check_and_round_trip() {
    for path in CAD2_EXAMPLES {
        let text = read(path);
        let (doc, expected) = parse_example(&text).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert!(expected.is_some(), "{path}: expected_resolved present");
        assert!(doc.check_schema().is_empty());
        assert!(doc.check_completed().is_empty(), "{path}: completed");

        let mut stripped: Value = serde_json::from_str(&text).unwrap();
        strip_example_keys(&mut stripped);
        let canonical = to_canonical_json(&doc);
        let reparsed: Value = serde_json::from_str(&canonical).unwrap();
        assert_eq!(
            normalize_numbers(&reparsed),
            normalize_numbers(&stripped),
            "{path}: canonical form differs from input"
        );
        assert_eq!(top_level_key_order(&canonical), TOP_LEVEL_KEYS, "{path}");

        // idempotent: canonical → parse → canonical is byte-identical
        let again = CadDocument2::from_json_str(&canonical).unwrap();
        assert_eq!(again, doc);
        assert_eq!(to_canonical_json(&again), canonical);
    }
}

#[test]
fn canonical_component_key_order() {
    let (doc, _) = parse_example(&read(CAD2_EXAMPLES[0])).unwrap();
    let hole = doc.component("e_h1").unwrap();
    let s = serde_json::to_string(hole).unwrap();
    let order: Vec<_> = [
        "\"id\"",
        "\"kind\"",
        "\"role\"",
        "\"shape\"",
        "\"label\"",
        "\"provenance\"",
    ]
    .iter()
    .map(|k| s.find(k).unwrap())
    .collect();
    assert!(order.windows(2).all(|w| w[0] < w[1]), "{s}");
    let shape_keys = [
        "\"axis\"",
        "\"diameter\"",
        "\"hole_type\"",
        "\"depth\"",
        "\"thread\"",
        "\"counterbore_diameter\"",
        "\"counterbore_depth\"",
        "\"countersink_diameter\"",
        "\"countersink_angle\"",
    ];
    let pos: Vec<_> = shape_keys.iter().map(|k| s.find(k).unwrap()).collect();
    assert!(pos.windows(2).all(|w| w[0] < w[1]), "{s}");
    let k = serde_json::to_string(&doc.constraints[0]).unwrap();
    assert!(k.starts_with(r#"{"id":"k_root","kind":"fixed","a":"v_plate.origin","b":null,"value":null,"u":null,"v":null,"frame":{"origin":[0.0,0.0,0.0],"x_axis":"+x","z_axis":"+z"}}"#), "{k}");
}

#[test]
fn example_contents_are_typed() {
    let (plate, _) = parse_example(&read(CAD2_EXAMPLES[0])).unwrap();
    assert_eq!(plate.revision, 4);
    assert_eq!(plate.root, Some(Id::new("p_plate").unwrap()));
    let kinds: Vec<_> = plate.components.iter().map(|c| c.kind()).collect();
    assert_eq!(kinds[0], ComponentKind::Volume);
    assert_eq!(
        plate.component("e_h3").unwrap().role(),
        Some(EmptyRole::Cut)
    );
    assert_eq!(
        plate
            .component("e_h3")
            .unwrap()
            .as_hole()
            .unwrap()
            .thread
            .as_deref(),
        Some("M6")
    );
    assert_eq!(
        plate.component("d_h1").unwrap().shape_kind(),
        ShapeKind::DotPoint
    );
    let f = |s: &str| get_field(&plate, &s.parse::<FieldRef>().unwrap()).unwrap();
    assert_eq!(f("v_plate:length"), 120.0);
    assert_eq!(f("k_d3:u"), 50.0);
    assert_eq!(f("k_d4:v"), 20.0);
    assert_eq!(plate.parts[0].fillets[0].radius, 5.0);

    let (enc, _) = parse_example(&read(CAD2_EXAMPLES[1])).unwrap();
    assert!(matches!(
        enc.component("v_body").unwrap().as_volume(),
        Some(VolumeShape::Enclosure(e)) if e.height == 33.0
    ));
    assert_eq!(
        enc.component("e_pcb").unwrap().role(),
        Some(EmptyRole::Reserve)
    );
    assert_eq!(
        enc.constraint("k_lid_mate").unwrap().effective_value(),
        Some(0.0)
    );
    assert_eq!(
        enc.constraints
            .iter()
            .filter(|k| k.kind == ConstraintKind::Fixed)
            .count(),
        1
    );
    assert_eq!(enc.assemblies[0].children.len(), 2);
}

#[test]
fn expected_resolved_maps_match_resolved_types() {
    for path in CAD2_EXAMPLES {
        let (_, expected) = parse_example(&read(path)).unwrap();
        let expected = expected.unwrap();
        let components: BTreeMap<Id, ResolvedComponent> =
            serde_json::from_value(expected["components"].clone()).unwrap();
        let ports: BTreeMap<PortRef, ResolvedPort> =
            serde_json::from_value(expected["ports"].clone()).unwrap();
        let holes: BTreeMap<Id, ResolvedHole> =
            serde_json::from_value(expected["holes"].clone()).unwrap();
        assert!(!components.is_empty() && !ports.is_empty(), "{path}");
        if path.contains("plate") {
            assert_eq!(holes.len(), 4);
            assert_eq!(holes["e_h1"].cutter_end, [-50.0, -20.0, -1.0]);
        }
    }
}

#[test]
fn unknown_fields_and_bad_refs_are_rejected() {
    let (doc, _) = parse_example(&read(CAD2_EXAMPLES[0])).unwrap();
    let base = serde_json::to_value(&doc).unwrap();

    let mutate = |f: &dyn Fn(&mut Value)| {
        let mut v = base.clone();
        f(&mut v);
        serde_json::from_value::<CadDocument2>(v)
    };

    // a component `origin` field (spec §3.1)
    assert!(mutate(&|v| { v["components"][0]["origin"] = json!([0, 0, 0]) }).is_err());
    // example-only key not stripped
    assert!(mutate(&|v| { v["expected_resolved"] = json!({}) }).is_err());
    // unknown top-level, constraint, part and shape fields
    assert!(mutate(&|v| { v["material"] = json!("aluminum") }).is_err());
    assert!(mutate(&|v| { v["constraints"][1]["w"] = json!(0) }).is_err());
    assert!(mutate(&|v| { v["parts"][0]["color"] = json!("red") }).is_err());
    assert!(mutate(&|v| { v["components"][0]["shape"]["x"] = json!(0) }).is_err());
    // wrong version
    assert!(mutate(&|v| { v["schema_version"] = json!("1.0") }).is_err());
    // bad ids / refs
    assert!(mutate(&|v| { v["components"][0]["id"] = json!("V_plate") }).is_err());
    assert!(mutate(&|v| { v["constraints"][1]["a"] = json!("d_h1:point") }).is_err());
    assert!(mutate(&|v| { v["constraints"][1]["b"] = json!("v_plate") }).is_err());
    assert!(mutate(&|v| { v["bindings"][0]["target"] = json!("v_plate.length") }).is_err());
    assert!(mutate(&|v| { v["parts"][0]["body"][0]["component"] = json!("1abc") }).is_err());
    assert!(mutate(&|v| { v["root"] = json!("p-plate") }).is_err());
    // non-perpendicular frame
    assert!(mutate(&|v| { v["constraints"][0]["frame"]["x_axis"] = json!("-z") }).is_err());
    // sanity: the unmodified value parses
    assert!(mutate(&|_| {}).is_ok());
}

#[test]
fn schema_violations_are_reported_with_paths() {
    let (doc, _) = parse_example(&read(CAD2_EXAMPLES[0])).unwrap();
    let mut v = serde_json::to_value(&doc).unwrap();
    v["components"][0]["shape"]["length"] = json!(0);
    v["components"][5]["shape"]["thread"] = json!("M6 tapped");
    v["components"][6]["shape"]["hole_type"] = json!("blind");
    v["constraints"][1]["value"] = json!(3);
    v["parts"][0]["quantity"] = json!(0);
    v["name"] = json!("");
    let err = CadDocument2::from_json_str(&v.to_string()).unwrap_err();
    let LoadError::Schema(violations) = err else {
        panic!("expected schema violations, got {err}");
    };
    let paths: Vec<_> = violations.iter().map(|x| x.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "/name",
            "/components/0/shape/length",
            "/components/5/shape/thread",
            "/components/6/shape",
            "/constraints/1/value",
            "/parts/0/quantity",
        ]
    );

    // completed-document lower bounds are separate from check_schema
    let mut empty = doc.clone();
    empty.components.clear();
    empty.parts.clear();
    empty.root = None;
    assert!(empty.check_schema().is_empty());
    let paths: Vec<_> = empty
        .check_completed()
        .into_iter()
        .map(|x| x.path)
        .collect();
    assert_eq!(paths, ["/components", "/parts", "/root"]);
}

#[test]
fn v1_example_round_trips() {
    let text = read("examples/generated/plate-four-holes/spec.json");
    let doc = CadDocument1::from_json_str(&text).unwrap();
    assert_eq!(doc.holes.len(), 4);
    let input: Value = serde_json::from_str(&text).unwrap();
    let output = serde_json::to_value(&doc).unwrap();
    // 1.0 files are written by Pydantic with float values, so equality is exact.
    assert_eq!(output, input);
    let pretty = promptcad_core::canonical::to_canonical_string(&doc);
    assert_eq!(pretty.trim_end(), text.trim_end().replace("\r\n", "\n"));
}

#[test]
fn v1_rejects_v2_and_unknown() {
    let text = read("examples/generated/plate-four-holes/spec.json");
    let mut v: Value = serde_json::from_str(&text).unwrap();
    v["holes"][0]["origin"] = json!(1);
    assert!(serde_json::from_value::<CadDocument1>(v).is_err());
    assert!(CadDocument1::from_json_str(&read(CAD2_EXAMPLES[0])).is_err());
}
