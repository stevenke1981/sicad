//! In-process host for the Rust core (`promptcad_core::api`, ADR 0005).
//!
//! # Frontend contract
//!
//! ```js
//! const response = await invoke('core_request', { request: { op: 'validate', document } });
//! if (response.status === 'ok') { /* response.result.op === 'validate', ... */ }
//! else { /* response.code, response.message, response.path, response.violations */ }
//! ```
//!
//! * Command: `core_request`; single argument named `request` (a JSON object, tagged by `op`:
//!   `validate`, `solve`, `normalize`, `upgrade`, `downgrade`, `apply_patch`, `describe`,
//!   `version`; same shape as the server path's request body, see
//!   `crates/promptcad-core/README.md`).
//! * Resolves with the `CoreResponse` object: `{status: "ok", result}` or
//!   `{status: "error", code, message, path, violations}`. A request that is not a valid
//!   `CoreRequest` resolves with `code: "bad_request"`; a request whose compact JSON encoding
//!   exceeds `MAX_REQUEST_BYTES` (8 MiB) resolves with `code: "too_large"`; a response that
//!   would exceed `MAX_RESPONSE_BYTES` (24 MiB) is replaced by `code: "response_too_large"`
//!   (the same cap as `handle_json` / `promptcad-core rpc`). None of these rejects.
//! * The invoke promise rejects (with a string) only on a host failure (the blocking task
//!   could not be joined), never for a bad request.
//! * ACL permission: `allow-core-request` (granted in `capabilities/default.json`).
//!
//! # Design
//!
//! The IPC layer has already parsed the request into a `serde_json::Value`, so the pure part
//! ([`respond`]) converts it straight into a `CoreRequest` (`from_value`, no re-encoding) and
//! measures the size with a counting writer (no allocation). The response is serialized once,
//! by the core's own `CoreResponse::to_json`, and returned as a `RawValue` so Tauri embeds
//! the bytes verbatim instead of re-encoding a `Value`. For a valid request the response bytes
//! are therefore identical to `promptcad_core::api::handle_json`. The size check measures the
//! compact encoding, which can be smaller than the original text if the sender used
//! whitespace; error message texts for malformed requests may differ from `handle_json`
//! (`from_value` has no line/column), the code and shape do not.
//!
//! The response cap is applied by `CoreResponse::to_json` itself (a bounded writer), so this
//! host and `handle_json` share one implementation and stay byte-identical for the
//! `response_too_large` case too.
//!
//! The command runs the core on a blocking thread so a large validation never stalls the
//! event loop.
//!
//! # Known divergences from the server host
//!
//! * **Duplicate JSON keys.** `handle_json` (server mode, `promptcad-core rpc`) parses the
//!   request text with serde's derived `CoreRequest` deserializer, which rejects a duplicate
//!   field: `{"op":"version","op":"validate",…}` is `bad_request` there. Here the IPC layer
//!   has already parsed the text into a `serde_json::Value`, which keeps the *last* value of
//!   a duplicate key, so the same text would be processed as `validate`. This is harmless and
//!   deliberately not checked (it would cost a re-scan of every request): a JavaScript
//!   object cannot carry duplicate keys, so `invoke` never produces such a request.
//!
//! # Panics: the release build aborts
//!
//! `promptcad_core::api::handle` wraps the core in `catch_unwind` and turns a panic into an
//! `internal` response — but only when panics unwind. This crate's `[profile.release]` sets
//! `panic = "abort"` (`Cargo.toml`), so in the shipped desktop app `catch_unwind` is a no-op:
//! a latent panic in the core aborts the whole app process (no `internal` response, the
//! invoke promise never settles because the process is gone). `spawn_blocking` does not help
//! either, since an abort is not a task failure. Debug builds (`cargo build`, `cargo test`)
//! unwind, so the guard works there and tests cannot observe the release behaviour. Changing
//! the profile is a bundle-size / shell-wide decision for the app owner; until then, treat
//! core panics as fatal in the desktop app.

use std::io;

use promptcad_core::api::{
    bad_request, handle, too_large, CoreRequest, CoreResponse, MAX_REQUEST_BYTES,
    MAX_RESPONSE_BYTES,
};
use serde_json::value::RawValue;
use serde_json::Value;

/// Counts bytes written without storing them.
struct CountingWriter(usize);

impl io::Write for CountingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0 += buf.len();
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Size of the compact JSON encoding of `value`.
fn encoded_len(value: &Value) -> usize {
    let mut counter = CountingWriter(0);
    // Serializing a `Value` into a counter cannot fail.
    let _ = serde_json::to_writer(&mut counter, value);
    counter.0
}

/// Pure core of the `core_request` command: request value in, response JSON out.
pub fn respond(request: Value) -> Box<RawValue> {
    respond_limited(request, MAX_RESPONSE_BYTES)
}

/// [`respond`] with an explicit response limit (tests of the `response_too_large` case).
fn respond_limited(request: Value, response_limit: usize) -> Box<RawValue> {
    let len = encoded_len(&request);
    let response = if len > MAX_REQUEST_BYTES {
        CoreResponse::Error(too_large(len))
    } else {
        match serde_json::from_value::<CoreRequest>(request) {
            Ok(request) => handle(request),
            Err(e) => CoreResponse::Error(bad_request(&e.to_string())),
        }
    };
    RawValue::from_string(response.to_json_limited(response_limit))
        .unwrap_or_else(|_| RawValue::from_string("null".into()).expect("`null` is valid JSON"))
}

