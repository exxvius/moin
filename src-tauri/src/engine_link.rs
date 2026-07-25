//! Finding — or starting — the engine.
//!
//! The app no longer hosts the download engine; `moin-engine` does, in its own
//! process. This module is how the shell gets hold of it: read the endpoint file,
//! check it's actually alive, and start a daemon if it isn't.
//!
//! Two UIs launching together can both reach the "start one" branch. That's fine
//! and deliberate — the daemon claims ownership by binding its port, so the second
//! one exits immediately and both UIs end up talking to the survivor. Racing here
//! costs a process that lives for a few milliseconds; checking first and trusting
//! the answer would cost a corrupted database.

use std::path::PathBuf;
use std::time::Duration;

use moin_core::endpoint::{self, Endpoint};

/// How long to wait for a freshly-spawned daemon to come up before giving up.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(20);
/// Gap between health checks while waiting for it.
const POLL_INTERVAL: Duration = Duration::from_millis(150);

/// Where the engine is, and how to authenticate to it. Handed to the frontend so
/// it can call the API directly.
#[derive(Clone, serde::Serialize)]
pub struct EngineLink {
    pub port: u16,
    pub token: String,
}

/// Connect to the running engine, starting one if there isn't one.
pub async fn connect(data_dir: PathBuf) -> Result<EngineLink, String> {
    if let Some(link) = probe(&data_dir).await? {
        tracing::info!("attached to the engine already running on {}", link.port);
        return Ok(link);
    }

    spawn_daemon(&data_dir)?;

    let deadline = std::time::Instant::now() + STARTUP_TIMEOUT;
    loop {
        tokio::time::sleep(POLL_INTERVAL).await;
        if let Some(link) = probe(&data_dir).await? {
            tracing::info!("engine started on {}", link.port);
            return Ok(link);
        }
        if std::time::Instant::now() >= deadline {
            return Err(
                "the download engine didn't start. Check moin-engine.log in the data folder."
                    .to_string(),
            );
        }
    }
}

/// Is there a live engine we can use? `Ok(None)` means "not yet" — a missing or
/// stale endpoint file, or nothing listening. `Err` means something is wrong in a
/// way that waiting won't fix.
async fn probe(data_dir: &std::path::Path) -> Result<Option<EngineLink>, String> {
    // A hard kill leaves the endpoint file behind, so it's a hint, never proof.
    let Some(record) = endpoint::read(data_dir) else {
        return Ok(None);
    };
    if !is_alive(&record).await {
        return Ok(None);
    }
    // A running daemon from a previous version, still held open by an older window.
    // Nothing good comes of talking to it across a schema change, and failing here
    // is far kinder than failing later in some unrelated call.
    let ours = env!("CARGO_PKG_VERSION");
    if record.version != ours {
        return Err(format!(
            "a moin engine from version {} is still running (this app is {ours}). \
             Close every moin window, then reopen it.",
            record.version
        ));
    }
    Ok(Some(EngineLink {
        port: record.port,
        token: record.token,
    }))
}

async fn is_alive(record: &Endpoint) -> bool {
    let url = format!("http://127.0.0.1:{}/health", record.port);
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    match client.get(&url).send().await {
        Ok(response) => response.status().is_success(),
        Err(_) => false,
    }
}

/// Start the daemon, detached, from beside our own executable — which is where
/// both the installers and a dev build put it.
fn spawn_daemon(data_dir: &std::path::Path) -> Result<(), String> {
    let exe = daemon_path()?;
    tracing::info!("starting the engine: {}", exe.display());

    let mut command = std::process::Command::new(&exe);
    // Pass our data dir explicitly so the daemon can't disagree with us about
    // where the store lives, whatever it would have resolved on its own.
    command.env("MOIN_DATA_DIR", data_dir);
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::null());
    command.stderr(std::process::Stdio::null());

    // Without this a console window flashes up on Windows every launch.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    command
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("couldn't start the download engine ({}): {e}", exe.display()))
}

fn daemon_path() -> Result<PathBuf, String> {
    let name = if cfg!(windows) {
        "moin-engine.exe"
    } else {
        "moin-engine"
    };
    let dir = std::env::current_exe()
        .map_err(|e| format!("couldn't locate moin's own executable: {e}"))?
        .parent()
        .ok_or_else(|| "moin's executable has no parent directory".to_string())?
        .to_path_buf();
    let beside = dir.join(name);
    if beside.exists() {
        return Ok(beside);
    }
    Err(format!(
        "the download engine ({name}) isn't installed next to moin. Expected it in {}.",
        dir.display()
    ))
}
