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

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::{AppHandle, Emitter as _, Manager};

use crate::commands::AppState;
use crate::core::engine::{Emitter, Engine};
use crate::core::task::{Task, TaskProgress};

/// How often the coalesced progress buffer is flushed to the UI as one event.
const PROGRESS_FLUSH_MS: u64 = 250;

/// Bridges the engine's [`Emitter`] onto Tauri's event system.
struct AppEmitter {
    app: AppHandle,
    /// Progress ticks buffered by task id (latest wins) between flushes. Emitted
    /// as a single batched event on a timer so hundreds of per-torrent ticks a
    /// second collapse into a few IPC messages — flat cost at any torrent count.
    progress: Arc<Mutex<HashMap<String, TaskProgress>>>,
    /// The last reading actually sent to the UI, per task. A poll that reports the
    /// same numbers again is dropped here instead of buffered — with many idle or
    /// seeding torrents re-reporting an unchanged tick several times a second, the
    /// UI would otherwise re-render (and spread its whole task map) for no visible
    /// change, which is the main driver of the renderer's memory growth over time.
    last_sent: Arc<Mutex<HashMap<String, TaskProgress>>>,
}

impl Emitter for AppEmitter {
    fn added(&self, task: &Task) {
        let _ = self.app.emit(events::TASK_ADDED, task);
    }
    fn progress(&self, p: &TaskProgress) {
        // Skip a reading identical to the one the UI already holds.
        if let Ok(last) = self.last_sent.lock() {
            if last.get(&p.id) == Some(p) {
                return;
            }
        }
        if let Ok(mut buf) = self.progress.lock() {
            buf.insert(p.id.clone(), p.clone());
        }
    }
    fn updated(&self, task: &Task) {
        let _ = self.app.emit(events::TASK_UPDATED, task);
    }
    fn removed(&self, id: &str) {
        // Drop the dedup record so the map doesn't retain gone tasks, and so a
        // re-added id starts fresh.
        if let Ok(mut last) = self.last_sent.lock() {
            last.remove(id);
        }
        let _ = self.app.emit(events::TASK_REMOVED, id);
    }
}

/// Drain the buffered progress ticks on a timer and emit them as one event. Runs
/// on Tauri's own runtime so it's always active (never gated on capturing the
/// engine's runtime handle), which also means the buffer can't grow unbounded.
fn spawn_progress_flusher(
    app: AppHandle,
    buf: Arc<Mutex<HashMap<String, TaskProgress>>>,
    last_sent: Arc<Mutex<HashMap<String, TaskProgress>>>,
) {
    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(PROGRESS_FLUSH_MS));
        loop {
            ticker.tick().await;
            let batch: Vec<TaskProgress> = {
                let mut b = buf.lock().unwrap();
                if b.is_empty() {
                    continue;
                }
                b.drain().map(|(_, v)| v).collect()
            };
            // Record what we're sending so a later identical reading is dropped.
            if let Ok(mut last) = last_sent.lock() {
                for p in &batch {
                    last.insert(p.id.clone(), p.clone());
                }
            }
            let _ = app.emit(events::TASK_PROGRESS_BATCH, &batch);
        }
    });
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

            let progress_buf = Arc::new(Mutex::new(HashMap::new()));
            let last_sent = Arc::new(Mutex::new(HashMap::new()));
            let emitter = Arc::new(AppEmitter {
                app: app.handle().clone(),
                progress: progress_buf.clone(),
                last_sent: last_sent.clone(),
            });
            let engine = Engine::new(data_dir, emitter)
                .map_err(|e| format!("failed to start the download engine: {e}"))?;

            // Coalesce progress ticks into one periodic event so IPC stays flat as
            // the number of active torrents grows (and the buffer never piles up).
            spawn_progress_flusher(app.handle().clone(), progress_buf, last_sent);

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
                rpc::spawn(engine.clone(), fallback_dir.clone(), rt.clone());
                // Poll each category's watched folders for dropped .torrent files
                // and auto-add them. No-op until a category configures a folder.
                core::watch::spawn(engine.clone(), fallback_dir, rt.clone());
                // Pick back up any torrent that was downloading or seeding at last
                // exit (loaded as Queued). Runs on the tokio runtime so the queue
                // can spawn the transfers.
                let resume = engine.clone();
                rt.spawn(async move { resume.resume_pending() });
            } else {
                tracing::warn!(
                    "couldn't capture the tokio runtime handle; browser integration off"
                );
                // Still resume, just off Tauri's own runtime.
                engine.resume_pending();
            }

            app.manage(AppState { engine });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::check_update,
            commands::add_download,
            commands::prepare_torrent,
            commands::add_torrent,
            commands::torrent_details,
            commands::set_torrent_files,
            commands::list_downloads,
            commands::pause_download,
            commands::resume_download,
            commands::start_seeding,
            commands::force_start,
            commands::force_recheck,
            commands::cancel_download,
            commands::remove_download,
            commands::delete_download,
            commands::remove_downloads,
            commands::delete_downloads,
            commands::retry_download,
            commands::forget_download,
            commands::get_settings,
            commands::set_settings,
            commands::list_backends,
            commands::regenerate_rpc_token,
            commands::default_download_dir,
            commands::category_folder,
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
        .build(tauri::generate_context!())
        .expect("error while running moin")
        .run(|app_handle, event| {
            // On exit, wind the engines down cleanly (flush torrent resume state,
            // stop the aria2 daemon) before the process goes away.
            if let tauri::RunEvent::ExitRequested { .. } = event {
                if let Some(state) = app_handle.try_state::<AppState>() {
                    tauri::async_runtime::block_on(state.engine.shutdown());
                }
            }
        });
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
