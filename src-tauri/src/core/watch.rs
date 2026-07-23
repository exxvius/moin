//! The watch-folder poller: a background task that periodically scans every
//! category's watched folders for dropped `.torrent` files and hands each new one
//! to the engine. It's the headless twin of the add-torrent modal — where the
//! modal drives an add from a user's clicks, this drives one from a file landing
//! in a folder.
//!
//! Deliberately a poll rather than filesystem events: torrents don't need
//! sub-second pickup, a poll needs no extra dependency, and it sidesteps the
//! cross-platform quirks of watching for half-written files. The actual work lives
//! on [`Engine::scan_watch_folders`]; this module is just the timer.

use std::path::PathBuf;
use std::time::Duration;

use tokio::runtime::Handle;

use super::engine::Engine;

/// Floor on the scan interval, so a stray `0`/tiny setting can't spin the disk.
const MIN_INTERVAL_SECS: u64 = 2;

/// Start the poller on the given runtime. It loops forever (cheap when no category
/// watches anything — the scan is a no-op), reading the interval live so a settings
/// change takes effect on the next tick. `fallback_dir` is the OS Downloads folder,
/// the destination when neither a category nor settings sets one.
pub fn spawn(engine: Engine, fallback_dir: PathBuf, rt: Handle) {
    rt.spawn(async move {
        loop {
            let secs = engine
                .settings()
                .watch_interval_secs
                .max(MIN_INTERVAL_SECS);
            tokio::time::sleep(Duration::from_secs(secs)).await;
            engine.scan_watch_folders(&fallback_dir).await;
        }
    });
}
