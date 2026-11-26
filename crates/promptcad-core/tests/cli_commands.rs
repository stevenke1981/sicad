//! Integration tests for the document subcommands of `promptcad-core`: validate, solve,
//! upgrade, downgrade, normalize and apply-patch.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{json, Value};

/// A scratch directory under the system temp dir, removed on drop. Names are unique per
/// process and per call, so parallel tests never share a directory.
struct TempDir(PathBuf);

static COUNTER: AtomicUsize = AtomicUsize::new(0);

impl TempDir {
    fn new(tag: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "promptcad-core-cli-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        TempDir(dir)
    }

    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_promptcad-core"))
        .args(args)
        .output()
        .expect("spawn promptcad-core")
}

fn exit_code(out: &Output) -> i32 {
    out.status.code().expect("process exited with a code")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn repo_file(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn path_str(path: &Path) -> &str {
    path.to_str().expect("utf-8 path")
}

fn read_json(path: &Path) -> Value {
    let text = fs::read_to_string(path).expect("read output file");
    serde_json::from_str(&text).expect("output is JSON")
}

const PLATE_2: &str = "examples/cad2/plate-four-holes.cad2.json";
const ENCLOSURE_2: &str = "examples/cad2/enclosure-with-lid.cad2.json";
const PLATE_1: &str = "examples/generated/plate-four-holes/spec.json";

/// Writes the 2.0 example at `rel` with its example-only key removed, returning the path.
fn stripped_copy(tmp: &TempDir, rel: &str, name: &str) -> PathBuf {
    let mut value: Value =
        serde_json::from_str(&fs::read_to_string(repo_file(rel)).unwrap()).unwrap();
    value.as_object_mut().unwrap().remove("expected_resolved");
    let path = tmp.file(name);
    fs::write(&path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
    path
}

#[test]
fn validate_2_examples_with_strip_flag() {
    for rel in [PLATE_2, ENCLOSURE_2] {
        let out = run(&[
            "validate",
            path_str(&repo_file(rel)),
            "--strip-example-keys",
        ]);
        assert_eq!(exit_code(&out), 0, "{rel}: {}", stderr(&out));
        let report: Value = serde_json::from_str(&stdout(&out)).expect("report is JSON");
        assert_eq!(report["valid"], json!(true), "{rel}");
    }
}

#[test]
fn validate_example_without_strip_flag_is_input_error() {
    let out = run(&["validate", path_str(&repo_file(PLATE_2))]);
    assert_eq!(exit_code(&out), 2);
    assert!(
        stderr(&out).contains("expected_resolved"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn validate_1_0_example() {
    let out = run(&["validate", path_str(&repo_file(PLATE_1))]);
    assert_eq!(exit_code(&out), 0, "{}", stderr(&out));
    let report: Value = serde_json::from_str(&stdout(&out)).expect("report is JSON");
    assert_eq!(report["valid"], json!(true));
}

#[test]
fn solve_plate_is_deterministic_and_contains_v_plate() {
    let first = run(&[
        "solve",
        path_str(&repo_file(PLATE_2)),
        "--strip-example-keys",
    ]);
    let second = run(&[
        "solve",
        path_str(&repo_file(PLATE_2)),
        "--strip-example-keys",
    ]);
    assert_eq!(exit_code(&first), 0, "{}", stderr(&first));
    assert_eq!(exit_code(&second), 0, "{}", stderr(&second));
    assert_eq!(
        first.stdout, second.stdout,
        "solve output must be byte-identical"
    );

    let resolved: Value = serde_json::from_str(&stdout(&first)).expect("resolved is JSON");
    assert!(
        resolved["components"].get("v_plate").is_some(),
        "components must contain v_plate"
    );
}

#[test]
fn upgrade_then_downgrade_round_trips_1_0_example() {
    let tmp = TempDir::new("roundtrip");
    let upgraded = tmp.file("upgraded.cad2.json");

    let out = run(&["upgrade", path_str(&repo_file(PLATE_1))]);
    assert_eq!(exit_code(&out), 0, "{}", stderr(&out));
    fs::write(&upgraded, &out.stdout).unwrap();

    let out = run(&["downgrade", path_str(&upgraded)]);
    assert_eq!(exit_code(&out), 0, "{}", stderr(&out));

    let original: Value =
        serde_json::from_str(&fs::read_to_string(repo_file(PLATE_1)).unwrap()).unwrap();
    let round_tripped: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(round_tripped, original);
}

#[test]
fn upgrade_rejects_2_0_input() {
    let out = run(&["upgrade", path_str(&repo_file(PLATE_2))]);
    assert_eq!(exit_code(&out), 2);
}

#[test]
fn downgrade_enclosure_is_not_expressible() {
    let tmp = TempDir::new("downgrade-enclosure");
    let spec = stripped_copy(&tmp, ENCLOSURE_2, "enclosure.cad2.json");
    let out = run(&["downgrade", path_str(&spec)]);
    assert_eq!(exit_code(&out), 1, "{}", stderr(&out));
    assert!(stdout(&out).is_empty(), "no document on failure");
}

#[test]
fn normalize_plate_is_idempotent() {
    let tmp = TempDir::new("normalize");
    let spec = stripped_copy(&tmp, PLATE_2, "plate.cad2.json");

    let first = run(&["normalize", path_str(&spec)]);
    assert_eq!(exit_code(&first), 0, "{}", stderr(&first));
    let once = tmp.file("once.cad2.json");
    fs::write(&once, &first.stdout).unwrap();

    let second = run(&["normalize", path_str(&once)]);
    assert_eq!(exit_code(&second), 0, "{}", stderr(&second));
    assert_eq!(second.stdout, first.stdout, "normalize must be idempotent");
}

#[test]
fn apply_patch_stale_revision_is_rejected() {
    let tmp = TempDir::new("patch-stale");
    let doc = stripped_copy(&tmp, PLATE_2, "plate.cad2.json");
    let revision = read_json(&doc)["revision"].as_u64().unwrap();
    let patch = tmp.file("stale.patch.json");
    fs::write(
        &patch,
        json!({
            "agent": "assembler",
            "base_revision": revision + 99,
            "rationale": "stale test",
            "resolves": [],
            "ops": [{"op": "add_note", "text": "should not land"}],
        })
        .to_string(),
    )
    .unwrap();

    let out = run(&["apply-patch", path_str(&doc), path_str(&patch)]);
    assert_eq!(exit_code(&out), 1, "{}", stderr(&out));
    assert!(stdout(&out).contains("stale_revision"), "{}", stdout(&out));
}

#[test]
fn apply_patch_add_note_is_accepted_and_bumps_revision() {
    let tmp = TempDir::new("patch-note");
    let doc = stripped_copy(&tmp, PLATE_2, "plate.cad2.json");
    let before = read_json(&doc);
    let revision = before["revision"].as_u64().unwrap();
    let patch = tmp.file("note.patch.json");
    fs::write(
        &patch,
        json!({
            "agent": "assembler",
            "base_revision": revision,
            "rationale": "cli note test",
            "resolves": [],
            "ops": [{"op": "add_note", "text": "cli note from test"}],
        })
        .to_string(),
    )
    .unwrap();

    let out = run(&["apply-patch", path_str(&doc), path_str(&patch)]);
    assert_eq!(exit_code(&out), 0, "{}", stderr(&out));
    let after: Value = serde_json::from_str(&stdout(&out)).expect("document is JSON");
    assert_eq!(after["revision"].as_u64().unwrap(), revision + 1);
    let notes = after["notes"].as_array().unwrap();
    assert!(
        notes.iter().any(|n| n == "cli note from test"),
        "note missing from {notes:?}"
    );
}

#[test]
fn missing_file_is_input_error() {
    let tmp = TempDir::new("missing");
    let missing = tmp.file("does-not-exist.json");
    let path = path_str(&missing);
    assert_eq!(exit_code(&run(&["validate", path])), 2);
    assert_eq!(exit_code(&run(&["solve", path])), 2);
    assert_eq!(exit_code(&run(&["normalize", path])), 2);
    assert_eq!(exit_code(&run(&["upgrade", path])), 2);
    assert_eq!(exit_code(&run(&["downgrade", path])), 2);
}

#[test]
fn out_flag_writes_file_instead_of_stdout() {
    let tmp = TempDir::new("out-flag");
    let out_file = tmp.file("resolved.json");
    let out = run(&[
        "solve",
        path_str(&repo_file(PLATE_1)),
        "--out",
        path_str(&out_file),
    ]);
    assert_eq!(exit_code(&out), 0, "{}", stderr(&out));
    assert!(out.stdout.is_empty());
    let text = fs::read_to_string(&out_file).unwrap();
    assert!(text.ends_with("}\n"));
}

#[test]
fn unknown_flag_and_wrong_arity_are_usage_errors() {
    assert_eq!(exit_code(&run(&["validate"])), 2);
    assert_eq!(
        exit_code(&run(&[
            "validate",
            path_str(&repo_file(PLATE_1)),
            "--bogus"
        ])),
        2
    );
    assert_eq!(
        exit_code(&run(&["apply-patch", path_str(&repo_file(PLATE_2))])),
        2
    );
}
