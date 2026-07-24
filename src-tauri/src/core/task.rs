//! The unified download task: one shape for every source (HTTP now, torrent and
//! media later), plus its state machine. The engine and the UI both speak this.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Where a download comes from. Only HTTP is live in this phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskKind {
    Http,
    Torrent,
    Media,
}

/// How a torrent task was added — a `magnet:` URI or a picked `.torrent` file.
/// `None` on non-torrent tasks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TorrentSource {
    Magnet,
    File,
}

/// One file inside a (possibly multi-file) torrent. Empty on non-torrent tasks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TorrentFile {
    /// Path relative to the torrent's output folder, using `/` separators.
    pub path: String,
    /// File size in bytes.
    pub size: u64,
    /// Whether this file is picked for download. Defaults on.
    #[serde(default = "default_true")]
    pub selected: bool,
    /// Bytes fetched for this file so far.
    #[serde(default)]
    pub received: u64,
}

fn default_true() -> bool {
    true
}

/// A torrent resolved to its file list up front, before the user commits to
/// downloading it — what the add-torrent modal needs to show a file picker.
#[derive(Debug, Clone, Serialize)]
pub struct ResolvedTorrent {
    pub info_hash: String,
    pub name: Option<String>,
    /// Total size of every file in the torrent.
    pub total: u64,
    /// Files in torrent order (index = position, which is what selection uses).
    pub files: Vec<TorrentFile>,
}

/// The resolved torrent plus the engine's filing suggestions, sent to the
/// add-torrent modal so it can pre-fill the save folder and category. When a
/// category with automation is suggested, its exclusions are already reflected in
/// `resolved.files[].selected` and its renames in `resolved.files[].path`, and
/// `suggested_layout` carries its layout — so the modal opens with automation
/// applied but every choice still overridable.
#[derive(Debug, Clone, Serialize)]
pub struct TorrentPreview {
    #[serde(flatten)]
    pub resolved: ResolvedTorrent,
    /// Category a rule would file this under, if any (the modal pre-selects it).
    pub suggested_category: Option<String>,
    /// Folder the download would default to (the suggested category's folder, or
    /// the plain download dir) — the modal shows it and lets the user override.
    pub default_dir: String,
    /// Content layout the suggested category prefers (the modal pre-selects it).
    /// `Original` when nothing is suggested.
    pub suggested_layout: super::category::LayoutMode,
}

/// One connected peer, for the detail panel's Peers tab.
#[derive(Debug, Clone, Serialize)]
pub struct PeerInfo {
    /// Remote address (`ip:port`).
    pub addr: String,
    /// Connection state ("live", "connecting", …).
    pub state: String,
    /// Bytes fetched from this peer so far.
    pub downloaded: u64,
}

/// Live per-torrent detail the expanded card polls while it's open — the heavy
/// stuff that doesn't belong on every progress tick.
#[derive(Debug, Clone, Serialize)]
pub struct TorrentDetails {
    /// Files with live per-file progress and current selection.
    pub files: Vec<TorrentFile>,
    /// Connected peers.
    pub peers: Vec<PeerInfo>,
    /// Tracker URLs (librqbit doesn't expose per-tracker announce status).
    pub trackers: Vec<String>,
    /// Have-pieces bucketed into ≤200 segments — each is the fraction of pieces
    /// in that range we hold, for the piece-level Progress bar.
    pub pieces: Vec<f32>,
    /// Distributed copies of the torrent reachable in the swarm (the rarest
    /// piece's availability, counting ourselves) — qBittorrent's "availability".
    pub availability: f64,
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
///
/// `Moving` is a transient side-state: the file is being relocated to a new
/// category's folder. When the move finishes the task returns to `Completed` (if
/// it was already done) or re-queues to resume.
///
/// `Stalled` means data stopped arriving past the stall window — the connection
/// went quiet without an outright error. The partial is kept; for HTTP it waits
/// for a manual retry, while a torrent can slip back into `Downloading` on its
/// own once a peer delivers data again.
///
/// `Seeding` is torrent-only: the download finished but the task keeps uploading
/// to peers until a ratio/time limit or a manual stop moves it to `Completed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Queued,
    Connecting,
    /// Torrent-only: verifying already-present pieces / resolving metadata before
    /// data actually transfers.
    Checking,
    Downloading,
    Paused,
    Moving,
    Completed,
    Seeding,
    Failed,
    Stalled,
    Canceled,
}

