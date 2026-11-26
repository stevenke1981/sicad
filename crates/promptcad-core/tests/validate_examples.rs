//! Both `examples/cad2` documents validate without errors; the exact warnings/infos are pinned.

use std::path::PathBuf;

use promptcad_core::canonical::parse_example;
use promptcad_core::model::{CadDocument2, Role, Severity};
use promptcad_core::validate::{blocking_issues, compute_root, validate, ValidateOptions};

fn load(name: &str) -> CadDocument2 {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/cad2")
        .join(name);
    parse_example(&std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{p:?}: {e}")))
        .unwrap()
        .0
}

fn completed() -> ValidateOptions {
    ValidateOptions {
        completed: true,
        submitter: None,
    }
}

fn summary(doc: &CadDocument2, opts: &ValidateOptions) -> Vec<(String, Severity, Option<String>)> {
    validate(doc, opts)
        .report
        .issues
        .into_iter()
        .map(|i| (i.code, i.severity, i.target))
        .collect()
}

fn s(code: &str, sev: Severity, target: Option<&str>) -> (String, Severity, Option<String>) {
    (code.to_owned(), sev, target.map(str::to_owned))
}

#[test]
fn plate_four_holes() {
    let doc = load("plate-four-holes.cad2.json");
    let v = validate(&doc, &completed());
    assert!(v.report.valid, "{:#?}", v.report.issues);
    // planner.review_required and the assumption both force review
    assert!(v.report.review_required);
    assert_eq!(
        summary(&doc, &completed()),
        vec![
            // the four M6 clearance holes carry a thread annotation (1.0 rule)
            s("thread_annotation_only", Severity::Info, Some("e_h1")),
            s("thread_annotation_only", Severity::Info, Some("e_h2")),
            s("thread_annotation_only", Severity::Info, Some("e_h3")),
            s("thread_annotation_only", Severity::Info, Some("e_h4")),
            // "M6 未提供實際孔徑…"
            s("assumption", Severity::Info, None),
        ]
    );
    // solver ran: resolved matches the solver
    assert_eq!(v.resolved, promptcad_core::solver::solve(&doc));
    assert!(blocking_issues(&v.report).is_empty());
    assert_eq!(compute_root(&doc), doc.root);
}

#[test]
fn enclosure_with_lid() {
    let doc = load("enclosure-with-lid.cad2.json");
    let v = validate(&doc, &completed());
    assert!(v.report.valid, "{:#?}", v.report.issues);
    assert!(v.report.review_required);
    assert_eq!(
        summary(&doc, &completed()),
        vec![
            // the PCB reserve sits in the closed box: floor and walls of v_body plus the lid
            s("reserve_unreachable", Severity::Info, Some("e_pcb")),
            s("assumption", Severity::Info, None),
            s("assumption", Severity::Info, None),
            s("assumption", Severity::Info, None),
            s("assumption", Severity::Info, None),
        ]
    );
    let unreachable = &v.report.issues[0];
    assert_eq!(unreachable.related, ["p_body", "p_lid"]);
    assert_eq!(unreachable.owner, Some(Role::SpacePlanner));
    assert_eq!(compute_root(&doc), doc.root);
}

#[test]
fn examples_are_valid_as_intermediate_documents_and_for_every_submitter() {
    for name in ["plate-four-holes.cad2.json", "enclosure-with-lid.cad2.json"] {
        let doc = load(name);
        let base = summary(&doc, &completed());
        for submitter in [None, Some(Role::Joiner), Some(Role::Assembler)] {
            let opts = ValidateOptions {
                completed: false,
                submitter,
            };
            assert_eq!(summary(&doc, &opts), base, "{name}");
        }
    }
}

#[test]
fn validation_is_deterministic() {
    for name in ["plate-four-holes.cad2.json", "enclosure-with-lid.cad2.json"] {
        let doc = load(name);
        assert_eq!(validate(&doc, &completed()), validate(&doc, &completed()));
    }
}
