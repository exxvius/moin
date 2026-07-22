//! The unified download task: one shape for every source (HTTP now, torrent and
//! media later), plus its state machine. The engine and the UI both speak this.

use serde::{Deserialize, Serialize};

/// Where a download comes from. Only HTTP is live in this phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskKind {
    Http,
    Torrent,
    Media,
}

/// A task's point in its lifecycle.
///
/// ```text
/// Queued → Connecting → Downloading ⇄ Paused
///                           │            │
///                           ▼            ▼
///                       Completed    (resume)
///                        Failed / Canceled
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Queued,
    Connecting,
    Downloading,
    Paused,
    Completed,
    Failed,
    Canceled,
}

impl TaskStatus {
    /// A terminal state won't change on its own.
    pub fn is_terminal(self) -> bool {
        matches!(self, TaskStatus::Completed | TaskStatus::Canceled)
    }

    /// Waiting for or occupying a worker slot.
    pub fn is_active(self) -> bool {
        matches!(
            self,
            TaskStatus::Queued | TaskStatus::Connecting | TaskStatus::Downloading
        )
    }
}

/// A single download, as persisted and as sent to the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub kind: TaskKind,
    /// The source: a URL for HTTP (a magnet/torrent later).
    pub url: String,
    pub filename: String,
    /// Absolute path the finished file will live at.
    pub dest: String,
    pub status: TaskStatus,
    /// Total size in bytes once known (some servers don't report it).
    pub total: Option<u64>,
    /// Bytes on disk so far.
    pub received: u64,
    pub error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    /// When the download first reached Completed (ms since epoch), if ever.
    #[serde(default)]
    pub completed_at: Option<i64>,
    /// Archived tasks are hidden from the normal list (they live in the Archive
    /// filter) but kept in the manifest so their stats still count.
    #[serde(default)]
    pub archived: bool,
    /// Total time spent actively downloading, in ms — used for average speed.
    #[serde(default)]
    pub active_ms: i64,
    /// Id of the backend that actually ran this download (e.g. `"embedded"` or
    /// `"aria2"`). `None` until the task first starts.
    #[serde(default)]
    pub backend: Option<String>,
}

impl Task {
    /// The partial-download file we stream into before the final rename.
    pub fn part_path(&self) -> String {
        format!("{}.part", self.dest)
    }

    /// Sidecar next to the `.part` that records per-segment progress for a
    /// multi-connection download, so it can resume after a pause or a restart.
    pub fn meta_path(&self) -> String {
        format!("{}.part.meta", self.dest)
    }
}

/// A lightweight progress tick during an active download. Emitted often, so it
/// carries only what the bars need — not the whole [`Task`].
#[derive(Debug, Clone, Serialize)]
pub struct TaskProgress {
    pub id: String,
    pub received: u64,
    pub total: Option<u64>,
    /// Bytes per second, smoothed by the engine.
    pub speed: u64,
    pub status: TaskStatus,
}

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Best-effort filename from a URL: the last non-empty path segment, percent
/// left as-is, query/fragment stripped. Falls back to "download".
pub fn filename_from_url(url: &str) -> String {
    let no_query = url.split(['?', '#']).next().unwrap_or(url);
    let name = no_query
        .rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or("")
        .trim();
    if name.is_empty() {
        "download".to_string()
    } else {
        name.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::filename_from_url;

    #[test]
    fn filename_from_url_takes_last_segment() {
        assert_eq!(filename_from_url("https://x.com/a/b/file.zip"), "file.zip");
    }

    #[test]
    fn filename_from_url_strips_query_and_fragment() {
        assert_eq!(
            filename_from_url("https://x.com/dir/movie.mkv?token=abc#frag"),
            "movie.mkv"
        );
    }

    #[test]
    fn filename_from_url_handles_trailing_slash_and_empties() {
        assert_eq!(filename_from_url("https://x.com/downloads/"), "downloads");
        assert_eq!(filename_from_url("https://x.com"), "x.com");
    }
}
