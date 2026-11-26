//! Op log (ADR 0005 item 5): logging, JSONL, undo byte-identity, replay.

mod patch_support;

use patch_support::*;
use promptcad_core::canonical::{canonical_sha256, sha256_hex, to_canonical_string};
use promptcad_core::model::{CadDocument2, Patch, Role, Severity};
use promptcad_core::oplog::{
    replay, EntryKind, OpLog, OpLogError, ReplayError, Session, UndoError,
};
use promptcad_core::patch_engine::{apply_patch, undo, PatchOrigin};
use proptest::prelude::*;
use serde_json::{json, Value};

fn agent(role: Role) -> PatchOrigin {
    PatchOrigin::Agent(role)
}

/// Builds example A in a session, with one rejected patch in between.
fn session_a() -> Session {
    let mut s = Session::new(empty_a());
    for (i, (role, p)) in example_a_patches().into_iter().enumerate() {
        if i == 2 {
            // A stale patch (base_revision 0 at revision 2) is rejected and logged.
            let stale = patch(Role::Joiner, 0, json!([{"op": "add_note", "text": "late"}]));
            assert!(!s.submit(&stale, agent(Role::Joiner)).is_accepted());
        }
        let ts = format!("2026-11-26T00:00:0{i}Z");
        let out = s.submit_at(&p, agent(role), Some(ts));
        accepted(out);
    }
    s
}

#[test]
fn every_submission_is_logged_with_who_what_and_outcome() {
    let s = session_a();
    let log = s.log();
    assert_eq!(log.len(), 5);
    let seqs: Vec<u64> = log.entries().iter().map(|e| e.seq).collect();
    assert_eq!(seqs, [0, 1, 2, 3, 4]);
    for e in log.entries() {
        assert_eq!(serde_json::to_value(e.schema_version).unwrap(), "2.0");
        assert_eq!(e.kind, EntryKind::Patch);
        let p = e.patch.as_ref().unwrap();
        assert_eq!(
            e.patch_sha256.as_deref(),
            Some(sha256_hex(&to_canonical_string(p)).as_str())
        );
        assert_eq!(e.doc_sha256_before.len(), 64);
    }
    let rejected = &log.entries()[2];
    assert!(!rejected.accepted);
    assert_eq!(rejected.origin, agent(Role::Joiner));
    assert_eq!(rejected.reject_code(), Some("stale_revision"));
    assert_eq!(rejected.base_revision, 2);
    assert_eq!(rejected.revision_after, None);
    assert_eq!(rejected.doc_sha256_after, None);
    assert!(rejected.revert.is_none());
    assert_eq!(rejected.timestamp, None);

    let last = &log.entries()[4];
    assert!(last.accepted);
    assert_eq!(last.origin, agent(Role::Assembler));
    assert_eq!((last.base_revision, last.revision_after), (3, Some(4)));
    assert_eq!(last.timestamp.as_deref(), Some("2026-11-26T00:00:03Z"));
    assert_eq!(
        last.doc_sha256_after.as_deref(),
        Some(canonical_sha256(s.document()).as_str())
    );
    // The issue summary mirrors the validation report of the accepted document.
    let out = s.last_outcome().unwrap();
    let codes: Vec<(&str, Severity)> = out
        .issues()
        .iter()
        .map(|i| (i.code.as_str(), i.severity))
        .collect();
    let logged: Vec<(&str, Severity)> = last
        .issues
        .iter()
        .map(|i| (i.code.as_str(), i.severity))
        .collect();
    assert_eq!(codes, logged);
}

#[test]
fn rejected_patch_leaves_document_untouched_and_is_logged() {
    let mut s = Session::new(example_a());
    let before = canon(s.document());
    let p = patch(
        Role::ComponentSmith,
        4,
        json!([dot("d_a"), {"op": "add_note", "text": "x"},
               {"op": "set_field", "target": "v_plate:length", "value": 1}]),
    );
    let out = s.submit(&p, agent(Role::ComponentSmith));
    rejected_with(out, promptcad_core::model::RejectCode::FieldIsBound);
    assert_eq!(canon(s.document()), before);
    let e = s.log().last().unwrap();
    assert!(!e.accepted);
    assert_eq!(e.reject.as_ref().unwrap().op_index, Some(2));
    assert_eq!(e.doc_sha256_before, canonical_sha256(s.document()));
    assert_eq!(s.undo_depth(), 0);
}

