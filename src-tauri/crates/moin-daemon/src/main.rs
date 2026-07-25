//! moin's engine daemon.
//!
//! Hosts the download engine and serves it to whatever UIs are attached. Runs as
//! its own process so the engine outlives any one window: closing a UI while
//! another still has it open must not drop transfers, and a live torrent session
//! (open peer sockets, file handles, DHT state) can't be handed between processes.
//!
//! Lifecycle, deliberately quiet: a UI spawns it, it runs while at least one UI is
//! attached, and it winds the engine down and exits shortly after the last one
//! leaves. So quitting every window still stops your transfers — the same promise
//! the app made when it hosted the engine itself.
//!
//! Two UIs starting at the same instant can't both become the engine: binding the
//! control port is the claim, and the loser exits. No check-then-act, so no race.

use moin_daemon::{api, extension, hub, paths};

use std::net::SocketAddr;
use std::time::Duration;

use moin_core::endpoint;
use moin_core::engine::Engine;

/// Fixed loopback port for the control API.
///
/// Fixed on purpose: binding it is what arbitrates ownership of the engine
/// between simultaneously-launched UIs, and an OS-assigned port would let two
/// daemons both succeed on different ports and quietly fight over one database.
const CONTROL_PORT: u16 = 47654;
/// Override for development, so a dev build can run beside an installed one.
const PORT_ENV: &str = "MOIN_ENGINE_PORT";

/// Grace after the last client leaves before winding down. Long enough to ride out
/// a UI reload or restart, short enough that quitting feels like quitting.
const EXIT_GRACE: Duration = Duration::from_secs(5);
/// If a UI spawns us and then dies before ever connecting, don't linger forever.
const STARTUP_GRACE: Duration = Duration::from_secs(60);

#[tokio::main]
async fn main() {
    let data_dir = paths::data_dir();
    std::fs::create_dir_all(&data_dir).ok();
    init_logging(&data_dir);

    let port: u16 = std::env::var(PORT_ENV)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(CONTROL_PORT);
    let addr = SocketAddr::from(([127, 0, 0, 1], port));

    // Claim the engine by binding, before touching the database. A second daemon
    // fails here and exits without ever opening the store.
    let listener = match std::net::TcpListener::bind(addr) {
        Ok(l) => l,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            tracing::info!("another engine already owns {addr}; exiting");
            return;
        }
        Err(e) => {
            tracing::error!("couldn't bind the control port {addr}: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = listener.set_nonblocking(true) {
        tracing::error!("couldn't set the control listener non-blocking: {e}");
        std::process::exit(1);
    }
    let listener = match tokio::net::TcpListener::from_std(listener) {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("couldn't adopt the control listener: {e}");
            std::process::exit(1);
        }
    };

    tracing::info!(
        "moin engine {} starting on {addr}, data dir: {}",
        env!("CARGO_PKG_VERSION"),
        data_dir.display()
    );

    let hub = hub::Hub::new();
    let engine = match Engine::new(data_dir.clone(), hub.clone()) {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("failed to start the download engine: {e}");
            std::process::exit(1);
        }
    };

    let downloads = paths::downloads_dir();

    // Poll each category's watched folders for dropped .torrent files and auto-add
    // them. No-op until a category configures a folder.
    moin_core::watch::spawn(
        engine.clone(),
        downloads.clone(),
        tokio::runtime::Handle::current(),
    );
    // Pick back up anything that was downloading or seeding at last exit.
    engine.resume_pending();

    // A token minted per run, so it can't outlive the process it belongs to.
    let token = uuid::Uuid::new_v4().to_string();
    let record = endpoint::Endpoint {
        port,
        pid: std::process::id(),
        token: token.clone(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    };
    if let Err(e) = endpoint::write(&data_dir, &record) {
        tracing::error!("couldn't publish the endpoint file: {e}");
        std::process::exit(1);
    }

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    // The browser extension's listener, on its own user-configurable port.
    tokio::spawn(extension::serve(
        engine.clone(),
        downloads.clone(),
        shutdown_rx.clone(),
    ));

    // Watch the client count and call it a day once everyone's gone.
    tokio::spawn(supervise(hub.clone(), shutdown_tx.clone()));

    // Ctrl-C / a service stop should still wind the engine down cleanly.
    {
        let shutdown_tx = shutdown_tx.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                tracing::info!("interrupted; winding down");
                let _ = shutdown_tx.send(true);
            }
        });
    }

    let app = api::router(api::Api {
        engine: engine.clone(),
        hub,
        token,
        downloads,
    });
    let mut serve_shutdown = shutdown_rx.clone();
    let served = axum::serve(listener, app).with_graceful_shutdown(async move {
        let _ = serve_shutdown.changed().await;
    });
    if let Err(e) = served.await {
        tracing::error!("the control API stopped: {e}");
    }

    // Flush torrent resume state and stop the aria2 daemon before we go.
    tracing::info!("shutting the engine down");
    engine.shutdown().await;
    endpoint::remove(&data_dir);
    tracing::info!("engine stopped");
}

/// Exit once every UI has detached.
///
/// Two phases: until the first client ever connects we're only waiting to be
/// picked up (a UI spawns us, then connects a moment later), so an empty count
/// means "not yet" rather than "everyone left". After that, an empty count that
/// stays empty through the grace period means the last window really did close.
async fn supervise(hub: std::sync::Arc<hub::Hub>, shutdown: tokio::sync::watch::Sender<bool>) {
    let started = tokio::time::Instant::now();
    let mut empty_since: Option<tokio::time::Instant> = None;
    let mut tick = tokio::time::interval(Duration::from_secs(1));

    loop {
        tick.tick().await;

        if !hub.has_ever_had_a_client() {
            if started.elapsed() >= STARTUP_GRACE {
                tracing::warn!("no client attached within the startup grace; exiting");
                let _ = shutdown.send(true);
                return;
            }
            continue;
        }

        if hub.client_count() > 0 {
            empty_since = None;
            continue;
        }
        match empty_since {
            // A reload drops the connection for a moment; don't mistake that for
            // the user quitting.
            Some(since) if since.elapsed() >= EXIT_GRACE => {
                tracing::info!("the last client left; winding down");
                let _ = shutdown.send(true);
                return;
            }
            Some(_) => {}
            None => empty_since = Some(tokio::time::Instant::now()),
        }
    }
}

/// The daemon logs beside the app but to its own file, so two processes never
/// interleave into one.
fn init_logging(data_dir: &std::path::Path) {
    use tracing_subscriber::{fmt, EnvFilter};
    let file_appender = tracing_appender::rolling::never(data_dir, "moin-engine.log");
    let filter = std::env::var("MOIN_LOG")
        .ok()
        .and_then(|s| EnvFilter::try_new(s).ok())
        .unwrap_or_else(|| EnvFilter::new("info"));
    let _ = fmt()
        .with_env_filter(filter)
        .with_writer(file_appender)
        .with_ansi(false)
        .try_init();
}
