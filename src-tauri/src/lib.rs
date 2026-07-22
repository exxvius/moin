//! moin — Tauri application entry point.
//!
//! The headless download engine lives in [`core`]; this module wires it to the
//! Tauri shell (state, plugins, command handlers). `run()` is invoked by both
//! the desktop binary (`main.rs`) and any mobile entry points Tauri generates.
//!
//! The tray, richer state, and the actual engines land in later phases. Right
//! now this is a working shell: it opens the window, sets up the per-user data
//! dir + logging, and exposes a single info command.

pub mod commands;
pub mod core;
pub mod events;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // Per-user data dir will hold the downloads DB, settings, logs, and
            // any app-managed binaries (yt-dlp/ffmpeg) we fetch on demand.
            let data_dir = app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| std::env::temp_dir().join("moin"));
            std::fs::create_dir_all(&data_dir).ok();

            init_logging(&data_dir);
            tracing::info!("moin starting, data dir: {}", data_dir.display());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![commands::app_info])
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