impl TaskStatus {
    /// A terminal state won't change on its own.
    pub fn is_terminal(self) -> bool {
        matches!(self, TaskStatus::Completed | TaskStatus::Canceled)
    }

    /// A short human label for the status, for surfacing in action errors
    /// ("nothing to pause — this download is already paused").
    pub fn label(self) -> &'static str {
        match self {
            TaskStatus::Queued => "queued",
            TaskStatus::Connecting => "connecting",
            TaskStatus::Checking => "checking",
            TaskStatus::Downloading => "downloading",
            TaskStatus::Paused => "paused",
            TaskStatus::Moving => "being moved",
            TaskStatus::Completed => "finished",
            TaskStatus::Seeding => "seeding",
            TaskStatus::Failed => "failed",
            TaskStatus::Stalled => "stalled",
            TaskStatus::Canceled => "canceled",
        }
    }

    /// Waiting for or occupying a download worker slot. Seeding runs off the
    /// download queue, so it doesn't count here.
    pub fn is_active(self) -> bool {
        matches!(
            self,
            TaskStatus::Queued
                | TaskStatus::Connecting
                | TaskStatus::Checking
                | TaskStatus::Downloading
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
    /// Id of the category this download is filed under, or `None` if uncategorized.
    #[serde(default)]
    pub category: Option<String>,
    /// A pending post-completion relocation. Set when this download stages into a
    /// category's incomplete folder: `dest` points inside that folder while the
    /// transfer runs, and on completion the finished content is moved into this
    /// folder (the category's real save folder), after which the field clears.
    /// `None` means the download already lives in its final folder.
    #[serde(default)]
    pub final_dir: Option<String>,
    /// Extra HTTP request headers to send with this download — Cookie, Referer,
    /// User-Agent and friends, captured by the browser extension so an auth-gated
    /// link downloads the same way the browser would. Persisted, so a resume after
    /// a restart still authenticates. Empty for hand-added links.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,

    // --- Torrent-only fields. All default/empty on HTTP + media tasks and
    // ignored by their paths. For a torrent, `dest` is the output *folder*. ---
    /// BitTorrent info hash (hex) once known — identifies the swarm and dedupes
    /// a re-add of the same torrent.
    #[serde(default)]
    pub info_hash: Option<String>,
    /// Whether the torrent came from a magnet URI or a `.torrent` file.
    #[serde(default)]
    pub torrent_source: Option<TorrentSource>,
    /// Files inside the torrent, with per-file size, selection, and progress.
    /// Empty until metadata resolves.
    #[serde(default)]
    pub files: Vec<TorrentFile>,
    /// Total bytes uploaded to peers. Accumulates across sessions (persisted);
    /// feeds the ratio and the all-time stats.
    #[serde(default)]
    pub uploaded: u64,
    /// Live swarm readings — the count of complete seeders, of partial leechers,
    /// and of currently connected peers. Transient: not persisted, refreshed by
    /// the engine while the task is active, 0 otherwise.
    #[serde(default)]
    pub seeders: u32,
    #[serde(default)]
    pub leechers: u32,
    #[serde(default)]
    pub peers: u32,
    /// Upload speed in bytes/sec. Transient, like the swarm counts.
    #[serde(default)]
    pub up_speed: u64,
    /// True when `dest` is a folder this torrent created for itself (the
    /// "create subfolder" layout), so it's safe to remove on delete. False when
    /// files were saved directly into a folder the user chose (which may hold
    /// other downloads) — we only ever delete our own files there, never the folder.
    #[serde(default)]
    pub own_dir: bool,
    /// Suppress the category's `.torrent`-file export for this torrent. Set on a
    /// torrent produced by converting a captured `.torrent` download: that source
    /// file is deliberately consumed and left no trace (no archive, no saved copy),
    /// so the converted torrent must not re-save a `.torrent` either.
    #[serde(default)]
    pub skip_torrent_export: bool,
    /// Transient: the user chose to keep seeding this torrent past the ratio/time
    /// limit ("Start seeding" on a finished torrent). Read when the run starts to
    /// disable the auto-stop for that session. Not persisted (seeding doesn't
    /// survive a restart yet) and not sent to the UI.
    #[serde(skip)]
    pub force_seed: bool,
    /// Transient: the user forced this torrent to run ("Force start"). It bypasses
    /// the concurrency limit and never drops to `Stalled`, so it keeps trying even
    /// with no peers. Sent to the UI (to mark the row and toggle the menu) but not
    /// persisted — a restart clears it.
    #[serde(default)]
    pub force_start: bool,
}

impl Task {
    /// True for torrent tasks. A torrent manages its own partials + fastresume
    /// inside the output folder, so the single-file `.part`/`.part.meta` helpers
    /// below don't apply to it.
    pub fn is_torrent(&self) -> bool {
        matches!(self.kind, TaskKind::Torrent)
    }

    /// The partial-download file we stream into before the final rename.
    /// HTTP/media only — a torrent uses [`Task::is_torrent`] instead.
    pub fn part_path(&self) -> String {
        format!("{}.part", self.dest)
    }

    /// Sidecar next to the `.part` that records per-segment progress for a
    /// multi-connection download, so it can resume after a pause or a restart.
    pub fn meta_path(&self) -> String {
        format!("{}.part.meta", self.dest)
    }

    /// Upload/download ratio for a torrent (uploaded ÷ received). 0 before
    /// anything has been downloaded, so it never divides by zero.
    pub fn ratio(&self) -> f64 {
        if self.received == 0 {
            0.0
        } else {
            self.uploaded as f64 / self.received as f64
        }
    }
}

/// A lightweight progress tick during an active download. Emitted often, so it
/// carries only what the bars need — not the whole [`Task`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TaskProgress {
    pub id: String,
    pub received: u64,
    pub total: Option<u64>,
    /// Bytes per second, smoothed by the engine.
    pub speed: u64,
    pub status: TaskStatus,
    /// Torrent-only live readings — 0 for HTTP. Carried on the tick so a torrent
    /// row shows upload activity, peers, and swarm counts without waiting for a
    /// full task update.
    #[serde(default)]
    pub up_speed: u64,
    #[serde(default)]
    pub uploaded: u64,
    #[serde(default)]
    pub peers: u32,
    #[serde(default)]
    pub seeders: u32,
    #[serde(default)]
    pub leechers: u32,
}

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Sanitize a caller-supplied filename (e.g. one a browser capture passed along)
/// to a safe basename: directory components dropped, path separators and
/// characters illegal on Windows removed, trailing dots/spaces trimmed. Returns
/// `None` when nothing usable is left, so callers fall back to the URL-derived
/// name.
pub fn sanitize_filename(name: &str) -> Option<String> {
    // Keep only the last path component, however it was separated.
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let cleaned: String = base
        .chars()
        .filter(|c| {
            !c.is_control() && !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
        })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches(['.', ' ']);
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
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
    use super::{filename_from_url, sanitize_filename};

    #[test]
    fn sanitize_filename_keeps_a_plain_name() {
        assert_eq!(sanitize_filename("movie.mkv").as_deref(), Some("movie.mkv"));
    }

    #[test]
    fn sanitize_filename_strips_path_and_illegal_chars() {
        assert_eq!(
            sanitize_filename("../../etc/pa:ss?wd.txt").as_deref(),
            Some("passwd.txt")
        );
        assert_eq!(
            sanitize_filename("C:\\Users\\x\\a<b>.zip").as_deref(),
            Some("ab.zip")
        );
    }

    #[test]
    fn sanitize_filename_rejects_empty_or_dots() {
        assert_eq!(sanitize_filename("   "), None);
        assert_eq!(sanitize_filename("///"), None);
        assert_eq!(sanitize_filename("....").as_deref(), None);
    }

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
