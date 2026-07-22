//! The supervisor: owns the task registry, the queue, persistence, and backend
//! selection. It's Tauri-free — it reports out through the [`Emitter`] trait,
//! which the shell implements with `AppHandle::emit`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use uuid::Uuid;

use super::aria2::Aria2Backend;
use super::backend::{
    BackendInfo, Control, DownloadBackend, NetConfig, Outcome, ProgressFn, Signal, TransferOpts,
};
use super::category::{self, Candidate, Category};
use super::embedded::EmbeddedBackend;
use super::settings::{CategoryChangeBehavior, Settings};
use super::store::Store;
use super::task::{filename_from_url, now_ms, Task, TaskKind, TaskProgress, TaskStatus};
use super::tool::{Aria2Tool, ToolStatus};

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
    /// A category move requested while the task was still running: it's pausing,
    /// and once it stops the move begins instead of leaving it paused.
    pending_move: Option<PendingMove>,
    /// True while the file is being relocated (status `Moving`). Guards the task
    /// against pause/resume/cancel and defers a remove until the move finishes.
    moving: bool,
}

/// A queued request to relocate a task's file into another category's folder.
struct PendingMove {
    /// The category to file the download under afterwards (`None` = uncategorized).
    category: Option<String>,
    /// The folder the file is moving into.
    target_dir: PathBuf,
}

impl Entry {
    /// A fresh entry for `task`, not yet running and with nothing pending.
    fn idle(task: Task) -> Self {
        Self {
            task,
            control: None,
            pending_archive: None,
            pending_move: None,
            moving: false,
        }
    }
}

/// Everything the spawned relocation needs: the old and new paths for the file
/// (and its `.part`/`.meta` siblings) plus what to do once the bytes have landed.
struct MoveJob {
    id: String,
    /// The download was already finished, so its payload is the final file rather
    /// than a `.part`; it returns to `Completed` afterwards instead of re-queuing.
    was_completed: bool,
    /// Category to file the download under once the move lands.
    category: Option<String>,
    new_dest: String,
    new_filename: String,
    old_dest: String,
    old_part: String,
    old_meta: String,
    new_part: String,
    new_meta: String,
    /// Known total size, used for the progress bar when the file size can't be
    /// read from disk.
    total: Option<u64>,
}

struct Inner {
    data_dir: PathBuf,
    emitter: Arc<dyn Emitter>,
    backends: Vec<Arc<dyn DownloadBackend>>,
    /// The managed aria2c binary, shared with the aria2 backend so a fresh
    /// download or a new bring-your-own path takes effect everywhere at once.
    tool: Arc<Aria2Tool>,
    settings: Mutex<Settings>,
    /// User-defined categories (rules that file downloads into buckets),
    /// persisted as `categories.json`.
    categories: Mutex<Vec<Category>>,
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
        let categories = category::load_or_seed(&data_dir);

        let mut tasks = HashMap::new();
        for mut task in store.all()? {
            // Anything that was mid-flight when we last quit comes back paused —
            // never silently resume a download the user didn't ask to restart. A
            // move interrupted by a quit is treated the same: the file is still at
            // its recorded path (the dest only advances after the move lands), so
            // resuming from where it sits is safe.
            if matches!(
                task.status,
                TaskStatus::Connecting | TaskStatus::Downloading | TaskStatus::Moving
            ) {
                task.status = TaskStatus::Paused;
            }
            tasks.insert(task.id.clone(), Entry::idle(task));
        }

        sweep_orphan_parts(tasks.values().map(|e| &e.task));

        let tool = Arc::new(Aria2Tool::new(
            data_dir.clone(),
            settings.aria2_path.clone(),
        ));
        let backends: Vec<Arc<dyn DownloadBackend>> = vec![
            Arc::new(EmbeddedBackend::new()),
            Arc::new(Aria2Backend::new(tool.clone())),
        ];
        // Apply the persisted network settings (e.g. connect timeout) to the
        // backends' clients before anything runs.
        let net = net_config(&settings);
        for b in &backends {
            b.reconfigure(net);
        }

