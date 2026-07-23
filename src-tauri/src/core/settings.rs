//! Engine settings: which backend handles each source type, how many downloads
//! run at once, and the default download folder. Persisted as JSON in the data
//! dir (small, human-readable, easy to hand-edit).

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

const DEFAULT_CONCURRENCY: usize = 4;
const DEFAULT_CONNECTIONS: usize = 8;
/// Smallest slice worth its own connection. Below this a download stays single
/// stream; above it, pieces never split smaller than this. 1 MiB matches aria2's
/// minimum for `--min-split-size`, so the same value drives both engines.
const DEFAULT_MIN_SPLIT_SIZE: u64 = 1 << 20; // 1 MiB
/// Seconds of no incoming data before a transfer is marked stalled. 0 = never.
const DEFAULT_STALL_TIMEOUT: u64 = 60;
/// Seconds to wait for a connection to be established. 0 = OS default.
const DEFAULT_CONNECT_TIMEOUT: u64 = 30;
/// Loopback port the browser-integration RPC server listens on. A fixed default
/// keeps the extension zero-config; the user can move it if something else has it.
const DEFAULT_RPC_PORT: u16 = 47653;
/// Seed until the upload/download ratio reaches this, then stop. 0 = seed forever.
const DEFAULT_SEED_RATIO_LIMIT: f64 = 0.0;
/// Seed for at most this many minutes after finishing, then stop. 0 = forever.
const DEFAULT_SEED_TIME_LIMIT_MINS: u64 = 0;
/// TCP port the torrent engine starts listening on for incoming peers. A small
/// range from here is tried so a busy port falls through to the next.
const DEFAULT_TORRENT_LISTEN_PORT: u16 = 4240;
/// Torrent up/download rate caps in bytes per second. 0 = unlimited.
const DEFAULT_TORRENT_RATE_LIMIT: u64 = 0;

/// What happens to a download's file when it's moved to a different category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum CategoryChangeBehavior {
    /// Only re-tag the download; the file stays where it is.
    #[default]
    ChangeOnly,
    /// Re-tag and relocate the file into the new category's folder.
    MoveFile,
}

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
    /// Smallest piece worth its own connection, in bytes. Files below this size
    /// download in a single stream; larger files split into parallel pieces no
    /// smaller than this. Drives both the built-in engine and aria2c.
    #[serde(default = "default_min_split_size")]
    pub min_split_size: u64,
    /// Hide the in-progress `.part` files while downloading (via the OS hidden
    /// attribute, where supported — Windows now); the finished file appears when
    /// the download completes.
    #[serde(default)]
    pub hide_part_files: bool,
    /// What moving a download to another category does to its file: just re-tag,
    /// or relocate the file into the new category's folder.
    #[serde(default)]
    pub category_change: CategoryChangeBehavior,
    /// Seconds a transfer may go without receiving data before it's marked
    /// stalled. 0 means never — wait indefinitely for data to resume.
    #[serde(default = "default_stall_timeout")]
    pub stall_timeout_secs: u64,
    /// Seconds to wait while establishing a connection. 0 means the OS default.
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_secs: u64,
    /// Default destination folder; `None` means the OS Downloads folder.
    pub download_dir: Option<String>,
    /// Explicit path to a user-supplied aria2c binary (the "bring your own"
    /// override in the resolve chain). `None` means fall back to the managed
    /// copy, a binary beside the exe, or `PATH`.
    #[serde(default)]
    pub aria2_path: Option<String>,
    /// Whether the loopback RPC server (the browser extension's entry point) runs.
    /// Toggling it or changing the port takes effect on the next app start.
    #[serde(default = "default_rpc_enabled")]
    pub rpc_enabled: bool,
    /// Loopback port the RPC server binds. Applied at startup.
    #[serde(default = "default_rpc_port")]
    pub rpc_port: u16,
    /// Stop seeding a torrent once its upload/download ratio reaches this. 0 means
    /// seed indefinitely (until stopped by hand or by the time limit).
    #[serde(default = "default_seed_ratio_limit")]
    pub seed_ratio_limit: f64,
    /// Stop seeding a torrent this many minutes after it finishes downloading. 0
    /// means no time limit. Whichever of ratio/time is hit first stops seeding.
    #[serde(default = "default_seed_time_limit_mins")]
    pub seed_time_limit_mins: u64,
    /// TCP port the torrent engine listens on for incoming peers (a small range
    /// from here is tried so a busy port falls through). Applied on the next app
    /// start — the shared session binds its port once at build.
    #[serde(default = "default_torrent_listen_port")]
    pub torrent_listen_port: u16,
    /// Whether the torrent engine runs the DHT (finds peers without a tracker).
    /// Applied on the next app start.
    #[serde(default = "default_true")]
    pub torrent_dht: bool,
    /// Whether the torrent engine asks the router to forward its listen port
    /// (UPnP / NAT-PMP), which helps peers reach you. Applied on the next start.
    #[serde(default = "default_true")]
    pub torrent_upnp: bool,
    /// Cap torrent download speed to this many bytes per second. 0 = unlimited.
    /// Applies live (no restart needed).
    #[serde(default = "default_torrent_rate_limit")]
    pub torrent_download_limit: u64,
    /// Cap torrent upload speed to this many bytes per second. 0 = unlimited.
    /// Applies live (no restart needed).
    #[serde(default = "default_torrent_rate_limit")]
    pub torrent_upload_limit: u64,
    /// Bearer token the extension must present on `/add`. Generated on first run
    /// (see `Engine::new`) and read live, so regenerating it takes effect at once.
    #[serde(default)]
    pub rpc_token: String,
}

