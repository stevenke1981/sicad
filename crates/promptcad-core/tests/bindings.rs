//! Tests for `expr` and `bindings` (spec §4): the shipped examples, one test per issue code,
//! dependency chains and cycles, and properties of the expression parser.

use std::path::PathBuf;

use promptcad_core::bindings::{
    check_bindings, check_bindings_ignoring_cache, evaluate_binding, is_bound, recompute_bindings,
};
use promptcad_core::canonical::parse_example;
use promptcad_core::expr::{BinOp, EvalError, Expr, Func, Resolver};
use promptcad_core::fields::get_field;
use promptcad_core::model::{CadDocument2, Code, FieldRef, Id, ValidationIssue};
use proptest::prelude::*;
use serde_json::{json, Value};

fn fr(s: &str) -> FieldRef {
    s.parse().unwrap()
}

fn load_example(name: &str) -> CadDocument2 {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/cad2")
        .join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
    parse_example(&text).unwrap().0
}

fn plate_a() -> CadDocument2 {
    load_example("plate-four-holes.cad2.json")
}

fn enclosure_b() -> CadDocument2 {
    load_example("enclosure-with-lid.cad2.json")
}

fn set_param(doc: &mut CadDocument2, name: &str, value: f64) {
    doc.parameters
        .iter_mut()
        .find(|p| p.name.as_str() == name)
        .unwrap()
        .value = value;
}

/// A small document with every field kind the tests bind. Not schema-checked on purpose.
fn base() -> Value {
    json!({
        "schema_version": "2.0",
        "source_prompt": "t",
        "revision": 0,
        "parameters": [
            {"name": "len", "value": 100},
            {"name": "wid", "value": 50},
            {"name": "zero", "value": 0}
        ],
        "components": [
            {"id": "v_plate", "kind": "volume", "shape": {"kind": "plate", "length": 100, "width": 50, "thickness": 10}},
            {"id": "v_ring", "kind": "volume", "shape": {"kind": "ring", "outer_diameter": 50, "inner_diameter": 20, "height": 5}},
            {"id": "v_enc", "kind": "volume", "shape": {"kind": "enclosure", "length": 80, "width": 60, "height": 30, "wall_thickness": 2}},
            {"id": "p_s", "kind": "plane", "shape": {"kind": "section", "source": "v_plate", "offset": 0}},
            {"id": "v_r", "kind": "volume", "shape": {"kind": "revolve", "profile": "p_s", "axis": "l_s", "angle": 90}},
            {"id": "d_a", "kind": "dot", "shape": {"kind": "point"}},
            {"id": "e_h1", "kind": "empty", "role": "cut", "shape": {
                "kind": "hole", "diameter": 6, "hole_type": "counterbore", "depth": 5, "thread": null,
                "counterbore_diameter": 10, "counterbore_depth": 3,
                "countersink_diameter": null, "countersink_angle": null}}
        ],
        "constraints": [
            {"id": "k_d1", "kind": "on_face", "a": "d_a.point", "b": "v_plate.top", "u": -50, "v": 0},
            {"id": "k_m", "kind": "mate", "a": "v_plate.bottom", "b": "v_plate.top", "value": 1}
        ],
        "planner": {"planner": "t"}
    })
}

fn doc_with(bindings: Value) -> CadDocument2 {
    let mut v = base();
    v["bindings"] = bindings;
    serde_json::from_value(v).unwrap()
}

fn b(target: &str, expr: &str) -> Value {
    json!({"target": target, "expr": expr})
}

fn codes(issues: &[ValidationIssue]) -> Vec<&str> {
    issues.iter().map(|i| i.code.as_str()).collect()
}

fn only(issues: &[ValidationIssue], code: Code) -> &ValidationIssue {
    assert_eq!(codes(issues), [code.as_str()], "{issues:#?}");
    &issues[0]
}

// ---- examples -------------------------------------------------------------------------------