        Ok(Self {
            inner: Arc::new(Inner {
                data_dir,
                emitter,
                backends,
                tool,
                settings: Mutex::new(settings),
                categories: Mutex::new(categories),
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

    /// Queue a direct HTTP download. Files it under `category` when given (a valid
    /// id), and lands it in that category's `save_dir` if it sets one — otherwise
    /// the passed `dir`.
    pub fn add_http(
        &self,
        url: String,
        dir: PathBuf,
        category: Option<String>,
    ) -> Result<Task, String> {
        let url = url.trim().to_string();
        if url.is_empty() {
            return Err("no URL given".to_string());
        }

        // Resolve the category: keep only an id that still exists, and honor its
        // save-folder override for the destination.
        let (category, dir) = {
            let cats = self.inner.categories.lock().unwrap();
            match category.and_then(|id| cats.iter().find(|c| c.id == id)) {
                Some(c) => {
                    let dir = c.save_dir.clone().map(PathBuf::from).unwrap_or(dir);
                    (Some(c.id.clone()), dir)
                }
                None => (None, dir),
            }
        };

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
                backend: None,
                category,
            };
            tasks.insert(task.id.clone(), Entry::idle(task.clone()));
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
            if entry.moving {
                return; // mid-relocation; let the move finish first
            }
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
            if entry.moving
                || entry.control.is_some()
                || entry.task.status == TaskStatus::Completed
            {
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
            if entry.moving {
                return; // mid-relocation; let the move finish first
            }
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
                // Mid-relocation: there's no control to cancel, so just record the
                // request; the move-finish handler archives once the file lands.
                Some(entry) if entry.moving => {
                    entry.pending_archive = Some(delete_file);
                    return Ok(());
                }
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
            if entry.moving || entry.control.is_some() {
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
        // Keep the aria2c resolver in step with the persisted override so both
        // save paths (general settings and the tool picker) agree.
        self.inner.tool.set_override(next.aria2_path.clone());
        // Push network settings (connect timeout) to the backends' clients.
        let net = net_config(&next);
        for b in &self.inner.backends {
            b.reconfigure(net);
        }
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

    /// Current aria2c status (resolved path, version, where it came from).
    pub async fn tool_status(&self) -> ToolStatus {
        self.inner.tool.status().await
    }

    /// Download and install the managed aria2c build (Windows). `progress` reports
    /// the archive download so the UI can show a bar.
    pub async fn install_tool(
        &self,
        progress: impl Fn(u64, Option<u64>) + Send + Sync,
    ) -> Result<ToolStatus, String> {
        self.inner.tool.install(progress).await
    }

    /// Point aria2c at a user-supplied binary (or clear the override with `None`),
    /// persist it, and return the refreshed status.
    pub async fn set_tool_path(&self, path: Option<String>) -> ToolStatus {
        let path = path.filter(|p| !p.trim().is_empty());
        self.inner.tool.set_override(path.clone());
        {
            let mut s = self.inner.settings.lock().unwrap();
            s.aria2_path = path;
            s.save(&self.inner.data_dir);
        }
        self.inner.tool.status().await
    }

    fn persist_emit(&self, task: &Task) {
        let _ = self.inner.store.lock().unwrap().upsert(task);
        self.inner.emitter.updated(task);
    }

    /// Every category, in priority order.
    pub fn categories(&self) -> Vec<Category> {
        let mut cats = self.inner.categories.lock().unwrap().clone();
        cats.sort_by_key(|c| c.order);
        cats
    }

    /// The category a manually-added `url` would fall into, if any — used to
    /// pre-select the picker in the add view.
    pub fn suggest_category(&self, url: &str) -> Option<String> {
        use super::category::AddMethodKind;
        let cand = Candidate::from_url(url.trim(), AddMethodKind::ManualLink);
        category::categorize(&cand, &self.inner.categories.lock().unwrap())
    }

    /// Add a new category (server assigns its id and priority). Returns the list.
    pub fn create_category(&self, mut cat: Category) -> Vec<Category> {
        {
            let mut cats = self.inner.categories.lock().unwrap();
            cat.id = Uuid::new_v4().to_string();
            cat.order = cats.iter().map(|c| c.order).max().unwrap_or(-1) + 1;
            cats.push(cat);
            category::save(&cats, &self.inner.data_dir);
        }
        self.categories()
    }

    /// Replace an existing category by id (no-op if it's gone). Returns the list.
    pub fn update_category(&self, cat: Category) -> Vec<Category> {
        {
            let mut cats = self.inner.categories.lock().unwrap();
            if let Some(slot) = cats.iter_mut().find(|c| c.id == cat.id) {
                *slot = cat;
                category::save(&cats, &self.inner.data_dir);
            }
        }
        self.categories()
    }

    /// Delete a category and un-file every download that referenced it (the
    /// downloads themselves are kept). Returns the remaining categories.
    pub fn delete_category(&self, id: &str) -> Vec<Category> {
        {
            let mut cats = self.inner.categories.lock().unwrap();
            cats.retain(|c| c.id != id);
            category::save(&cats, &self.inner.data_dir);
        }

        // Orphan the tag on any task that pointed at this category.
        let touched: Vec<Task> = {
            let mut tasks = self.inner.tasks.lock().unwrap();
            tasks
                .values_mut()
                .filter(|e| e.task.category.as_deref() == Some(id))
                .map(|e| {
                    e.task.category = None;
                    e.task.updated_at = now_ms();
                    e.task.clone()
                })
                .collect()
        };
        for task in &touched {
            self.persist_emit(task);
        }
        self.categories()
    }

    /// Reorder categories by priority (position in `ids` = new order). Returns
    /// the reordered list.
    pub fn reorder_categories(&self, ids: Vec<String>) -> Vec<Category> {
        {
            let mut cats = self.inner.categories.lock().unwrap();
            for (idx, id) in ids.iter().enumerate() {
                if let Some(c) = cats.iter_mut().find(|c| &c.id == id) {
                    c.order = idx as i32;
                }
            }
            category::save(&cats, &self.inner.data_dir);
        }
        self.categories()
    }

    /// Move one or more downloads to `category` (`None` = uncategorized). The
    /// change-only behavior just re-tags them; move-file relocates each file into
    /// the category's folder, showing a `Moving` status while it copies.
    /// `default_dir` is the fallback folder for a category (or uncategorized) with
    /// no save-folder of its own.
    pub fn move_to_category(
        &self,
        ids: Vec<String>,
        category: Option<String>,
        default_dir: PathBuf,
    ) {
        // Validate the category id and resolve where its files live.
        let (category, target_dir) = {
            let cats = self.inner.categories.lock().unwrap();
            match category.and_then(|id| cats.iter().find(|c| c.id == id)) {
                Some(c) => {
                    let dir = c.save_dir.clone().map(PathBuf::from).unwrap_or(default_dir);
                    (Some(c.id.clone()), dir)
                }
                None => (None, default_dir),
            }
        };
        let behavior = self.inner.settings.lock().unwrap().category_change;

        for id in ids {
            if behavior == CategoryChangeBehavior::ChangeOnly {
                self.retag(&id, category.clone());
                continue;
            }
            // Move-file: relocate into the category folder. A running download is
            // paused first; the move begins when it stops (see `finish`).
            let start_now = {
                let mut tasks = self.inner.tasks.lock().unwrap();
                let Some(entry) = tasks.get_mut(&id) else {
                    continue;
                };
                if entry.moving {
                    continue; // already relocating
                }
                match &entry.control {
                    Some(control) => {
                        entry.pending_move = Some(PendingMove {
                            category: category.clone(),
                            target_dir: target_dir.clone(),
                        });
                        control.set(Signal::Pause);
                        false
                    }
                    None => true,
                }
            };
            if start_now {
                Inner::begin_move(self.inner.clone(), id, category.clone(), target_dir.clone());
            }
        }
    }

    /// Re-tag a download's category without touching its file (change-only mode).
    fn retag(&self, id: &str, category: Option<String>) {
        let task = {
            let mut tasks = self.inner.tasks.lock().unwrap();
            let Some(entry) = tasks.get_mut(id) else {
                return;
            };
            if entry.task.category == category {
                return;
            }
            entry.task.category = category;
            entry.task.updated_at = now_ms();
            entry.task.clone()
        };
        self.persist_emit(&task);
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
        // Resolve the backend before flipping the task to Connecting, so the record
        // of which backend ran (persisted below) reflects the one that actually
        // handles the transfer — including any fallback from the user's pick.
        let kind = {
            let tasks = inner.tasks.lock().unwrap();
            match tasks.get(&id) {
                Some(entry) if entry.control.is_none() => entry.task.kind,
                _ => return,
            }
        };
        let backend = inner.backend_for(kind);

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
            if let Some(b) = &backend {
                entry.task.backend = Some(b.id().to_string());
            }
            entry.task.updated_at = now_ms();
            (entry.task.clone(), control)
        };

        let _ = inner.store.lock().unwrap().upsert(&task);
        inner.emitter.updated(&task);

        let Some(backend) = backend else {
            Inner::finish(
                inner,
                id,
                Outcome::Failed("no backend is set up for this source".to_string()),
            );
            return;
        };

        let opts = {
            let s = inner.settings.lock().unwrap();
            TransferOpts {
                connections: s.connections,
                min_split_size: s.min_split_size,
                hide_part: s.hide_part_files,
                stall_timeout: Duration::from_secs(s.stall_timeout_secs),
            }
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
        // A remove/delete requested mid-download archives now; a category move
        // requested mid-download begins now — either way the task has stopped and
        // released its file handle. Otherwise settle on the transfer's outcome.
        enum Post {
            Archive(Task, bool),
            Move(PendingMove),
            Settle(Task),
        }

        let post = {
            let mut tasks = inner.tasks.lock().unwrap();
            let Some(entry) = tasks.get_mut(&id) else {
                return;
            };
            entry.control = None;
            if let Some(delete_file) = entry.pending_archive.take() {
                entry.pending_move = None; // a pending remove wins over a move
                entry.task.archived = true;
                entry.task.status = TaskStatus::Canceled;
                entry.task.updated_at = now_ms();
                Post::Archive(entry.task.clone(), delete_file)
            } else {
                // Settle on the real outcome first — even when a move is pending, so
                // a transfer that finished the instant we asked it to pause is
                // recorded as Completed and gets its finished file relocated (not a
                // phantom `.part`).
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
                    Outcome::Stalled => {
                        // Not an error — the partial is kept and it can resume.
                        entry.task.status = TaskStatus::Stalled;
                        entry.task.error = None;
                    }
                    Outcome::Failed(msg) => {
                        entry.task.status = TaskStatus::Failed;
                        entry.task.error = Some(msg);
                    }
                }
                entry.task.updated_at = now_ms();
                match entry.pending_move.take() {
                    // `begin_move` re-reads the just-settled status and flips it to
                    // Moving.
                    Some(pm) => Post::Move(pm),
                    None => Post::Settle(entry.task.clone()),
                }
            }
        };

        match post {
            Post::Archive(task, delete_file) => {
                purge_files(&task, delete_file);
                let _ = inner.store.lock().unwrap().upsert(&task);
                inner.emitter.updated(&task);
                Inner::pump(inner);
            }
            Post::Move(pm) => {
                Inner::begin_move(inner, id, pm.category, pm.target_dir);
            }
            Post::Settle(task) => {
                if task.status == TaskStatus::Canceled {
                    cleanup_partial(&task);
                }
                let _ = inner.store.lock().unwrap().upsert(&task);
                inner.emitter.updated(&task);
                Inner::pump(inner);
            }
        }
    }

    /// Kick off a relocation into `target_dir`, filing the download under
    /// `category` once it lands. If the file is already in `target_dir` this is
    /// just a re-tag; otherwise the task flips to `Moving` and a background task
    /// copies the bytes, reporting progress, then `finish_move` settles it.
    fn begin_move(
        inner: Arc<Inner>,
        id: String,
        category: Option<String>,
        target_dir: PathBuf,
    ) {
        // The category folder may not exist yet — surface a failure to create it
        // on the task and leave the file where it is.
        if let Err(e) = std::fs::create_dir_all(&target_dir) {
            let task = {
                let mut tasks = inner.tasks.lock().unwrap();
                let Some(entry) = tasks.get_mut(&id) else {
                    return;
                };
                entry.task.error = Some(format!("couldn't create the category folder: {e}"));
                entry.task.updated_at = now_ms();
                entry.task.clone()
            };
            let _ = inner.store.lock().unwrap().upsert(&task);
            inner.emitter.updated(&task);
            return;
        }

        enum Plan {
            Nothing,
            Retag(Task),
            Move(Box<MoveJob>, Task),
        }

        let plan = {
            let mut tasks = inner.tasks.lock().unwrap();
            if !tasks.contains_key(&id) {
                Plan::Nothing
            } else {
                // Destinations already claimed by other tasks, so the move can't
                // land on top of one.
                let taken: HashSet<String> = tasks
                    .values()
                    .filter(|e| e.task.id != id)
                    .map(|e| e.task.dest.clone())
                    .collect();
                let entry = tasks.get_mut(&id).unwrap();
                if entry.moving || entry.control.is_some() {
                    Plan::Nothing
                } else if Path::new(&entry.task.dest).parent() == Some(target_dir.as_path()) {
                    // Same folder — nothing to move, just re-tag.
                    entry.task.category = category;
                    entry.task.updated_at = now_ms();
                    Plan::Retag(entry.task.clone())
                } else {
                    let old_dest = entry.task.dest.clone();
                    let new_filename = unique_filename(&target_dir, &entry.task.filename, &taken);
                    let new_dest = target_dir
                        .join(&new_filename)
                        .to_string_lossy()
                        .into_owned();
                    let was_completed = entry.task.status == TaskStatus::Completed;
                    let total = entry.task.total;

                    entry.moving = true;
                    entry.task.status = TaskStatus::Moving;
                    entry.task.error = None;
                    entry.task.updated_at = now_ms();

                    let job = MoveJob {
                        id: id.clone(),
                        was_completed,
                        category,
                        old_part: format!("{old_dest}.part"),
                        old_meta: format!("{old_dest}.part.meta"),
                        old_dest,
                        new_part: format!("{new_dest}.part"),
                        new_meta: format!("{new_dest}.part.meta"),
                        new_dest,
                        new_filename,
                        total,
                    };
                    Plan::Move(Box::new(job), entry.task.clone())
                }
            }
        };

        match plan {
            Plan::Nothing => {}
            Plan::Retag(task) => {
                let _ = inner.store.lock().unwrap().upsert(&task);
                inner.emitter.updated(&task);
            }
            Plan::Move(job, task) => {
                let _ = inner.store.lock().unwrap().upsert(&task);
                inner.emitter.updated(&task);
                // Seed the bar at a determinate 0% right away: the wait before bytes
                // start moving (a rename settling, a handle releasing) then reads as
                // "starting" instead of an indeterminate slide.
                let payload = if job.was_completed {
                    &job.old_dest
                } else {
                    &job.old_part
                };
                let start_total = std::fs::metadata(payload)
                    .ok()
                    .map(|m| m.len())
                    .or(job.total);
                emit_move(&inner, &job.id, 0, start_total);
                tokio::spawn(async move {
                    let result = run_move(&inner, &job).await;
                    Inner::finish_move(inner, *job, result);
                });
            }
        }
    }

    /// Settle a finished relocation: on success the task now points at the new
    /// path under the new category and either returns to `Completed` or re-queues
    /// to resume; on failure it falls back to a resumable state with the reason. A
    /// remove requested mid-move is honored here.
    fn finish_move(inner: Arc<Inner>, job: MoveJob, result: Result<(), String>) {
        let mut resume = false;
        let (task, archive) = {
            let mut tasks = inner.tasks.lock().unwrap();
            let Some(entry) = tasks.get_mut(&job.id) else {
                return;
            };
            entry.moving = false;

            // On success the bytes now live at the new path under the new category.
            if result.is_ok() {
                entry.task.dest = job.new_dest.clone();
                entry.task.filename = job.new_filename.clone();
                entry.task.category = job.category.clone();
            }

            if let Some(delete_file) = entry.pending_archive.take() {
                entry.task.archived = true;
                entry.task.status = if job.was_completed {
                    TaskStatus::Completed
                } else {
                    TaskStatus::Canceled
                };
                entry.task.updated_at = now_ms();
                (entry.task.clone(), Some(delete_file))
            } else {
                match &result {
                    Ok(()) => {
                        entry.task.error = None;
                        if job.was_completed {
                            entry.task.status = TaskStatus::Completed;
                        } else {
                            entry.task.status = TaskStatus::Queued;
                            resume = true;
                        }
                    }
                    Err(msg) => {
                        // The file never moved; fall back to where it was.
                        entry.task.status = if job.was_completed {
                            TaskStatus::Completed
                        } else {
                            TaskStatus::Paused
                        };
                        entry.task.error = Some(msg.clone());
                    }
                }
                entry.task.updated_at = now_ms();
                (entry.task.clone(), None)
            }
        };

        if let Some(delete_file) = archive {
            purge_files(&task, delete_file);
        }
        let _ = inner.store.lock().unwrap().upsert(&task);
        inner.emitter.updated(&task);
        if resume {
            Inner::pump(inner);
        }
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

/// Relocate a task's files to their new home. The tiny resume sidecar moves
/// silently; the payload (the finished file, or the in-progress `.part`) moves
/// with progress so the UI's bar tracks the copy. A missing file is skipped.
async fn run_move(inner: &Arc<Inner>, job: &MoveJob) -> Result<(), String> {
    move_file(&job.old_meta, &job.new_meta)
        .await
        .map_err(|e| format!("couldn't move the resume data: {e}"))?;

    let (src, dst) = if job.was_completed {
        (&job.old_dest, &job.new_dest)
    } else {
        (&job.old_part, &job.new_part)
    };
    move_payload(inner, &job.id, src, dst, job.total).await
}

/// Move the main payload, reporting `Moving` progress as it goes. A same-volume
/// rename is instant (the bar jumps to full); a cross-volume move streams the
/// bytes so the bar fills as it copies. A missing source is treated as done.
async fn move_payload(
    inner: &Arc<Inner>,
    id: &str,
    src: &str,
    dst: &str,
    hint_total: Option<u64>,
) -> Result<(), String> {
    let size = tokio::fs::metadata(src).await.ok().map(|m| m.len());
    if size.is_none() {
        return Ok(()); // nothing on disk to move (e.g. a never-started task)
    }
    let total = size.or(hint_total);

    match rename_retry(src, dst).await {
        Ok(()) => {
            emit_move(inner, id, total.unwrap_or(0), total);
            Ok(())
        }
        // Cross-volume (or a rename the OS refused): stream the bytes across.
        Err(_) => {
            let inner2 = inner.clone();
            let id2 = id.to_string();
            copy_across(src, dst, move |moved, total| {
                emit_move(&inner2, &id2, moved, Some(total));
            })
            .await
            .map_err(|e| format!("couldn't move the file: {e}"))
        }
    }
}

/// Move `src` onto `dst` when `src` exists (a missing source is a no-op), trying
/// a fast rename before falling back to a cross-volume copy.
async fn move_file(src: &str, dst: &str) -> std::io::Result<()> {
    if tokio::fs::metadata(src).await.is_err() {
        return Ok(());
    }
    match rename_retry(src, dst).await {
        Ok(()) => Ok(()),
        Err(_) => copy_across(src, dst, |_, _| {}).await,
    }
}

/// Rename `src` to `dst`, retrying briefly: a file handle the OS just released can
/// linger for a moment on Windows, so a rename right after a pause can fail once.
/// A cross-device error can't be retried away, so it returns straight to the
/// caller's copy fallback.
async fn rename_retry(src: &str, dst: &str) -> std::io::Result<()> {
    let mut last: Option<std::io::Error> = None;
    for attempt in 0u32..5 {
        match tokio::fs::rename(src, dst).await {
            Ok(()) => return Ok(()),
            Err(e) if is_cross_device(&e) => return Err(e),
            Err(e) => {
                last = Some(e);
                tokio::time::sleep(Duration::from_millis(60 * u64::from(attempt) + 40)).await;
            }
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("rename failed")))
}

/// Stream `src` to `dst` then remove the original, calling `on_progress(moved,
/// total)` as it copies — the cross-volume path where a rename won't do.
async fn copy_across(
    src: &str,
    dst: &str,
    on_progress: impl Fn(u64, u64),
) -> std::io::Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut reader = open_retry(src).await?;
    let total = reader.metadata().await.map(|m| m.len()).unwrap_or(0);
    let mut writer = tokio::fs::File::create(dst).await?;
    let mut buf = vec![0u8; 1 << 20];
    let mut moved = 0u64;
    let mut last = Instant::now();
    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n]).await?;
        moved += n as u64;
        if last.elapsed() >= Duration::from_millis(120) {
            on_progress(moved, total);
            last = Instant::now();
        }
    }
    writer.flush().await?;
    drop(writer);
    drop(reader);
    remove_retry(src).await?;
    on_progress(total, total);
    Ok(())
}

/// Open `path` for reading, retrying through the brief post-pause handle lag.
async fn open_retry(path: &str) -> std::io::Result<tokio::fs::File> {
    let mut last: Option<std::io::Error> = None;
    for attempt in 0u32..5 {
        match tokio::fs::File::open(path).await {
            Ok(f) => return Ok(f),
            Err(e) => {
                last = Some(e);
                tokio::time::sleep(Duration::from_millis(60 * u64::from(attempt) + 40)).await;
            }
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("open failed")))
}

/// Remove `path`, retrying the same way (and treating "already gone" as success).
async fn remove_retry(path: &str) -> std::io::Result<()> {
    for attempt in 0u32..5 {
        match tokio::fs::remove_file(path).await {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) if attempt == 4 => return Err(e),
            Err(_) => {
                tokio::time::sleep(Duration::from_millis(60 * u64::from(attempt) + 40)).await;
            }
        }
    }
    Ok(())
}

/// Whether a rename failed because source and destination are on different
/// volumes — the one error a retry can't fix, so the caller copies instead.
fn is_cross_device(e: &std::io::Error) -> bool {
    #[cfg(windows)]
    {
        e.raw_os_error() == Some(17) // ERROR_NOT_SAME_DEVICE
    }
    #[cfg(not(windows))]
    {
        e.raw_os_error() == Some(18) // EXDEV
    }
}

/// The network config the backends' clients should use, drawn from settings.
fn net_config(s: &Settings) -> NetConfig {
    NetConfig {
        connect_timeout: (s.connect_timeout_secs > 0)
            .then(|| Duration::from_secs(s.connect_timeout_secs)),
    }
}

/// Emit a `Moving` progress tick so the card's bar tracks the relocation.
fn emit_move(inner: &Arc<Inner>, id: &str, moved: u64, total: Option<u64>) {
    inner.emitter.progress(&TaskProgress {
        id: id.to_string(),
        received: moved,
        total,
        speed: 0,
        status: TaskStatus::Moving,
    });
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
