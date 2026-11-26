//! `promptcad-core` command-line host.
//!
//! std-only on purpose: each subcommand is one function, dispatched by a `match` on the first
//! argument. Add new subcommands there.

use std::fs;
use std::io::{self, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use promptcad_core::bindings::recompute_bindings;
use promptcad_core::canonical::{
    strip_example_keys, to_canonical_json, to_canonical_string, to_canonical_value,
};
use promptcad_core::migrate::{downgrade_2_to_1, upgrade_1_to_2};
use promptcad_core::model::{
    CadDocument1, CadDocument2, Patch, Resolved, Severity, ValidationReport,
};
use promptcad_core::patch_engine::{apply_patch, PatchOrigin, PatchOutcome};
use promptcad_core::solver::solve;
use promptcad_core::validate::{validate, ValidateOptions};
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
    let doc = match load_any(&a.positional[0], a.has("--strip-example-keys")) {
        Ok(doc) => doc,
        Err(msg) => return input_error(&msg),
    };
    let opts = ValidateOptions {
        completed: !a.has("--draft"),
        submitter: None,
    };
    let report: ValidationReport = validate(&doc, &opts).report;
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
    let doc = match load_any(&a.positional[0], a.has("--strip-example-keys")) {
        Ok(doc) => doc,
        Err(msg) => return input_error(&msg),
    };
    let resolved: Resolved = solve(&doc);
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
    let text = match read_input(path) {
        Ok(text) => text,
        Err(msg) => return input_error(&msg),
    };
    // `CadDocument1` only accepts `schema_version` "1.0", so a 2.0 file fails here.
    let doc = match CadDocument1::from_json_str(&text) {
        Ok(doc) => doc,
        Err(err) => return input_error(&format!("{path}: {err}")),
    };
    let upgraded = upgrade_1_to_2(&doc);
    finish(emit(&a, &to_canonical_json(&upgraded)))
}

/// `downgrade <spec-2.0.json> [--strip-example-keys] [--out <file>]`
///
/// Exit 1 with a one-line message when the document cannot be expressed as 1.0.
fn cmd_downgrade(args: &[String]) -> ExitCode {
    let a = match parse_args("downgrade", args, &["--strip-example-keys"], 1) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let doc = match load_v2(&a.positional[0], a.has("--strip-example-keys")) {
        Ok(doc) => doc,
        Err(msg) => return input_error(&msg),
    };
    match downgrade_2_to_1(&doc) {
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
    let mut doc = match load_v2(&a.positional[0], a.has("--strip-example-keys")) {
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
    let doc = match load_v2(&a.positional[0], false) {
        Ok(doc) => doc,
        Err(msg) => return input_error(&msg),
    };
    let patch_path = &a.positional[1];
    let patch_text = match read_input(patch_path) {
        Ok(text) => text,
        Err(msg) => return input_error(&msg),
    };
    let patch = match Patch::from_json_str(&patch_text) {
        Ok(patch) => patch,
        Err(err) => return input_error(&format!("{patch_path}: {err}")),
    };
    let origin = if a.has("--human") {
        PatchOrigin::Human
    } else {
        PatchOrigin::Agent(patch.agent)
    };

    match apply_patch(&doc, &patch, origin) {
        PatchOutcome::Accepted(accepted) => {
            finish(emit(&a, &to_canonical_json(&accepted.document)))
        }
        PatchOutcome::Rejected(rejected) => {
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
fn read_value(path: &str, strip: bool) -> Result<(String, Value), String> {
    let text = read_input(path)?;
    let mut value: Value =
        serde_json::from_str(&text).map_err(|err| format!("{path}: invalid JSON: {err}"))?;
    if strip {
        strip_example_keys(&mut value);
    }
    Ok((text, value))
}

/// Parses a CadDocument 2.0 document.
fn load_v2(path: &str, strip: bool) -> Result<CadDocument2, String> {
    let (_, value) = read_value(path, strip)?;
    let version = value.get("schema_version").and_then(Value::as_str);
    if version != Some("2.0") {
        return Err(format!(
            "{path}: expected schema_version \"2.0\", found {}",
            version.map_or_else(|| "none".to_owned(), |v| format!("\"{v}\""))
        ));
    }
    CadDocument2::from_value(value).map_err(|err| format!("{path}: {err}"))
}

/// Parses a document of either version into the 2.0 model: 1.0 documents are upgraded.
fn load_any(path: &str, strip: bool) -> Result<CadDocument2, String> {
    let (text, value) = read_value(path, strip)?;
    match value.get("schema_version").and_then(Value::as_str) {
        Some("1.0") => {
            let doc1 =
                CadDocument1::from_json_str(&text).map_err(|err| format!("{path}: {err}"))?;
            Ok(upgrade_1_to_2(&doc1))
        }
        Some("2.0") => CadDocument2::from_value(value).map_err(|err| format!("{path}: {err}")),
        other => Err(format!(
            "{path}: unsupported schema_version {}",
            other.map_or_else(|| "(none)".to_owned(), |v| format!("\"{v}\""))
        )),
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
    ]
}

fn render<T: schemars::JsonSchema>() -> String {
    let value: Value = serde_json::to_value(schemars::schema_for!(T)).expect("schema is JSON");
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