#[test]
fn examples_cached_values_match_evaluation() {
    for doc in [plate_a(), enclosure_b()] {
        assert!(!doc.bindings.is_empty());
        for binding in &doc.bindings {
            let v = evaluate_binding(&doc, binding).unwrap();
            let cached = get_field(&doc, &binding.target).unwrap();
            assert!(
                (v - cached).abs() <= 1e-6,
                "{}: {v} vs {cached}",
                binding.target
            );
            assert!(is_bound(&doc, &binding.target));
        }
        assert!(
            check_bindings(&doc).is_empty(),
            "{:#?}",
            check_bindings(&doc)
        );
    }
}

#[test]
fn recompute_is_a_noop_on_examples() {
    for doc in [plate_a(), enclosure_b()] {
        let mut again = doc.clone();
        assert!(recompute_bindings(&mut again).is_empty());
        assert_eq!(again, doc);
    }
}

#[test]
fn example_a_parameter_change_moves_dependents() {
    let mut doc = plate_a();
    set_param(&mut doc, "len", 200.0);
    // cache is now stale and check_bindings says so, once per affected binding
    let stale = check_bindings(&doc);
    assert!(!stale.is_empty());
    assert!(stale.iter().all(|i| i.code == "binding_eval_error"));
    assert!(check_bindings_ignoring_cache(&doc).is_empty());

    assert!(recompute_bindings(&mut doc).is_empty());
    assert_eq!(get_field(&doc, &fr("v_plate:length")), Ok(200.0));
    assert_eq!(get_field(&doc, &fr("v_plate:width")), Ok(60.0));
    assert_eq!(get_field(&doc, &fr("k_d1:u")), Ok(-90.0));
    assert_eq!(get_field(&doc, &fr("k_d2:u")), Ok(90.0));
    assert_eq!(get_field(&doc, &fr("k_d3:u")), Ok(90.0));
    assert_eq!(get_field(&doc, &fr("k_d4:u")), Ok(-90.0));
    assert_eq!(get_field(&doc, &fr("k_d1:v")), Ok(-20.0));
    assert!(check_bindings(&doc).is_empty());
}

#[test]
fn example_b_parameter_change_updates_height() {
    let mut doc = enclosure_b();
    set_param(&mut doc, "pcb_stack", 30.0);
    assert!(recompute_bindings(&mut doc).is_empty());
    assert_eq!(get_field(&doc, &fr("v_body:height")), Ok(38.0));
    assert!(check_bindings(&doc).is_empty());
}

// ---- one test per code ----------------------------------------------------------------------

#[test]
fn duplicate_binding() {
    let doc = doc_with(json!([
        b("v_plate:length", "len"),
        b("v_plate:length", "wid")
    ]));
    let issues = check_bindings_ignoring_cache(&doc);
    let i = only(&issues, Code::DuplicateBinding);
    assert_eq!(i.target.as_deref(), Some("v_plate:length"));
    // the first binding still applies
    let mut doc = doc;
    recompute_bindings(&mut doc);
    assert_eq!(get_field(&doc, &fr("v_plate:length")), Ok(100.0));
}

#[test]
fn binding_parse_error() {
    for expr in ["1e3", ".5", "len ** 2", "len %", "(len", "", "min", "a b"] {
        let doc = doc_with(json!([b("v_plate:length", expr)]));
        let issues = check_bindings_ignoring_cache(&doc);
        let i = only(&issues, Code::BindingParseError);
        assert_eq!(i.target.as_deref(), Some("v_plate:length"), "{expr}");
        assert!(
            i.message.contains("position") || i.message.contains("length"),
            "{expr}"
        );
    }
    let doc = doc_with(json!([b("v_plate:length", &"1+".repeat(81))]));
    only(
        &check_bindings_ignoring_cache(&doc),
        Code::BindingParseError,
    );
}

#[test]
fn binding_unknown_name() {
    let doc = doc_with(json!([b("v_plate:length", "len + nope")]));
    let issues = check_bindings_ignoring_cache(&doc);
    let i = only(&issues, Code::BindingUnknownName);
    assert_eq!(i.related, ["nope"]);
    let err = evaluate_binding(&doc, &doc.bindings[0]).unwrap_err();
    assert_eq!(err.code, "binding_unknown_name");
}

