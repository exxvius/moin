//! moin — Tauri application entry point.
//!
//! The headless download engine lives in [`core`]; this module wires it to the
//! Tauri shell (state, plugins, command handlers) and forwards engine events to
//! the UI. `run()` is invoked by both the desktop binary (`main.rs`) and any
//! mobile entry points Tauri generates.

pub mod commands;
pub mod core;
pub mod events;

use std::sync::Arc;

use tauri::{AppHandle, Emitter as _, Manager};

use crate::commands::AppState;
use crate::core::engine::{Emitter, Engine};
use crate::core::task::{Task, TaskProgress};

/// Bridges the engine's [`Emitter`] onto Tauri's event system.
struct AppEmitter {
    app: AppHandle,
}

impl Emitter for AppEmitter {
    fn added(&self, task: &Task) {
        let _ = self.app.emit(events::TASK_ADDED, task);
    }
    fn progress(&self, p: &TaskProgress) {
        let _ = self.app.emit(events::TASK_PROGRESS, p);
    }
    fn updated(&self, task: &Task) {
        let _ = self.app.emit(events::TASK_UPDATED, task);
    }
    fn removed(&self, id: &str) {
        let _ = self.app.emit(events::TASK_REMOVED, id);
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // Per-user data dir holds the downloads DB, settings, and logs.
            let data_dir = app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| std::env::temp_dir().join("moin"));
            std::fs::create_dir_all(&data_dir).ok();

            init_logging(&data_dir);
            tracing::info!("moin starting, data dir: {}", data_dir.display());

            let emitter = Arc::new(AppEmitter { app: app.handle().clone() });
            let engine = Engine::new(data_dir, emitter)
                .map_err(|e| format!("failed to start the download engine: {e}"))?;
            app.manage(AppState { engine });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::add_download,
            commands::list_downloads,
            commands::pause_download,
            commands::resume_download,
            commands::cancel_download,
            commands::remove_download,
            commands::get_settings,
            commands::set_settings,
            commands::list_backends,
            commands::default_download_dir,
        ])
        .run(tauri::generate_context!())
        .expect("error while running moin");
}

fn init_logging(data_dir: &std::path::Path) {
    use tracing_subscriber::{fmt, EnvFilter};
    let file_appender = tracing_appender::rolling::never(data_dir, "moin.log");
    let filter = std::env::var("MOIN_LOG")
        .ok()
        .and_then(|s| EnvFilter::try_new(s).ok())
        .unwrap_or_else(|| EnvFilter::new("info"));
    let _ = fmt()
        .with_env_filter(filter)
        .with_writer(file_appender)
        .with_ansi(false)
        .try_init();
}
