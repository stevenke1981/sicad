//! ADR 0005 differential test: the Rust validator on `upgrade_1_to_2(doc)` gives the same
//! verdicts as the 1.0 Python `DesignValidator` on `doc`.
//!
//! The corpus `tests/fixtures/v1_parity.json` is produced by running the Python validator
//! (`tests/fixtures/gen_v1_parity.py`, see its header for how to re-run): the fixed example
//! `examples/generated/plate-four-holes/spec.json`, every case of `tests/test_validation.py`,
//! and hand-built cases covering each base kind × each 1.0 rule.
//!
//! For each case the multiset of `(code, severity, location)` of the carried-over 1.0 codes
//! must be equal. Locations map as: hole codes `feature_index i` → `e_h{i+1}`; base codes →
//! `v_base`; fillet/chamfer codes → `p_main`; `assumption` → none.
//!
//! 2.0-only codes that legitimately appear for upgraded documents are listed per case in
//! [`EXTRAS`] with the reason; any other 2.0 issue fails the test.

use std::collections::BTreeMap;
use std::path::PathBuf;

use promptcad_core::migrate::upgrade_1_to_2;
use promptcad_core::model::{CadDocument1, Code};
use promptcad_core::validate::{validate, ValidateOptions};
use serde_json::Value;

const CARRIED: [Code; 16] = [
    Code::ThinWall,
    Code::ThinBracket,
    Code::ExtremeAspectRatio,
    Code::HoleOutsidePart,
    Code::HoleInInnerVoid,
    Code::HoleIntersectsInnerVoid,
    Code::LowEdgeClearance,
    Code::OverlappingHoles,
    Code::BlindDepthExceedsMaterial,
    Code::CounterboreDepthExceedsMaterial,
    Code::CountersinkDepthExceedsMaterial,
    Code::TappedHoleWithoutThread,
    Code::ThreadAnnotationOnly,
    Code::FilletTooLarge,
    Code::ChamferTooLarge,
    Code::Assumption,
];

/// `(case, [(code, severity, target)], reason)`: 2.0-only issues expected on upgraded docs.
/// `(code, severity, target)`.
type Issue = (&'static str, &'static str, &'static str);

const EXTRAS: &[(&str, &[Issue], &str)] = &[
    (
        "ring_hole_in_inner_void",
        &[("empty_cut_no_effect", "warning", "e_h1")],
        "the hole lies entirely in the ring bore (that is what 1.0 hole_in_inner_void says), so \
         the cut removes no material",
    ),
    (
        "lbracket_z_blind_too_deep",
        &[("empty_cut_no_effect", "warning", "e_h1")],
        "1.0 starts a z hole at max_z = vertical_height + thickness; at y = 5 there is only the \
         base plate (z 0..3), so a 3 mm blind cut from z = 33 removes nothing (true in 1.0 \
         geometry too; 1.0 had no rule for it)",
    ),
    (
        "enclosure_z_blind_too_deep",
        &[("empty_cut_no_effect", "warning", "e_h1")],
        "a z hole of an open-top enclosure starts at the rim height (max_z = 30); at (0, 0) the \
         2 mm blind cut stays inside the open cavity and removes nothing",
    ),
];

type Key = (String, String, Option<String>);

fn location(code: &str, feature_index: Option<i64>) -> Option<String> {
    match code {
        "thin_wall" | "thin_bracket" | "extreme_aspect_ratio" => Some("v_base".into()),
        "fillet_too_large" | "chamfer_too_large" => Some("p_main".into()),
        "assumption" => None,
        _ => feature_index.map(|i| format!("e_h{}", i + 1)),
    }
}

fn multiset(keys: impl IntoIterator<Item = Key>) -> BTreeMap<Key, usize> {
    let mut m = BTreeMap::new();
    for k in keys {
        *m.entry(k).or_insert(0) += 1;
    }
    m
}

fn corpus() -> Vec<Value> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v1_parity.json");
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

#[test]
fn rust_validator_matches_python_design_validator() {
    let cases = corpus();
    assert!(cases.len() >= 30, "corpus has {} cases", cases.len());
    let mut failures = Vec::new();
    for case in &cases {
        let name = case["name"].as_str().unwrap();
        let spec = serde_json::to_string(&case["spec"]).unwrap();
        let doc1 = CadDocument1::from_json_str(&spec).unwrap_or_else(|e| panic!("{name}: {e}"));
        let doc2 = upgrade_1_to_2(&doc1);
        let v = validate(
            &doc2,
            &ValidateOptions {
                completed: true,
                submitter: None,
            },
        );

        let expected = multiset(case["expected_issues"].as_array().unwrap().iter().map(|i| {
            let code = i["code"].as_str().unwrap().to_owned();
            let loc = location(&code, i["feature_index"].as_i64());
            (code, i["severity"].as_str().unwrap().to_owned(), loc)
        }));
        let key = |i: &promptcad_core::model::ValidationIssue| -> Key {
            (
                i.code.clone(),
                serde_json::to_value(i.severity)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned(),
                i.target.clone(),
            )
        };
        let (carried, other): (Vec<_>, Vec<_>) = v
            .report
            .issues
            .iter()
            .partition(|i| i.code().is_some_and(|c| CARRIED.contains(&c)));
        let actual = multiset(carried.into_iter().map(key));
        if actual != expected {
            failures.push(format!(
                "{name}: carried-over verdict differs\n  python: {expected:?}\n  rust:   {actual:?}"
            ));
        }
        let extras = multiset(other.into_iter().map(key));
        let want = multiset(
            EXTRAS
                .iter()
                .filter(|(n, _, _)| *n == name)
                .flat_map(|(_, items, _)| items.iter())
                .map(|(c, s, t)| ((*c).to_owned(), (*s).to_owned(), Some((*t).to_owned()))),
        );
        if extras != want {
            failures.push(format!(
                "{name}: 2.0-only issues differ\n  listed: {want:?}\n  actual: {extras:?}"
            ));
        }
        // valid / review_required agree with 1.0 whenever no 2.0-only issue is involved
        if want.is_empty() {
            let py_valid = !case["expected_issues"]
                .as_array()
                .unwrap()
                .iter()
                .any(|i| i["severity"] == "error");
            assert_eq!(v.report.valid, py_valid, "{name}: valid");
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn every_listed_extra_names_a_corpus_case() {
    let names: Vec<String> = corpus()
        .iter()
        .map(|c| c["name"].as_str().unwrap().to_owned())
        .collect();
    for (n, items, why) in EXTRAS {
        assert!(names.iter().any(|x| x == n), "{n}");
        assert!(!items.is_empty() && !why.is_empty(), "{n}");
    }
}

#[test]
fn raw_thread_divergence_is_only_the_documented_normalization() {
    // Cases whose raw 1.0 verdict differs from the normalized one: only thread normalization
    // (§10.1: an invalid thread becomes null, the original goes to notes).
    for case in corpus() {
        if let Some(raw) = case.get("raw_issues") {
            let name = case["name"].as_str().unwrap();
            let threads: Vec<&Value> = case["spec"]["holes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|h| &h["thread"])
                .collect();
            assert!(
                threads.iter().any(|t| t.is_string()),
                "{name}: divergence without a thread"
            );
            let codes = |v: &Value| -> Vec<String> {
                v.as_array()
                    .unwrap()
                    .iter()
                    .map(|i| i["code"].as_str().unwrap().to_owned())
                    .filter(|c| c != "thread_annotation_only" && c != "tapped_hole_without_thread")
                    .collect()
            };
            assert_eq!(codes(raw), codes(&case["expected_issues"]), "{name}");
        }
    }
}
