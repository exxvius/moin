//! Event names emitted to the frontend. Kept in one place so the TS side
//! (`src/lib/events.ts`) can mirror them exactly.
//!
//! Every source — HTTP now, torrent and media later — reports through this one
//! set, so the UI treats all downloads the same. Torrent-only fields (peers,
//! seeds, ratio) will ride along as optionals on the task payload.

/// The full task list, sent as the FIRST message on a fresh event subscription.
/// Subscribing and snapshotting in one stream is what closes the gap a separate
/// "fetch the list, then subscribe" would leave — no event can slip through
/// between the two. Also re-sent if a client ever falls far enough behind that
/// the broadcast dropped messages for it. Payload: `Vec<Task>`.
pub const TASK_SNAPSHOT: &str = "moin-task-snapshot";
/// A task was added to the queue. Payload: `Task`.
pub const TASK_ADDED: &str = "moin-task-added";
/// Frequent progress tick for an active task. Payload: `TaskProgress`.
pub const TASK_PROGRESS: &str = "moin-task-progress";
/// A whole interval's worth of progress ticks, coalesced into one event so the
/// IPC volume stays flat no matter how many torrents are active. Payload:
/// `Vec<TaskProgress>` (the latest reading per task since the last flush).
pub const TASK_PROGRESS_BATCH: &str = "moin-task-progress-batch";
/// A task's status changed (connecting, paused, done, failed…). Payload: `Task`.
pub const TASK_UPDATED: &str = "moin-task-updated";
/// A task was removed from the registry. Payload: the task id string.
pub const TASK_REMOVED: &str = "moin-task-removed";

/// A download just finished. The client turns this into an OS notification; the
/// engine only fires it when the user has notifications switched on. Payload: `Task`.
pub const TASK_COMPLETED: &str = "moin-task-completed";

/// Progress while a managed tool (aria2c) downloads its binary. Payload:
/// `ToolProgress` — received/total bytes of the archive.
pub const TOOL_PROGRESS: &str = "moin-tool-progress";

/// Settings changed, possibly from a different client. Every client applies this
/// so two open windows can't drift apart or overwrite each other's saves.
/// Payload: `Settings`.
pub const SETTINGS_CHANGED: &str = "moin-settings-changed";
/// The category list changed, from any client. Payload: `Vec<Category>`.
pub const CATEGORIES_CHANGED: &str = "moin-categories-changed";

/// The user tried to close the window while transfers are active and the tray
/// setting is off — the UI shows a confirm prompt (minimize / quit / cancel)
/// instead of quitting outright. No payload.
pub const CONFIRM_QUIT: &str = "moin-confirm-quit";
