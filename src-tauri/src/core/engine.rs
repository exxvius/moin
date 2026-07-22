//! The supervisor: owns the task registry, the queue, persistence, and backend
//! selection. It's Tauri-free — it reports out through the [`Emitter`] trait,
//! which the shell implements with `AppHandle::emit`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use uuid::Uuid;

use super::backend::{
    BackendInfo, Control, DownloadBackend, Outcome, ProgressFn, Signal, TransferOpts,
};
use super::embedded::EmbeddedBackend;
use super::settings::Settings;
use super::store::Store;
use super::task::{filename_from_url, now_ms, Task, TaskKind, TaskProgress, TaskStatus};

/// How the engine reports changes to the outside world (the UI, via Tauri).
pub trait Emitter: Send + Sync + 'static {
    fn added(&self, task: &Task);
    fn progress(&self, p: &TaskProgress);
    fn updated(&self, task: &Task);
    fn removed(&self, id: &str);
}

struct Entry {
    task: Task,
    /// Present while the task is running; flipping it pauses/cancels the transfer.
    control: Option<Control>,
    /// A drop requested while the task was still running. `Some(delete_file)` — the
    /// task is cancelling; once it stops (releasing its file handle) it gets
    /// dropped, and the completed file is deleted too when `true`.
    pending_archive: Option<bool>,
}

struct Inner {
    data_dir: PathBuf,
    emitter: Arc<dyn Emitter>,
    backends: Vec<Arc<dyn DownloadBackend>>,
    settings: Mutex<Settings>,
    tasks: Mutex<HashMap<String, Entry>>,
    store: Mutex<Store>,
}

/// The public handle. Cheap to clone; all state lives behind an `Arc`.
#[derive(Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}

impl Engine {
    /// Open the store, restore persisted tasks, and register the backends.
    pub fn new(data_dir: PathBuf, emitter: Arc<dyn Emitter>) -> Result<Self, String> {
        let store = Store::open(&data_dir)?;
        let settings = Settings::load(&data_dir);

        let mut tasks = HashMap::new();
        for mut task in store.all()? {
            // Anything that was mid-flight when we last quit comes back paused —
            // never silently resume a download the user didn't ask to restart.
            if task.status == TaskStatus::Connecting || task.status == TaskStatus::Downloading {
                task.status = TaskStatus::Paused;
            }
            tasks.insert(
                task.id.clone(),
                Entry {
                    task,
                    control: None,
                    pending_archive: None,
                },
            );
        }

        sweep_orphan_parts(tasks.values().map(|e| &e.task));

        let backends: Vec<Arc<dyn DownloadBackend>> = vec![Arc::new(EmbeddedBackend::new())];

        Ok(Self {
            inner: Arc::new(Inner {
                data_dir,
                emitter,
                backends,
                settings: Mutex::new(settings),
                tasks: Mutex::new(tasks),
                store: Mutex::new(store),
            }),
        })
    }

    /// Every task, newest first.
    pub fn list(&self) -> Vec<Task> {
        let tasks = self.inner.tasks.lock().unwrap();
        let mut out: Vec<Task> = tasks.values().map(|e| e.task.clone()).collect();
        out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        out
    }

    /// Queue a direct HTTP download into `dir`.
    pub fn add_http(&self, url: String, dir: PathBuf) -> Result<Task, String> {
        let url = url.trim().to_string();
        if url.is_empty() {
            return Err("no URL given".to_string());
        }
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let base = filename_from_url(&url);
        let now = now_ms();

        // Reserve a collision-free name while holding the registry lock, so two
        // adds of the same URL can't race onto the same file/.part.
        let task = {
            let mut tasks = self.inner.tasks.lock().unwrap();
            let taken: HashSet<String> = tasks.values().map(|e| e.task.dest.clone()).collect();
            let filename = unique_filename(&dir, &base, &taken);
            let dest = dir.join(&filename).to_string_lossy().into_owned();
            let task = Task {
                id: Uuid::new_v4().to_string(),
                kind: TaskKind::Http,
                url,
                filename,
                dest,
                status: TaskStatus::Queued,
                total: None,
                received: 0,
                error: None,
                created_at: now,
                updated_at: now,
                completed_at: None,
                archived: false,
                active_ms: 0,
            };
            tasks.insert(
                task.id.clone(),
                Entry {
                    task: task.clone(),
                    control: None,
                    pending_archive: None,
                },
            );
            task
        };

        self.inner.store.lock().unwrap().upsert(&task)?;
        self.inner.emitter.added(&task);
        Inner::pump(self.inner.clone());
        Ok(task)
    }