fn default_connections() -> usize {
    DEFAULT_CONNECTIONS
}

fn default_min_split_size() -> u64 {
    DEFAULT_MIN_SPLIT_SIZE
}

fn default_stall_timeout() -> u64 {
    DEFAULT_STALL_TIMEOUT
}

fn default_connect_timeout() -> u64 {
    DEFAULT_CONNECT_TIMEOUT
}

fn default_rpc_enabled() -> bool {
    true
}

fn default_rpc_port() -> u16 {
    DEFAULT_RPC_PORT
}

fn default_seed_ratio_limit() -> f64 {
    DEFAULT_SEED_RATIO_LIMIT
}

fn default_seed_time_limit_mins() -> u64 {
    DEFAULT_SEED_TIME_LIMIT_MINS
}

fn default_torrent_listen_port() -> u16 {
    DEFAULT_TORRENT_LISTEN_PORT
}

fn default_true() -> bool {
    true
}

fn default_torrent_rate_limit() -> u64 {
    DEFAULT_TORRENT_RATE_LIMIT
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            http_backend: "embedded".to_string(),
            torrent_backend: "embedded".to_string(),
            max_concurrent: DEFAULT_CONCURRENCY,
            connections: DEFAULT_CONNECTIONS,
            min_split_size: DEFAULT_MIN_SPLIT_SIZE,
            hide_part_files: false,
            category_change: CategoryChangeBehavior::default(),
            stall_timeout_secs: DEFAULT_STALL_TIMEOUT,
            connect_timeout_secs: DEFAULT_CONNECT_TIMEOUT,
            download_dir: None,
            aria2_path: None,
            rpc_enabled: true,
            rpc_port: DEFAULT_RPC_PORT,
            rpc_token: String::new(),
            seed_ratio_limit: DEFAULT_SEED_RATIO_LIMIT,
            seed_time_limit_mins: DEFAULT_SEED_TIME_LIMIT_MINS,
            torrent_listen_port: DEFAULT_TORRENT_LISTEN_PORT,
            torrent_dht: true,
            torrent_upnp: true,
            torrent_download_limit: DEFAULT_TORRENT_RATE_LIMIT,
            torrent_upload_limit: DEFAULT_TORRENT_RATE_LIMIT,
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
