//! moin — Tauri application entry point.
//!
//! The headless download engine lives in [`core`]; this module wires it to the
//! Tauri shell (state, plugins, command handlers) and forwards engine events to
//! the UI. `run()` is invoked by both the desktop binary (`main.rs`) and any
//! mobile entry points Tauri generates.

pub mod commands;
pub mod core;
pub mod events;
pub mod rpc;

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
        // Must be registered first: a second launch (e.g. the browser opening
        // `moin://launch` to wake moin) is funneled back to the running instance —
        // its window is focused and the new process exits, so the RPC port is never
        // double-bound.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // Register the `moin://` scheme at runtime so it works from a dev build
            // (the installer handles it for a packaged app). macOS registers via
            // its bundle Info.plist instead, so it's skipped there.
            #[cfg(any(windows, target_os = "linux"))]
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                if let Err(e) = app.deep_link().register_all() {
                    tracing::warn!("couldn't register the moin:// scheme: {e}");
                }
            }

            // Per-user data dir holds the downloads DB, settings, and logs.
            let data_dir = app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| std::env::temp_dir().join("moin"));
            std::fs::create_dir_all(&data_dir).ok();

            init_logging(&data_dir);
            tracing::info!("moin starting, data dir: {}", data_dir.display());

            let emitter = Arc::new(AppEmitter {
                app: app.handle().clone(),
            });
            let engine = Engine::new(data_dir, emitter)
                .map_err(|e| format!("failed to start the download engine: {e}"))?;

            // Start the loopback RPC server the browser extension talks to. It
            // needs the OS Downloads folder as the fallback destination (the same
            // one the command layer uses when no explicit dir is set), plus a
            // handle to Tauri's tokio runtime so engine-spawned transfers land on
            // the same executor as webview-added ones. Captured by bouncing through
            // a task on that runtime, which is the reliable way to get its handle.
            let fallback_dir = app
                .path()
                .download_dir()
                .unwrap_or_else(|_| std::env::temp_dir());
            let (tx, rx) = std::sync::mpsc::channel();
            tauri::async_runtime::spawn(async move {
                let _ = tx.send(tokio::runtime::Handle::current());
            });
            if let Ok(rt) = rx.recv() {
                rpc::spawn(engine.clone(), fallback_dir, rt);
            } else {
                tracing::warn!(
                    "couldn't capture the tokio runtime handle; browser integration off"
                );
            }

            app.manage(AppState { engine });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::add_download,
            commands::add_torrent,
            commands::list_downloads,
            commands::pause_download,
            commands::resume_download,
            commands::cancel_download,
            commands::remove_download,
            commands::delete_download,
            commands::retry_download,
            commands::forget_download,
            commands::get_settings,
            commands::set_settings,
            commands::list_backends,
            commands::regenerate_rpc_token,
            commands::default_download_dir,
            commands::tool_status,
            commands::download_tool,
            commands::set_tool_path,
            commands::list_categories,
            commands::suggest_category,
            commands::create_category,
            commands::update_category,
            commands::delete_category,
            commands::reorder_categories,
            commands::move_to_category,
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
