//! The pluggable download backend.
//!
//! Everything above this line (the [`Task`] model, the store, the queue, the
//! events) is engine-agnostic. A backend is the thing that actually moves bytes.
//! The embedded backend (reqwest + librqbit) is the default; aria2c and, later,
//! libtorrent are additional implementations the user can pick per source type.
//! Adding one means writing a new `impl DownloadBackend` and registering it —
//! nothing else in the engine changes.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

use super::task::{Task, TaskKind};

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
/// reads it.
#[derive(Debug, Clone, Default)]
pub struct Control(Arc<AtomicU8>);

impl Control {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&self, signal: Signal) {
        self.0.store(signal as u8, Ordering::Relaxed);
    }

    pub fn signal(&self) -> Signal {
        match self.0.load(Ordering::Relaxed) {
            1 => Signal::Pause,
            2 => Signal::Cancel,
            _ => Signal::Run,
        }
    }
}

/// Per-transfer tuning the supervisor hands a backend at run time. Pulled from
/// settings, not persisted on the task, so a change applies to the next run.
#[derive(Debug, Clone, Copy)]
pub struct TransferOpts {
    /// Max parallel connections for one HTTP download. 1 = single stream.
    /// Backends that can't split (torrent, media) ignore it.
    pub connections: usize,
}

/// How a transfer ended. The supervisor maps this onto a [`TaskStatus`].
#[derive(Debug)]
pub enum Outcome {
    Completed,
    Paused,
    Canceled,
    Failed(String),
}

/// Progress reporter handed to a backend. The backend calls it with the current
/// received/total; the supervisor turns that into speed, events, and persistence.
pub type ProgressFn = Arc<dyn Fn(u64, Option<u64>) + Send + Sync>;

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
