//! Engine settings: which backend handles each source type, how many downloads
//! run at once, and the default download folder. Persisted as JSON in the data
//! dir (small, human-readable, easy to hand-edit).

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

const DEFAULT_CONCURRENCY: usize = 4;
const DEFAULT_CONNECTIONS: usize = 8;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// Backend id used for direct HTTP downloads.
    pub http_backend: String,
    /// Backend id used for torrents.
    pub torrent_backend: String,
    /// How many downloads may run at once.
    pub max_concurrent: usize,
    /// Max parallel connections a single HTTP download may open. 1 means one
    /// stream; higher splits the file into ranges pulled in parallel.
    /// Sources that don't support ranges quietly fall back to a single stream.
    #[serde(default = "default_connections")]
    pub connections: usize,
    /// Default destination folder; `None` means the OS Downloads folder.
    pub download_dir: Option<String>,
    /// Explicit path to a user-supplied aria2c binary (the "bring your own"
    /// override in the resolve chain). `None` means fall back to the managed
    /// copy, a binary beside the exe, or `PATH`.
    #[serde(default)]
    pub aria2_path: Option<String>,
}

fn default_connections() -> usize {
    DEFAULT_CONNECTIONS
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            http_backend: "embedded".to_string(),
            torrent_backend: "embedded".to_string(),
            max_concurrent: DEFAULT_CONCURRENCY,
            connections: DEFAULT_CONNECTIONS,
            download_dir: None,
            aria2_path: None,
        }
    }
}

impl Settings {
    fn path(data_dir: &Path) -> std::path::PathBuf {
        data_dir.join("settings.json")
    }

    pub fn load(data_dir: &Path) -> Self {
        fs::read_to_string(Self::path(data_dir))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, data_dir: &Path) {
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = fs::write(Self::path(data_dir), text);
        }
    }
}
