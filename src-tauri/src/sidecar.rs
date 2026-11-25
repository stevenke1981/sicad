//! Python geometry sidecar lifecycle (ADR 0002 / 0006).
//!
//! The shell spawns the sidecar with `--host 127.0.0.1 --port 0`, hands it a one-time
//! token, data dir and CORS origins through `PROMPTCAD_*` environment variables (read by
//! `app/core/config.py`), and waits for exactly one stdout line
//! `PROMPTCAD_SIDECAR_READY {"port": N}`.
//!
//! The pure helpers at the top (`parse_ready_line`, `generate_token`, `split_command_line`)
//! carry the unit tests; the process code below them is exercised by running the app.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;

/// Prefix of the single machine-readable stdout line printed by `app/sidecar.py`.
pub const READY_PREFIX: &str = "PROMPTCAD_SIDECAR_READY";
/// How long the sidecar may take to print its READY line (PyInstaller onefile unpacks first).
pub const READY_TIMEOUT: Duration = Duration::from_secs(30);
/// Dev override: full command line used instead of the bundled sidecar binary, e.g.
/// `D:\repo\.venv\Scripts\python.exe -m app.sidecar`. The fixed args are appended.
pub const SIDECAR_CMD_ENV: &str = "PROMPTCAD_SIDECAR_CMD";
/// Optional working directory for the dev override (defaults to the repo root in debug builds).
pub const SIDECAR_CWD_ENV: &str = "PROMPTCAD_SIDECAR_CWD";

/// Environment variable names understood by `app.core.config.Settings` (prefix `PROMPTCAD_`).
pub const ENV_API_TOKEN: &str = "PROMPTCAD_API_TOKEN";
pub const ENV_DATA_DIR: &str = "PROMPTCAD_DATA_DIR";
/// Comma-separated list (`Settings.cors_origins` is a plain `str` split on ",").
pub const ENV_CORS_ORIGINS: &str = "PROMPTCAD_CORS_ORIGINS";

/// Fixed sidecar arguments: loopback only, OS-assigned port.
pub const SIDECAR_ARGS: [&str; 4] = ["--host", "127.0.0.1", "--port", "0"];

/// Name of the bundled sidecar next to the shell executable (tauri strips the target triple).
#[cfg(windows)]
const BUNDLED_SIDECAR: &str = "promptcad-sidecar.exe";
#[cfg(not(windows))]
const BUNDLED_SIDECAR: &str = "promptcad-sidecar";

/// Returned to the frontend by the `backend_info` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BackendInfo {
    pub base_url: String,
    pub token: String,
}

// ---------------------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------------------

/// Parses `PROMPTCAD_SIDECAR_READY {"port": N}` and returns `N` (1..=65535).
/// Anything else (log noise, malformed JSON, port 0, out of range) yields `None`.
pub fn parse_ready_line(line: &str) -> Option<u16> {
    let rest = line.trim().strip_prefix(READY_PREFIX)?;
    // Require a separator so e.g. `PROMPTCAD_SIDECAR_READYX{...}` is rejected.
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(rest.trim()).ok()?;
    let port = value.as_object()?.get("port")?.as_u64()?;
    match u16::try_from(port) {
        Ok(p) if p != 0 => Some(p),
        _ => None,
    }
}

/// 32 bytes from the OS CSPRNG, lowercase hex encoded (64 characters).
pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("OS random number generator unavailable");
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(char::from_digit(u32::from(b >> 4), 16).unwrap());
        out.push(char::from_digit(u32::from(b & 0x0f), 16).unwrap());
    }
    out
}

/// Splits a command line on whitespace; double quotes group (for paths with spaces) and
/// are removed. No escape processing, so Windows backslash paths pass through untouched.
pub fn split_command_line(input: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut has_token = false;
    for c in input.chars() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                has_token = true;
            }
            c if c.is_whitespace() && !in_quotes => {
                if has_token {
                    parts.push(std::mem::take(&mut current));
                    has_token = false;
                }
            }
            c => {
                current.push(c);
                has_token = true;
            }
        }
    }
    if has_token {
        parts.push(current);
    }
    parts
}