#[test]
fn unknown_field_and_dangling_reference() {
    // target problems
    for (target, code) in [
        ("v_plate:height", Code::UnknownField),
        ("v_plate:kind", Code::UnknownField),
        ("e_h1:thread", Code::UnknownField),
        ("e_h1:countersink_angle", Code::UnknownField), // null optional field
        ("k_m:u", Code::UnknownField),
        ("nope:length", Code::DanglingReference),
    ] {
        let doc = doc_with(json!([b(target, "1")]));
        let issues = check_bindings_ignoring_cache(&doc);
        assert_eq!(codes(&issues), [code.as_str()], "{target}");
        assert_eq!(issues[0].target.as_deref(), Some(target));
    }
    // reference problems
    for (expr, code) in [
        ("v_plate:height + 1", Code::UnknownField),
        ("e_h1:countersink_diameter", Code::UnknownField),
        ("nope:length", Code::DanglingReference),
    ] {
        let doc = doc_with(json!([b("v_plate:length", expr)]));
        let issues = check_bindings_ignoring_cache(&doc);
        assert_eq!(codes(&issues), [code.as_str()], "{expr}");
        assert_eq!(issues[0].target.as_deref(), Some("v_plate:length"));
        assert_eq!(issues[0].related.len(), 1);
        let err = evaluate_binding(&doc, &doc.bindings[0]).unwrap_err();
        assert_eq!(err.code, code.as_str(), "{expr}");
    }
}

#[test]
fn binding_cycles() {
    // self reference
    let doc = doc_with(json!([b("v_plate:length", "v_plate:length + 1")]));
    only(&check_bindings_ignoring_cache(&doc), Code::BindingCycle);

    // two
    let doc = doc_with(json!([
        b("v_plate:length", "v_plate:width + 1"),
        b("v_plate:width", "v_plate:length + 1"),
    ]));
    let issues = check_bindings_ignoring_cache(&doc);
    assert_eq!(codes(&issues), ["binding_cycle", "binding_cycle"]);
    assert_eq!(issues[0].target.as_deref(), Some("v_plate:length"));
    assert_eq!(issues[0].related, ["v_plate:width"]);
    assert_eq!(issues[1].related, ["v_plate:length"]);

    // three, plus an unrelated binding and a dependent of the cycle (not flagged itself)
    let doc = doc_with(json!([
        b("v_plate:length", "v_plate:thickness"),
        b("v_plate:width", "v_plate:length"),
        b("v_plate:thickness", "v_plate:width"),
        b("k_d1:u", "v_plate:length"),
        b("k_d1:v", "len"),
    ]));
    let issues = check_bindings_ignoring_cache(&doc);
    assert_eq!(codes(&issues), ["binding_cycle"; 3]);
    let targets: Vec<_> = issues.iter().map(|i| i.target.clone().unwrap()).collect();
    assert_eq!(
        targets,
        ["v_plate:length", "v_plate:width", "v_plate:thickness"]
    );

    // recompute leaves the cycle and its dependent alone but applies the unrelated one
    let mut doc = doc;
    set_param_in(&mut doc, "len", 77.0);
    let before = doc.clone();
    let issues = recompute_bindings(&mut doc);
    assert_eq!(issues.len(), 3);
    assert_eq!(get_field(&doc, &fr("k_d1:v")), Ok(77.0));
    assert_eq!(
        get_field(&doc, &fr("k_d1:u")),
        get_field(&before, &fr("k_d1:u"))
    );
    assert_eq!(get_field(&doc, &fr("v_plate:length")), Ok(100.0));
}

fn set_param_in(doc: &mut CadDocument2, name: &str, value: f64) {
    set_param(doc, name, value);
}

