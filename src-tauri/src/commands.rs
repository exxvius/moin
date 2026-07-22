//! The Tauri command surface — the typed bridge the frontend calls via `invoke`.

use std::path::PathBuf;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::core::backend::BackendInfo;
use crate::core::category::Category;
use crate::core::engine::Engine;
use crate::core::settings::Settings;
use crate::core::task::Task;
use crate::core::tool::ToolStatus;
use crate::events;

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
    category: Option<String>,
) -> Result<Task, String> {
    let dir = download_dir(&app, &state);
    state.engine.add_http(url, dir, category)
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
    state.engine.remove(&id)
}

/// Remove from the list and delete the downloaded file from disk.
#[tauri::command]
pub async fn delete_download(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.engine.delete(&id)
}

/// Retry an archived download from scratch (re-queues it fresh).
#[tauri::command]
pub async fn retry_download(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.engine.retry(&id);
    Ok(())
}

/// Permanently delete an archived record from the manifest.
#[tauri::command]
pub async fn forget_download(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.engine.forget(&id);
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

/// Byte progress while the aria2c archive downloads.
#[derive(Clone, Serialize)]
pub struct ToolProgress {
    pub received: u64,
    pub total: Option<u64>,
}

/// Current aria2c status: resolved path, version, and where it came from.
#[tauri::command]
pub async fn tool_status(state: State<'_, AppState>) -> Result<ToolStatus, String> {
    Ok(state.engine.tool_status().await)
}

/// Download and install the managed aria2c build (Windows). Emits
/// `moin-tool-progress` while the archive downloads.
#[tauri::command]
pub async fn download_tool(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<ToolStatus, String> {
    state
        .engine
        .install_tool(move |received, total| {
            let _ = app.emit(events::TOOL_PROGRESS, ToolProgress { received, total });
        })
        .await
}

/// Set (or clear, with `null`) the bring-your-own aria2c path.
#[tauri::command]
pub async fn set_tool_path(
    state: State<'_, AppState>,
    path: Option<String>,
) -> Result<ToolStatus, String> {
    Ok(state.engine.set_tool_path(path).await)
}

#[tauri::command]
pub fn list_categories(state: State<'_, AppState>) -> Vec<Category> {
    state.engine.categories()
}

/// The category a manually-added URL would fall into, for pre-selecting the picker.
#[tauri::command]
pub fn suggest_category(state: State<'_, AppState>, url: String) -> Option<String> {
    state.engine.suggest_category(&url)
}

/// Each mutator returns the full, reordered category list so the UI can replace
/// its copy in one step.
#[tauri::command]
pub async fn create_category(
    state: State<'_, AppState>,
    category: Category,
) -> Result<Vec<Category>, String> {
    Ok(state.engine.create_category(category))
}

#[tauri::command]
pub async fn update_category(
    state: State<'_, AppState>,
    category: Category,
) -> Result<Vec<Category>, String> {
    Ok(state.engine.update_category(category))
}

#[tauri::command]
pub async fn delete_category(
    state: State<'_, AppState>,
    id: String,
) -> Result<Vec<Category>, String> {
    Ok(state.engine.delete_category(&id))
}

#[tauri::command]
pub async fn reorder_categories(
    state: State<'_, AppState>,
    ids: Vec<String>,
) -> Result<Vec<Category>, String> {
    Ok(state.engine.reorder_categories(ids))
}

/// The default download folder, for display in the UI.
#[tauri::command]
pub fn default_download_dir(app: AppHandle, state: State<'_, AppState>) -> String {
    download_dir(&app, &state).to_string_lossy().into_owned()
}
