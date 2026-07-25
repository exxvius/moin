//! Where the daemon keeps things.
//!
//! These MUST resolve to the same directories the Tauri shell used when it hosted
//! the engine in-process, or an upgrade would silently strand every existing
//! download: same SQLite store, same settings.json, same managed-tool folder.
//!
//! Tauri's `app_data_dir()` is `dirs::data_dir()` joined with the bundle
//! identifier, and `download_dir()` is the OS Downloads folder — both reproduced
//! here. The identifier is duplicated from `tauri.conf.json` because the daemon
//! deliberately doesn't depend on Tauri; the test below is what keeps them honest.

use std::path::PathBuf;

/// Must match `identifier` in `tauri.conf.json`.
const IDENTIFIER: &str = "app.moin.desktop";

/// Points the daemon at a different data directory. For development and tests, so
/// a scratch engine can never open the real store — which holds live torrent
/// resume state and would happily restart someone's downloads.
const DATA_DIR_ENV: &str = "MOIN_DATA_DIR";

/// Per-user data directory: the downloads DB, settings, logs, managed tools.
pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(DATA_DIR_ENV) {
        return PathBuf::from(dir);
    }
    dirs::data_dir()
        .map(|d| d.join(IDENTIFIER))
        .unwrap_or_else(|| std::env::temp_dir().join("moin"))
}

/// The OS Downloads folder — the fallback destination when no download dir is set.
pub fn downloads_dir() -> PathBuf {
    dirs::download_dir().unwrap_or_else(std::env::temp_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The identifier is the whole contract with the old in-process layout. If
    /// someone edits tauri.conf.json without editing this, every user's download
    /// history disappears on upgrade — so read the real file and compare.
    #[test]
    fn identifier_matches_tauri_config() {
        let conf = include_str!("../../../tauri.conf.json");
        let want = format!("\"identifier\": \"{IDENTIFIER}\"");
        assert!(
            conf.contains(&want),
            "IDENTIFIER ({IDENTIFIER}) no longer matches tauri.conf.json — the \
             daemon would use a different data dir than the app used to"
        );
    }

    #[test]
    fn data_dir_is_under_the_identifier() {
        std::env::remove_var(DATA_DIR_ENV);
        assert!(data_dir().ends_with(IDENTIFIER));
    }
}
