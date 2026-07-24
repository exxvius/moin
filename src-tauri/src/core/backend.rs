//! The pluggable download backend.
//!
//! Everything above this line (the [`Task`] model, the store, the queue, the
//! events) is engine-agnostic. A backend is the thing that actually moves bytes.
//! The embedded backend (reqwest + librqbit) is the default; aria2c and, later,
//! libtorrent are additional implementations the user can pick per source type.
//! Adding one means writing a new `impl DownloadBackend` and registering it —
//! nothing else in the engine changes.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

use super::task::{ResolvedTorrent, Task, TaskKind, TaskStatus, TorrentDetails};

/// What the supervisor is asking a running transfer to do. Polled by the backend
/// between chunks, so pause/cancel take effect without tearing anything down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Signal {
    Run = 0,
    Pause = 1,
    Cancel = 2,
}

/// A shared, cheap-to-poll control handle. The supervisor flips it; the backend
/// reads it. Carries the run/pause/cancel signal plus a live "force" flag (the
/// user force-started a torrent), both readable and settable while a run is live.
#[derive(Debug, Clone, Default)]
pub struct Control(Arc<ControlState>);

#[derive(Debug, Default)]
struct ControlState {
    signal: AtomicU8,
    force: AtomicBool,
}

impl Control {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&self, signal: Signal) {
        self.0.signal.store(signal as u8, Ordering::Relaxed);
    }

    pub fn signal(&self) -> Signal {
        match self.0.signal.load(Ordering::Relaxed) {
            1 => Signal::Pause,
            2 => Signal::Cancel,
            _ => Signal::Run,
        }
    }

    /// Set the live force-start flag; a running torrent picks it up on its next
    /// poll (bypasses stall, keeps running with no peers).
    pub fn set_force(&self, force: bool) {
        self.0.force.store(force, Ordering::Relaxed);
    }

    pub fn force(&self) -> bool {
        self.0.force.load(Ordering::Relaxed)
    }
}

/// Per-transfer tuning the supervisor hands a backend at run time. Pulled from
/// settings, not persisted on the task, so a change applies to the next run.
#[derive(Debug, Clone, Copy)]
pub struct TransferOpts {
    /// Max parallel connections for one HTTP download. 1 = single stream.
    /// Backends that can't split (torrent, media) ignore it.
    pub connections: usize,
    /// Smallest piece worth its own connection, in bytes. A file below this size
    /// stays single-stream; larger files split into pieces no smaller than this.
    pub min_split_size: u64,
    /// Hide the in-progress `.part` artifacts via the OS hidden attribute.
    pub hide_part: bool,
    /// How long a transfer may receive no data before it's declared stalled.
    /// `Duration::ZERO` means never — wait indefinitely for data to resume.
    pub stall_timeout: Duration,
    /// Stop seeding a torrent once its ratio reaches this. 0 = seed indefinitely.
    /// Non-torrent backends ignore it.
    pub seed_ratio_limit: f64,
    /// Stop seeding a torrent once it has seeded this long. `Duration::ZERO` =
    /// no time limit. Whichever of ratio/time comes first wins.
    pub seed_time_limit: Duration,
    /// How long to wait for a magnet's metadata (from the swarm) before failing the
    /// add. Non-torrent backends ignore it.
    pub magnet_timeout: Duration,
    /// How often to re-scrape the trackers for seeder/leecher counts. Torrent-only.
    pub scrape_interval: Duration,
}

/// Network settings a backend applies to its client, refreshed when settings
/// change. Backends that build their own client (the embedded one) act on it;
/// others ignore it. Carries the torrent knobs too — the embedded backend forwards
/// them to its session (rate caps live, the rest on next session build).
#[derive(Debug, Clone, Copy)]
pub struct NetConfig {
    /// Max time to establish a connection, or `None` for the OS default.
    pub connect_timeout: Option<Duration>,
    /// Torrent-engine tuning: listen port, DHT/UPnP, and up/down rate caps.
    pub torrent: TorrentNet,
}

/// Torrent-engine network settings. The port/DHT/UPnP fields are read when the
/// shared session is (lazily) built, so a change takes effect on the next start;
/// the rate caps apply live to a running session.
#[derive(Debug, Clone, Copy)]
pub struct TorrentNet {
    /// First TCP port to try binding for incoming peers (a small range follows).
    pub listen_port: u16,
    /// Whether the DHT runs.
    pub dht: bool,
    /// Whether UPnP / NAT-PMP port forwarding is attempted.
    pub upnp: bool,
    /// Download cap in bytes/sec, or `None` for unlimited.
    pub download_bps: Option<std::num::NonZeroU32>,
    /// Upload cap in bytes/sec, or `None` for unlimited.
    pub upload_bps: Option<std::num::NonZeroU32>,
    /// How long to wait on a peer's handshake before dropping it, or `None` for the
    /// engine default. Baked in when the session is built.
    pub peer_connect_timeout: Option<Duration>,
    /// How many ports past the listen port to try if it's busy. Baked in at build.
    pub port_span: u16,
}