/// Origins the WebView uses to reach the sidecar. `http://tauri.localhost` is the Windows
/// (WebView2) origin, `tauri://localhost` macOS/Linux; debug builds add the Vite dev server.
pub fn cors_origins() -> String {
    let mut origins = vec![
        "http://tauri.localhost",
        "https://tauri.localhost",
        "tauri://localhost",
    ];
    if cfg!(debug_assertions) {
        origins.push("http://localhost:5173");
        origins.push("http://127.0.0.1:5173");
    }
    origins.join(",")
}

// ---------------------------------------------------------------------------------------
// Process management
// ---------------------------------------------------------------------------------------

/// A running sidecar. Dropping it does NOT stop the process; call [`Sidecar::shutdown`].
pub struct Sidecar {
    child: Child,
    /// Kill-on-close job: terminates the whole tree (PyInstaller onefile runs Python as a
    /// grandchild) on shutdown and also if the shell itself crashes.
    #[cfg(windows)]
    _job: Option<crate::job::KillOnCloseJob>,
}

impl Sidecar {
    pub fn shutdown(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        #[cfg(windows)]
        if let Some(job) = self._job.take() {
            job.terminate();
        }
    }
}

struct Launch {
    program: PathBuf,
    args: Vec<String>,
    cwd: PathBuf,
}

fn resolve_launch(app_data_dir: &Path) -> Result<Launch, String> {
    // The dev override is honoured only in debug builds: an installed release must
    // never hand the session token to an arbitrary program named by an env var.
    let override_cmd = if cfg!(debug_assertions) {
        std::env::var(SIDECAR_CMD_ENV).ok()
    } else {
        None
    };
    if let Some(raw) = override_cmd {
        let mut parts = split_command_line(&raw);
        if !parts.is_empty() {
            let program = PathBuf::from(parts.remove(0));
            let cwd = match std::env::var_os(SIDECAR_CWD_ENV) {
                Some(dir) => PathBuf::from(dir),
                // `python -m app.sidecar` must run from the repo root (src-tauri/..).
                None if cfg!(debug_assertions) => Path::new(env!("CARGO_MANIFEST_DIR")).join(".."),
                None => app_data_dir.to_path_buf(),
            };
            return Ok(Launch {
                program,
                args: parts,
                cwd,
            });
        }
    }

    let exe =
        std::env::current_exe().map_err(|e| format!("cannot locate shell executable: {e}"))?;
    let program = exe
        .parent()
        .ok_or("shell executable has no parent directory")?
        .join(BUNDLED_SIDECAR);
    match std::fs::metadata(&program) {
        Ok(meta) if meta.len() > 0 => {}
        Ok(_) => {
            return Err(format!(
                "sidecar {} is an empty debug placeholder; build it with \
                 `python scripts/build_sidecar.py` or set {SIDECAR_CMD_ENV}",
                program.display()
            ))
        }
        Err(e) => return Err(format!("sidecar {} not found: {e}", program.display())),
    }
    Ok(Launch {
        program,
        args: Vec::new(),
        cwd: app_data_dir.to_path_buf(),
    })
}

/// Keeps the last few stderr lines so a startup failure can say why.
#[derive(Clone, Default)]
struct StderrTail(Arc<Mutex<VecDeque<String>>>);

impl StderrTail {
    const MAX: usize = 20;
    fn push(&self, line: String) {
        let mut buf = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if buf.len() == Self::MAX {
            buf.pop_front();
        }
        buf.push_back(line);
    }
    fn render(&self) -> String {
        let buf = self.0.lock().unwrap_or_else(|e| e.into_inner());
        buf.iter().cloned().collect::<Vec<_>>().join("\n")
    }
}

/// Calls `f` for every line of `reader` (lossy UTF-8, trailing CR/LF removed) until EOF.
fn for_each_line(reader: impl Read, mut f: impl FnMut(String)) {
    let mut reader = BufReader::new(reader);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let line = String::from_utf8_lossy(&buf);
                f(line.trim_end_matches(['\r', '\n']).to_string());
            }
        }
    }
}

