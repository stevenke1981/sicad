use std::path::{Path, PathBuf};

/// Must match `bundle.externalBin` in tauri.conf.json (without the target-triple suffix).
const SIDECAR_STEM: &str = "binaries/promptcad-sidecar";

fn main() {
    ensure_sidecar_binary();

    tauri_build::try_build(
        tauri_build::Attributes::new()
            // Explicit app-command ACL: generates the `allow-backend-info` permission that
            // capabilities/default.json grants. Commands not listed here are unreachable.
            .app_manifest(tauri_build::AppManifest::new().commands(&["backend_info"])),
    )
    .expect("failed to run tauri-build");
}

/// tauri-build copies every `externalBin` next to the executable and fails when the file
/// is missing. The real sidecar is produced by `python scripts/build_sidecar.py` (gitignored),
/// so a fresh checkout cannot `cargo check`/`cargo test` without it.
///
/// * debug profile: create an EMPTY placeholder and warn. At runtime the shell detects the
///   0-byte file and reports "sidecar not built" (or use PROMPTCAD_SIDECAR_CMD in dev).
/// * release profile: never create anything; refuse to build with a missing or placeholder
///   sidecar so an installer can never ship without the real backend.
fn ensure_sidecar_binary() {
    let target = std::env::var("TARGET").expect("cargo sets TARGET");
    let ext = if target.contains("windows") {
        ".exe"
    } else {
        ""
    };
    let path = PathBuf::from(format!("{SIDECAR_STEM}-{target}{ext}"));
    println!("cargo:rerun-if-changed={}", path.display());

    let release = std::env::var("PROFILE")
        .map(|p| p == "release")
        .unwrap_or(false);
    let size = std::fs::metadata(&path).map(|m| m.len()).ok();

    match (size, release) {
        (Some(len), _) if len > 0 => {}
        (_, true) => panic!(
            "sidecar binary {} is missing or an empty placeholder; build it first with \
             `.venv/Scripts/python.exe scripts/build_sidecar.py`",
            path.display()
        ),
        (Some(_), false) => println!(
            "cargo:warning=using EMPTY placeholder sidecar {} (debug only); run \
             scripts/build_sidecar.py or set PROMPTCAD_SIDECAR_CMD to run the backend",
            path.display()
        ),
        (None, false) => {
            create_placeholder(&path);
            println!(
                "cargo:warning=created EMPTY placeholder sidecar {} so debug builds work; run \
                 scripts/build_sidecar.py or set PROMPTCAD_SIDECAR_CMD to run the backend",
                path.display()
            );
        }
    }
}

fn create_placeholder(path: &Path) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create binaries/ directory");
    }
    std::fs::write(path, b"").expect("write placeholder sidecar");
}