/// Runs a core request (see the module docs for the frontend contract).
#[tauri::command]
pub async fn core_request(request: Value) -> Result<Box<RawValue>, String> {
    tauri::async_runtime::spawn_blocking(move || respond(request))
        .await
        .map_err(|e| format!("core_request task failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use promptcad_core::api::{handle_json, handle_json_limited};
    use serde_json::json;

    fn parse(raw: &RawValue) -> Value {
        serde_json::from_str(raw.get()).unwrap()
    }

    #[test]
    fn version_round_trip() {
        let out = parse(&respond(json!({"op": "version"})));
        assert_eq!(out["status"], "ok");
        assert_eq!(out["result"]["op"], "version");
    }

    #[test]
    fn validates_the_plate_example() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../examples/cad2/plate-four-holes.cad2.json"
        );
        let mut document: Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        document
            .as_object_mut()
            .unwrap()
            .remove("expected_resolved");
        let out = parse(&respond(json!({"op": "validate", "document": document})));
        assert_eq!(out["status"], "ok", "{out}");
        assert_eq!(out["result"]["report"]["valid"], true, "{out}");
    }

    #[test]
    fn malformed_requests_are_bad_request_responses() {
        for request in [
            json!({"op": "nope"}),
            json!({"op": "validate"}),
            json!({"op": "version", "extra": 1}),
            json!([1, 2]),
            json!("version"),
            Value::Null,
        ] {
            let out = parse(&respond(request.clone()));
            assert_eq!(out["status"], "error", "{request}");
            assert_eq!(out["code"], "bad_request", "{request}");
        }
    }

    #[test]
    fn oversize_request_is_too_large() {
        let blob = "x".repeat(MAX_REQUEST_BYTES + 1);
        let out = parse(&respond(
            json!({"op": "validate", "document": {"blob": blob}}),
        ));
        assert_eq!(out["status"], "error");
        assert_eq!(out["code"], "too_large");
    }

    #[test]
    fn bytes_match_the_server_host() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../examples/cad2/plate-four-holes.cad2.json"
        );
        let mut document: Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        document
            .as_object_mut()
            .unwrap()
            .remove("expected_resolved");
        for request in [
            json!({"op": "version"}),
            json!({"op": "validate", "document": document}),
            json!({"op": "solve", "document": document}),
            json!({"op": "normalize", "document": document}),
            json!({"op": "describe", "document": document}),
            json!({"op": "validate", "document": {"schema_version": "2.0"}}),
        ] {
            let text = serde_json::to_string(&request).unwrap();
            let value: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(
                respond(value).get(),
                handle_json(&text),
                "{}",
                request["op"]
            );
        }
    }

    #[test]
    fn response_cap_matches_the_server_host() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../examples/cad2/plate-four-holes.cad2.json"
        );
        let mut document: Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        document
            .as_object_mut()
            .unwrap()
            .remove("expected_resolved");
        let request = json!({"op": "validate", "document": document});
        let text = serde_json::to_string(&request).unwrap();
        let full = handle_json(&text);
        for limit in [full.len(), full.len() - 1, 64] {
            let ours = respond_limited(request.clone(), limit);
            assert_eq!(
                ours.get(),
                handle_json_limited(&text, limit),
                "limit {limit}"
            );
            let out = parse(&ours);
            if limit == full.len() {
                assert_eq!(out["status"], "ok");
            } else {
                assert_eq!(out["status"], "error");
                assert_eq!(out["code"], "response_too_large");
            }
        }
        // the real command uses the real cap
        assert_eq!(respond(request).get(), full);
    }

    #[test]
    fn describe_of_a_fan_out_chain_stays_small() {
        // The reviewer's blow-up input (was 1.8 GB / 7.3 GB peak at 5 levels).
        let assemblies: Vec<Value> = (0..5)
            .map(|i| {
                let child = if i < 4 {
                    format!("a{}", i + 1)
                } else {
                    "p_x".into()
                };
                json!({"id": format!("a{i}"), "name": "n", "children": vec![child; 32]})
            })
            .collect();
        let document = json!({
            "schema_version": "2.0", "source_prompt": "x", "revision": 0,
            "components": [{"id": "v_x", "kind": "volume",
                            "shape": {"kind": "plate", "length": 1, "width": 1, "thickness": 1}}],
            "parts": [{"id": "p_x", "name": "p", "quantity": 1, "source": "made",
                       "body": [{"op": "add", "component": "v_x"}]}],
            "assemblies": assemblies,
            "planner": {"planner": "agent_team", "confidence": 0.9, "review_required": true},
        });
        let request = json!({"op": "describe", "document": document});
        let out = respond(request.clone());
        assert!(out.get().len() < 64 * 1024, "{} bytes", out.get().len());
        assert_eq!(
            out.get(),
            handle_json(&serde_json::to_string(&request).unwrap())
        );
        assert_eq!(
            parse(&out)["result"]["tree"][0]["children"][1]["repeated"],
            true
        );
    }

    #[test]
    fn duplicate_keys_are_last_wins_here_but_bad_request_on_the_server() {
        // Documented divergence (module docs): not reachable from JavaScript.
        let text = r#"{"op":"version","op":"validate","document":{"schema_version":"2.0"}}"#;
        let server: Value = serde_json::from_str(&handle_json(text)).unwrap();
        assert_eq!(server["code"], "bad_request");
        let value: Value = serde_json::from_str(text).unwrap();
        // processed as `validate` (the last `op`): the stub document is the problem, not the
        // request
        let ours = parse(&respond(value));
        assert_eq!(ours["status"], "error");
        assert_ne!(ours["code"], "bad_request", "{ours}");
        assert_eq!(ours["path"], "/document", "{ours}");
    }
}
