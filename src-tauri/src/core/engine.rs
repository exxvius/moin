//! The supervisor: owns the task registry, the queue, persistence, and backend
//! selection. It's Tauri-free — it reports out through the [`Emitter`] trait,
//! which the shell implements with `AppHandle::emit`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use uuid::Uuid;

use super::aria2::Aria2Backend;
use super::backend::{
    BackendInfo, Control, DownloadBackend, NetConfig, Outcome, ProgressFn, Signal, TorrentNet,
    TorrentTick, TransferOpts,
};
use super::category::{self, Candidate, Category};
use super::embedded::EmbeddedBackend;
use super::settings::{CategoryChangeBehavior, Settings};
use super::store::Store;
use super::task::{
    filename_from_url, now_ms, sanitize_filename, Task, TaskKind, TaskProgress, TaskStatus,
    TorrentDetails, TorrentPreview,
};
use super::tool::{Aria2Tool, ToolStatus};
use super::torrent::{self, parse_meta};

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
        let mut settings = Settings::load(&data_dir);
        // Mint the RPC bearer token on first run so the browser-integration server
        // always has one to check against (and the settings UI something to show).
        if settings.rpc_token.is_empty() {
            settings.rpc_token = Uuid::new_v4().to_string();
            settings.save(&data_dir);
        }
        let categories = category::load_or_seed(&data_dir);

        let mut tasks = HashMap::new();
        for mut task in store.all()? {
            let torrent = task.is_torrent();
            if torrent
                && matches!(
                    task.status,
                    TaskStatus::Connecting
                        | TaskStatus::Checking
                        | TaskStatus::Downloading
                        | TaskStatus::Seeding
                )
            {
                // A torrent that was downloading or seeding at last exit resumes on
                // its own: re-queued so the run loop re-adds it to the session, where
                // librqbit fastresume (or aria2's saved control file) carries it on
                // from the pieces already on disk. A torrent the user had paused
                // stays paused. (The store already zeroes swarm readings on load.)
                task.status = TaskStatus::Queued;
            } else if matches!(
                task.status,
                TaskStatus::Connecting
                    | TaskStatus::Checking
                    | TaskStatus::Downloading
                    | TaskStatus::Moving
            ) {
                // A direct download (or a torrent interrupted mid-move) comes back
                // paused — we never silently resume a plain download the user didn't
                // restart, and a move's file is still at its recorded path.
                task.status = TaskStatus::Paused;
            } else if task.status == TaskStatus::Seeding {
                // A non-torrent can't seed; settle any stray record as done.
                task.status = TaskStatus::Completed;
            }
            tasks.insert(task.id.clone(), Entry::idle(task));
        }

        sweep_orphan_parts(tasks.values().map(|e| &e.task));

        let tool = Arc::new(Aria2Tool::new(
            data_dir.clone(),
            settings.aria2_path.clone(),
        ));
        let backends: Vec<Arc<dyn DownloadBackend>> = vec![
            Arc::new(EmbeddedBackend::new(data_dir.clone())),
            Arc::new(Aria2Backend::new(tool.clone(), data_dir.clone())),
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
        headers: BTreeMap<String, String>,
        filename: Option<String>,
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
        // A caller-supplied name (e.g. from a browser capture) wins when it's
        // usable; otherwise fall back to guessing from the URL.
        let base = filename
            .as_deref()
            .and_then(sanitize_filename)
            .unwrap_or_else(|| filename_from_url(&url));
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
                headers,
                info_hash: None,
                torrent_source: None,
                files: Vec::new(),
                uploaded: 0,
                seeders: 0,
                leechers: 0,
                peers: 0,
                up_speed: 0,
                own_dir: false,
                force_seed: false,
            };
            tasks.insert(task.id.clone(), Entry::idle(task.clone()));
            task
        };

        self.inner.store.lock().unwrap().upsert(&task)?;
        self.inner.emitter.added(&task);
        Inner::pump(self.inner.clone());
        Ok(task)
    }

    /// Resolve a torrent's metadata (its file list) so the add-torrent modal can
    /// show a file picker, plus the category + folder the download would default
    /// to. The resolved `.torrent` is cached, so the eventual add is cheap.
    pub async fn prepare_torrent(
        &self,
        source: String,
        dir: PathBuf,
    ) -> Result<TorrentPreview, String> {
        let source = source.trim().to_string();
        if source.is_empty() {
            return Err("no torrent given".to_string());
        }
        // Resolution + the `.torrent` cache are backend-independent, so the built-in
        // engine always does them — aria2 then downloads from the cached metadata.
        let resolved = self
            .inner
            .embedded()
            .resolve_torrent(&source)
            .await
            .ok_or_else(|| "couldn't resolve the torrent".to_string())??;

        // Suggest a category from the torrent name, then the folder it implies.
        let name = resolved.name.clone().unwrap_or_default();
        let suggested = {
            let cand = Candidate::from_url_named(
                &source,
                category::AddMethodKind::ManualTorrent,
                Some(&name),
            );
            category::categorize(&cand, &self.inner.categories.lock().unwrap())
        };
        let default_dir = {
            let cats = self.inner.categories.lock().unwrap();
            suggested
                .as_ref()
                .and_then(|id| cats.iter().find(|c| c.id == *id))
                .and_then(|c| c.save_dir.clone())
                .unwrap_or_else(|| dir.to_string_lossy().into_owned())
        };

        Ok(TorrentPreview {
            resolved,
            suggested_category: suggested,
            default_dir,
        })
    }

    /// Live detail (files, peers, trackers) for a torrent task — what the expanded
    /// card polls while it's open.
    pub async fn torrent_details(&self, id: &str) -> Result<TorrentDetails, String> {
        let info_hash = {
            let tasks = self.inner.tasks.lock().unwrap();
            let entry = tasks
                .get(id)
                .ok_or_else(|| "no such download".to_string())?;
            entry
                .task
                .info_hash
                .clone()
                .ok_or_else(|| "not a torrent".to_string())?
        };
        let backend = self
            .inner
            .backend_for(TaskKind::Torrent)
            .ok_or_else(|| "no torrent backend is available".to_string())?;
        backend
            .torrent_details(&info_hash)
            .await
            .ok_or_else(|| "live torrent detail isn't available for this engine".to_string())?
    }

    /// Change which files a torrent downloads (`selected` = indices to keep),
    /// applied live to the running torrent and saved to the task.
    pub async fn set_torrent_files(&self, id: &str, selected: Vec<usize>) -> Result<(), String> {
        let info_hash = {
            let tasks = self.inner.tasks.lock().unwrap();
            let entry = tasks
                .get(id)
                .ok_or_else(|| "no such download".to_string())?;
            entry
                .task
                .info_hash
                .clone()
                .ok_or_else(|| "not a torrent".to_string())?
        };
        let backend = self
            .inner
            .backend_for(TaskKind::Torrent)
            .ok_or_else(|| "no torrent backend is available".to_string())?;
        backend
            .set_torrent_files(&info_hash, &selected)
            .await
            .ok_or_else(|| "changing files live isn't supported by this engine".to_string())??;

        // Reflect the new selection on the task. We deliberately do NOT delete a
        // deselected file from disk: librqbit tracks piece completion internally,
        // so removing a file behind its back leaves those pieces marked "have" —
        // the file then shows 100% but is gone, and re-selecting never re-downloads
        // it. Excluding via `only_files` (above) is the safe operation; a file that
        // was never downloaded simply isn't fetched, and a downloaded one keeps its
        // data (matching qBittorrent's "don't download" behavior).
        let (task, requeue) = {
            let mut tasks = self.inner.tasks.lock().unwrap();
            let Some(entry) = tasks.get_mut(id) else {
                return Ok(());
            };
            // Did the user newly include a file that wasn't selected before?
            let added = entry
                .task
                .files
                .iter()
                .enumerate()
                .any(|(i, f)| !f.selected && selected.contains(&i));
            for (i, f) in entry.task.files.iter_mut().enumerate() {
                f.selected = selected.contains(&i);
            }
            let total: u64 = entry
                .task
                .files
                .iter()
                .filter(|f| f.selected)
                .map(|f| f.size)
                .sum();
            entry.task.total = (total > 0).then_some(total);

            // If files were added while the torrent isn't running (it finished, or
            // is paused), it needs to fetch the newly-wanted pieces — re-queue it so
            // the download loop restarts with the updated selection. A running
            // torrent already picks up the change on its next poll.
            let requeue = added && entry.control.is_none() && !entry.moving;
            if requeue {
                entry.task.status = TaskStatus::Queued;
                entry.task.completed_at = None;
                entry.task.error = None;
            }
            entry.task.updated_at = now_ms();
            (entry.task.clone(), requeue)
        };
        self.persist_emit(&task);
        if requeue {
            Inner::pump(self.inner.clone());
        }
        Ok(())
    }

    /// Add a torrent to the queue with the user's chosen folder, category, and
    /// file selection (indices to include; empty = all). `dir` is the output
    /// *folder* — librqbit manages its own partials + fastresume inside it. The
    /// file list is read from the `.torrent` cached during [`Self::prepare_torrent`].
    pub fn add_torrent(
        &self,
        source: String,
        dir: PathBuf,
        category: Option<String>,
        selected: Vec<usize>,
        folder: Option<String>,
        renames: Vec<String>,
    ) -> Result<Task, String> {
        let source = source.trim().to_string();
        let meta = parse_meta(&source)?;

        // A real display name if the source carried one, else the info hash, else
        // a plain fallback — enough for the card to show something immediately.
        let display = meta
            .name
            .as_deref()
            .and_then(sanitize_filename)
            .or_else(|| meta.info_hash.clone())
            .unwrap_or_else(|| "torrent".to_string());

        // Content layout: `Some(name)` nests the files under that folder (its name
        // may have been renamed in the modal); `None` saves them directly in the
        // chosen folder. `dest` is the actual output folder either way, so the file
        // paths (`dest`/`file.path`) stay correct for progress + deletion.
        let own_dir = folder.is_some();
        let dir = match folder.as_deref().and_then(sanitize_filename) {
            Some(name) => dir.join(name),
            None => dir,
        };

        // Load the file list from the cached .torrent, apply the selection, and
        // apply any per-file renames (index-aligned relative paths; blank keeps
        // the original). An empty selection means "all files".
        let files = meta
            .info_hash
            .as_deref()
            .map(|h| torrent::meta_path(&self.inner.data_dir, h))
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| torrent::files_from_bytes(&b).ok())
            .unwrap_or_default();
        let files: Vec<_> = files
            .into_iter()
            .enumerate()
            .map(|(i, mut f)| {
                if !selected.is_empty() {
                    f.selected = selected.contains(&i);
                }
                if let Some(path) = renames.get(i) {
                    if !path.trim().is_empty() {
                        f.path = path.clone();
                    }
                }
                f
            })
            .collect();
        let total: u64 = files.iter().filter(|f| f.selected).map(|f| f.size).sum();
        let total = (total > 0).then_some(total);

        // The folder is the caller's explicit choice (the modal pre-filled it from
        // the category's folder but let the user override), so use it as-is — just
        // validate the category id still exists.
        let category = {
            let cats = self.inner.categories.lock().unwrap();
            category.filter(|id| cats.iter().any(|c| c.id == *id))
        };

        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let dest = dir.to_string_lossy().into_owned();
        let now = now_ms();

        let task = {
            let mut tasks = self.inner.tasks.lock().unwrap();
            let task = Task {
                id: Uuid::new_v4().to_string(),
                kind: TaskKind::Torrent,
                url: source,
                filename: display,
                dest,
                status: TaskStatus::Queued,
                total,
                received: 0,
                error: None,
                created_at: now,
                updated_at: now,
                completed_at: None,
                archived: false,
                active_ms: 0,
                backend: None,
                category,
                headers: BTreeMap::new(),
                info_hash: meta.info_hash,
                torrent_source: Some(meta.source),
                files,
                uploaded: 0,
                seeders: 0,
                leechers: 0,
                peers: 0,
                up_speed: 0,
                own_dir,
                force_seed: false,
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

    /// Kick the queue once at startup so torrents that were downloading or seeding
    /// when moin last closed pick back up (their status was set to `Queued` while
    /// loading). Must be called from within the tokio runtime — pump spawns the
    /// run tasks. HTTP downloads stay paused, so they aren't affected.
    pub fn resume_pending(&self) {
        Inner::pump(self.inner.clone());
    }

    /// Cleanly stop the download engines before the app exits: pause active
    /// transfers so their resume state flushes, and shut the aria2 daemon down so
    /// it saves its control files and doesn't linger. Best-effort — a hard kill
    /// still resumes from the periodically-saved state, this just tightens it.
    pub async fn shutdown(&self) {
        let backends = self.inner.backends.clone();
        for b in backends {
            b.shutdown().await;
        }
    }

    pub fn resume(&self, id: &str) {
        let task = {
            let mut tasks = self.inner.tasks.lock().unwrap();
            let Some(entry) = tasks.get_mut(id) else {
                return;
            };
            if entry.moving || entry.control.is_some() || entry.task.status == TaskStatus::Completed
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

    /// Keep seeding a finished torrent past the ratio/time limit. Re-runs a
    /// stopped (Completed) torrent in force-seed mode, so the auto-stop is lifted
    /// for that session and it seeds until stopped by hand. No-op for a
    /// non-torrent, a busy task, or one that's already running.
    pub fn start_seeding(&self, id: &str) {
        let task = {
            let mut tasks = self.inner.tasks.lock().unwrap();
            let Some(entry) = tasks.get_mut(id) else {
                return;
            };
            if !entry.task.is_torrent() || entry.moving || entry.control.is_some() {
                return;
            }
            entry.task.force_seed = true;
            entry.task.status = TaskStatus::Queued;
            entry.task.error = None;
            entry.task.updated_at = now_ms();
            entry.task.clone()
        };
        self.persist_emit(&task);
        Inner::pump(self.inner.clone());
    }

    pub async fn cancel(&self, id: &str) {
        // A torrent is detached from the session first (releasing its file handles);
        // a canceled download keeps nothing, so its partial data is dropped after.
        let torrent_hash = {
            let tasks = self.inner.tasks.lock().unwrap();
            tasks
                .get(id)
                .filter(|e| e.task.is_torrent())
                .and_then(|e| e.task.info_hash.clone())
        };
        if let Some(hash) = torrent_hash {
            let _ = self.remove_torrent_from_session(&hash).await;
        }

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
        // A canceled torrent discards its partial data; HTTP drops its `.part`.
        purge_files(&task, true);
        self.persist_emit(&task);
        Inner::pump(self.inner.clone());
    }

    /// Remove from the list: archive the record (kept for stats), delete the
    /// partial file, and leave any finished file on disk.
    pub async fn remove(&self, id: &str) -> Result<(), String> {
        self.archive(id, false).await
    }

    /// Remove from the list AND delete the downloaded file from disk. Fails (and
    /// leaves the download in the list) if the file can't be deleted.
    pub async fn delete(&self, id: &str) -> Result<(), String> {
        self.archive(id, true).await
    }

    /// Ask the torrent backend to detach a torrent from its session, releasing the
    /// file handles. Files are always kept — moin deletes data itself.
    async fn remove_torrent_from_session(&self, info_hash: &str) -> Result<(), String> {
        // The built-in engine owns the cached `.torrent` and the librqbit session,
        // so it does the detach/cache-drop even when aria2 ran the download (whose
        // own transfer is already stopped by the run loop's cancel).
        self.inner
            .embedded()
            .remove_torrent(info_hash)
            .await
            .unwrap_or(Ok(()))
    }

    async fn archive(&self, id: &str, delete_file: bool) -> Result<(), String> {
        // A torrent is detached from librqbit first (it holds the file handles);
        // moin then deletes only this torrent's own files — never a shared folder.
        let torrent = {
            let tasks = self.inner.tasks.lock().unwrap();
            tasks
                .get(id)
                .filter(|e| e.task.is_torrent())
                .map(|e| (e.task.info_hash.clone(), e.control.is_some()))
        };
        if let Some((hash, running)) = torrent {
            if let Some(hash) = hash {
                self.remove_torrent_from_session(&hash).await?;
            }
            // Running: the loop notices the torrent is gone and stops; `finish`
            // archives the record and purges its files. Otherwise do it here.
            let mut tasks = self.inner.tasks.lock().unwrap();
            let Some(entry) = tasks.get_mut(id) else {
                return Ok(());
            };
            if running {
                entry.pending_archive = Some(delete_file);
                if let Some(control) = &entry.control {
                    control.set(Signal::Cancel);
                }
                return Ok(());
            }
            let task = entry.task.clone();
            drop(tasks);

            // Delete the files first so a failure (a file/folder open elsewhere)
            // holds the download in the list — nothing is archived, and the user
            // can free it and retry.
            if delete_file {
                delete_torrent_files(&task)
                    .map_err(|e| format!("Couldn't delete the files: {e}. The download is still in your list — free the files (e.g. close a window that's in the folder) and try again."))?;
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
            let _ = self.inner.store.lock().unwrap().upsert(&task);
            self.inner.emitter.updated(&task);
            Inner::pump(self.inner.clone());
            return Ok(());
        }

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

    /// Mint a fresh RPC bearer token, persist it, and return it. The server reads
    /// the token live, so a previously-paired extension stops working the moment
    /// this returns — the point of a regenerate.
    pub fn regenerate_rpc_token(&self) -> String {
        let token = Uuid::new_v4().to_string();
        let mut s = self.inner.settings.lock().unwrap();
        s.rpc_token = token.clone();
        s.save(&self.inner.data_dir);
        token
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

    /// The category a browser-captured `url` should be filed under, if any. The
    /// extension doesn't pick one, so the engine runs the same rules the manual
    /// add does — tagged as a browser capture so source-filtered categories treat
    /// it correctly. A known `filename` (from the capture) feeds the name/extension
    /// triggers, so a URL with no obvious extension still categorizes.
    pub fn categorize_capture(&self, url: &str, filename: Option<&str>) -> Option<String> {
        use super::category::AddMethodKind;
        let cand = Candidate::from_url_named(url.trim(), AddMethodKind::BrowserCapture, filename);
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

    /// The folder a download would save into under `category`: the category's
    /// `save_dir` override if it sets one, else `default_dir`. Mirrors the
    /// resolution in [`Self::add_http`] / [`Self::move_to_category`] so the
    /// add-torrent modal can pre-fill the right folder as the category changes.
    pub fn category_folder(&self, category: Option<String>, default_dir: PathBuf) -> String {
        let cats = self.inner.categories.lock().unwrap();
        let dir = category
            .and_then(|id| cats.iter().find(|c| c.id == id).cloned())
            .and_then(|c| c.save_dir)
            .map(PathBuf::from)
            .unwrap_or(default_dir);
        dir.to_string_lossy().into_owned()
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
    /// The built-in engine, always registered. It's the canonical torrent
    /// metadata resolver + owner of the cached `.torrent` files, so metadata and
    /// cache-cleanup ops route here regardless of which engine downloads.
    fn embedded(&self) -> Arc<dyn DownloadBackend> {
        self.backends
            .iter()
            .find(|b| b.id() == "embedded")
            .cloned()
            .expect("the built-in backend is always registered")
    }

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
        // A seeding torrent keeps its run loop alive but isn't using a download
        // slot, so it mustn't count against the concurrency limit.
        self.tasks
            .lock()
            .unwrap()
            .values()
            .filter(|e| e.control.is_some() && e.task.status != TaskStatus::Seeding)
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
            // A force-seed run (the user chose to keep seeding past the limit)
            // ignores the auto-stop; a normal run honors the configured limits.
            let (seed_ratio_limit, seed_time_limit) = if task.force_seed {
                (0.0, Duration::ZERO)
            } else {
                (
                    s.seed_ratio_limit,
                    Duration::from_secs(s.seed_time_limit_mins.saturating_mul(60)),
                )
            };
            TransferOpts {
                connections: s.connections,
                min_split_size: s.min_split_size,
                hide_part: s.hide_part_files,
                stall_timeout: Duration::from_secs(s.stall_timeout_secs),
                seed_ratio_limit,
                seed_time_limit,
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

        Arc::new(
            move |received: u64, total: Option<u64>, torrent: Option<TorrentTick>| {
                let now = Instant::now();

                // Snapshot the task + decide what to do while briefly holding locks.
                let (task, newly_started, newly_seeding, do_emit, do_persist, speed) = {
                    let mut st = state.lock().unwrap();
                    let mut tasks = inner.tasks.lock().unwrap();
                    let Some(entry) = tasks.get_mut(&id) else {
                        return;
                    };
                    let prev_status = entry.task.status;
                    entry.task.received = received;
                    if let Some(t) = total {
                        entry.task.total = Some(t);
                    }
                    // Fold in the live torrent readings (upload, peers, swarm).
                    if let Some(tick) = torrent {
                        entry.task.uploaded = tick.uploaded;
                        entry.task.up_speed = tick.up_speed;
                        entry.task.peers = tick.peers;
                        entry.task.seeders = tick.seeders;
                        entry.task.leechers = tick.leechers;
                    }
                    // A torrent reports its own phase (Checking/Downloading/Seeding);
                    // HTTP is always Downloading while bytes flow.
                    let status = torrent.map(|t| t.status).unwrap_or(TaskStatus::Downloading);

                    let newly_started = !st.started;
                    if newly_started {
                        st.started = true;
                        st.last_tick = now;
                        entry.task.updated_at = now_ms();
                    } else {
                        // Active time only accrues while actually downloading.
                        if status == TaskStatus::Downloading {
                            entry.task.active_ms +=
                                now.duration_since(st.last_tick).as_millis() as i64;
                        }
                        st.last_tick = now;
                    }
                    if entry.task.status != status {
                        entry.task.status = status;
                        entry.task.updated_at = now_ms();
                    }
                    // Record the completion time the first time it reaches seeding.
                    if status == TaskStatus::Seeding && entry.task.completed_at.is_none() {
                        entry.task.completed_at = Some(now_ms());
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

                    let newly_seeding =
                        status == TaskStatus::Seeding && prev_status != TaskStatus::Seeding;
                    (
                        entry.task.clone(),
                        newly_started,
                        newly_seeding,
                        do_emit,
                        do_persist,
                        st.speed,
                    )
                };

                if newly_started {
                    inner.emitter.updated(&task);
                }
                if newly_seeding {
                    // Freed a download slot + it's a meaningful status change — push
                    // a full update and let the next queued download start.
                    inner.emitter.updated(&task);
                    let _ = inner.store.lock().unwrap().upsert(&task);
                    Inner::pump(inner.clone());
                }
                if do_emit {
                    inner.emitter.progress(&TaskProgress {
                        id: id.clone(),
                        received,
                        total,
                        speed: speed as u64,
                        status: task.status,
                        up_speed: task.up_speed,
                        uploaded: task.uploaded,
                        peers: task.peers,
                        seeders: task.seeders,
                        leechers: task.leechers,
                    });
                }
                if do_persist {
                    let _ = inner.store.lock().unwrap().upsert(&task);
                }
            },
        )
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
                // The run has ended, so the live-only swarm readings are stale —
                // zero them so a paused/stopped torrent doesn't keep showing peers
                // or seeders that the detail panel (reading the session live) can't.
                entry.task.peers = 0;
                entry.task.seeders = 0;
                entry.task.leechers = 0;
                entry.task.up_speed = 0;
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
                    // Canceled discards partial data — a torrent's own files, or an
                    // HTTP `.part` (torrent-aware via `purge_files`).
                    purge_files(&task, true);
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
    fn begin_move(inner: Arc<Inner>, id: String, category: Option<String>, target_dir: PathBuf) {
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

        // A torrent is a folder of files managed by librqbit — relocating it is a
        // detach + folder move + re-add, not a single-file rename.
        let is_torrent = inner
            .tasks
            .lock()
            .unwrap()
            .get(&id)
            .map(|e| e.task.is_torrent())
            .unwrap_or(false);
        if is_torrent {
            Inner::begin_torrent_move(inner, id, category, target_dir);
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

    /// Relocate a torrent into another category's folder: detach it from the
    /// session, move only its own files to the new home, then re-queue so it
    /// re-adds there (librqbit verifies the moved files and resumes/seeds).
    fn begin_torrent_move(
        inner: Arc<Inner>,
        id: String,
        category: Option<String>,
        target_dir: PathBuf,
    ) {
        enum Plan {
            Nothing,
            Retag(Task),
            Move {
                info_hash: Option<String>,
                old_dest: String,
                new_dest: String,
                files: Vec<crate::core::task::TorrentFile>,
                own_dir: bool,
                category: Option<String>,
                task: Task,
            },
        }

        let plan = {
            let mut tasks = inner.tasks.lock().unwrap();
            match tasks.get_mut(&id) {
                None => Plan::Nothing,
                Some(entry) if entry.moving || entry.control.is_some() => Plan::Nothing,
                Some(entry) => {
                    let new_dest = if entry.task.own_dir {
                        target_dir.join(&entry.task.filename)
                    } else {
                        target_dir.clone()
                    }
                    .to_string_lossy()
                    .into_owned();
                    if new_dest == entry.task.dest {
                        entry.task.category = category;
                        entry.task.updated_at = now_ms();
                        Plan::Retag(entry.task.clone())
                    } else {
                        let old_dest = entry.task.dest.clone();
                        let files = entry.task.files.clone();
                        let info_hash = entry.task.info_hash.clone();
                        let own_dir = entry.task.own_dir;
                        entry.moving = true;
                        entry.task.status = TaskStatus::Moving;
                        entry.task.error = None;
                        entry.task.updated_at = now_ms();
                        Plan::Move {
                            info_hash,
                            old_dest,
                            new_dest,
                            files,
                            own_dir,
                            category,
                            task: entry.task.clone(),
                        }
                    }
                }
            }
        };

        match plan {
            Plan::Nothing => {}
            Plan::Retag(task) => {
                let _ = inner.store.lock().unwrap().upsert(&task);
                inner.emitter.updated(&task);
            }
            Plan::Move {
                info_hash,
                old_dest,
                new_dest,
                files,
                own_dir,
                category,
                task,
            } => {
                let _ = inner.store.lock().unwrap().upsert(&task);
                inner.emitter.updated(&task);
                emit_move(&inner, &id, 0, task.total);
                tokio::spawn(async move {
                    // Detach from the session so the files aren't held open (the
                    // built-in engine owns the session + cached metadata).
                    if let Some(hash) = &info_hash {
                        let _ = inner.embedded().remove_torrent(hash).await;
                    }
                    let result =
                        move_torrent_files(old_dest, new_dest.clone(), files, own_dir).await;

                    let task = {
                        let mut tasks = inner.tasks.lock().unwrap();
                        let Some(entry) = tasks.get_mut(&id) else {
                            return;
                        };
                        entry.moving = false;
                        match &result {
                            Ok(()) => {
                                entry.task.dest = new_dest;
                                entry.task.category = category;
                                entry.task.error = None;
                                // Re-queue so it re-adds at the new folder + resumes.
                                entry.task.status = TaskStatus::Queued;
                                entry.task.completed_at = None;
                            }
                            Err(e) => {
                                entry.task.status = TaskStatus::Paused;
                                entry.task.error = Some(e.clone());
                            }
                        }
                        entry.task.updated_at = now_ms();
                        entry.task.clone()
                    };
                    let _ = inner.store.lock().unwrap().upsert(&task);
                    inner.emitter.updated(&task);
                    Inner::pump(inner);
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
    if task.is_torrent() {
        if delete_file {
            // Best-effort on the deferred (finish/cancel) path.
            let _ = delete_torrent_files(task);
        }
        return;
    }
    cleanup_partial(task);
    if delete_file {
        cleanup_file(task.dest.clone());
    }
}

/// Delete a torrent's own files (and only those). We remove each file under
/// `dest`, then any now-empty sub-directory the torrent created, and finally the
/// output folder itself *only* when the torrent owns it (the "create subfolder"
/// layout). A folder saved into directly (which may hold other downloads) is
/// never removed. Returns an error if something couldn't be removed (e.g. a file
/// or the folder is open in another program) so the caller can hold the delete.
fn delete_torrent_files(task: &Task) -> Result<(), String> {
    let base = Path::new(&task.dest);
    let mut failed: Option<String> = None;
    for f in &task.files {
        let path = base.join(&f.path);
        if let Err(e) = remove_if_exists(&path.to_string_lossy()) {
            failed.get_or_insert_with(|| e.to_string());
        }
    }
    // Empty sub-directories, deepest first so nested ones clear before parents.
    let mut dirs: Vec<PathBuf> = task
        .files
        .iter()
        .filter_map(|f| Path::new(&f.path).parent())
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| base.join(p))
        .collect();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    dirs.dedup();
    for d in dirs {
        let _ = std::fs::remove_dir(d); // only removes empties; leftovers surface below
    }
    // Our own content folder: surface a failure (it's likely open elsewhere) so the
    // delete can be retried after the user frees it. `remove_dir` also fails on a
    // non-empty folder, which catches any file that couldn't be deleted above.
    if task.own_dir && base.exists() {
        if let Err(e) = std::fs::remove_dir(base) {
            failed.get_or_insert_with(|| e.to_string());
        }
    }
    match failed {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Drop a task's in-progress artifacts: the `.part` file and the multi-connection
/// `.meta` resume sidecar (absent for single-stream downloads — a no-op then). A
/// torrent has no `.part`/`.meta` — librqbit manages its own files, so skip it.
fn cleanup_partial(task: &Task) {
    if task.is_torrent() {
        return;
    }
    cleanup_file(task.part_path());
    cleanup_file(task.meta_path());
}

/// Move a torrent's own files from `old_dest` to `new_dest`, off the async
/// runtime. In the "create subfolder" layout the whole folder moves; otherwise
/// each file is moved individually (leaving a shared folder's other files, and
/// the folder itself, untouched). Same-drive moves rename; cross-drive copy.
async fn move_torrent_files(
    old_dest: String,
    new_dest: String,
    files: Vec<crate::core::task::TorrentFile>,
    own_dir: bool,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        let old = Path::new(&old_dest);
        let new = Path::new(&new_dest);
        if own_dir {
            if let Some(parent) = new.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            if std::fs::rename(old, new).is_ok() {
                return Ok(());
            }
            copy_dir_all(old, new).map_err(|e| e.to_string())?;
            let _ = std::fs::remove_dir_all(old);
            Ok(())
        } else {
            for f in &files {
                let src = old.join(&f.path);
                if !src.exists() {
                    continue;
                }
                let dst = new.join(&f.path);
                if let Some(parent) = dst.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                if std::fs::rename(&src, &dst).is_err() {
                    std::fs::copy(&src, &dst).map_err(|e| e.to_string())?;
                    let _ = std::fs::remove_file(&src);
                }
            }
            // Clear now-empty source sub-directories (never the shared root).
            let mut dirs: Vec<PathBuf> = files
                .iter()
                .filter_map(|f| Path::new(&f.path).parent())
                .filter(|p| !p.as_os_str().is_empty())
                .map(|p| old.join(p))
                .collect();
            dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
            dirs.dedup();
            for d in dirs {
                let _ = std::fs::remove_dir(d);
            }
            Ok(())
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Recursively copy a directory tree.
fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
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
async fn copy_across(src: &str, dst: &str, on_progress: impl Fn(u64, u64)) -> std::io::Result<()> {
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
        torrent: TorrentNet {
            listen_port: s.torrent_listen_port,
            dht: s.torrent_dht,
            upnp: s.torrent_upnp,
            download_bps: bps(s.torrent_download_limit),
            upload_bps: bps(s.torrent_upload_limit),
        },
    }
}

/// A bytes-per-second rate limit as librqbit wants it: `None` for 0/unlimited,
/// else the value clamped into a `NonZeroU32` (u32 caps out around 4 GB/s).
fn bps(bytes_per_sec: u64) -> Option<std::num::NonZeroU32> {
    std::num::NonZeroU32::new(bytes_per_sec.min(u32::MAX as u64) as u32)
}

/// Emit a `Moving` progress tick so the card's bar tracks the relocation.
fn emit_move(inner: &Arc<Inner>, id: &str, moved: u64, total: Option<u64>) {
    inner.emitter.progress(&TaskProgress {
        id: id.to_string(),
        received: moved,
        total,
        speed: 0,
        status: TaskStatus::Moving,
        up_speed: 0,
        uploaded: 0,
        peers: 0,
        seeders: 0,
        leechers: 0,
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