/// Spawns the sidecar and blocks until it reports READY (or fails / times out).
pub fn start(app_data_dir: &Path) -> Result<(Sidecar, BackendInfo), String> {
    let data_dir = app_data_dir.join("generated");
    std::fs::create_dir_all(&data_dir)
        .map_err(|e| format!("cannot create data dir {}: {e}", data_dir.display()))?;

    let launch = resolve_launch(app_data_dir)?;
    let token = generate_token();

    let mut cmd = Command::new(&launch.program);
    cmd.args(&launch.args)
        .args(SIDECAR_ARGS)
        .current_dir(&launch.cwd)
        .env(ENV_API_TOKEN, &token)
        .env(ENV_DATA_DIR, &data_dir)
        .env(ENV_CORS_ORIGINS, cors_origins())
        .env("PYTHONUNBUFFERED", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .env_remove(SIDECAR_CMD_ENV)
        .env_remove(SIDECAR_CWD_ENV)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_SUSPENDED: u32 = 0x0000_0004;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        // Start suspended so the process is inside the job before it can spawn children.
        cmd.creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to spawn sidecar {}: {e}", launch.program.display()))?;

    #[cfg(windows)]
    let job = {
        let job = match crate::job::KillOnCloseJob::new() {
            Ok(job) => match job.assign(&child) {
                Ok(()) => Some(job),
                Err(e) => {
                    eprintln!("[promptcad] warning: cannot assign sidecar to job object: {e}");
                    None
                }
            },
            Err(e) => {
                eprintln!("[promptcad] warning: cannot create job object: {e}");
                None
            }
        };
        if let Err(e) = crate::job::resume_process(child.id()) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("failed to resume sidecar process: {e}"));
        }
        job
    };

    let sidecar_stdout = child.stdout.take().expect("stdout is piped");
    let sidecar_stderr = child.stderr.take().expect("stderr is piped");

    let (tx, rx) = mpsc::channel::<u16>();
    std::thread::Builder::new()
        .name("sidecar-stdout".into())
        .spawn(move || {
            let mut tx = Some(tx);
            // Keep draining after READY so the pipe never fills and blocks the sidecar.
            for_each_line(sidecar_stdout, |line| {
                if tx.is_some() {
                    if let Some(port) = parse_ready_line(&line) {
                        let _ = tx.take().unwrap().send(port);
                        return;
                    }
                }
                eprintln!("[sidecar:stdout] {line}");
            });
        })
        .map_err(|e| format!("cannot start stdout reader: {e}"))?;

    let tail = StderrTail::default();
    let tail_writer = tail.clone();
    std::thread::Builder::new()
        .name("sidecar-stderr".into())
        .spawn(move || {
            for_each_line(sidecar_stderr, |line| {
                eprintln!("[sidecar] {line}");
                tail_writer.push(line);
            });
        })
        .map_err(|e| format!("cannot start stderr reader: {e}"))?;

    let sidecar = Sidecar {
        child,
        #[cfg(windows)]
        _job: job,
    };

    match rx.recv_timeout(READY_TIMEOUT) {
        Ok(port) => Ok((
            sidecar,
            BackendInfo {
                base_url: format!("http://127.0.0.1:{port}"),
                token,
            },
        )),
        Err(err) => {
            let reason = match err {
                mpsc::RecvTimeoutError::Timeout => format!(
                    "sidecar did not report ready within {}s",
                    READY_TIMEOUT.as_secs()
                ),
                mpsc::RecvTimeoutError::Disconnected => "sidecar exited before ready".to_string(),
            };
            let status = exit_status_after_kill(sidecar);
            // Give the stderr reader a moment to collect the final lines.
            std::thread::sleep(Duration::from_millis(100));
            let stderr = tail.render();
            let mut msg = reason;
            if let Some(status) = status {
                msg.push_str(&format!(" ({status})"));
            }
            if !stderr.is_empty() {
                msg.push_str(":\n");
                msg.push_str(&stderr);
            }
            Err(msg)
        }
    }
}

