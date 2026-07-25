//! What's left of the Tauri command surface.
//!
//! Everything about downloads now goes straight from the frontend to the engine
//! daemon over HTTP — see `engine_link` and `src/lib/api.ts`. What stays here is
//! the handful of things only the desktop shell can do: report the endpoint, move
//! its own window, and hold the two facts the native close handler needs.

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, State};

use crate::engine_link::EngineLink;

/// Shared app state: how to reach the engine, plus the cached quit policy.
pub struct AppState {
    pub engine: EngineLink,
    pub quit: QuitPolicy,
}

/// The two facts the window's close handler needs, cached because that handler is
/// synchronous and native: it can't await an HTTP round-trip to the engine, and it
/// runs at exactly the moment the user is expecting the window to respond.
///
/// The frontend already tracks both — it holds the settings and the task list — so
/// it pushes them here whenever either flips. See `set_quit_policy`.
#[derive(Default)]
pub struct QuitPolicy {
    pub close_to_tray: AtomicBool,
    pub has_active_transfers: AtomicBool,
}

/// Where the engine is and how to authenticate to it. The frontend asks once at
/// startup and talks to the engine directly from then on.
#[tauri::command]
pub fn engine_endpoint(state: State<'_, AppState>) -> EngineLink {
    state.engine.clone()
}

/// Keep the native close handler's view current. Called by the frontend whenever
/// the close-to-tray setting or "is anything actually running" changes.
#[tauri::command]
pub fn set_quit_policy(
    state: State<'_, AppState>,
    close_to_tray: bool,
    has_active_transfers: bool,
) {
    state
        .quit
        .close_to_tray
        .store(close_to_tray, Ordering::SeqCst);
    state
        .quit
        .has_active_transfers
        .store(has_active_transfers, Ordering::SeqCst);
}

/// Hide the main window to the tray — the "minimize to tray" choice in the
/// quit-confirm prompt. The engine is a separate process and keeps running
/// regardless; what matters is that the webview stays alive, since its event
/// subscription is what tells the daemon someone is still here.
#[tauri::command]
pub fn hide_window(window: tauri::WebviewWindow) {
    let _ = window.hide();
}

/// Quit now — the "quit anyway" choice in the quit-confirm prompt. The webview
/// goes with us, so the daemon sees its last client leave and winds the engine
/// down cleanly on its own.
#[tauri::command]
pub fn quit_app(app: AppHandle) {
    app.exit(0);
}