#[test]
fn jsonl_round_trip_is_byte_identical() {
    let mut s = session_a();
    s.undo().unwrap();
    let text = s.log().to_jsonl();
    assert_eq!(text.lines().count(), s.log().len());
    assert!(text.ends_with('\n'));
    assert!(!text.contains('\r'));
    for line in text.lines() {
        assert!(line.starts_with("{\"seq\":"));
    }
    let parsed = OpLog::from_jsonl(&text).unwrap();
    assert_eq!(&parsed, s.log());
    assert_eq!(parsed.to_jsonl(), text);
    // Deterministic: building the same session again gives the same bytes.
    let mut again = session_a();
    again.undo().unwrap();
    assert_eq!(again.log().to_jsonl(), text);

    // Sequence gaps and garbage are rejected.
    let mut lines: Vec<&str> = text.lines().collect();
    lines.swap(0, 1);
    assert!(matches!(
        OpLog::from_jsonl(&lines.join("\n")),
        Err(OpLogError::Sequence {
            expected: 0,
            found: 1
        })
    ));
    assert!(matches!(
        OpLog::from_jsonl("{\"seq\":0}\n"),
        Err(OpLogError::Parse { line: 1, .. })
    ));
}

#[test]
fn undo_restores_byte_identical_documents_and_is_logged() {
    let mut s = session_a();
    let mut states = vec![canon(&empty_a())];
    let initial = empty_a();
    let mut doc = initial.clone();
    for (role, p) in example_a_patches() {
        doc = accepted(&apply_patch(&doc, &p, agent(role)));
        states.push(canon(&doc));
    }
    assert_eq!(canon(s.document()), states[4]);
    for k in (0..4).rev() {
        let undone_seq = {
            let e = s.undo().unwrap();
            assert_eq!(e.kind, EntryKind::Undo);
            assert_eq!(e.origin, PatchOrigin::Human);
            assert!(e.patch.is_none() && e.patch_sha256.is_none());
            assert_eq!(e.revision_after, Some(k as u64));
            e.undoes.unwrap()
        };
        assert!(s.log().get(undone_seq).unwrap().accepted);
        assert_eq!(
            canon(s.document()),
            states[k],
            "after undoing to revision {k}"
        );
        assert_eq!(s.document().revision, k as u64);
    }
    assert!(matches!(s.undo(), Err(UndoError::NothingToUndo)));
    // 5 patch entries (1 rejected) + 4 undo entries.
    assert_eq!(s.log().len(), 9);

    // Undo after a new patch undoes the new patch first, then older ones (stack).
    let mut s = Session::new(example_a());
    let a = patch(Role::Joiner, 4, json!([{"op": "add_note", "text": "a"}]));
    accepted(s.submit(&a, agent(Role::Joiner)));
    s.undo().unwrap();
    let b = patch(Role::Joiner, 4, json!([{"op": "add_note", "text": "b"}]));
    accepted(s.submit(&b, agent(Role::Joiner)));
    assert_eq!(s.document().revision, 5);
    assert_eq!(s.document().notes, ["b"]);
    let e = s.undo_by(agent(Role::Joiner), Some("t".into())).unwrap();
    assert_eq!(e.undoes, Some(2));
    assert_eq!(e.timestamp.as_deref(), Some("t"));
    assert_eq!(canon(s.document()), canon(&example_a()));
}

#[test]
fn replay_reproduces_the_document_and_verifies_the_log() {
    let mut s = session_a();
    let a = patch(Role::Joiner, 4, json!([{"op": "add_note", "text": "a"}]));
    accepted(s.submit(&a, agent(Role::Joiner)));
    s.undo().unwrap();
    s.undo().unwrap();
    let b = patch(Role::Assembler, 3, json!([{"op": "add_note", "text": "b"}]));
    accepted(s.submit(&b, agent(Role::Assembler)));
    let (doc, log) = s.clone().into_parts();
    let replayed = replay(&empty_a(), &log).unwrap();
    assert_eq!(canon(&replayed), canon(&doc));
    // Through JSONL too.
    let reloaded = OpLog::from_jsonl(&log.to_jsonl()).unwrap();
    assert_eq!(canon(&replay(&empty_a(), &reloaded).unwrap()), canon(&doc));
    // A resumed session can keep undoing with the logged reverse deltas.
    let mut resumed = Session::resume(empty_a(), reloaded).unwrap();
    assert_eq!(resumed.undo_depth(), s.undo_depth());
    resumed.undo().unwrap();
    s.undo().unwrap();
    assert_eq!(canon(resumed.document()), canon(s.document()));

    // Wrong initial document.
    assert_eq!(
        replay(&example_a(), &log),
        Err(ReplayError::StateBefore { seq: 0 })
    );
    // Tampered patch body: hash mismatch.
    let mut text: Vec<Value> = log
        .to_jsonl()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    text[0]["patch"]["rationale"] = json!("edited");
    let tampered = text
        .iter()
        .map(|v| serde_json::to_string(v).unwrap() + "\n")
        .collect::<String>();
    let tampered = OpLog::from_jsonl(&tampered).unwrap();
    assert_eq!(
        replay(&empty_a(), &tampered),
        Err(ReplayError::PatchHash { seq: 0 })
    );
    // Recorded outcome that the engine would not reproduce.
    text[0]["patch"]["rationale"] = json!("test patch");
    text[2]["accepted"] = json!(true);
    text[2]["reject"] = Value::Null;
    let forged = text
        .iter()
        .map(|v| serde_json::to_string(v).unwrap() + "\n")
        .collect::<String>();
    let forged = OpLog::from_jsonl(&forged).unwrap();
    assert!(matches!(
        replay(&empty_a(), &forged),
        Err(ReplayError::Outcome { seq: 2, .. })
    ));
}