    pub fn pause(&self, id: &str) {
        let updated = {
            let mut tasks = self.inner.tasks.lock().unwrap();
            let Some(entry) = tasks.get_mut(id) else {
                return;
            };
            if let Some(control) = &entry.control {
                // Running: ask it to stop; `finish` will mark it Paused.
                control.set(Signal::Pause);
                None
            } else if entry.task.status == TaskStatus::Queued {
                entry.task.status = TaskStatus::Paused;
                entry.task.updated_at = now_ms();
                Some(entry.task.clone())
            } else {
                None
            }
        };
        if let Some(task) = updated {
            self.persist_emit(&task);
        }
    }

    pub fn resume(&self, id: &str) {
        let task = {
            let mut tasks = self.inner.tasks.lock().unwrap();
            let Some(entry) = tasks.get_mut(id) else {
                return;
            };
            if entry.control.is_some() || entry.task.status == TaskStatus::Completed {
                return;
            }
            entry.task.status = TaskStatus::Queued;
            entry.task.error = None;
            entry.task.updated_at = now_ms();
            entry.task.clone()
        };
        self.persist_emit(&task);
        Inner::pump(self.inner.clone());
    }

    pub fn cancel(&self, id: &str) {
        let task = {
            let mut tasks = self.inner.tasks.lock().unwrap();
            let Some(entry) = tasks.get_mut(id) else {
                return;
            };
            if let Some(control) = &entry.control {
                control.set(Signal::Cancel);
                return; // `finish` handles the rest
            }
            entry.task.status = TaskStatus::Canceled;
            entry.task.updated_at = now_ms();
            entry.task.clone()
        };
        cleanup_partial(&task);
        self.persist_emit(&task);
        Inner::pump(self.inner.clone());
    }

    /// Remove from the list: archive the record (kept for stats), delete the
    /// partial file, and leave any finished file on disk.
    pub fn remove(&self, id: &str) -> Result<(), String> {
        self.archive(id, false)
    }

    /// Remove from the list AND delete the downloaded file from disk. Fails (and
    /// leaves the download in the list) if the file can't be deleted.
    pub fn delete(&self, id: &str) -> Result<(), String> {
        self.archive(id, true)
    }

    fn archive(&self, id: &str, delete_file: bool) -> Result<(), String> {
        // Running: defer. Deleting the `.part` now would fail on Windows (the
        // download still holds the handle open), so wait for the task to stop and
        // let `finish` archive it. A running task is incomplete, so there's no
        // finished file to lock — the deferred delete can't fail this way.
        let dest = {
            let mut tasks = self.inner.tasks.lock().unwrap();
            match tasks.get_mut(id) {
                Some(entry) if entry.control.is_some() => {
                    entry.pending_archive = Some(delete_file);
                    if let Some(control) = &entry.control {
                        control.set(Signal::Cancel);
                    }
                    return Ok(());
                }
                Some(entry) => entry.task.dest.clone(),
                None => return Ok(()),
            }
        };

        // Delete the finished file first. If that fails (e.g. it's open in another
        // program), bail out before touching the list so the user can retry.
        if delete_file {
            remove_if_exists(&dest).map_err(|e| format!("Couldn't delete the file: {e}"))?;
        }

        let task = {
            let mut tasks = self.inner.tasks.lock().unwrap();
            let Some(entry) = tasks.get_mut(id) else {
                return Ok(());
            };
            entry.task.archived = true;
            if entry.task.status.is_active() {
                entry.task.status = TaskStatus::Canceled;
            }
            entry.task.updated_at = now_ms();
            entry.task.clone()
        };
        cleanup_partial(&task);
        let _ = self.inner.store.lock().unwrap().upsert(&task);
        self.inner.emitter.updated(&task);
        Inner::pump(self.inner.clone());
        Ok(())
    }

