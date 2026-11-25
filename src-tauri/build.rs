use std::path::Path;

/// PyInstaller ONEDIR output (ADR 0010 Phase 1b). Must match the source key of
/// `bundle.resources` in tauri.conf.json, which installs it as `<resource_dir>/sidecar/`.
const SIDECAR_DIR: &str = "binaries/promptcad-sidecar";
const SIDECAR_STEM: &str = "promptcad-sidecar";
/// PyInstaller >= 6 puts the interpreter, DLLs and packages here, next to the bootloader.
const SIDECAR_INTERNAL: &str = "_internal";

fn main() {
    ensure_sidecar_onedir();

    tauri_build::try_build(
        tauri_build::Attributes::new()
            // Explicit app-command ACL: generates the `allow-backend-info` permission that
            // capabilities/default.json grants. Commands not listed here are unreachable.
            .app_manifest(tauri_build::AppManifest::new().commands(&["backend_info"])),
    )
    .expect("failed to run tauri-build");
}

/// tauri-build copies every `bundle.resources` entry next to the executable and fails when
/// a source path is missing. The real onedir sidecar is produced by
/// `python scripts/build_sidecar.py` (gitignored), so a fresh checkout cannot
/// `cargo check`/`cargo test` without it.
///
/// * debug profile: if the onedir directory is missing or empty, create it with an EMPTY
///   placeholder exe and warn. At runtime the shell detects the 0-byte file and reports
///   "sidecar not built" (or use PROMPTCAD_SIDECAR_CMD in dev). An existing non-empty
///   directory is never touched (it may be a real build, or one in progress).
/// * release profile: never create anything; refuse to build unless the exe is non-empty
///   and `_internal/` exists, so an installer can never ship without the real backend.
fn ensure_sidecar_onedir() {
    // Build scripts run on the host, so derive the extension from the *target* triple.
    let target = std::env::var("TARGET").expect("cargo sets TARGET");
    let exe_name = if target.contains("windows") {
        format!("{SIDECAR_STEM}.exe")
    } else {
        SIDECAR_STEM.to_string()
    };
    let dir = Path::new(SIDECAR_DIR);
    let exe = dir.join(&exe_name);
    let internal = dir.join(SIDECAR_INTERNAL);
    // tauri-build already watches the whole resource directory; these make the intent
    // explicit and also cover the "directory missing" case.
    println!("cargo:rerun-if-changed={}", dir.display());
    println!("cargo:rerun-if-changed={}", exe.display());
    println!("cargo:rerun-if-changed={}", internal.display());

    let release = std::env::var("PROFILE")
        .map(|p| p == "release")
        .unwrap_or(false);
    let exe_len = std::fs::metadata(&exe)
        .ok()
        .filter(|m| m.is_file())
        .map(|m| m.len());

    if release {
        let exe_ok = matches!(exe_len, Some(len) if len > 0);
        if !exe_ok || !internal.is_dir() {
            panic!(
                "sidecar onedir {} is missing, an empty placeholder, or lacks {}/; build it \
                 first with `.venv-desktop/Scripts/python.exe scripts/build_sidecar.py`",
                exe.display(),
                SIDECAR_INTERNAL
            );
        }
        return;
    }

    match exe_len {
        Some(len) if len > 0 => {
            if !internal.is_dir() {
                println!(
                    "cargo:warning=sidecar {} has no {}/ next to it; the onedir build looks \
                     incomplete, re-run scripts/build_sidecar.py",
                    exe.display(),
                    SIDECAR_INTERNAL
                );
            }
        }
        Some(_) => println!(
            "cargo:warning=using EMPTY placeholder sidecar {} (debug only); run \
             scripts/build_sidecar.py or set PROMPTCAD_SIDECAR_CMD to run the backend",
            exe.display()
        ),
        None if dir_is_missing_or_empty(dir) => {
            std::fs::create_dir_all(dir).expect("create sidecar onedir placeholder directory");
            std::fs::write(&exe, b"").expect("write placeholder sidecar");
            println!(
                "cargo:warning=created EMPTY placeholder sidecar {} so debug builds work; run \
                 scripts/build_sidecar.py or set PROMPTCAD_SIDECAR_CMD to run the backend",
                exe.display()
            );
        }
        // Directory has content but no exe: probably a sidecar build in progress. Leave it
        // alone; tauri-build only needs the directory to exist.
        None => println!(
            "cargo:warning=sidecar directory {} has no {}; not creating a placeholder \
             (is scripts/build_sidecar.py running?)",
            dir.display(),
            exe_name
        ),
    }
}

fn dir_is_missing_or_empty(dir: &Path) -> bool {
    match std::fs::read_dir(dir) {
        Ok(mut entries) => entries.next().is_none(),
        Err(_) => true,
    }
}