// ---- property: random valid patches on the examples, then undo ------------------------------

fn example_docs() -> [CadDocument2; 2] {
    [example_a(), example_b()]
}

/// Ops a human may submit against either example; most are valid, some are rejected
/// (duplicate ids, missing bindings), which is fine: only accepted patches are undone.
fn op_strategy(doc: &CadDocument2) -> BoxedStrategy<Value> {
    let params: Vec<(String, f64)> = doc
        .parameters
        .iter()
        .map(|p| (p.name.to_string(), p.value))
        .collect();
    let bindings: Vec<String> = doc.bindings.iter().map(|b| b.target.to_string()).collect();
    let n_params = params.len();
    let n_bindings = bindings.len().max(1);
    prop_oneof![
        (0..n_params, 0.8f64..1.25).prop_map(move |(i, f)| {
            let (name, v) = &params[i];
            json!({"op": "set_parameter", "parameter":
                {"name": name, "value": v * f, "min": null, "max": null, "note": null}})
        }),
        "[a-z ]{1,20}".prop_map(|t| json!({"op": "add_note", "text": t})),
        "[a-z ]{1,20}".prop_map(|t| json!({"op": "add_assumption", "text": t})),
        "d_p[0-9]{1,2}".prop_map(|id| dot(&id)),
        (0..n_bindings).prop_map(move |i| {
            json!({"op": "remove_binding", "target": bindings.get(i).cloned().unwrap_or_default()})
        }),
        ("e_r[0-9]{1,2}", 1.0f64..40.0).prop_map(|(id, h)| box_empty(&id, "keep_out", h, h, h)),
    ]
    .boxed()
}

fn human_patch(doc: &CadDocument2, ops: Vec<Value>) -> Option<Patch> {
    // remove_binding with an empty target cannot even be parsed: drop such patches.
    serde_json::from_value(json!({
        "agent": "space_planner", "base_revision": doc.revision,
        "rationale": "random", "resolves": [], "ops": ops
    }))
    .ok()
}

fn ops_for(doc: &CadDocument2) -> impl Strategy<Value = Vec<Vec<Value>>> {
    prop::collection::vec(prop::collection::vec(op_strategy(doc), 1..6), 1..5)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn undo_after_random_patches_is_byte_identical(
        which in 0usize..2,
        seed_ops in ops_for(&example_a()),
        seed_ops_b in ops_for(&example_b()),
    ) {
        let docs = example_docs();
        let start = docs[which].clone();
        let batches = if which == 0 { seed_ops } else { seed_ops_b };
        let mut s = Session::new(start.clone());
        let mut history = vec![canon(&start)];
        for ops in batches {
            let Some(p) = human_patch(s.document(), ops) else { continue };
            let before = canon(s.document());
            let out = apply_patch(s.document(), &p, PatchOrigin::Human);
            if let Some(a) = out.accepted() {
                // Single-step undo straight from the outcome.
                prop_assert_eq!(&canon(&undo(&a.document, &a.revert).unwrap()), &before);
            }
            let accepted_now = s.submit(&p, PatchOrigin::Human).is_accepted();
            if accepted_now {
                history.push(canon(s.document()));
            } else {
                prop_assert_eq!(&canon(s.document()), &before);
            }
        }
        // Undo everything, checking each intermediate state.
        while s.undo_depth() > 0 {
            history.pop();
            s.undo().unwrap();
            prop_assert_eq!(&canon(s.document()), history.last().unwrap());
        }
        prop_assert_eq!(canon(s.document()), canon(&start));
        // And the whole log replays.
        let (doc, log) = s.into_parts();
        prop_assert_eq!(canon(&replay(&start, &log).unwrap()), canon(&doc));
    }
}

#[test]
fn some_random_style_patches_are_accepted() {
    // Guards the property test against vacuity: the generator's op shapes are acceptable.
    for doc in example_docs() {
        let name = doc.parameters[0].name.to_string();
        let value = doc.parameters[0].value * 1.1;
        let p = human_patch(
            &doc,
            vec![
                json!({"op": "set_parameter", "parameter":
                    {"name": name, "value": value, "min": null, "max": null, "note": null}}),
                json!({"op": "add_note", "text": "n"}),
                dot("d_p1"),
                box_empty("e_rnd", "keep_out", 5.0, 5.0, 5.0),
                json!({"op": "remove_binding", "target": doc.bindings[0].target.to_string()}),
            ],
        )
        .unwrap();
        let out = apply_patch(&doc, &p, PatchOrigin::Human);
        let new = accepted(&out);
        let a = out.accepted().unwrap();
        assert_eq!(canon(&undo(&new, &a.revert).unwrap()), canon(&doc));
    }
}