fn exit_status_after_kill(mut sidecar: Sidecar) -> Option<ExitStatus> {
    let early = sidecar.child.try_wait().ok().flatten();
    sidecar.shutdown();
    early
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ready_line_valid() {
        assert_eq!(
            parse_ready_line(r#"PROMPTCAD_SIDECAR_READY {"port": 51234}"#),
            Some(51234)
        );
        assert_eq!(
            parse_ready_line("PROMPTCAD_SIDECAR_READY {\"port\":8000}\r\n"),
            Some(8000)
        );
        assert_eq!(
            parse_ready_line(r#"  PROMPTCAD_SIDECAR_READY   {"port": 1}  "#),
            Some(1)
        );
        assert_eq!(
            parse_ready_line(r#"PROMPTCAD_SIDECAR_READY {"port": 65535}"#),
            Some(65535)
        );
        // Unknown extra keys are tolerated (forward compatible).
        assert_eq!(
            parse_ready_line(r#"PROMPTCAD_SIDECAR_READY {"port": 9000, "pid": 4}"#),
            Some(9000)
        );
    }

    #[test]
    fn ready_line_garbage() {
        for line in [
            "",
            "INFO:     Uvicorn running on http://127.0.0.1:8000",
            "PROMPTCAD_SIDECAR_READY",
            "PROMPTCAD_SIDECAR_READY ",
            "PROMPTCAD_SIDECAR_READY not-json",
            "PROMPTCAD_SIDECAR_READY {\"port\": }",
            r#"PROMPTCAD_SIDECAR_READY{"port": 8000}"#,
            r#"PROMPTCAD_SIDECAR_READYX {"port": 8000}"#,
            r#"prefix PROMPTCAD_SIDECAR_READY {"port": 8000}"#,
            r#"PROMPTCAD_SIDECAR_READY {"port": 0}"#,
            r#"PROMPTCAD_SIDECAR_READY {"port": 65536}"#,
            r#"PROMPTCAD_SIDECAR_READY {"port": -1}"#,
            r#"PROMPTCAD_SIDECAR_READY {"port": 80.5}"#,
            r#"PROMPTCAD_SIDECAR_READY {"port": "8000"}"#,
            r#"PROMPTCAD_SIDECAR_READY {"host": "127.0.0.1"}"#,
            r#"PROMPTCAD_SIDECAR_READY [8000]"#,
            r#"PROMPTCAD_SIDECAR_READY {"port": 8000} trailing"#,
        ] {
            assert_eq!(parse_ready_line(line), None, "should reject {line:?}");
        }
    }

    #[test]
    fn token_is_64_lowercase_hex_chars() {
        let token = generate_token();
        assert_eq!(token.len(), 64);
        assert!(
            token.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
            "{token}"
        );
    }

    #[test]
    fn tokens_are_unique() {
        let tokens: HashSet<String> = (0..256).map(|_| generate_token()).collect();
        assert_eq!(tokens.len(), 256);
    }

    #[test]
    fn split_command_line_handles_quotes() {
        assert_eq!(
            split_command_line(r#"D:\repo\.venv\Scripts\python.exe -m app.sidecar"#),
            vec![r"D:\repo\.venv\Scripts\python.exe", "-m", "app.sidecar"]
        );
        assert_eq!(
            split_command_line(r#"  "C:\Program Files\Py\python.exe"   -m  app.sidecar "#),
            vec![r"C:\Program Files\Py\python.exe", "-m", "app.sidecar"]
        );
        assert_eq!(split_command_line(r#"a "" b"#), vec!["a", "", "b"]);
        assert!(split_command_line("   ").is_empty());
    }

    #[test]
    fn cors_origins_cover_tauri_webviews() {
        let origins = cors_origins();
        let list: Vec<&str> = origins.split(',').collect();
        assert!(list.contains(&"http://tauri.localhost"));
        assert!(list.contains(&"tauri://localhost"));
        assert!(list.iter().all(|o| !o.is_empty() && o.trim() == *o));
    }
}
