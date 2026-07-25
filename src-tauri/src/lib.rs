//! moin — Tauri application entry point.
//!
//! The download engine lives in its own process (`moin-engine`); this crate is the
//! desktop shell around it. It finds or starts that daemon, hands the frontend an
//! endpoint to talk to it directly, and owns the things only a desktop shell can:
//! the window, the tray, the `moin://` scheme, and the close button.
//!
//! Nothing here touches downloads. The engine is in `crates/moin-core`; the API
//! the UI calls is in `crates/moin-daemon`.

pub mod commands;
pub mod engine_link;

use std::sync::atomic::Ordering;

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter as _, Manager};

use moin_core::events;

use crate::commands::{AppState, QuitPolicy};

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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Must be registered first: a second launch (e.g. the browser opening
        // `moin://launch` to wake moin) is funneled back to the running instance —
        // its window is focused and the new process exits, so we never end up with
        // two windows fighting over one tray icon.
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

            // Per-user data dir holds the downloads DB, settings, and logs. We
            // resolve it and hand it to the daemon rather than letting it work the
            // path out again, so the two processes can never disagree about where
            // the store is.
            let data_dir = app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| std::env::temp_dir().join("moin"));
            std::fs::create_dir_all(&data_dir).ok();

            init_logging(&data_dir);
            tracing::info!("moin starting, data dir: {}", data_dir.display());

            // Blocking on purpose: the frontend needs somewhere to talk to before
            // it can render anything useful, exactly as it needed a live engine
            // before. Normally this is a health check against an engine that's
            // already up, or a spawn plus a few hundred milliseconds.
            let engine = tauri::async_runtime::block_on(engine_link::connect(data_dir))?;

            app.manage(AppState {
                engine,
                quit: QuitPolicy::default(),
            });

            // System tray, so "minimize to tray" on close has somewhere to go.
            if let Err(e) = setup_tray(&app.handle().clone()) {
                tracing::warn!("couldn't create the tray icon: {e}");
            }
            Ok(())
        })
        // Close button: when the setting says so, hide to the tray instead of
        // quitting. Both facts come from the quit policy the frontend keeps
        // current — this handler is synchronous and native, so it can't go and ask
        // the engine at the moment the user expects the window to react.
        //
        // Hiding keeps the webview alive, and the webview's event subscription is
        // what tells the daemon someone is still here. So minimizing keeps
        // transfers running and quitting really does stop them, same as ever.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() != "main" {
                    return;
                }
                let Some(state) = window.app_handle().try_state::<AppState>() else {
                    return;
                };
                if state.quit.close_to_tray.load(Ordering::SeqCst) {
                    api.prevent_close();
                    let _ = window.hide();
                } else if state.quit.has_active_transfers.load(Ordering::SeqCst) {
                    // Quitting would stop active downloads/seeding — let the UI
                    // confirm (minimize / quit anyway / cancel) instead of quitting.
                    api.prevent_close();
                    let _ = window.emit(events::CONFIRM_QUIT, ());
                }
                // Otherwise nothing is running: let the close through and quit.
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::engine_endpoint,
            commands::set_quit_policy,
            commands::hide_window,
            commands::quit_app,
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
