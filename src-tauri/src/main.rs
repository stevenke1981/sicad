// Release builds must not open a console window next to the app on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
mod job;
mod sidecar;

use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use tauri::{Manager, RunEvent};

use sidecar::{BackendInfo, Sidecar};

/// Slightly longer than the READY timeout so the startup thread always decides first.
const BACKEND_INFO_WAIT: Duration = Duration::from_secs(35);

enum Status {
    Starting,
    Ready(BackendInfo),
    Failed(String),
}

/// Shared between the startup thread, the `backend_info` command and the exit handler.
struct Backend {
    status: Mutex<Status>,
    changed: Condvar,
    /// `exiting` lives under the same lock as the process so start/shutdown cannot race.
    process: Mutex<ProcessSlot>,
}

#[derive(Default)]
struct ProcessSlot {
    exiting: bool,
    sidecar: Option<Sidecar>,
}

impl Backend {
    fn new() -> Self {
        Self {
            status: Mutex::new(Status::Starting),
            changed: Condvar::new(),
            process: Mutex::new(ProcessSlot::default()),
        }
    }

    fn set_status(&self, status: Status) {
        *self.status.lock().unwrap_or_else(|e| e.into_inner()) = status;
        self.changed.notify_all();
    }

    /// Blocks until the sidecar is ready or failed (or `timeout` elapses).
    fn wait(&self, timeout: Duration) -> Result<BackendInfo, String> {
        let deadline = Instant::now() + timeout;
        let mut status = self.status.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            match &*status {
                Status::Ready(info) => return Ok(info.clone()),
                Status::Failed(err) => return Err(err.clone()),
                Status::Starting => {}
            }
            let now = Instant::now();
            if now >= deadline {
                return Err("backend is still starting".into());
            }
            status = self
                .changed
                .wait_timeout(status, deadline - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    fn start(self: Arc<Self>, app_data_dir: PathBuf, resource_dir: Result<PathBuf, String>) {
        let result = resource_dir
            .map_err(|e| format!("cannot locate resource directory: {e}"))
            .and_then(|resource_dir| sidecar::start(&app_data_dir, &resource_dir));
        match result {
            Ok((process, info)) => {
                let mut slot = self.process.lock().unwrap_or_else(|e| e.into_inner());
                // The app may have started exiting while we waited for READY.
                if slot.exiting {
                    drop(slot);
                    process.shutdown();
                    self.set_status(Status::Failed("application is exiting".into()));
                    return;
                }
                slot.sidecar = Some(process);
                drop(slot);
                eprintln!("[promptcad] sidecar ready at {}", info.base_url);
                self.set_status(Status::Ready(info));
            }
            Err(err) => {
                eprintln!("[promptcad] sidecar failed to start: {err}");
                self.set_status(Status::Failed(err));
            }
        }
    }

    fn shutdown(&self) {
        let process = {
            let mut slot = self.process.lock().unwrap_or_else(|e| e.into_inner());
            slot.exiting = true;
            slot.sidecar.take()
        };
        if let Some(process) = process {
            process.shutdown();
        }
    }
}

/// Base URL and one-time token of the local sidecar, for the frontend API client.
#[tauri::command]
async fn backend_info(state: tauri::State<'_, Arc<Backend>>) -> Result<BackendInfo, String> {
    let backend = Arc::clone(state.inner());
    tauri::async_runtime::spawn_blocking(move || backend.wait(BACKEND_INFO_WAIT))
        .await
        .map_err(|e| format!("backend_info task failed: {e}"))?
}

fn main() {
    let backend = Arc::new(Backend::new());

    let app = tauri::Builder::default()
        .manage(Arc::clone(&backend))
        .invoke_handler(tauri::generate_handler![backend_info])
        .setup(|app| {
            let backend = Arc::clone(app.state::<Arc<Backend>>().inner());
            let app_data_dir = app.path().app_data_dir()?;
            // The onedir sidecar is a bundle resource (`<resource_dir>/sidecar/`). A failure
            // here is reported through `backend_info` instead of aborting setup, so the
            // window still opens and can show it.
            let resource_dir = app.path().resource_dir().map_err(|e| e.to_string());
            // Start in the background so the window appears immediately; the frontend's
            // `backend_info` call waits for READY.
            std::thread::Builder::new()
                .name("sidecar-start".into())
                .spawn(move || backend.start(app_data_dir, resource_dir))?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building PromptCAD Studio");

    app.run(move |_app, event| {
        if let RunEvent::Exit = event {
            backend.shutdown();
        }
    });
}
