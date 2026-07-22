//! Event names emitted to the frontend. Kept in one place so the TS side
//! (`src/lib/events.ts`, added with the store) can mirror them exactly.
//!
//! The unified download model reports every source — HTTP, BitTorrent, media —
//! through one progress schema, so torrent-only fields (peers/seeds/ratio) ride
//! along as optionals. These land for real with the engine in Phase 2.

/// A task was added to the queue.
pub const TASK_ADDED: &str = "moin-task-added";
/// Periodic progress for an active task.
pub const TASK_PROGRESS: &str = "moin-task-progress";
/// A task finished successfully.
pub const TASK_DONE: &str = "moin-task-done";
/// A task failed.
pub const TASK_ERROR: &str = "moin-task-error";