#[test]
fn chain_is_evaluated_in_dependency_order() {
    // listed against dependency order on purpose
    let mut doc = doc_with(json!([
        b("k_d1:v", "k_d1:u + 1"),
        b("k_d1:u", "v_plate:thickness * 2"),
        b("v_plate:thickness", "v_plate:width + 1"),
        b("v_plate:width", "v_plate:length + 1"),
        b("v_plate:length", "len"),
    ]));
    assert!(recompute_bindings(&mut doc).is_empty());
    assert_eq!(get_field(&doc, &fr("v_plate:length")), Ok(100.0));
    assert_eq!(get_field(&doc, &fr("v_plate:width")), Ok(101.0));
    assert_eq!(get_field(&doc, &fr("v_plate:thickness")), Ok(102.0));
    assert_eq!(get_field(&doc, &fr("k_d1:u")), Ok(204.0));
    assert_eq!(get_field(&doc, &fr("k_d1:v")), Ok(205.0));
    assert!(check_bindings(&doc).is_empty());
    // idempotent
    let again = doc.clone();
    recompute_bindings(&mut doc);
    assert_eq!(doc, again);
}

#[test]
fn unbound_reference_uses_current_value() {
    let mut doc = doc_with(json!([b("v_plate:length", "v_plate:width * 2")]));
    assert!(recompute_bindings(&mut doc).is_empty());
    assert_eq!(get_field(&doc, &fr("v_plate:length")), Ok(100.0));
}

#[test]
fn binding_eval_error() {
    for expr in ["len / zero", "len / (wid - 50)", "len / 0"] {
        let doc = doc_with(json!([b("v_plate:length", expr)]));
        let issues = check_bindings_ignoring_cache(&doc);
        only(&issues, Code::BindingEvalError);
        assert_eq!(
            evaluate_binding(&doc, &doc.bindings[0]).unwrap_err().code,
            "binding_eval_error"
        );
    }
    // overflow to infinity
    let mut v = base();
    v["parameters"] = json!([{"name": "big", "value": 1e300}]);
    v["bindings"] = json!([b("v_plate:length", "big * big")]);
    let doc: CadDocument2 = serde_json::from_value(v).unwrap();
    only(&check_bindings_ignoring_cache(&doc), Code::BindingEvalError);
    // a failing binding leaves its field untouched
    let mut doc = doc_with(json!([b("v_plate:length", "len / zero")]));
    let before = doc.clone();
    assert_eq!(recompute_bindings(&mut doc).len(), 1);
    assert_eq!(doc, before);
}

#[test]
fn binding_out_of_range() {
    let cases = [
        ("v_plate:length", "0"),            // Dim > 0
        ("v_plate:length", "len - 100"),    // Dim > 0
        ("v_plate:length", "100001"),       // Dim <= MAX
        ("k_d1:u", "100001"),               // Coord
        ("k_d1:v", "0 - 100001"),           // Coord
        ("p_s:offset", "len * 10000"),      // Coord
        ("v_r:angle", "361"),               // (0, 360]
        ("v_r:angle", "0"),                 // (0, 360]
        ("k_m:value", "0 - 1"),             // mate gap >= 0
        ("e_h1:counterbore_diameter", "4"), // hole subtype: cb must exceed diameter
        ("e_h1:diameter", "12"),            // hole subtype from the other side
        ("v_ring:inner_diameter", "60"),    // ring rule
        ("v_ring:outer_diameter", "10"),    // ring rule
        ("v_enc:wall_thickness", "40"),     // enclosure rule
    ];
    for (target, expr) in cases {
        let doc = doc_with(json!([b(target, expr)]));
        let issues = check_bindings_ignoring_cache(&doc);
        let i = only(&issues, Code::BindingOutOfRange);
        assert_eq!(i.target.as_deref(), Some(target), "{target} = {expr}");
        // the field is untouched by recompute
        let mut d = doc.clone();
        assert_eq!(recompute_bindings(&mut d).len(), 1);
        assert_eq!(d, doc, "{target} = {expr}");
    }
    // in range, including boundaries
    for (target, expr) in [
        ("v_plate:length", "100000"),
        ("v_r:angle", "360"),
        ("k_m:value", "0"),
        ("k_d1:u", "0 - 100000"),
        ("e_h1:diameter", "9"),
        ("e_h1:depth", "4"),
    ] {
        let doc = doc_with(json!([b(target, expr)]));
        assert!(
            check_bindings_ignoring_cache(&doc).is_empty(),
            "{target} = {expr}"
        );
    }
}

