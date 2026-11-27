//! `promptcad-core` command-line host.
//!
//! std-only on purpose: each subcommand is one function, dispatched by a `match` on the first
//! argument. Add new subcommands there.

use std::fs;
use std::io::{self, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use promptcad_core::api::{
    self, CoreError, CoreRequest, CoreResponse, CoreResult, PatchOutcomeView, MAX_REQUEST_BYTES,
};
use promptcad_core::bindings::recompute_bindings;
use promptcad_core::canonical::{
    strip_example_keys, to_canonical_json, to_canonical_string, to_canonical_value,
};
use promptcad_core::model::{
    CadDocument1, CadDocument2, Patch, Resolved, Severity, ValidationReport,
};
use promptcad_core::patch_engine::PatchOrigin;
use serde_json::{json, Value};

const USAGE: &str = "\
usage: promptcad-core <command> [options]

commands:
  schema --out-dir <dir> [--check]
                    write the JSON Schemas (ADR 0008) into <dir>; with --check,
                    write nothing and exit 1 if any file there differs or is missing
  validate <spec.json> [--draft] [--strip-example-keys] [--out <file>]
                    print the ValidationReport of a 1.0 or 2.0 document (exit 1 if invalid)
  solve <spec.json> [--strip-example-keys] [--out <file>]
                    print resolved.json of a 1.0 or 2.0 document (exit 1 on solver errors)
  upgrade <spec-1.0.json> [--out <file>]
                    print the canonical CadDocument 2.0 JSON of a 1.0 document
  downgrade <spec-2.0.json> [--strip-example-keys] [--out <file>]
                    print the CadDocument 1.0 JSON (exit 1 if not expressible in 1.0)
  normalize <spec-2.0.json> [--strip-example-keys] [--out <file>]
                    recompute bindings and print the canonical 2.0 JSON (exit 1 on binding errors)
  apply-patch <spec-2.0.json> <patch.json> [--human] [--out <file>]
                    apply a patch and print the new canonical document (exit 1 and the reject
                    reason as JSON if the patch is rejected)
  rpc               read ONE JSON request (promptcad_core::api) from stdin and write ONE
                    single-line JSON response to stdout; exit 0 whenever a response was
                    written (also for \"status\": \"error\"), 1 if stdout cannot be
                    written, 2 on usage errors or unreadable stdin
  help              print this message

options:
  -h, --help        print this message
  -V, --version     print the version

A path of `-` reads from stdin. Exit codes: 0 success, 1 a domain failure (invalid,
unsolvable, rejected or not expressible), 2 usage or unreadable / unparsable input.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("-h" | "--help" | "help") => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some("-V" | "--version") => {
            println!("promptcad-core {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("schema") => cmd_schema(&args[1..]),
        Some("validate") => cmd_validate(&args[1..]),
        Some("solve") => cmd_solve(&args[1..]),
        Some("upgrade") => cmd_upgrade(&args[1..]),
        Some("downgrade") => cmd_downgrade(&args[1..]),
        Some("normalize") => cmd_normalize(&args[1..]),
        Some("apply-patch") => cmd_apply_patch(&args[1..]),
        Some("rpc") => cmd_rpc(&args[1..]),
        Some(other) => {
            eprintln!("promptcad-core: unknown command `{other}`\n");
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

/// `schema --out-dir <dir> [--check]`
fn cmd_schema(args: &[String]) -> ExitCode {
    let mut out_dir: Option<PathBuf> = None;
    let mut check = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            "--out-dir" => match iter.next() {
                Some(dir) => out_dir = Some(PathBuf::from(dir)),
                None => return usage_error("`--out-dir` needs a directory"),
            },
            "--check" => check = true,
            other => return usage_error(&format!("unexpected argument `{other}`")),
        }
    }
    let Some(out_dir) = out_dir else {
        return usage_error("`schema` requires `--out-dir <dir>`");
    };

    let files = schema_files();
    if check {
        check_schemas(&out_dir, &files)
    } else {
        write_schemas(&out_dir, &files)
    }
}

/// `validate <spec.json> [--draft] [--strip-example-keys] [--out <file>]`
///
/// Exit 0 when the report is valid, 1 when it has errors, 2 on bad input.
fn cmd_validate(args: &[String]) -> ExitCode {
    let a = match parse_args("validate", args, &["--draft", "--strip-example-keys"], 1) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let path = &a.positional[0];
    let document = match read_value(path, a.has("--strip-example-keys")) {
        Ok(value) => value,
        Err(msg) => return input_error(&msg),
    };
    let request = CoreRequest::Validate {
        document,
        completed: !a.has("--draft"),
    };
    let report: ValidationReport = match call(path, request) {
        Ok(CoreResult::Validate(r)) => r.report,
        Ok(_) => return unexpected_result(),
        Err(code) => return code,
    };
    if let Err(code) = emit(&a, &to_canonical_string(&report)) {
        return code;
    }
    if report.valid {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// `solve <spec.json> [--strip-example-keys] [--out <file>]`
///
/// Exit 1 when the resolved result contains error-severity issues (the result is still printed).
fn cmd_solve(args: &[String]) -> ExitCode {
    let a = match parse_args("solve", args, &["--strip-example-keys"], 1) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let path = &a.positional[0];
    let document = match read_value(path, a.has("--strip-example-keys")) {
        Ok(value) => value,
        Err(msg) => return input_error(&msg),
    };
    let resolved: Resolved = match call(path, CoreRequest::Solve { document }) {
        Ok(CoreResult::Solve(r)) => r.resolved,
        Ok(_) => return unexpected_result(),
        Err(code) => return code,
    };
    let has_error = resolved
        .issues
        .iter()
        .any(|issue| issue.severity == Severity::Error);
    if let Err(code) = emit(&a, &to_canonical_string(&resolved)) {
        return code;
    }
    if has_error {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

/// `upgrade <spec-1.0.json> [--out <file>]`
fn cmd_upgrade(args: &[String]) -> ExitCode {
    let a = match parse_args("upgrade", args, &[], 1) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let path = &a.positional[0];
    let document = match read_value(path, false) {
        Ok(value) => value,
        Err(msg) => return input_error(&msg),
    };
    // `upgrade` only accepts 1.0 (or no `schema_version`), so a 2.0 file fails here.
    match call(path, CoreRequest::Upgrade { document }) {
        Ok(CoreResult::Upgrade(r)) => finish(emit(&a, &to_canonical_json(&r.document))),
        Ok(_) => unexpected_result(),
        Err(code) => code,
    }
}

/// `downgrade <spec-2.0.json> [--strip-example-keys] [--out <file>]`
///
/// Exit 1 with a one-line message when the document cannot be expressed as 1.0.
fn cmd_downgrade(args: &[String]) -> ExitCode {
    let a = match parse_args("downgrade", args, &["--strip-example-keys"], 1) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let path = &a.positional[0];
    let document = match read_value(path, a.has("--strip-example-keys")) {
        Ok(value) => value,
        Err(msg) => return input_error(&msg),
    };
    let doc1: Option<CadDocument1> = match call(path, CoreRequest::Downgrade { document }) {
        Ok(CoreResult::Downgrade(r)) => r.document,
        Ok(_) => return unexpected_result(),
        Err(code) => return code,
    };
    match doc1 {
        Some(doc1) => finish(emit(&a, &to_canonical_string(&doc1))),
        None => {
            eprintln!("promptcad-core: document cannot be expressed as CadDocument 1.0");
            ExitCode::from(1)
        }
    }
}

/// `normalize <spec-2.0.json> [--strip-example-keys] [--out <file>]`
///
/// Exit 1 and the binding issues as JSON on stderr when recomputing the bindings reports errors.
fn cmd_normalize(args: &[String]) -> ExitCode {
    let a = match parse_args("normalize", args, &["--strip-example-keys"], 1) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let path = &a.positional[0];
    // Unlike the `normalize` API op (which also accepts 1.0 and recomputes `root`), this
    // subcommand only accepts 2.0 and only recomputes bindings (its output is unchanged).
    let mut doc: CadDocument2 = match read_value(path, a.has("--strip-example-keys"))
        .and_then(|value| api::parse_document_v2(value).map_err(|e| describe_error(path, &e)))
    {
        Ok(doc) => doc,
        Err(msg) => return input_error(&msg),
    };
    let issues = recompute_bindings(&mut doc);
    if issues.iter().any(|issue| issue.severity == Severity::Error) {
        eprintln!("{}", to_canonical_string(&issues));
        return ExitCode::from(1);
    }
    finish(emit(&a, &to_canonical_json(&doc)))
}

/// `apply-patch <spec-2.0.json> <patch.json> [--human] [--out <file>]`
///
/// The origin is the patch's `agent` unless `--human` is given. Exit 1 and a JSON reject
/// object when the patch is rejected.
fn cmd_apply_patch(args: &[String]) -> ExitCode {
    let a = match parse_args("apply-patch", args, &["--human"], 2) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let path = &a.positional[0];
    let document = match read_value(path, false) {
        Ok(value) => value,
        Err(msg) => return input_error(&msg),
    };
    let patch_path = &a.positional[1];
    let patch_text = match read_input(patch_path) {
        Ok(text) => text,
        Err(msg) => return input_error(&msg),
    };
    // Schema-checked here (exit 2 on violations, as before), whereas the API rejects such a
    // patch as `schema_invalid`.
    let patch = match Patch::from_json_str(&patch_text) {
        Ok(patch) => patch,
        Err(err) => return input_error(&format!("{patch_path}: {err}")),
    };
    let origin = if a.has("--human") {
        PatchOrigin::Human
    } else {
        PatchOrigin::Agent(patch.agent)
    };
    let request = CoreRequest::ApplyPatch {
        document,
        patch: to_canonical_value(&patch),
        origin,
        timestamp: None,
        log_seq: 0,
    };
    let outcome = match call(path, request) {
        Ok(CoreResult::ApplyPatch(r)) => r.outcome,
        Ok(_) => return unexpected_result(),
        Err(code) => return code,
    };

    match outcome {
        PatchOutcomeView::Accepted(accepted) => {
            finish(emit(&a, &to_canonical_json(&accepted.document)))
        }
        PatchOutcomeView::Rejected(rejected) => {
            let body = json!({
                "accepted": false,
                "reason": {
                    "code": rejected.reason.code.as_str(),
                    "op_index": rejected.reason.op_index,
                    "target": rejected.reason.target,
                    "message": rejected.reason.message,
                },
                "issues": to_canonical_value(&rejected.issues),
            });
            if let Err(code) = emit(&a, &to_canonical_string(&body)) {
                return code;
            }
            ExitCode::from(1)
        }
    }
}

/// Parsed arguments of a subcommand.
#[derive(Default)]
struct Args {
    positional: Vec<String>,
    flags: Vec<String>,
    out: Option<PathBuf>,
}

impl Args {
    fn has(&self, flag: &str) -> bool {
        self.flags.iter().any(|f| f == flag)
    }
}

/// Parses a subcommand's arguments: the listed boolean `flags`, the common `--out <file>` and
/// `-h`/`--help`, and exactly `positional` path arguments. `Err` carries the exit code to return.
fn parse_args(
    cmd: &str,
    args: &[String],
    flags: &[&str],
    positional: usize,
) -> Result<Args, ExitCode> {
    let mut parsed = Args::default();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return Err(ExitCode::SUCCESS);
            }
            "--out" => match iter.next() {
                Some(path) => parsed.out = Some(PathBuf::from(path)),
                None => return Err(usage_error("`--out` needs a file path")),
            },
            flag if flags.contains(&flag) => parsed.flags.push(flag.to_owned()),
            other if other.starts_with('-') && other != "-" => {
                return Err(usage_error(&format!("unexpected argument `{other}`")));
            }
            _ => parsed.positional.push(arg.clone()),
        }
    }
    if parsed.positional.len() != positional {
        return Err(usage_error(&format!(
            "`{cmd}` takes {positional} path argument(s), got {}",
            parsed.positional.len()
        )));
    }
    Ok(parsed)
}

/// Reads a file, or stdin for `-`.
fn read_input(path: &str) -> Result<String, String> {
    if path == "-" {
        let mut text = String::new();
        io::stdin()
            .read_to_string(&mut text)
            .map_err(|err| format!("cannot read stdin: {err}"))?;
        Ok(text)
    } else {
        fs::read_to_string(path).map_err(|err| format!("cannot read {path}: {err}"))
    }
}

/// Reads and parses a JSON document, removing example-only keys when `strip` is set.
fn read_value(path: &str, strip: bool) -> Result<Value, String> {
    let text = read_input(path)?;
    let mut value: Value =
        serde_json::from_str(&text).map_err(|err| format!("{path}: invalid JSON: {err}"))?;
    if strip {
        strip_example_keys(&mut value);
    }
    Ok(value)
}

/// Runs one API request; a request-level error becomes an input error (exit 2) whose message
/// names `path`.
fn call(path: &str, request: CoreRequest) -> Result<CoreResult, ExitCode> {
    match api::handle(request) {
        CoreResponse::Ok { result } => Ok(result),
        CoreResponse::Error(err) => Err(input_error(&describe_error(path, &err))),
    }
}

fn describe_error(path: &str, err: &CoreError) -> String {
    format!("{path}: {}", err.message)
}

fn unexpected_result() -> ExitCode {
    eprintln!("promptcad-core: internal error: unexpected result kind");
    ExitCode::from(1)
}

/// `rpc`: one JSON request on stdin, one JSON response line on stdout (`promptcad_core::api`).
///
/// Exit 0 whenever a response was written (including `"status": "error"` responses), 1 when
/// stdout cannot be written, 2 on usage errors or unreadable stdin. Input beyond
/// [`MAX_REQUEST_BYTES`] is drained and answered with `too_large`; non-UTF-8 input with
/// `bad_request`.
fn cmd_rpc(args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        None => {}
        Some("-h" | "--help") => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Some(other) => return usage_error(&format!("`rpc` takes no arguments, got `{other}`")),
    }
    let limit = MAX_REQUEST_BYTES as u64;
    let mut bytes = Vec::new();
    let mut stdin = io::stdin().lock();
    if let Err(err) = (&mut stdin).take(limit + 1).read_to_end(&mut bytes) {
        return input_error(&format!("cannot read stdin: {err}"));
    }
    let response = if bytes.len() as u64 > limit {
        // Drain the rest so the writer never sees a broken pipe, counting the total size.
        match io::copy(&mut stdin, &mut io::sink()) {
            Ok(rest) => {
                let total = bytes
                    .len()
                    .saturating_add(usize::try_from(rest).unwrap_or(usize::MAX));
                CoreResponse::Error(api::too_large(total)).to_json()
            }
            Err(err) => return input_error(&format!("cannot read stdin: {err}")),
        }
    } else {
        match String::from_utf8(bytes) {
            Ok(text) => api::handle_json(&text),
            Err(_) => CoreResponse::Error(api::bad_request("request is not valid UTF-8")).to_json(),
        }
    };
    let mut stdout = io::stdout().lock();
    match writeln!(stdout, "{response}").and_then(|()| stdout.flush()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("promptcad-core: cannot write stdout: {err}");
            ExitCode::from(1)
        }
    }
}

/// Writes `json` plus a trailing newline to `--out` or stdout.
fn emit(args: &Args, json: &str) -> Result<(), ExitCode> {
    let text = format!("{json}\n");
    let result = match &args.out {
        Some(path) => fs::write(path, text.as_bytes())
            .map_err(|err| format!("cannot write {}: {err}", path.display())),
        None => io::stdout()
            .write_all(text.as_bytes())
            .map_err(|err| format!("cannot write stdout: {err}")),
    };
    result.map_err(|msg| {
        eprintln!("promptcad-core: {msg}");
        ExitCode::from(1)
    })
}

/// Maps the result of [`emit`] to an exit code.
fn finish(result: Result<(), ExitCode>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => code,
    }
}

fn input_error(msg: &str) -> ExitCode {
    eprintln!("promptcad-core: {msg}");
    ExitCode::from(2)
}

fn usage_error(msg: &str) -> ExitCode {
    eprintln!("promptcad-core: {msg}\n");
    eprint!("{USAGE}");
    ExitCode::from(2)
}

/// Every generated file as `(file name, exact content)`. Content always ends in `\n`.
fn schema_files() -> Vec<(&'static str, String)> {
    vec![
        ("cad-document-2.schema.json", render::<CadDocument2>()),
        ("patch.schema.json", render::<Patch>()),
        ("cad-document-1.schema.json", render::<CadDocument1>()),
        ("resolved.schema.json", render::<Resolved>()),
        (
            "validation-report.schema.json",
            render::<ValidationReport>(),
        ),
        (
            "core-request.schema.json",
            render_schema(api::request_schema()),
        ),
        (
            "core-response.schema.json",
            render_schema(api::response_schema()),
        ),
    ]
}

fn render<T: schemars::JsonSchema>() -> String {
    render_schema(schemars::schema_for!(T))
}

fn render_schema(schema: schemars::Schema) -> String {
    let value: Value = serde_json::to_value(schema).expect("schema is JSON");
    // serde_json maps are sorted, so the output is deterministic.
    let mut text = serde_json::to_string_pretty(&value).expect("JSON value serializes");
    text.push('\n');
    text
}

fn write_schemas(out_dir: &Path, files: &[(&str, String)]) -> ExitCode {
    if let Err(err) = fs::create_dir_all(out_dir) {
        eprintln!("promptcad-core: cannot create {}: {err}", out_dir.display());
        return ExitCode::from(1);
    }
    for (name, content) in files {
        let path = out_dir.join(name);
        if let Err(err) = fs::write(&path, content.as_bytes()) {
            eprintln!("promptcad-core: cannot write {}: {err}", path.display());
            return ExitCode::from(1);
        }
        println!("Wrote {}", path.display());
    }
    ExitCode::SUCCESS
}

fn check_schemas(out_dir: &Path, files: &[(&str, String)]) -> ExitCode {
    let mut problems = Vec::new();
    for (name, expected) in files {
        let path = out_dir.join(name);
        match fs::read(&path) {
            Ok(actual) if actual == expected.as_bytes() => {}
            Ok(_) => problems.push(format!("differs: {}", path.display())),
            Err(err) if err.kind() == ErrorKind::NotFound => {
                problems.push(format!("missing: {}", path.display()));
            }
            Err(err) => problems.push(format!("unreadable: {} ({err})", path.display())),
        }
    }
    if problems.is_empty() {
        println!(
            "{} schema file(s) in {} are up to date.",
            files.len(),
            out_dir.display()
        );
        ExitCode::SUCCESS
    } else {
        for problem in &problems {
            eprintln!("{problem}");
        }
        eprintln!(
            "schemas are out of date; run `promptcad-core schema --out-dir {}`.",
            out_dir.display()
        );
        ExitCode::from(1)
    }
}
