//! The supervisor: owns the task registry, the queue, persistence, and backend
//! selection. It's Tauri-free — it reports out through the [`Emitter`] trait,
//! which the shell implements with `AppHandle::emit`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use uuid::Uuid;

use super::backend::{BackendInfo, Control, DownloadBackend, Outcome, ProgressFn, Signal};
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
            tasks.insert(task.id.clone(), Entry { task, control: None });
        }

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

        let filename = unique_filename(&dir, &filename_from_url(&url));
        let dest = dir.join(&filename).to_string_lossy().into_owned();
        let now = now_ms();
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
        };

        self.inner.store.lock().unwrap().upsert(&task)?;
        self.inner
            .tasks
            .lock()
            .unwrap()
            .insert(task.id.clone(), Entry { task: task.clone(), control: None });
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
        let _ = std::fs::remove_file(task.part_path());
        self.persist_emit(&task);
        Inner::pump(self.inner.clone());
    }

    /// Drop a task entirely: stop it, forget the row, delete the partial file.
    pub fn remove(&self, id: &str) {
        let task = {
            let mut tasks = self.inner.tasks.lock().unwrap();
            if let Some(entry) = tasks.get(id) {
                if let Some(control) = &entry.control {
                    control.set(Signal::Cancel);
                }
            }
            tasks.remove(id).map(|e| e.task)
        };
        if let Some(task) = task {
            let _ = std::fs::remove_file(task.part_path());
            let _ = self.inner.store.lock().unwrap().delete(&task.id);
            self.inner.emitter.removed(&task.id);
            Inner::pump(self.inner.clone());
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
            .or_else(|| self.backends.iter().find(|b| b.supports(kind) && b.available()))
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

    /// Start queued tasks until the concurrency limit is reached.
    fn pump(inner: Arc<Inner>) {
        let max = inner.settings.lock().unwrap().max_concurrent.max(1);
        while inner.running_count() < max {
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

        let progress = Inner::make_progress(inner.clone(), id.clone());
        let inner_done = inner.clone();
        tokio::spawn(async move {
            let outcome = backend.run(task, control, progress).await;
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
            last_bytes: u64,
            speed: f64,
        }
        let state = Arc::new(Mutex::new(State {
            started: false,
            last_emit: Instant::now() - Duration::from_secs(1),
            last_persist: Instant::now(),
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
                    entry.task.status = TaskStatus::Downloading;
                    entry.task.updated_at = now_ms();
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

                (entry.task.clone(), newly_started, do_emit, do_persist, st.speed)
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
        let task = {
            let mut tasks = inner.tasks.lock().unwrap();
            let Some(entry) = tasks.get_mut(&id) else {
                return;
            };
            entry.control = None;
            match outcome {
                Outcome::Completed => {
                    entry.task.status = TaskStatus::Completed;
                    entry.task.error = None;
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
            entry.task.clone()
        };

        if task.status == TaskStatus::Canceled {
            let _ = std::fs::remove_file(task.part_path());
        }
        let _ = inner.store.lock().unwrap().upsert(&task);
        inner.emitter.updated(&task);
        Inner::pump(inner);
    }
}

/// Pick a filename that doesn't collide in `dir`, adding " (n)" before the
/// extension if needed.
fn unique_filename(dir: &Path, name: &str) -> String {
    if !dir.join(name).exists() {
        return name.to_string();
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    for n in 1..10_000 {
        let candidate = format!("{stem} ({n}){ext}");
        if !dir.join(&candidate).exists() {
            return candidate;
        }
    }
    name.to_string()
}
