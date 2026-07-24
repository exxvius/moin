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

        // Trim librqbit's session store + our .torrent cache to the torrents we still
        // track, before the session is built. librqbit keeps a persisted entry and a
        // fastresume file per torrent ever added and never prunes them itself, so they
        // accumulate; we own the manifest, so we own the cleanup.
        let keep: HashSet<String> = tasks
            .values()
            .filter_map(|e| e.task.info_hash.as_deref())
            .map(|h| h.to_ascii_lowercase())
            .collect();
        torrent::prune_store(&data_dir, &keep);

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
                force_start: false,
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
        let mut resolved = self
            .inner
            .embedded()
            .resolve_torrent(&source)
            .await
            .ok_or_else(|| "couldn't resolve the torrent".to_string())??;

        // Suggest a category from the torrent name.
        let name = resolved.name.clone().unwrap_or_default();
        let suggested = {
            let cand = Candidate::from_url_named(
                &source,
                category::AddMethodKind::ManualTorrent,
                Some(&name),
            );
            category::categorize(&cand, &self.inner.categories.lock().unwrap())
        };

        // Fold the suggested category's automation into the preview so the modal
        // opens with it applied but overridable: exclusions uncheck files, renames
        // rewrite paths, and the layout is pre-selected. Also resolve the folder.
        let (default_dir, suggested_layout) = {
            let cats = self.inner.categories.lock().unwrap();
            let cat = suggested.as_ref().and_then(|id| cats.iter().find(|c| c.id == *id));
            let default_dir = cat
                .and_then(|c| c.save_dir.clone())
                .unwrap_or_else(|| dir.to_string_lossy().into_owned());
            let layout = match cat {
                Some(cat) => {
                    // Exclusions → deselect matching files (empty = keep all).
                    let keep = category::auto_selection(Some(cat), &resolved.files);
                    if !keep.is_empty() {
                        for (i, f) in resolved.files.iter_mut().enumerate() {
                            f.selected = keep.contains(&i);
                        }
                    }
                    // Renames → rewrite paths (index-aligned; blank keeps original).
                    // Runs on the original paths, before any are mutated.
                    let renames = category::plan_renames(cat, &resolved.files);
                    for (f, r) in resolved.files.iter_mut().zip(renames) {
                        if !r.is_empty() {
                            f.path = r;
                        }
                    }
                    cat.automation.layout
                }
                None => category::LayoutMode::Original,
            };
            (default_dir, layout)
        };

        Ok(TorrentPreview {
            resolved,
            suggested_category: suggested,
            default_dir,
            suggested_layout,
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

        // Reject a torrent that's already in the list (same info hash, not archived)
        // so the same magnet added twice doesn't run two copies over one folder.
        if let Some(hash) = &meta.info_hash {
            let dupe = {
                let tasks = self.inner.tasks.lock().unwrap();
                tasks
                    .values()
                    .any(|e| !e.task.archived && e.task.info_hash.as_deref() == Some(hash.as_str()))
            };
            if dupe {
                return Err("that torrent is already in your downloads".to_string());
            }
        }

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

        // Resolve the category record up front, both to validate its id and to read
        // its automation (the exclude rules below). `None` = uncategorized.
        let cat = {
            let cats = self.inner.categories.lock().unwrap();
            category.and_then(|id| cats.iter().find(|c| c.id == id).cloned())
        };

        // Load the file list from the cached .torrent. Selection: an explicit list
        // from the caller (the modal) is honored as-is; an empty list is the
        // headless path (watch folder), where the category's exclude rules decide
        // which files to keep (`auto_selection` returns empty = keep all).
        let cached_bytes = meta
            .info_hash
            .as_deref()
            .map(|h| torrent::meta_path(&self.inner.data_dir, h))
            .and_then(|p| std::fs::read(p).ok());
        let raw_files = cached_bytes
            .as_deref()
            .and_then(|b| torrent::files_from_bytes(b).ok())
            .unwrap_or_default();

        // Normalize the stored source to a magnet whenever we know the info hash, so
        // the torrent can always be re-fetched from the swarm later even if the
        // original `.torrent` (and our cached copy) are gone — e.g. a browser-captured
        // torrent whose `.torrent` was consumed on conversion, then removed and
        // re-downloaded from Archive.
        let stored_source = match (&meta.info_hash, &cached_bytes) {
            (Some(hash), Some(bytes)) => {
                torrent::magnet_from_bytes(bytes, hash, meta.name.as_deref())
            }
            (Some(hash), None) => format!("magnet:?xt=urn:btih:{hash}"),
            _ => source.clone(),
        };
        let effective: Vec<usize> = if selected.is_empty() {
            category::auto_selection(cat.as_ref(), &raw_files)
        } else {
            selected
        };
        // Apply the selection and any per-file renames (index-aligned relative
        // paths; blank keeps the original).
        let files: Vec<_> = raw_files
            .into_iter()
            .enumerate()
            .map(|(i, mut f)| {
                if !effective.is_empty() {
                    f.selected = effective.contains(&i);
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
        // the category's folder but let the user override), so use it as-is.
        let category = cat.map(|c| c.id);

        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let dest = dir.to_string_lossy().into_owned();
        let now = now_ms();

        let task = {
            let mut tasks = self.inner.tasks.lock().unwrap();
            let task = Task {
                id: Uuid::new_v4().to_string(),
                kind: TaskKind::Torrent,
                url: stored_source,
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
                force_start: false,
            };
            tasks.insert(task.id.clone(), Entry::idle(task.clone()));
            task
        };

        self.inner.store.lock().unwrap().upsert(&task)?;
        self.inner.emitter.added(&task);
        Inner::pump(self.inner.clone());
        Ok(task)
    }

    pub fn pause(&self, id: &str) -> Result<(), String> {
        let updated = {
            let mut tasks = self.inner.tasks.lock().unwrap();
            let entry = tasks
                .get_mut(id)
                .ok_or("that download is no longer in your list")?;
            if entry.moving {
                return Err(
                    "can't pause while the file is being moved — let the move finish first"
                        .to_string(),
                );
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
                return Err(format!(
                    "nothing to pause — this download is {}",
                    entry.task.status.label()
                ));
            }
        };
        if let Some(task) = updated {
            self.persist_emit(&task);
        }
        Ok(())
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

    pub fn resume(&self, id: &str) -> Result<(), String> {
        let task = {
            let mut tasks = self.inner.tasks.lock().unwrap();
            let entry = tasks
                .get_mut(id)
                .ok_or("that download is no longer in your list")?;
            if entry.moving {
                return Err(
                    "can't resume while the file is being moved — let the move finish first"
                        .to_string(),
                );
            }
            if entry.control.is_some() {
                return Err("that download is already running".to_string());
            }
            if entry.task.status == TaskStatus::Completed {
                return Err("that download has already finished".to_string());
            }
            entry.task.status = TaskStatus::Queued;
            entry.task.error = None;
            entry.task.updated_at = now_ms();
            entry.task.clone()
        };
        self.persist_emit(&task);
        Inner::pump(self.inner.clone());
        Ok(())
    }

    /// Toggle force-start on a torrent: it bypasses the concurrency limit and never
    /// drops to `Stalled`. Applied live to a running torrent (via its control) and
    /// starts a stopped one. Toggling it off lets it queue and stall normally again.
    pub fn force_start(&self, id: &str) -> Result<(), String> {
        let (task, start) = {
            let mut tasks = self.inner.tasks.lock().unwrap();
            let entry = tasks
                .get_mut(id)
                .ok_or("that download is no longer in your list")?;
            if !entry.task.is_torrent() {
                return Err("only torrents can be force-started".to_string());
            }
            if entry.moving {
                return Err(
                    "can't force-start while the file is being moved — let the move finish first"
                        .to_string(),
                );
            }
            if entry.task.status == TaskStatus::Completed {
                return Err("that torrent has already finished".to_string());
            }
            let next = !entry.task.force_start;
            entry.task.force_start = next;
            entry.task.error = None;
            entry.task.updated_at = now_ms();
            match &entry.control {
                // Running (incl. stalled): push the flag live; the poll loop reacts.
                Some(control) => {
                    control.set_force(next);
                    (entry.task.clone(), false)
                }
                // Stopped and turning on: queue it — pump starts it past the limit.
                None if next => {
                    entry.task.status = TaskStatus::Queued;
                    (entry.task.clone(), true)
                }
                None => (entry.task.clone(), false),
            }
        };
        self.persist_emit(&task);
        if start {
            Inner::pump(self.inner.clone());
        }
        Ok(())
    }

    /// Keep seeding a finished torrent past the ratio/time limit. Re-runs a
    /// stopped (Completed) torrent in force-seed mode, so the auto-stop is lifted
    /// for that session and it seeds until stopped by hand. No-op for a
    /// non-torrent, a busy task, or one that's already running.
    pub fn start_seeding(&self, id: &str) -> Result<(), String> {
        let task = {
            let mut tasks = self.inner.tasks.lock().unwrap();
            let entry = tasks
                .get_mut(id)
                .ok_or("that download is no longer in your list")?;
            if !entry.task.is_torrent() {
                return Err("only torrents can seed".to_string());
            }
            if entry.moving {
                return Err(
                    "can't start seeding while the file is being moved — let the move finish first"
                        .to_string(),
                );
            }
            if entry.control.is_some() {
                return Err("that torrent is already running".to_string());
            }
            entry.task.force_seed = true;
            entry.task.status = TaskStatus::Queued;
            entry.task.error = None;
            entry.task.updated_at = now_ms();
            entry.task.clone()
        };
        self.persist_emit(&task);
        Inner::pump(self.inner.clone());
        Ok(())
    }

    pub async fn cancel(&self, id: &str) -> Result<(), String> {
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
            let entry = tasks
                .get_mut(id)
                .ok_or("that download is no longer in your list")?;
            if entry.moving {
                return Err(
                    "can't stop while the file is being moved — let the move finish first"
                        .to_string(),
                );
            }
            if let Some(control) = &entry.control {
                control.set(Signal::Cancel);
                return Ok(()); // `finish` handles the rest
            }
            entry.task.status = TaskStatus::Canceled;
            entry.task.updated_at = now_ms();
            entry.task.clone()
        };
        // A canceled torrent discards its partial data; HTTP drops its `.part`.
        purge_files(&task, true);
        self.persist_emit(&task);
        Inner::pump(self.inner.clone());
        Ok(())
    }

    /// Archive a whole selection at once (remove, or delete files). The batch is the
    /// point: it takes *every* target out of the startable pool under a single lock
    /// before any per-item detach/delete, so `pump` can't start one of them in the
    /// window between individual commands — which was re-adding a torrent to the
    /// session and re-downloading its files right after they were deleted. Returns
    /// the first error, if any (a deferred delete surfaces its own error on the task).
    pub async fn archive_many(&self, ids: Vec<String>, delete_file: bool) -> Result<(), String> {
        {
            let mut tasks = self.inner.tasks.lock().unwrap();
            for id in &ids {
                let Some(entry) = tasks.get_mut(id) else {
                    continue;
                };
                if entry.control.is_some() {
                    // Running: record the intent now so it's off-limits immediately.
                    entry.pending_archive = Some(delete_file);
                } else if entry.task.status == TaskStatus::Queued {
                    // Queued: pull it out of the queue so pump won't start it.
                    entry.task.status = TaskStatus::Canceled;
                }
            }
        }
        let mut first_err = None;
        for id in ids {
            if let Err(e) = self.archive(&id, delete_file).await {
                first_err.get_or_insert(e);
            }
        }
        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
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
        if let Some((hash, _)) = torrent {
            // Record the archive intent (and, for a moving task, defer to the move
            // handler) BEFORE detaching from the session. Detaching can make the run
            // loop notice the torrent is gone and settle — if `pending_archive` isn't
            // already set by then, it settles as a bare Canceled that drops the intent
            // and leaves the row un-archived (needing a second delete).
            let was_running = {
                let mut tasks = self.inner.tasks.lock().unwrap();
                let Some(entry) = tasks.get_mut(id) else {
                    return Ok(());
                };
                if entry.moving {
                    entry.pending_archive = Some(delete_file);
                    return Ok(()); // the move-finish handler archives once it lands
                }
                if entry.control.is_some() {
                    entry.pending_archive = Some(delete_file);
                    true
                } else {
                    // Not running. If it's queued, take it out of the queue right now so
                    // pump can't start it (re-adding to the session and re-writing the
                    // files) during the delete below.
                    if entry.task.status == TaskStatus::Queued {
                        entry.task.status = TaskStatus::Canceled;
                    }
                    false
                }
            };
            // Detach: releases the file handles and (for a running torrent) makes the
            // loop stop. `finish` then sees `pending_archive` and does the delete +
            // archive with the handles already freed.
            if let Some(hash) = hash {
                self.remove_torrent_from_session(&hash).await?;
            }
            if was_running {
                // Belt-and-suspenders: cancel the loop too, in case it didn't notice
                // the detach. Handles are already released by the awaited detach.
                let tasks = self.inner.tasks.lock().unwrap();
                if let Some(entry) = tasks.get(id) {
                    if let Some(control) = &entry.control {
                        control.set(Signal::Cancel);
                    }
                }
                return Ok(());
            }

            // Not running: delete the files first so a failure (a file/folder open
            // elsewhere) holds the download in the list for a retry, then archive.
            let task = {
                let tasks = self.inner.tasks.lock().unwrap();
                match tasks.get(id) {
                    Some(entry) => entry.task.clone(),
                    None => return Ok(()),
                }
            };
            if delete_file {
                // Off the async executor — the delete verifies + retries with sleeps.
                let t = task.clone();
                tokio::task::spawn_blocking(move || delete_torrent_files(&t))
                    .await
                    .unwrap_or_else(|e| Err(e.to_string()))
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
            // A retry is a fresh download, not a resume — clear every leftover
            // progress reading so nothing stale carries into the re-run (the torrent
            // path also had its librqbit fastresume dropped when it was archived, so
            // it re-checks/re-fetches from real disk state).
            entry.task.uploaded = 0;
            entry.task.force_seed = false;
            for f in entry.task.files.iter_mut() {
                f.received = 0;
            }
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
                // Already relocating: record the new target as the pending move so the
                // latest command wins — `finish_move` chains into it once the current
                // move lands, instead of dropping the request.
                if entry.moving {
                    entry.pending_move = Some(PendingMove {
                        category: category.clone(),
                        target_dir: target_dir.clone(),
                    });
                    continue;
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

    /// Scan every watched folder once: for each new `.torrent` dropped in, resolve
    /// it, decide under the owning category's rules whether to take it, and (if so)
    /// add it with that category's automation applied. Called on a timer by the
    /// watch poller (`core/watch.rs`). `fallback_dir` is the default download folder
    /// (the OS Downloads dir), used when neither the category nor settings sets one.
    pub async fn scan_watch_folders(&self, fallback_dir: &Path) {
        let (watched, default_dir) = {
            let cats = self.inner.categories.lock().unwrap();
            let s = self.inner.settings.lock().unwrap();
            let default_dir = s
                .download_dir
                .clone()
                .map(PathBuf::from)
                .unwrap_or_else(|| fallback_dir.to_path_buf());
            let watched: Vec<Category> = cats
                .iter()
                .filter(|c| !c.watch_folders.is_empty())
                .cloned()
                .collect();
            (watched, default_dir)
        };
        for cat in &watched {
            for folder in &cat.watch_folders {
                let entries = match std::fs::read_dir(folder) {
                    Ok(e) => e,
                    Err(_) => continue, // folder gone or unreadable; try again next tick
                };
                for entry in entries.flatten() {
                    let path = entry.path();
                    // Only unprocessed `.torrent` files — handled ones are renamed to
                    // `.added`/`.skipped`/`.error`, which no longer end in `.torrent`.
                    if !is_pending_torrent(&path) || !is_stable(&path) {
                        continue;
                    }
                    self.process_watched(cat, &path, &default_dir).await;
                }
            }
        }
    }

    /// Handle one dropped `.torrent`: resolve, gate on the category's triggers, add
    /// (with automation, or uncategorized under `fallback_download`, or skip), then
    /// mark the file so it isn't picked up again.
    async fn process_watched(&self, cat: &Category, path: &Path, default_dir: &Path) {
        let source = path.to_string_lossy().into_owned();
        let resolved = match self.inner.embedded().resolve_torrent(&source).await {
            Some(Ok(r)) => r,
            _ => {
                tracing::warn!(?path, "watch: couldn't resolve the torrent");
                mark_processed(path, "error");
                return;
            }
        };
        let name = resolved.name.clone().unwrap_or_default();

        let outcome = if category::watch_accepts(cat, &name, resolved.total, &resolved.files) {
            // Take it under this category, with its automation. An empty selection
            // lets `add_torrent` apply the exclude rules; renames + layout folder are
            // computed here (add_torrent uses them as given).
            let dir = cat
                .save_dir
                .clone()
                .map(PathBuf::from)
                .unwrap_or_else(|| default_dir.to_path_buf());
            let folder = category::layout_folder(cat, &name, resolved.files.len());
            let renames = category::plan_renames(cat, &resolved.files);
            self.add_torrent(source, dir, Some(cat.id.clone()), Vec::new(), folder, renames)
        } else if cat.fallback_download {
            // Doesn't match the category's triggers, but the category opts to grab
            // non-matching drops anyway — uncategorized, no automation.
            self.add_torrent(
                source,
                default_dir.to_path_buf(),
                None,
                Vec::new(),
                None,
                Vec::new(),
            )
        } else {
            mark_processed(path, "skipped");
            return;
        };

        match outcome {
            Ok(_) => mark_processed(path, "added"),
            Err(e) => {
                tracing::warn!(?path, error = %e, "watch: couldn't add the torrent");
                mark_processed(path, "error");
            }
        }
    }

    /// Turn a finished `.torrent` download into an actual torrent: resolve the
    /// downloaded file, add it as a torrent under the same category (with that
    /// category's automation), then archive the HTTP download and delete the spent
    /// `.torrent`. Fired by `finish` when a completed HTTP `.torrent` landed in a
    /// category with `capture_torrent_downloads` on. Best-effort — on any failure
    /// the original download is left in place.
    async fn convert_download(&self, id: String) {
        let (source, category, parent) = {
            let tasks = self.inner.tasks.lock().unwrap();
            let Some(entry) = tasks.get(&id) else { return };
            let dest = entry.task.dest.clone();
            let parent = Path::new(&dest).parent().map(Path::to_path_buf);
            (dest, entry.task.category.clone(), parent)
        };

        let resolved = match self.inner.embedded().resolve_torrent(&source).await {
            Some(Ok(r)) => r,
            _ => {
                tracing::warn!(id = %id, "convert: couldn't read the downloaded .torrent");
                return;
            }
        };
        let name = resolved.name.clone().unwrap_or_default();

        let cat = {
            let cats = self.inner.categories.lock().unwrap();
            category
                .as_ref()
                .and_then(|cid| cats.iter().find(|c| &c.id == cid).cloned())
        };
        // Save into the category's folder if it sets one, else beside the .torrent.
        let dir = cat
            .as_ref()
            .and_then(|c| c.save_dir.clone())
            .map(PathBuf::from)
            .or(parent)
            .unwrap_or_else(|| self.inner.data_dir.clone());
        let (folder, renames) = match &cat {
            Some(c) => (
                category::layout_folder(c, &name, resolved.files.len()),
                category::plan_renames(c, &resolved.files),
            ),
            None => (None, Vec::new()),
        };

        match self.add_torrent(source, dir, category, Vec::new(), folder, renames) {
            // The .torrent was only a vehicle: drop the HTTP record + its file.
            Ok(_) => {
                let _ = self.archive(&id, true).await;
            }
            Err(e) => tracing::warn!(id = %id, error = %e, "convert: couldn't add the torrent"),
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
        // A seeding torrent keeps its run loop alive but isn't using a download slot.
        // A stalled torrent is getting no data, so it releases its slot too — a
        // queued torrent that could actually download shouldn't wait behind it. Both
        // keep their run loop; they just don't count against the concurrency limit.
        self.tasks
            .lock()
            .unwrap()
            .values()
            .filter(|e| {
                e.control.is_some()
                    && e.task.status != TaskStatus::Seeding
                    && e.task.status != TaskStatus::Stalled
            })
            .count()
    }

    /// If `task` is a finished `.torrent` download filed into a category that opts
    /// to capture torrent downloads, kick off its conversion into a real torrent on
    /// the runtime. A no-op for anything else, so it's cheap to call on every
    /// settle. Spawns because the conversion resolves metadata asynchronously.
    fn maybe_convert_download(inner: &Arc<Inner>, task: &Task) {
        if task.kind != TaskKind::Http
            || task.status != TaskStatus::Completed
            || !is_dot_torrent(&task.dest)
        {
            return;
        }
        let Some(cat_id) = task.category.clone() else {
            return;
        };
        let enabled = {
            let cats = inner.categories.lock().unwrap();
            cats.iter()
                .any(|c| c.id == cat_id && c.capture_torrent_downloads)
        };
        if !enabled {
            return;
        }
        let engine = Engine {
            inner: inner.clone(),
        };
        let id = task.id.clone();
        tokio::spawn(async move { engine.convert_download(id).await });
    }

    /// Start queued tasks until the concurrency limit is reached. A limit of 0
    /// means unlimited — every queued task starts. Force-started tasks run first and
    /// ignore the limit.
    fn pump(inner: Arc<Inner>) {
        // Force-started tasks bypass the concurrency limit entirely.
        loop {
            let next = {
                let tasks = inner.tasks.lock().unwrap();
                tasks
                    .values()
                    .filter(|e| {
                        e.task.status == TaskStatus::Queued
                            && e.control.is_none()
                            && e.task.force_start
                    })
                    .min_by_key(|e| e.task.created_at)
                    .map(|e| e.task.id.clone())
            };
            match next {
                Some(id) => Inner::start(inner.clone(), id),
                None => break,
            }
        }

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
        // Only start a task that's still Queued and not archived. Between pump
        // picking it and here, a concurrent archive/delete may have pulled it out of
        // the queue — starting it anyway would re-add it to the session and re-write
        // files that were just deleted.
        let startable = |e: &Entry| {
            e.control.is_none() && e.task.status == TaskStatus::Queued && !e.task.archived
        };
        let kind = {
            let tasks = inner.tasks.lock().unwrap();
            match tasks.get(&id) {
                Some(entry) if startable(entry) => entry.task.kind,
                _ => return,
            }
        };
        let backend = inner.backend_for(kind);

        let (task, control) = {
            let mut tasks = inner.tasks.lock().unwrap();
            let Some(entry) = tasks.get_mut(&id) else {
                return;
            };
            if !startable(entry) {
                return;
            }
            let control = Control::new();
            control.set_force(entry.task.force_start);
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
            /// The task's stored `uploaded` when this run began, plus the backend's
            /// upload counter at the first tick — so `uploaded` accumulates across
            /// restarts as baseline + this-session delta, surviving a backend whose
            /// own counter reset (e.g. after a crash-recovery recheck).
            up_baseline: u64,
            up_start: u64,
        }
        let state = Arc::new(Mutex::new(State {
            started: false,
            last_emit: Instant::now() - Duration::from_secs(1),
            last_persist: Instant::now(),
            last_tick: Instant::now(),
            last_bytes: 0,
            speed: 0.0,
            up_baseline: 0,
            up_start: 0,
        }));

        Arc::new(
            move |received: u64, total: Option<u64>, torrent: Option<TorrentTick>| {
                let now = Instant::now();

                // Snapshot the task + decide what to do while briefly holding locks.
                let (task, newly_started, newly_seeding, frees_slot, do_emit, do_persist, speed) = {
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
                    // `uploaded` accumulates as baseline + delta rather than taking
                    // the backend's counter directly, so it survives a session whose
                    // counter restarted at 0 (a crash-recovery recheck clears
                    // librqbit's persisted upload total, which would otherwise zero
                    // the ratio).
                    if let Some(tick) = torrent {
                        if !st.started {
                            st.up_baseline = entry.task.uploaded;
                            st.up_start = tick.uploaded;
                        }
                        entry.task.uploaded =
                            st.up_baseline + tick.uploaded.saturating_sub(st.up_start);
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
                        // Baseline the speed counters from this first reading. A resume
                        // starts with `received` already at the bytes on disk, so without
                        // this the first sample would be that whole amount "in one tick"
                        // — a huge spike the smoothing then has to walk back down. Anchor
                        // the byte + time marks here so the first real speed is a proper
                        // delta from the next tick.
                        st.last_bytes = received;
                        st.last_emit = now;
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
                        // Only the Downloading phase has a download speed. While a
                        // torrent is Checking, `received` climbs at disk-read speed as
                        // pieces are verified — folding that into the average produced a
                        // huge phantom reading that then leaked into the row the moment
                        // the phase flipped to Downloading. Seeding has no download
                        // speed either. So hold speed at 0 off-phase, and keep the byte
                        // mark current so the first real Downloading sample is a clean
                        // delta, not a jump from wherever verification left off.
                        if let Some(tick) = torrent {
                            // Torrent: take the engine's live per-byte estimate
                            // directly. Deriving from the progress delta is piece-
                            // level — flat between piece completions even while bytes
                            // pour in — which is what made a downloading torrent read
                            // as ~0 KB/s (and mislabelled it stalled).
                            st.speed = if tick.status == TaskStatus::Downloading {
                                tick.down_speed as f64
                            } else {
                                0.0
                            };
                        } else if status == TaskStatus::Downloading {
                            let inst = (received.saturating_sub(st.last_bytes)) as f64 / dt;
                            // Exponential smoothing so the number doesn't jitter.
                            st.speed = if st.speed == 0.0 {
                                inst
                            } else {
                                st.speed * 0.7 + inst * 0.3
                            };
                        } else {
                            st.speed = 0.0;
                        }
                        st.last_emit = now;
                        st.last_bytes = received;
                    }
                    let do_persist = now.duration_since(st.last_persist) >= Duration::from_secs(2);
                    if do_persist {
                        st.last_persist = now;
                    }

                    // Seeding and Stalled don't hold a download slot, so a transition
                    // into either frees one — pump so a queued task can start.
                    let frees_slot = matches!(status, TaskStatus::Seeding | TaskStatus::Stalled)
                        && !matches!(prev_status, TaskStatus::Seeding | TaskStatus::Stalled);
                    let newly_seeding =
                        status == TaskStatus::Seeding && prev_status != TaskStatus::Seeding;
                    (
                        entry.task.clone(),
                        newly_started,
                        newly_seeding,
                        frees_slot,
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
                } else if frees_slot {
                    // Slipped into Stalled — free the slot for a queued task.
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
                                           // Stop it being re-queued, but hold off on `archived` until any
                                           // requested file delete actually succeeds — see the Archive handler,
                                           // which keeps the item (with an error) if the delete fails instead of
                                           // archiving and orphaning the files.
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
                // Deleting a torrent's files: do it off an async task that first
                // re-detaches the torrent from the session. A delete that raced the
                // torrent's add can leave it still attached and writing, which
                // re-creates the files right after they're removed (so the delete
                // "succeeds" but the folder comes back). Re-detaching stops the writer;
                // `delete_torrent_files` then verifies + retries. On genuine failure the
                // item is kept with the error instead of archived-and-orphaned.
                if delete_file && task.is_torrent() {
                    let inner2 = inner.clone();
                    let id2 = id.clone();
                    tokio::spawn(async move {
                        if let Some(hash) = task.info_hash.clone() {
                            let _ = inner2.embedded().remove_torrent(&hash).await;
                        }
                        let deleted = tokio::task::spawn_blocking({
                            let task = task.clone();
                            move || delete_torrent_files(&task)
                        })
                        .await
                        .unwrap_or_else(|e| Err(e.to_string()));

                        let task = {
                            let mut tasks = inner2.tasks.lock().unwrap();
                            let Some(entry) = tasks.get_mut(&id2) else {
                                return;
                            };
                            match &deleted {
                                Ok(()) => {
                                    entry.task.archived = true;
                                    entry.task.error = None;
                                }
                                Err(e) => {
                                    entry.task.error = Some(format!("Couldn't delete the files: {e}. The download is still in your list — free the files (e.g. close a window that's in the folder) and use Delete again."));
                                }
                            }
                            entry.task.updated_at = now_ms();
                            entry.task.clone()
                        };
                        let _ = inner2.store.lock().unwrap().upsert(&task);
                        inner2.emitter.updated(&task);
                        Inner::pump(inner2);
                    });
                    return;
                }

                // Keep-file, or a non-torrent: no writer race, settle synchronously.
                purge_files(&task, delete_file);
                let task = {
                    let mut tasks = inner.tasks.lock().unwrap();
                    let Some(entry) = tasks.get_mut(&id) else {
                        return;
                    };
                    entry.task.archived = true;
                    entry.task.error = None;
                    entry.task.updated_at = now_ms();
                    entry.task.clone()
                };
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
                // A completed `.torrent` download in a capture-enabled category is
                // re-added as a torrent (then this HTTP record is archived).
                Inner::maybe_convert_download(&inner, &task);
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
        let (task, archive, next_move) = {
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
                // A remove/delete requested during the move wins over any queued move.
                entry.pending_move = None;
                entry.task.archived = true;
                entry.task.status = if job.was_completed {
                    TaskStatus::Completed
                } else {
                    TaskStatus::Canceled
                };
                entry.task.updated_at = now_ms();
                (entry.task.clone(), Some(delete_file), None)
            } else if let Some(pm) = entry.pending_move.take() {
                // A newer category change arrived mid-move — chain into it from the
                // just-settled location so the latest command wins.
                entry.task.error = None;
                entry.task.updated_at = now_ms();
                (entry.task.clone(), None, Some(pm))
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
                (entry.task.clone(), None, None)
            }
        };

        if let Some(delete_file) = archive {
            purge_files(&task, delete_file);
        }
        let _ = inner.store.lock().unwrap().upsert(&task);
        inner.emitter.updated(&task);
        // Chain into the latest queued category change, if one arrived mid-move.
        if let Some(pm) = next_move {
            Inner::begin_move(inner, job.id, pm.category, pm.target_dir);
            return;
        }
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

/// Whether `dest` names a `.torrent` file (by extension) — the trigger for the
/// download-to-torrent conversion.
fn is_dot_torrent(dest: &str) -> bool {
    Path::new(dest)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("torrent"))
}

/// Whether `path` is a `.torrent` file the watch poller hasn't handled yet. A
/// handled file has a marker suffix appended (`.added`/`.skipped`/`.error`), so it
/// no longer ends in `.torrent` and is skipped.
fn is_pending_torrent(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("torrent"))
}

/// Whether a file has been settled on disk long enough to read safely — guards
/// against reading a `.torrent` mid-copy. A file modified within the last second
/// is left for the next tick. An unreadable mtime is treated as stable.
fn is_stable(path: &Path) -> bool {
    match path.metadata().and_then(|m| m.modified()) {
        Ok(modified) => modified
            .elapsed()
            .map(|age| age >= Duration::from_secs(1))
            .unwrap_or(true),
        Err(_) => true,
    }
}

/// Mark a watched `.torrent` as handled by appending a `.<suffix>` marker, so the
/// next scan skips it. Best-effort — a failed rename just means it's retried.
fn mark_processed(path: &Path, suffix: &str) {
    let mut target = path.as_os_str().to_owned();
    target.push(".");
    target.push(suffix);
    let _ = std::fs::rename(path, target);
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
    // Retry a few times: right after a torrent detaches — especially an aborted
    // mid-add during a rapid start-then-delete — the OS can still hold a file handle
    // for a beat, so the first delete fails ("in use"/"not empty") even though it'll
    // succeed moments later. Cheap insurance against a transient failure leaving a
    // stray errored task with un-deleted files.
    let mut last = Ok(());
    for attempt in 0..6 {
        last = delete_torrent_files_once(task);
        if last.is_ok() {
            return Ok(());
        }
        if attempt < 5 {
            std::thread::sleep(Duration::from_millis(150 * (attempt as u64 + 1)));
        }
    }
    last
}

fn delete_torrent_files_once(task: &Task) -> Result<(), String> {
    let base = Path::new(&task.dest);

    // "Create subfolder" layout: moin made this folder exclusively for the torrent,
    // so remove the whole tree in one go. This also clears the intermediate and
    // deselected-file directories librqbit created but never filled, which a
    // file-by-file delete leaves behind (the "directory is not empty" failure).
    if task.own_dir {
        match std::fs::remove_dir_all(base) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.to_string()),
        }
        // Verify: a still-attached writer can re-create files right after the tree is
        // gone, so `remove_dir_all` returning Ok isn't proof. If the folder is back,
        // report it as a failure so the retry (and the re-detach that precedes it)
        // gets another go.
        return if base.exists() {
            Err(format!(
                "folder came back after delete (still being written?): {}",
                base.display()
            ))
        } else {
            Ok(())
        };
    }

    // Saved directly into a shared folder: delete only this torrent's own files,
    // then every empty directory it created — every ancestor prefix of every file,
    // deepest first — while never touching the shared folder or a dir that still
    // holds someone else's files (`remove_dir` no-ops on a non-empty dir).
    let mut failed: Option<String> = None;
    for f in &task.files {
        let path = base.join(&f.path);
        if let Err(e) = remove_if_exists(&path.to_string_lossy()) {
            failed.get_or_insert_with(|| e.to_string());
        }
    }
    let mut dirs: Vec<PathBuf> = task
        .files
        .iter()
        .flat_map(|f| Path::new(&f.path).ancestors().skip(1)) // skip the file itself
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| base.join(p))
        .collect();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    dirs.dedup();
    for d in dirs {
        let _ = std::fs::remove_dir(d); // only removes empties; shared content stays
    }
    // Verify none of our files came back (a still-attached writer re-creating them),
    // and surface a genuine leftover so the caller retries / holds the item.
    if let Some(f) = task.files.iter().find(|f| base.join(&f.path).exists()) {
        return Err(format!(
            "file came back after delete (still being written?): {}",
            base.join(&f.path).display()
        ));
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
