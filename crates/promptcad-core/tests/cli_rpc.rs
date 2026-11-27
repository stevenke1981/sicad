//! `promptcad-core rpc`: one JSON request on stdin, one JSON response line on stdout.

mod api_support;

use std::io::Write;
use std::process::{Command, Output, Stdio};
use std::thread;

use api_support::*;
use promptcad_core::api::MAX_REQUEST_BYTES;
use serde_json::{json, Value};

fn rpc_with_args(args: &[&str], input: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_promptcad-core"))
        .arg("rpc")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn promptcad-core rpc");
    let mut stdin = child.stdin.take().unwrap();
    let input = input.to_vec();
    // write on a thread so a large input cannot deadlock against the child's output
    let writer = thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let out = child.wait_with_output().expect("wait for rpc");
    writer.join().unwrap();
    out
}

fn rpc(input: &[u8]) -> Output {
    rpc_with_args(&[], input)
}

fn exit_code(out: &Output) -> i32 {
    out.status.code().expect("process exited with a code")
}

/// The single response line, checked like every API response.
fn response(out: &Output) -> Value {
    let text = String::from_utf8(out.stdout.clone()).expect("UTF-8 stdout");
    let line = text
        .strip_suffix('\n')
        .expect("response ends with a newline");
    assert!(!line.contains('\n'), "exactly one line");
    let (_, _, value) = call_text_checked(line);
    value
}

/// Re-checks a response line produced by the binary: it must equal what the library returns
/// for the same request, so here only parse + schema are verified.
fn call_text_checked(line: &str) -> (String, promptcad_core::api::CoreResponse, Value) {
    let typed: promptcad_core::api::CoreResponse =
        serde_json::from_str(line).expect("response parses as CoreResponse");
    assert_eq!(typed.to_json(), line);
    let value: Value = serde_json::from_str(line).unwrap();
    let errors = check_schema(&response_schema(), &value);
    assert!(errors.is_empty(), "{errors:#?}");
    (line.to_owned(), typed, value)
}

#[test]
fn version_over_stdio() {
    let out = rpc(br#"{"op":"version"}"#);
    assert_eq!(exit_code(&out), 0);
    let v = response(&out);
    assert_eq!(v["result"]["crate_version"], env!("CARGO_PKG_VERSION"));
}

#[test]
fn validate_example_matches_library() {
    let req = request("validate", json!({"document": example(ENCLOSURE_2)}));
    let text = req.to_string();
    let out = rpc(text.as_bytes());
    assert_eq!(exit_code(&out), 0);
    let v = response(&out);
    assert_eq!(v["result"]["report"]["valid"], true);
    let expected = format!("{}\n", promptcad_core::api::handle_json(&text));
    assert_eq!(String::from_utf8(out.stdout).unwrap(), expected);
}

#[test]
fn apply_patch_over_stdio() {
    let doc = example(PLATE_2);
    let req = request(
        "apply_patch",
        json!({"document": doc, "patch": note_patch(&doc, "assembler", "rpc"), "origin": "assembler", "log_seq": 3}),
    );
    let out = rpc(req.to_string().as_bytes());
    assert_eq!(exit_code(&out), 0);
    let v = response(&out);
    let o = &v["result"]["outcome"];
    assert_eq!(o["status"], "accepted");
    assert!(o["log_line"].as_str().unwrap().starts_with(r#"{"seq":3,"#));
}

#[test]
fn error_responses_exit_zero() {
    for (input, code) in [
        (&b"garbage"[..], "bad_request"),
        (b"", "bad_request"),
        (br#"{"op":"nope"}"#, "bad_request"),
        (b"\xff\xfe{}", "bad_request"),
        (
            br#"{"op":"validate","document":{"schema_version":"9.9"}}"#,
            "unsupported_schema_version",
        ),
    ] {
        let out = rpc(input);
        assert_eq!(
            exit_code(&out),
            0,
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let v = response(&out);
        assert_eq!(v["status"], "error");
        assert_eq!(v["code"], code, "{}", String::from_utf8_lossy(input));
    }
}

#[test]
fn oversize_stdin_is_too_large() {
    let input = vec![b' '; MAX_REQUEST_BYTES + 1024];
    let out = rpc(&input);
    assert_eq!(exit_code(&out), 0);
    let v = response(&out);
    assert_eq!(v["code"], "too_large");
    assert!(v["message"]
        .as_str()
        .unwrap()
        .contains(&format!("{}", MAX_REQUEST_BYTES + 1024)));
}

#[test]
fn deterministic_bytes() {
    let req = request("describe", json!({"document": example(ENCLOSURE_2)})).to_string();
    let a = rpc(req.as_bytes());
    let b = rpc(req.as_bytes());
    assert_eq!(a.stdout, b.stdout);
}

#[test]
fn usage_errors_exit_two() {
    let out = rpc_with_args(&["extra"], br#"{"op":"version"}"#);
    assert_eq!(exit_code(&out), 2);
    assert!(out.stdout.is_empty());
    let out = rpc_with_args(&["--help"], b"");
    assert_eq!(exit_code(&out), 0);
    assert!(String::from_utf8_lossy(&out.stdout).contains("rpc"));
}