    /// Retry an archived download from scratch (not a resume) — un-archive it and
    /// re-queue with everything reset. Fails at run time if the link is dead.
    pub fn retry(&self, id: &str) {
        let task = {
            let mut tasks = self.inner.tasks.lock().unwrap();
            let Some(entry) = tasks.get_mut(id) else {
                return;
            };
            if entry.control.is_some() {
                return;
            }
            entry.task.archived = false;
            entry.task.status = TaskStatus::Queued;
            entry.task.received = 0;
            entry.task.total = None;
            entry.task.error = None;
            entry.task.active_ms = 0;
            entry.task.completed_at = None;
            entry.task.updated_at = now_ms();
            entry.task.clone()
        };
        // Fresh start: drop any leftover partial and the old finished file.
        cleanup_partial(&task);
        cleanup_file(task.dest.clone());
        let _ = self.inner.store.lock().unwrap().upsert(&task);
        self.inner.emitter.updated(&task);
        Inner::pump(self.inner.clone());
    }

    /// Permanently delete an archived record from the manifest (leaving any
    /// finished file on disk).
    pub fn forget(&self, id: &str) {
        let task = self.inner.tasks.lock().unwrap().remove(id).map(|e| e.task);
        if let Some(task) = task {
            cleanup_partial(&task);
            let _ = self.inner.store.lock().unwrap().delete(&task.id);
            self.inner.emitter.removed(&task.id);
        }
    }

    pub fn settings(&self) -> Settings {
        self.inner.settings.lock().unwrap().clone()
    }

    pub fn set_settings(&self, next: Settings) {
        {
            let mut s = self.inner.settings.lock().unwrap();
            *s = next.clone();
            s.save(&self.inner.data_dir);
        }
        Inner::pump(self.inner.clone());
    }

    /// The registered backends and what they can do (for the settings picker).
    pub fn backends(&self) -> Vec<BackendInfo> {
        self.inner
            .backends
            .iter()
            .map(|b| BackendInfo::of(b.as_ref()))
            .collect()
    }

    fn persist_emit(&self, task: &Task) {
        let _ = self.inner.store.lock().unwrap().upsert(task);
        self.inner.emitter.updated(task);
    }
}

impl Inner {
    fn backend_for(&self, kind: TaskKind) -> Option<Arc<dyn DownloadBackend>> {
        let want = {
            let s = self.settings.lock().unwrap();
            match kind {
                TaskKind::Http => s.http_backend.clone(),
                TaskKind::Torrent | TaskKind::Media => s.torrent_backend.clone(),
            }
        };
        // Preferred backend if it fits, else the first that supports this kind.
        self.backends
            .iter()
            .find(|b| b.id() == want && b.supports(kind) && b.available())
            .or_else(|| {
                self.backends
                    .iter()
                    .find(|b| b.supports(kind) && b.available())
            })
            .cloned()
    }

    fn running_count(&self) -> usize {
        self.tasks
            .lock()
            .unwrap()
            .values()
            .filter(|e| e.control.is_some())
            .count()
    }

    /// Start queued tasks until the concurrency limit is reached. A limit of 0
    /// means unlimited — every queued task starts.
    fn pump(inner: Arc<Inner>) {
        let max = inner.settings.lock().unwrap().max_concurrent;
        let unlimited = max == 0;
        while unlimited || inner.running_count() < max {
            let next = {
                let tasks = inner.tasks.lock().unwrap();
                tasks
                    .values()
                    .filter(|e| e.task.status == TaskStatus::Queued && e.control.is_none())
                    .min_by_key(|e| e.task.created_at)
                    .map(|e| e.task.id.clone())
            };
            match next {
                Some(id) => Inner::start(inner.clone(), id),
                None => break,
            }
        }
    }

