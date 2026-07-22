//! The Tauri command surface — the typed bridge the frontend calls via `invoke`.

use std::path::PathBuf;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::core::backend::BackendInfo;
use crate::core::engine::Engine;
use crate::core::settings::Settings;
use crate::core::task::Task;

/// Shared app state: just the engine for now.
pub struct AppState {
    pub engine: Engine,
}

#[derive(Serialize)]
pub struct AppInfo {
    pub name: &'static str,
    pub version: &'static str,
}

/// Basic app identity — handy as a first end-to-end `invoke` smoke test.
#[tauri::command]
pub fn app_info() -> AppInfo {
    AppInfo {
        name: "moin",
        version: env!("CARGO_PKG_VERSION"),
    }
}

/// The folder new downloads land in: the user's override, else the OS Downloads.
fn download_dir(app: &AppHandle, state: &AppState) -> PathBuf {
    if let Some(dir) = state.engine.settings().download_dir {
        return PathBuf::from(dir);
    }
    app.path()
        .download_dir()
        .unwrap_or_else(|_| std::env::temp_dir())
}

/// Add a direct-HTTP download. (Torrent/media add-commands arrive with their
/// backends.) Async so it runs on Tauri's tokio runtime, where the engine spawns
/// its workers.
#[tauri::command]
pub async fn add_download(
    app: AppHandle,
    state: State<'_, AppState>,
    url: String,
) -> Result<Task, String> {
    let dir = download_dir(&app, &state);
    state.engine.add_http(url, dir)
}

#[tauri::command]
pub fn list_downloads(state: State<'_, AppState>) -> Vec<Task> {
    state.engine.list()
}

#[tauri::command]
pub async fn pause_download(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.engine.pause(&id);
    Ok(())
}

#[tauri::command]
pub async fn resume_download(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.engine.resume(&id);
    Ok(())
}

#[tauri::command]
pub async fn cancel_download(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.engine.cancel(&id);
    Ok(())
}

#[tauri::command]
pub async fn remove_download(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.engine.remove(&id);
    Ok(())
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Settings {
    state.engine.settings()
}

#[tauri::command]
pub async fn set_settings(state: State<'_, AppState>, settings: Settings) -> Result<(), String> {
    state.engine.set_settings(settings);
    Ok(())
}

#[tauri::command]
pub fn list_backends(state: State<'_, AppState>) -> Vec<BackendInfo> {
    state.engine.backends()
}

/// The default download folder, for display in the UI.
#[tauri::command]
pub fn default_download_dir(app: AppHandle, state: State<'_, AppState>) -> String {
    download_dir(&app, &state).to_string_lossy().into_owned()
}
