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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter as _, Manager};

use crate::commands::AppState;
use crate::core::engine::{Emitter, Engine};
use crate::core::task::{Task, TaskProgress};

/// Bring the main window back to the foreground — shared by the tray, the tray
/// menu, and the single-instance relaunch handler.
fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Install the system tray: an icon that restores the window on click, with a
/// menu to show or quit. The tray is always present, so a user who sets the close
/// button to minimize has a way back to the window (and out of the app).
fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show moin", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit moin", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;
    let Some(icon) = app.default_window_icon().cloned() else {
        tracing::warn!("no window icon available; skipping the tray");
        return Ok(());
    };
    TrayIconBuilder::with_id("main")
        .icon(icon)
        .tooltip("moin")
        .menu(&menu)
        // Left-clicking the tray shows the window; right-click opens the menu.
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main_window(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

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
    fn completed(&self, task: &Task) {
        // The engine only calls this when the user has notifications on, so just
        // show it. Best-effort — a denied permission or headless run is a no-op.
        use tauri_plugin_notification::NotificationExt;
        let _ = self
            .app
            .notification()
            .builder()
            .title("Download finished")
            .body(&task.filename)
            .show();
    }
}

/// Drain the buffered progress ticks on a timer and emit them as one event. Runs
/// on Tauri's own runtime so it's always active (never gated on capturing the
/// engine's runtime handle), which also means the buffer can't grow unbounded.
///
/// The cadence is re-read from settings before every sleep rather than fixed at a
/// `tokio::time::interval`, so changing it in the UI applies from the next tick
/// with no restart. Sleeping (rather than ticking) also means a slower cadence
/// never accumulates a backlog of missed ticks to fire back-to-back.
fn spawn_progress_flusher(
    app: AppHandle,
    engine: Engine,
    buf: Arc<Mutex<HashMap<String, TaskProgress>>>,
    last_sent: Arc<Mutex<HashMap<String, TaskProgress>>>,
) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(engine.settings().progress_flush_interval()).await;
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
            show_main_window(app);
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
            // Takes the engine so it can read the (user-adjustable) cadence live.
            spawn_progress_flusher(
                app.handle().clone(),
                engine.clone(),
                progress_buf,
                last_sent,
            );

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

            // System tray, so "minimize to tray" on close has somewhere to go.
            if let Err(e) = setup_tray(&app.handle().clone()) {
                tracing::warn!("couldn't create the tray icon: {e}");
            }
            Ok(())
        })
        // Close button: when the setting says so, hide to the tray (downloads and
        // seeding keep running) instead of quitting. Read live so the choice applies
        // without a restart.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() != "main" {
                    return;
                }
                let Some(state) = window.app_handle().try_state::<AppState>() else {
                    return;
                };
                if state.engine.settings().close_to_tray {
                    // Minimize to tray instead of quitting.
                    api.prevent_close();
                    let _ = window.hide();
                } else if state.engine.has_active_transfers() {
                    // Quitting would stop active downloads/seeding — let the UI
                    // confirm (minimize / quit anyway / cancel) instead of quitting.
                    api.prevent_close();
                    let _ = window.emit(events::CONFIRM_QUIT, ());
                }
                // Otherwise nothing is running: let the close through and quit.
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::hide_window,
            commands::quit_app,
            commands::check_update,
            commands::add_download,
            commands::prepare_torrent,
            commands::add_torrent,
            commands::merge_torrent_trackers,
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
            // stop the aria2 daemon) before the process goes away. Guarded so it runs
            // exactly once — a window-close quit fires `ExitRequested`, while the
            // tray's `app.exit(0)` fires `Exit`; we cover both.
            let winding_down = matches!(
                event,
                tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit
            );
            if winding_down && !SHUTDOWN_DONE.swap(true, Ordering::SeqCst) {
                if let Some(state) = app_handle.try_state::<AppState>() {
                    tauri::async_runtime::block_on(state.engine.shutdown());
                }
            }
        });
}

/// Guards the one-time engine shutdown so it runs once across the exit events.
static SHUTDOWN_DONE: AtomicBool = AtomicBool::new(false);

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
