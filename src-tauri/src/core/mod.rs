//! Headless download engine — deliberately free of any Tauri types so it can be
//! unit-tested without the shell.
//!
//! Modules land phase by phase:
//!   - `task`    unified DownloadTask model + state machine
//!   - `engine`  supervisor: queue, concurrency, bandwidth caps
//!   - `http`    segmented/resumable direct downloads (reqwest)
//!   - `torrent` librqbit session wrapper
//!   - `media`   yt-dlp subprocess driver
//!   - `tools`   generic external-binary resolve/download/capability probe
//!   - `store`   rusqlite persistence + resume state
//!
//! Empty for now; Phase 2 opens it up with the task model + HTTP engine.
