//! The handshake file that tells a UI where the engine is.
//!
//! Lives here rather than in the daemon because it's a contract *between* the
//! daemon and every client — and a serialized contract duplicated across two
//! crates is a contract that drifts.
//!
//! The daemon writes `engine.json` into the data dir once it has bound its port,
//! and deletes it on a clean exit. A UI reads it, checks the endpoint is alive,
//! and only spawns a daemon if it isn't.
//!
//! The file is a hint, never the source of truth — a hard kill leaves it behind,
//! so a client must always health-check before trusting it. The *port bind* is
//! what actually arbitrates who owns the engine (see `main`), which is why that
//! can't race even if two UIs start at the same instant.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Written by the daemon, read by every client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Endpoint {
    /// Loopback port the control API is bound to.
    pub port: u16,
    /// Daemon process id — lets a client report a stale file usefully.
    pub pid: u32,
    /// Bearer token for the control API. Minted fresh on every daemon start, and
    /// deliberately NOT the browser-integration token: regenerating that one from
    /// settings must not knock the UI off its own engine.
    pub token: String,
    /// Daemon version, so a client can refuse a mismatched pair loudly instead of
    /// failing in confusing ways after a partial upgrade.
    pub version: String,
}

pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join("engine.json")
}

/// Write the file, best-effort. Written to a temp name and renamed so a client
/// can never read a half-written file.
pub fn write(data_dir: &Path, endpoint: &Endpoint) -> std::io::Result<()> {
    let final_path = path(data_dir);
    let tmp = final_path.with_extension("json.tmp");
    let body = serde_json::to_string_pretty(endpoint)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, &final_path)
}

pub fn read(data_dir: &Path) -> Option<Endpoint> {
    let body = std::fs::read_to_string(path(data_dir)).ok()?;
    serde_json::from_str(&body).ok()
}

/// Remove the file on the way out. Best-effort: if we're being killed hard we
/// never get here, which is exactly why clients health-check.
pub fn remove(data_dir: &Path) {
    let _ = std::fs::remove_file(path(data_dir));
}