    fn start(inner: Arc<Inner>, id: String) {
        let (task, control) = {
            let mut tasks = inner.tasks.lock().unwrap();
            let Some(entry) = tasks.get_mut(&id) else {
                return;
            };
            if entry.control.is_some() {
                return;
            }
            let control = Control::new();
            entry.control = Some(control.clone());
            entry.task.status = TaskStatus::Connecting;
            entry.task.error = None;
            entry.task.updated_at = now_ms();
            (entry.task.clone(), control)
        };

        let _ = inner.store.lock().unwrap().upsert(&task);
        inner.emitter.updated(&task);

        let Some(backend) = inner.backend_for(task.kind) else {
            Inner::finish(
                inner,
                id,
                Outcome::Failed("no backend is set up for this source".to_string()),
            );
            return;
        };

        let opts = TransferOpts {
            connections: inner.settings.lock().unwrap().connections,
        };
        let progress = Inner::make_progress(inner.clone(), id.clone());
        let inner_done = inner.clone();
        tokio::spawn(async move {
            let outcome = backend.run(task, opts, control, progress).await;
            Inner::finish(inner_done, id, outcome);
        });
    }

    /// Build the progress reporter for a task: updates memory, computes a smoothed
    /// speed, throttles events, and persists occasionally.
    fn make_progress(inner: Arc<Inner>, id: String) -> ProgressFn {
        struct State {
            started: bool,
            last_emit: Instant,
            last_persist: Instant,
            last_tick: Instant,
            last_bytes: u64,
            speed: f64,
        }
        let state = Arc::new(Mutex::new(State {
            started: false,
            last_emit: Instant::now() - Duration::from_secs(1),
            last_persist: Instant::now(),
            last_tick: Instant::now(),
            last_bytes: 0,
            speed: 0.0,
        }));

        Arc::new(move |received: u64, total: Option<u64>| {
            let now = Instant::now();

            // Snapshot the task + decide what to do while briefly holding locks.
            let (task, newly_started, do_emit, do_persist, speed) = {
                let mut st = state.lock().unwrap();
                let mut tasks = inner.tasks.lock().unwrap();
                let Some(entry) = tasks.get_mut(&id) else {
                    return;
                };
                entry.task.received = received;
                if let Some(t) = total {
                    entry.task.total = Some(t);
                }

                let newly_started = !st.started;
                if newly_started {
                    st.started = true;
                    st.last_tick = now;
                    entry.task.status = TaskStatus::Downloading;
                    entry.task.updated_at = now_ms();
                } else {
                    // Accumulate active download time (for average speed).
                    entry.task.active_ms += now.duration_since(st.last_tick).as_millis() as i64;
                    st.last_tick = now;
                }

                let do_emit = now.duration_since(st.last_emit) >= Duration::from_millis(200);
                if do_emit {
                    let dt = now.duration_since(st.last_emit).as_secs_f64().max(0.001);
                    let inst = (received.saturating_sub(st.last_bytes)) as f64 / dt;
                    // Exponential smoothing so the number doesn't jitter.
                    st.speed = if st.speed == 0.0 {
                        inst
                    } else {
                        st.speed * 0.7 + inst * 0.3
                    };
                    st.last_emit = now;
                    st.last_bytes = received;
                }
                let do_persist = now.duration_since(st.last_persist) >= Duration::from_secs(2);
                if do_persist {
                    st.last_persist = now;
                }

                (
                    entry.task.clone(),
                    newly_started,
                    do_emit,
                    do_persist,
                    st.speed,
                )
            };

            if newly_started {
                inner.emitter.updated(&task);
            }
            if do_emit {
                inner.emitter.progress(&TaskProgress {
                    id: id.clone(),
                    received,
                    total,
                    speed: speed as u64,
                    status: TaskStatus::Downloading,
                });
            }
            if do_persist {
                let _ = inner.store.lock().unwrap().upsert(&task);
            }
        })
    }