#[test]
fn shape_rule_is_checked_after_all_bindings_apply() {
    // Ring: outer 50 -> 30 and inner 20 -> 25 is fine only as a pair; either alone would be
    // checked against the stale value of the other if ordering mattered.
    let doc = doc_with(json!([
        b("v_ring:inner_diameter", "25"),
        b("v_ring:outer_diameter", "30"),
    ]));
    assert!(check_bindings_ignoring_cache(&doc).is_empty());
    let mut doc = doc;
    assert!(recompute_bindings(&mut doc).is_empty());
    assert_eq!(get_field(&doc, &fr("v_ring:outer_diameter")), Ok(30.0));
    assert_eq!(get_field(&doc, &fr("v_ring:inner_diameter")), Ok(25.0));

    // violated pair: one issue for the component; both fields are left alone
    let mut doc = doc_with(json!([
        b("v_ring:inner_diameter", "40"),
        b("v_ring:outer_diameter", "30"),
    ]));
    let before = doc.clone();
    let issues = recompute_bindings(&mut doc);
    let i = only(&issues, Code::BindingOutOfRange);
    assert_eq!(i.target.as_deref(), Some("v_ring:inner_diameter"));
    assert_eq!(i.related, ["v_ring:outer_diameter"]);
    assert_eq!(doc, before);
}

#[test]
fn parameter_problems() {
    let mut v = base();
    v["parameters"] = json!([
        {"name": "a", "value": 5, "min": 6, "max": null},
        {"name": "b", "value": 5, "min": null, "max": 4},
        {"name": "c", "value": 5, "min": 5, "max": 5},
        {"name": "a", "value": 1},
        {"name": "max", "value": 1}
    ]);
    let doc: CadDocument2 = serde_json::from_value(v).unwrap();
    let issues = check_bindings(&doc);
    assert_eq!(
        codes(&issues),
        [
            "parameter_out_of_range",
            "parameter_out_of_range",
            "duplicate_id",
            "binding_unknown_name"
        ]
    );
    assert_eq!(issues[0].target.as_deref(), Some("a"));
    assert_eq!(issues[1].target.as_deref(), Some("b"));
    assert_eq!(issues[2].target.as_deref(), Some("a"));
    assert_eq!(issues[3].target.as_deref(), Some("max"));
}

#[test]
fn cache_mismatch_is_reported_within_tolerance() {
    let mut doc = doc_with(json!([b("v_plate:length", "len")]));
    set_param(&mut doc, "len", 100.0 + 5e-7);
    assert!(check_bindings(&doc).is_empty(), "within 1e-6");
    set_param(&mut doc, "len", 100.0 + 5e-6);
    let issues = check_bindings(&doc);
    let i = only(&issues, Code::BindingEvalError);
    assert!(i.message.contains("cached value"));
    assert!(check_bindings_ignoring_cache(&doc).is_empty());
    assert!(recompute_bindings(&mut doc).is_empty());
    assert!(check_bindings(&doc).is_empty());
}

#[test]
fn messages_and_determinism() {
    let doc = doc_with(json!([
        b("v_plate:length", "len / zero"),
        b("v_plate:width", "nope"),
        b("v_plate:length", "1"),
        b("k_d1:u", "1e3"),
    ]));
    let a = check_bindings(&doc);
    let b2 = check_bindings(&doc);
    assert_eq!(a, b2);
    assert_eq!(
        codes(&a),
        [
            "binding_unknown_name",
            "duplicate_binding",
            "binding_parse_error",
            "binding_eval_error"
        ]
    );
    for i in &a {
        assert!(i.target.is_some());
        assert!(!i.message.contains("1e3"), "no echo of expression text");
    }
}