/// How a transfer ended. The supervisor maps this onto a [`TaskStatus`].
#[derive(Debug)]
pub enum Outcome {
    Completed,
    Paused,
    Canceled,
    /// No data arrived within the stall window. Distinct from `Failed`: the
    /// partial is kept and the download can resume from where it left off.
    Stalled,
    Failed(String),
}

/// Live torrent-only readings a backend reports alongside byte progress: upload
/// activity, the connected-peer count, and the swarm's seeder/leecher counts
/// (the latter from tracker scrape, since librqbit doesn't expose them). HTTP
/// backends pass `None`.
#[derive(Debug, Clone, Copy)]
pub struct TorrentTick {
    pub uploaded: u64,
    pub up_speed: u64,
    /// Live download speed in bytes/sec, straight from the engine's per-byte
    /// estimator. The engine reports this directly rather than deriving it from the
    /// progress delta, which only moves when a whole piece verifies (jumpy, and
    /// zero between piece completions even while bytes are flowing).
    pub down_speed: u64,
    pub peers: u32,
    pub seeders: u32,
    pub leechers: u32,
    /// The phase this reading reflects — `Checking` while verifying/resolving,
    /// `Downloading` while fetching, `Seeding` once complete. Drives the task's
    /// status directly (a torrent doesn't go through the HTTP status path).
    pub status: TaskStatus,
}

/// Progress reporter handed to a backend. The backend calls it with the current
/// received/total (and, for torrents, a swarm/upload tick); the supervisor turns
/// that into speed, events, and persistence.
pub type ProgressFn = Arc<dyn Fn(u64, Option<u64>, Option<TorrentTick>) + Send + Sync>;

/// A concrete download engine. One instance handles many tasks concurrently; the
/// supervisor spawns a task per `run` call.
#[async_trait::async_trait]
pub trait DownloadBackend: Send + Sync {
    /// Stable identifier stored in settings, e.g. `"embedded"` or `"aria2"`.
    fn id(&self) -> &'static str;

    /// Human-readable name for the settings picker.
    fn label(&self) -> &'static str;

    /// Whether this backend can handle a given source type.
    fn supports(&self, kind: TaskKind) -> bool;

    /// Whether the backend is usable right now (e.g. aria2c is installed). The
    /// embedded backend is always available.
    fn available(&self) -> bool {
        true
    }

    /// Apply network settings (e.g. connect timeout) that changed at run time.
    /// Backends that build their own client rebuild it; others ignore it.
    fn reconfigure(&self, _net: NetConfig) {}

    /// Resolve a torrent's metadata (its file list) without downloading it, so the
    /// add-torrent modal can offer a file picker. `None` for backends that don't
    /// do torrents; a magnet resolves from the swarm (and may time out).
    async fn resolve_torrent(&self, _source: &str) -> Option<Result<ResolvedTorrent, String>> {
        None
    }

    /// Live detail for an active torrent (files, peers, trackers), keyed by info
    /// hash. `None` for non-torrent backends; `Some(Err)` if the torrent isn't
    /// currently in the engine.
    async fn torrent_details(&self, _info_hash: &str) -> Option<Result<TorrentDetails, String>> {
        None
    }

    /// Change which files an active torrent downloads (the indices to keep).
    /// `None` for non-torrent backends.
    async fn set_torrent_files(
        &self,
        _info_hash: &str,
        _selected: &[usize],
    ) -> Option<Result<(), String>> {
        None
    }

    /// Detach a torrent from the engine, keeping its files (the caller deletes
    /// data itself). `None` for non-torrent backends.
    async fn remove_torrent(&self, _info_hash: &str) -> Option<Result<(), String>> {
        None
    }

    /// Drive `task` to a terminal [`Outcome`], honoring `control` and reporting
    /// progress via `progress`. `task.received` is the byte offset to resume from;
    /// `opts` carries run-time tuning like the parallel-connection count.
    async fn run(
        &self,
        task: Task,
        opts: TransferOpts,
        control: Control,
        progress: ProgressFn,
    ) -> Outcome;

    /// Cleanly wind down before the app exits — flush resume state, stop any
    /// managed subprocess. Default no-op; a backend with an external daemon (aria2)
    /// uses it to shut down gracefully so its control files are saved.
    async fn shutdown(&self) {}
}

/// A backend's identity + capabilities, sent to the settings UI.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BackendInfo {
    pub id: String,
    pub label: String,
    pub http: bool,
    pub torrent: bool,
    pub available: bool,
}

impl BackendInfo {
    pub fn of(b: &dyn DownloadBackend) -> Self {
        Self {
            id: b.id().to_string(),
            label: b.label().to_string(),
            http: b.supports(TaskKind::Http),
            torrent: b.supports(TaskKind::Torrent),
            available: b.available(),
        }
    }
}