    fn finish(inner: Arc<Inner>, id: String, outcome: Outcome) {
        // If a remove/delete was requested mid-download, archive it now — the task
        // has stopped and released its file handle.
        let (task, archived) = {
            let mut tasks = inner.tasks.lock().unwrap();
            let Some(entry) = tasks.get_mut(&id) else {
                return;
            };
            entry.control = None;
            if let Some(delete_file) = entry.pending_archive.take() {
                entry.task.archived = true;
                entry.task.status = TaskStatus::Canceled;
                entry.task.updated_at = now_ms();
                (entry.task.clone(), Some(delete_file))
            } else {
                match outcome {
                    Outcome::Completed => {
                        entry.task.status = TaskStatus::Completed;
                        entry.task.error = None;
                        entry.task.completed_at = Some(now_ms());
                        if let Some(total) = entry.task.total {
                            entry.task.received = total;
                        }
                    }
                    Outcome::Paused => entry.task.status = TaskStatus::Paused,
                    Outcome::Canceled => entry.task.status = TaskStatus::Canceled,
                    Outcome::Failed(msg) => {
                        entry.task.status = TaskStatus::Failed;
                        entry.task.error = Some(msg);
                    }
                }
                entry.task.updated_at = now_ms();
                (entry.task.clone(), None)
            }
        };

        if let Some(delete_file) = archived {
            purge_files(&task, delete_file);
        } else if task.status == TaskStatus::Canceled {
            cleanup_partial(&task);
        }
        let _ = inner.store.lock().unwrap().upsert(&task);
        inner.emitter.updated(&task);
        Inner::pump(inner);
    }
}

/// At startup, delete `.part` files that a terminal/archived task still points
/// to — a leftover from a cleanup that didn't finish (e.g. a crash). Only paths
/// referenced by our own manifest are touched, so `.part` files from other apps
/// (Firefox, etc.) in the same folder are never affected; paused/failed partials
/// are kept so they can still be resumed.
fn sweep_orphan_parts<'a>(tasks: impl Iterator<Item = &'a Task>) {
    for t in tasks {
        let should_be_gone =
            t.archived || matches!(t.status, TaskStatus::Completed | TaskStatus::Canceled);
        if !should_be_gone {
            continue;
        }
        for leftover in [t.part_path(), t.meta_path()] {
            if let Err(e) = std::fs::remove_file(&leftover) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    eprintln!("moin: couldn't sweep leftover {leftover}: {e}");
                }
            }
        }
    }
}

/// Remove a file, treating "already gone" as success but surfacing real errors
/// (e.g. the file is locked by another process).
fn remove_if_exists(path: &str) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Best-effort but robust background file removal. A `.part` (or a just-finished
/// file) can briefly stay locked after a transfer stops — especially on Windows,
/// where the OS releases the handle a moment after the writer drops it — so a
/// single `remove_file` right after cancelling races the close and fails. This
/// retries with backoff, treats "already gone" as success, and logs (never
/// panics) if it truly can't, so cleanup neither blocks a caller nor fails
/// silently. Must be called from within the tokio runtime (all callers are).
fn cleanup_file(path: String) {
    tokio::spawn(async move {
        for attempt in 0u32..6 {
            match tokio::fs::remove_file(&path).await {
                Ok(()) => return,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
                Err(e) => {
                    if attempt == 5 {
                        eprintln!("moin: gave up removing {path}: {e}");
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(60 * u64::from(attempt) + 60)).await;
                }
            }
        }
    });
}

/// Delete a task's partial (`.part`) file and its `.meta` sidecar, plus the
/// finished file when `delete_file` is set — all robustly, in the background.
fn purge_files(task: &Task, delete_file: bool) {
    cleanup_partial(task);
    if delete_file {
        cleanup_file(task.dest.clone());
    }
}

/// Drop a task's in-progress artifacts: the `.part` file and the multi-connection
/// `.meta` resume sidecar (absent for single-stream downloads — a no-op then).
fn cleanup_partial(task: &Task) {
    cleanup_file(task.part_path());
    cleanup_file(task.meta_path());
}

/// Pick a filename that doesn't collide in `dir`, adding " (n)" before the
/// extension if needed. Avoids: an existing file, a partial download (`.part`),
/// and any destination already claimed by another task in `taken`.
fn unique_filename(dir: &Path, name: &str, taken: &HashSet<String>) -> String {
    let is_free = |candidate: &str| {
        let path = dir.join(candidate);
        if path.exists() || dir.join(format!("{candidate}.part")).exists() {
            return false;
        }
        !taken.contains(path.to_string_lossy().as_ref())
    };
    if is_free(name) {
        return name.to_string();
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    for n in 1..100_000 {
        let candidate = format!("{stem} ({n}){ext}");
        if is_free(&candidate) {
            return candidate;
        }
    }
    name.to_string()
}
