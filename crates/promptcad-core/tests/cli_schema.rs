//! Integration tests for the `promptcad-core schema` subcommand (ADR 0008).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FILES: [&str; 5] = [
    "cad-document-2.schema.json",
    "patch.schema.json",
    "cad-document-1.schema.json",
    "resolved.schema.json",
    "validation-report.schema.json",
];

/// A scratch directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("promptcad-core-test-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        TempDir(dir)
    }

    fn path(&self) -> &Path {
        &self.0
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

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn schema_args<'a>(dir: &'a Path, extra: &[&'a str]) -> Vec<&'a str> {
    let mut args = vec!["schema", "--out-dir"];
    args.push(dir.to_str().expect("utf-8 temp path"));
    args.extend_from_slice(extra);
    args
}

#[test]
fn generate_then_check_round_trip() {
    let tmp = TempDir::new("roundtrip");
    let out = run(&schema_args(tmp.path(), &[]));
    assert_eq!(exit_code(&out), 0, "generate failed: {}", stderr(&out));

    for name in FILES {
        let text = fs::read_to_string(tmp.path().join(name)).expect(name);
        assert!(text.ends_with("}\n"), "{name} must end with a newline");
        assert!(!text.contains('\r'), "{name} must use LF line endings");
    }

    let out = run(&schema_args(tmp.path(), &["--check"]));
    assert_eq!(exit_code(&out), 0, "fresh check failed: {}", stderr(&out));

    // Corrupt one file: --check must fail and name it.
    fs::write(tmp.path().join("patch.schema.json"), "{}\n").unwrap();
    let out = run(&schema_args(tmp.path(), &["--check"]));
    assert_eq!(exit_code(&out), 1);
    assert!(
        stderr(&out).contains("patch.schema.json"),
        "{}",
        stderr(&out)
    );

    // Regenerate, then delete one file: --check must report it as missing.
    run(&schema_args(tmp.path(), &[]));
    fs::remove_file(tmp.path().join("resolved.schema.json")).unwrap();
    let out = run(&schema_args(tmp.path(), &["--check"]));
    assert_eq!(exit_code(&out), 1);
    assert!(stderr(&out).contains("missing"), "{}", stderr(&out));
    assert!(stderr(&out).contains("resolved.schema.json"));
}

#[test]
fn check_writes_nothing() {
    let tmp = TempDir::new("check-readonly");
    let out = run(&schema_args(tmp.path(), &["--check"]));
    assert_eq!(exit_code(&out), 1);
    let entries = fs::read_dir(tmp.path()).unwrap().count();
    assert_eq!(entries, 0, "--check must not create files");
}

#[test]
fn output_is_deterministic() {
    let first = TempDir::new("det-a");
    let second = TempDir::new("det-b");
    run(&schema_args(first.path(), &[]));
    run(&schema_args(second.path(), &[]));
    for name in FILES {
        let a = fs::read(first.path().join(name)).unwrap();
        let b = fs::read(second.path().join(name)).unwrap();
        assert_eq!(a, b, "{name} differs between runs");
    }
}

#[test]
fn committed_schemas_are_up_to_date() {
    let committed = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas/cad2");
    let out = run(&schema_args(&committed, &["--check"]));
    assert_eq!(
        exit_code(&out),
        0,
        "schemas/cad2 is out of date; run `cargo run -- schema --out-dir ../../schemas/cad2`: {}",
        stderr(&out)
    );
}

#[test]
fn usage_and_version() {
    assert_eq!(exit_code(&run(&[])), 0);
    assert_eq!(exit_code(&run(&["--help"])), 0);
    assert_eq!(exit_code(&run(&["bogus"])), 2);
    assert_eq!(exit_code(&run(&["schema"])), 2, "--out-dir is required");

    let out = run(&["--version"]);
    assert_eq!(exit_code(&out), 0);
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert_eq!(
        text,
        format!("promptcad-core {}\n", env!("CARGO_PKG_VERSION"))
    );
}
