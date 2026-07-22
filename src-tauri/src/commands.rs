//! Tauri command surface — the typed bridge the frontend calls via `invoke`.
//!
//! Just an info command for now; the download/queue/tool commands arrive with
//! their engines in later phases.

use serde::Serialize;

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