#[test]
fn is_bound_and_evaluate() {
    let doc = doc_with(json!([b("v_plate:length", "min(len, wid) * 2")]));
    assert!(is_bound(&doc, &fr("v_plate:length")));
    assert!(!is_bound(&doc, &fr("v_plate:width")));
    assert_eq!(evaluate_binding(&doc, &doc.bindings[0]), Ok(100.0));
    // -0.0 is normalised
    let doc = doc_with(json!([b("k_d1:u", "-0")]));
    let v = evaluate_binding(&doc, &doc.bindings[0]).unwrap();
    assert!(v == 0.0 && v.is_sign_positive());
}

// ---- expr properties ------------------------------------------------------------------------

struct Env;

impl Resolver for Env {
    fn param(&self, name: &str) -> Option<f64> {
        Some((name.len() as f64) * 1.5)
    }
    fn field(&self, r: &FieldRef) -> Option<f64> {
        Some(r.as_str().len() as f64 - 3.0)
    }
}

fn arb_ident() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_]{0,10}".prop_filter("reserved", |s| s != "min" && s != "max")
}

fn arb_expr() -> impl Strategy<Value = Expr> {
    let leaf = prop_oneof![
        (0u32..10_000, 0u32..100).prop_map(|(a, b)| Expr::Num(format!("{a}.{b}").parse().unwrap())),
        (0u32..1000).prop_map(|a| Expr::Num(f64::from(a))),
        arb_ident().prop_map(|s| Expr::Param(Id::new(s).unwrap())),
        (arb_ident(), arb_ident())
            .prop_map(|(a, b)| Expr::Field(FieldRef::new(format!("{a}:{b}")).unwrap())),
    ];
    leaf.prop_recursive(4, 24, 4, |inner| {
        prop_oneof![
            inner.clone().prop_map(|e| Expr::Neg(Box::new(e))),
            (
                prop_oneof![
                    Just(BinOp::Add),
                    Just(BinOp::Sub),
                    Just(BinOp::Mul),
                    Just(BinOp::Div)
                ],
                inner.clone(),
                inner.clone()
            )
                .prop_map(|(op, a, b)| Expr::Bin(op, Box::new(a), Box::new(b))),
            (
                prop_oneof![Just(Func::Min), Just(Func::Max)],
                prop::collection::vec(inner, 1..=4)
            )
                .prop_map(|(f, args)| Expr::Call(f, args)),
        ]
    })
}

proptest! {
    #[test]
    fn print_and_reparse_round_trips(e in arb_expr()) {
        let text = e.to_string();
        prop_assume!(text.chars().count() <= 160);
        let back = Expr::parse(&text).unwrap();
        prop_assert_eq!(&back, &e);
        let (x, y) = (e.eval(&Env), back.eval(&Env));
        prop_assert_eq!(x.clone(), y);
        if let Err(err) = x {
            prop_assert!(matches!(err, EvalError::DivisionByZero | EvalError::NonFinite));
        }
        prop_assert_eq!(back.refs(), e.refs());
        prop_assert_eq!(back.params(), e.params());
    }

    #[test]
    fn parser_never_panics_on_arbitrary_strings(s in ".{0,200}") {
        if let Ok(e) = Expr::parse(&s) {
            let _ = e.eval(&Env);
            let _ = e.refs();
            prop_assert_eq!(Expr::parse(&e.to_string()).unwrap(), e);
        }
    }

    #[test]
    fn parser_never_panics_on_grammar_alphabet(s in "[a-z0-9_:.,()+*/ \\-]{0,200}") {
        if let Ok(e) = Expr::parse(&s) {
            let _ = e.eval(&Env);
            prop_assert_eq!(Expr::parse(&e.to_string()).unwrap(), e);
        }
    }

    #[test]
    fn deep_parentheses_are_safe(n in 0usize..400) {
        let s = format!("{}1{}", "(".repeat(n), ")".repeat(n));
        let r = Expr::parse(&s);
        if s.len() <= 160 {
            prop_assert!(r.is_ok());
        } else {
            prop_assert!(r.is_err());
        }
    }
}
