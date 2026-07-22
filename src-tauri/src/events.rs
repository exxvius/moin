//! Event names emitted to the frontend. Kept in one place so the TS side
//! (`src/lib/events.ts`) can mirror them exactly.
//!
//! Every source — HTTP now, torrent and media later — reports through this one
//! set, so the UI treats all downloads the same. Torrent-only fields (peers,
//! seeds, ratio) will ride along as optionals on the task payload.

/// A task was added to the queue. Payload: `Task`.
pub const TASK_ADDED: &str = "moin-task-added";
/// Frequent progress tick for an active task. Payload: `TaskProgress`.
pub const TASK_PROGRESS: &str = "moin-task-progress";
/// A task's status changed (connecting, paused, done, failed…). Payload: `Task`.
pub const TASK_UPDATED: &str = "moin-task-updated";
/// A task was removed from the registry. Payload: the task id string.
pub const TASK_REMOVED: &str = "moin-task-removed";
