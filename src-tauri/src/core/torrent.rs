//! The embedded BitTorrent engine: an in-process librqbit [`Session`] shared by
//! every torrent task. Where the HTTP path owns a byte loop, a torrent runs
//! inside librqbit's own session — so this drives it by *observing* a handle
//! (poll its stats, map them onto progress + an [`Outcome`]) and nudging it
//! through [`Control`], much closer to the aria2 daemon pattern than to
//! `http::download`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use librqbit::{
    limits::LimitsConfig, torrent_from_bytes, AddTorrent, AddTorrentOptions, AddTorrentResponse,
    ByteBufOwned, Magnet, Session, SessionOptions, SessionPersistenceConfig, TorrentMetaV1Info,
};
use tokio::sync::OnceCell;

use super::backend::{Control, Outcome, ProgressFn, Signal, TorrentNet, TorrentTick, TransferOpts};
use super::task::{
    PeerInfo, ResolvedTorrent, Task, TaskStatus, TorrentDetails, TorrentFile, TorrentSource,
};

/// How often we poll a torrent's stats into progress + re-check the control.
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// How long to wait for a magnet's metadata before giving up — a magnet with no
/// reachable peers would otherwise hang forever.
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(60);

/// How long `add_torrent` may take before we give up. A magnet resolves its
/// metadata from the swarm inside the add, so this bounds a peerless magnet
/// instead of leaving the task wedged in "Connecting" forever.
const ADD_TIMEOUT: Duration = Duration::from_secs(180);

/// Wait until the control asks us to stop (Pause/Cancel), polling the shared
/// atomic. Used to race a possibly-slow `add_torrent` so a stuck add stays
/// cancellable — otherwise pause/cancel/remove do nothing until the add returns.
async fn wait_for_stop(control: &Control) -> Signal {
    loop {
        match control.signal() {
            Signal::Run => tokio::time::sleep(Duration::from_millis(150)).await,
            stop => return stop,
        }
    }
}

/// How often to re-scrape the trackers for seeder/leecher counts.
const SCRAPE_INTERVAL: Duration = Duration::from_secs(90);

/// Aborts a spawned task when dropped, so the scrape loop stops the moment the
/// download run returns (on any path).
struct AbortGuard(tokio::task::JoinHandle<()>);
impl Drop for AbortGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// How many ports past the configured listen port to try if it's busy, so a
/// taken port falls through to a neighbour instead of failing the whole session.
const LISTEN_PORT_SPAN: u16 = 20;

/// The config a session builds with before the engine's real settings are pushed
/// in (which happens at startup, before any torrent runs). Mirrors the settings
/// defaults so a session built without a reconfigure still behaves sensibly.
fn default_net() -> TorrentNet {
    TorrentNet {
        listen_port: 4240,
        dht: true,
        upnp: true,
        download_bps: None,
        upload_bps: None,
    }
}

/// Owns the lazily-built shared session. Building a session binds a listen port
/// and starts DHT, so it's deferred until the first torrent actually needs it —
/// users who only ever download over HTTP never pay for it. The current network
/// config is held behind a mutex: it's read when the session is built (port /
/// DHT / UPnP) and pushed live to a running session (rate caps).
pub struct TorrentEngine {
    data_dir: PathBuf,
    session: OnceCell<Arc<Session>>,
    net: Mutex<TorrentNet>,
}

impl TorrentEngine {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            session: OnceCell::new(),
            net: Mutex::new(default_net()),
        }
    }

    /// Apply changed network settings. Rate caps take effect immediately on a
    /// running session; the listen port, DHT, and UPnP toggles are baked when the
    /// session is built, so a change to those lands on the next app start.
    pub fn reconfigure(&self, net: TorrentNet) {
        *self.net.lock().unwrap() = net;
        if let Some(session) = self.session.get() {
            session.ratelimits.set_download_bps(net.download_bps);
            session.ratelimits.set_upload_bps(net.upload_bps);
        }
    }

    /// The shared session, built on first use. librqbit keeps its own state
    /// (DHT cache, fastresume) under `<data>/torrent`.
    async fn session(&self) -> Result<Arc<Session>, String> {
        self.session
            .get_or_try_init(|| async {
                let store_dir = self.data_dir.join("torrent");
                let net = *self.net.lock().unwrap();
                let start = net.listen_port.max(1);
                let opts = SessionOptions {
                    // Persist the have-bitfield so a reopened torrent restores from
                    // disk instead of re-hashing every file. `fastresume` only writes
                    // that state to disk when persistence is set (otherwise the fork
                    // uses a non-persistent bitfield and re-checks on every launch).
                    fastresume: true,
                    persistence: Some(SessionPersistenceConfig::Json {
                        folder: Some(store_dir.join("session")),
                    }),
                    // moin re-adds torrents from its own manifest, so librqbit must
                    // NOT auto-re-add every persisted torrent on startup (it would
                    // fight our resume and bloat the session with finished ones). This
                    // is the fork flag that keeps the persistent bitfield without the
                    // auto-restore. Deselected/removed torrents stay moin's call.
                    dont_restore_persisted: true,
                    listen_port_range: Some(start..start.saturating_add(LISTEN_PORT_SPAN)),
                    disable_dht: !net.dht,
                    enable_upnp_port_forwarding: net.upnp,
                    ratelimits: LimitsConfig {
                        download_bps: net.download_bps,
                        upload_bps: net.upload_bps,
                    },
                    ..Default::default()
                };
                // The default output folder is always overridden per task; it just
                // has to exist for the session to build.
                Session::new_with_opts(store_dir, opts)
                    .await
                    .map_err(|e| format!("couldn't start the torrent engine: {e:#}"))
            })
            .await
            .cloned()
    }

    /// Wind the session down on app exit: pause every managed torrent (which
    /// flushes its have-bitfield to disk) and cancel the session's tasks, so the
    /// next launch resumes from complete state instead of re-fetching the tail. A
    /// no-op if the session was never built (HTTP-only run).
    pub async fn shutdown(&self) {
        if let Some(session) = self.session.get() {
            session.stop().await;
        }
    }

    /// Where the resolved `.torrent` for an info hash is cached, so a magnet
    /// doesn't have to re-resolve from the swarm on download / restart.
    fn meta_path(&self, info_hash: &str) -> PathBuf {
        meta_path(&self.data_dir, info_hash)
    }

    /// Resolve a torrent's metadata (file list) without downloading it. For a
    /// magnet this fetches metadata from the swarm (bounded by [`RESOLVE_TIMEOUT`]);
    /// a `.torrent` file resolves from its own bytes. The resolved `.torrent` is
    /// cached so the real download reuses it.
    pub async fn resolve(&self, source: &str) -> Result<ResolvedTorrent, String> {
        let session = self.session().await?;
        let add = torrent_input(source)?;
        let opts = AddTorrentOptions {
            list_only: true,
            ..Default::default()
        };
        let resp = tokio::time::timeout(RESOLVE_TIMEOUT, session.add_torrent(add, Some(opts)))
            .await
            .map_err(|_| "timed out resolving the torrent — no peers answered".to_string())?
            .map_err(|e| format!("couldn't resolve the torrent: {e:#}"))?;

        let listing = match resp {
            AddTorrentResponse::ListOnly(l) => l,
            _ => return Err("unexpected response while resolving the torrent".to_string()),
        };
        let files = files_from_info(&listing.info)?;
        let total = files.iter().map(|f| f.size).sum();
        let name = listing
            .info
            .name
            .as_ref()
            .map(|n| String::from_utf8_lossy(n.as_ref()).into_owned())
            .filter(|s| !s.is_empty());
        let info_hash = listing.info_hash.as_string();
        // Cache the resolved .torrent next to our other torrent state.
        let path = self.meta_path(&info_hash);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, &listing.torrent_bytes).map_err(|e| e.to_string())?;

        Ok(ResolvedTorrent {
            info_hash,
            name,
            total,
            files,
        })
    }

    /// Live detail for a torrent that's currently in the session, looked up by
    /// info hash: its files (with per-file progress + selection), connected peers,
    /// and tracker URLs.
    pub async fn details(&self, info_hash: &str) -> Result<TorrentDetails, String> {
        let session = self.session().await?;
        let id = librqbit::api::TorrentIdOrHash::try_from(info_hash)
            .map_err(|_| "invalid info hash".to_string())?;
        let handle = session
            .get(id)
            .ok_or_else(|| "torrent isn't active".to_string())?;

        // Files from the resolved output layout (`file_infos` carries our rename
        // overrides — the actual on-disk paths), overlaid with live progress +
        // current selection.
        let mut files = handle
            .with_metadata(|md| {
                md.file_infos
                    .iter()
                    .map(|fi| TorrentFile {
                        path: fi.relative_filename.to_string_lossy().replace('\\', "/"),
                        size: fi.len,
                        selected: true,
                        received: 0,
                    })
                    .collect::<Vec<_>>()
            })
            .map_err(|e| e.to_string())?;
        let stats = handle.stats();
        for (i, f) in files.iter_mut().enumerate() {
            if let Some(recv) = stats.file_progress.get(i) {
                f.received = *recv;
            }
        }
        if let Some(only) = handle.only_files() {
            for (i, f) in files.iter_mut().enumerate() {
                f.selected = only.contains(&i);
            }
        }

        // Connected peers. `per_peer_stats_snapshot` needs a filter whose type
        // isn't publicly nameable — `Default::default()` infers it (defaults to
        // live peers only).
        let peers = handle
            .live()
            .map(|live| {
                live.per_peer_stats_snapshot(Default::default())
                    .peers
                    .into_iter()
                    .map(|(addr, p)| PeerInfo {
                        addr,
                        state: p.state.to_string(),
                        downloaded: p.counters.fetched_bytes,
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let mut trackers: Vec<String> = handle
            .shared()
            .trackers
            .iter()
            .map(|u| u.to_string())
            .collect();
        trackers.sort();

        // Piece-level have map (bucketed) + swarm availability, from the moin fork's
        // accessors (upstream rqbit keeps these internal).
        let total_pieces = handle.total_pieces().unwrap_or(0) as usize;
        let have = handle.have_pieces_bytes().unwrap_or_default();
        let avail = handle.piece_availability().unwrap_or_default();
        let pieces = bucket_haves(&have, total_pieces);
        let availability = distributed_copies(&avail, &have, total_pieces);

        Ok(TorrentDetails {
            files,
            peers,
            trackers,
            pieces,
            availability,
        })
    }

    /// Detach a torrent from the session (releasing its file handles), always
    /// *keeping* its files — moin deletes the actual data itself so it can delete
    /// only this torrent's files, never a shared folder it doesn't own. The cached
    /// `.torrent` is deliberately kept: an archived torrent can be re-downloaded,
    /// and the cache lets that re-add skip a slow swarm metadata resolve. Orphaned
    /// caches (info hashes no longer in moin's manifest) are swept by `prune_store`
    /// at launch. A torrent that was never started isn't in the session — fine.
    pub async fn remove(&self, info_hash: &str) -> Result<(), String> {
        let session = self.session().await?;
        let id = librqbit::api::TorrentIdOrHash::try_from(info_hash)
            .map_err(|_| "invalid info hash".to_string())?;
        let Some(handle) = session.get(id) else {
            return Ok(());
        };
        session
            .delete(handle.id().into(), false)
            .await
            .map_err(|e| format!("couldn't remove the torrent: {e:#}"))
    }

    /// Change which files an active torrent downloads, live. `selected` is the set
    /// of file indices to keep; librqbit stops fetching the rest.
    pub async fn set_files(&self, info_hash: &str, selected: &[usize]) -> Result<(), String> {
        let session = self.session().await?;
        let id = librqbit::api::TorrentIdOrHash::try_from(info_hash)
            .map_err(|_| "invalid info hash".to_string())?;
        let handle = session
            .get(id)
            .ok_or_else(|| "torrent isn't active".to_string())?;
        let set: HashSet<usize> = selected.iter().copied().collect();
        session
            .update_only_files(&handle, &set)
            .await
            .map_err(|e| format!("couldn't change the file selection: {e:#}"))
    }

    /// Drive one torrent task to a terminal [`Outcome`]. `task.dest` is the output
    /// folder; librqbit manages its own partials + fastresume inside it. `opts`
    /// carries the seed ratio/time limits that stop seeding once hit.
    pub async fn download(
        &self,
        task: &Task,
        opts: &TransferOpts,
        control: &Control,
        progress: &ProgressFn,
    ) -> Outcome {
        let session = match self.session().await {
            Ok(s) => s,
            Err(e) => return Outcome::Failed(e),
        };

        // Prefer the cached `.torrent` (saved when the torrent was resolved) so a
        // magnet doesn't re-resolve from the swarm; fall back to the raw source.
        let cached = task
            .info_hash
            .as_deref()
            .map(|h| self.meta_path(h))
            .filter(|p| p.exists());
        let add = match &cached {
            Some(path) => match AddTorrent::from_local_filename(&path.to_string_lossy()) {
                Ok(add) => add,
                Err(e) => return Outcome::Failed(format!("couldn't read the torrent: {e:#}")),
            },
            None => match torrent_input(&task.url) {
                Ok(add) => add,
                Err(e) => return Outcome::Failed(e),
            },
        };

        // Honor the task's file paths as output overrides so renames chosen at add
        // time land on disk (a no-op when a path equals the torrent's original).
        let overrides: Option<Vec<Option<PathBuf>>> = (!task.files.is_empty()).then(|| {
            task.files
                .iter()
                .map(|f| Some(PathBuf::from(&f.path)))
                .collect()
        });
        // Trackers stored on the task (its magnet, including any merged in from a
        // duplicate add) are passed as custom trackers so they augment the cached
        // `.torrent`'s own on every add. A no-op when the source is already the magnet.
        let extra_trackers = trackers_from_magnet(&task.url);
        let add_opts = AddTorrentOptions {
            output_folder: Some(task.dest.clone()),
            // Write over existing partials so a resume picks up where it left off.
            overwrite: true,
            only_files: selected_indices(&task.files),
            output_overrides: overrides,
            trackers: (!extra_trackers.is_empty()).then_some(extra_trackers),
            ..Default::default()
        };

        // Race the add against the control so a slow add (a magnet resolves its
        // metadata from the swarm here, before it returns a handle) stays responsive
        // to pause/cancel/remove instead of wedging the task in "Connecting". A hard
        // timeout bounds a peerless magnet.
        let handle = tokio::select! {
            biased;
            stop = wait_for_stop(control) => {
                return match stop {
                    Signal::Pause => Outcome::Paused,
                    _ => Outcome::Canceled,
                };
            }
            resp = tokio::time::timeout(ADD_TIMEOUT, session.add_torrent(add, Some(add_opts))) => {
                match resp {
                    Err(_) => {
                        return Outcome::Failed(
                            "timed out adding the torrent — no metadata or peers answered".to_string(),
                        )
                    }
                    Ok(Err(e)) => {
                        return Outcome::Failed(format!("couldn't add the torrent: {e:#}"))
                    }
                    Ok(Ok(resp)) => match resp.into_handle() {
                        Some(h) => h,
                        None => return Outcome::Failed("torrent produced no handle".to_string()),
                    },
                }
            }
        };

        // A resume re-adds an already-managed torrent that we left paused; wake it.
        if handle.is_paused() {
            let _ = session.unpause(&handle).await;
        }

        // Push the selection to the storage so it never creates/writes deselected
        // files (the add-time `only_files` reaches the piece picker but not the
        // storage; `update_only_files` reaches both). A no-op when everything's
        // selected. Deselected files then stay at 0 bytes on disk.
        if let Some(selected) = selected_indices(&task.files) {
            let _ = session
                .update_only_files(&handle, &selected.into_iter().collect())
                .await;
        }

        // Scrape the trackers for seeder/leecher counts in the background (librqbit
        // doesn't expose them), refreshing a shared value the poll loop reads.
        let scrape_state = Arc::new(std::sync::Mutex::new((0u32, 0u32)));
        let _scrape = {
            let state = scrape_state.clone();
            let hash = handle.info_hash().0;
            let trackers: Vec<String> = handle
                .shared()
                .trackers
                .iter()
                .map(|u| u.to_string())
                .collect();
            AbortGuard(tokio::spawn(async move {
                let client = reqwest::Client::new();
                loop {
                    if let Some(c) = super::scrape::scrape_best(&client, &trackers, &hash).await {
                        *state.lock().unwrap() = (c.seeders, c.leechers);
                    }
                    tokio::time::sleep(SCRAPE_INTERVAL).await;
                }
            }))
        };

        // Upload speed is derived from how much `uploaded` grew between polls.
        let mut last_uploaded = 0u64;
        // When this seeding session began — set the first time we observe finished,
        // so the seed-time limit counts from completion, not from add.
        let mut seed_start: Option<Instant> = None;
        // The bytes we resumed with. librqbit reports progress_bytes = 0 for the brief
        // Initializing window before it's live with the trusted have-set, so we floor
        // the reported progress at this to stop the bar dipping to 0 and crawling back
        // up on launch. Real progress only ever grows past it.
        let resumed_received = task.received;
        // Stall tracking: the last received total we saw grow, and when. If it
        // doesn't grow for `stall_timeout` while downloading, the torrent is stalled
        // (no data flowing) — unless the user force-started it. It flips back to
        // Downloading on its own the moment bytes arrive again.
        let mut last_progress = resumed_received;
        let mut last_progress_at = Instant::now();
        let info_hash = handle.info_hash();
        loop {
            let stats = handle.stats();
            let finished = stats.finished && stats.total_bytes > 0;
            // The received total, floored at what we resumed with (progress_bytes is
            // 0 during the brief Initializing window).
            let received = if finished {
                stats.total_bytes
            } else {
                stats.progress_bytes.max(resumed_received)
            };
            // Live per-byte download speed from the engine's estimator (MiB/s → B/s).
            let down_bps = stats
                .live
                .as_ref()
                .map(|l| (l.download_speed.mbps * 1024.0 * 1024.0) as u64)
                .unwrap_or(0);
            // Data is flowing if bytes are being fetched (per-byte) or a piece just
            // finished (per-piece). Either resets the stall timer, so the torrent
            // leaves Stalled the moment anything arrives — not only when a whole
            // piece verifies.
            if down_bps > 0 || received > last_progress {
                last_progress_at = Instant::now();
            }
            last_progress = last_progress.max(received);

            // Once finished, stop seeding when the ratio or time limit is hit (0 /
            // zero means unlimited; a force-seed run passes both as unlimited). We
            // pause the handle so uploading actually stops, then settle as done.
            if finished {
                let started = *seed_start.get_or_insert_with(Instant::now);
                let ratio = if stats.total_bytes > 0 {
                    stats.uploaded_bytes as f64 / stats.total_bytes as f64
                } else {
                    0.0
                };
                let ratio_hit = opts.seed_ratio_limit > 0.0 && ratio >= opts.seed_ratio_limit;
                let time_hit =
                    !opts.seed_time_limit.is_zero() && started.elapsed() >= opts.seed_time_limit;
                if ratio_hit || time_hit {
                    let _ = session.pause(&handle).await;
                    return Outcome::Completed;
                }
            }

            match control.signal() {
                Signal::Pause => {
                    let _ = session.pause(&handle).await;
                    // Pausing a finished torrent stops seeding → it's done; pausing
                    // one still downloading just parks it.
                    return if finished {
                        Outcome::Completed
                    } else {
                        Outcome::Paused
                    };
                }
                // Removal is engine-driven (it owns the keep-vs-delete choice and
                // calls `remove`), so the loop just stops — it never deletes here.
                Signal::Cancel => return Outcome::Canceled,
                Signal::Run => {}
            }

            // If the torrent was removed from the session out from under us (a
            // remove/delete), stop cleanly instead of polling a dead handle.
            if session.get(info_hash.into()).is_none() {
                return Outcome::Canceled;
            }

            if let librqbit::TorrentStatsState::Error = stats.state {
                let msg = stats.error.unwrap_or_else(|| "torrent error".to_string());
                return Outcome::Failed(msg);
            }

            // Map librqbit's phase onto our status: verifying/resolving = Checking,
            // Seeding once the selected files are done, Stalled when no data has
            // flowed for the stall window (not force-started), otherwise Downloading.
            let stalled = !control.force()
                && !opts.stall_timeout.is_zero()
                && last_progress_at.elapsed() >= opts.stall_timeout;
            let status = if finished {
                TaskStatus::Seeding
            } else if matches!(stats.state, librqbit::TorrentStatsState::Initializing) {
                TaskStatus::Checking
            } else if stalled {
                TaskStatus::Stalled
            } else {
                TaskStatus::Downloading
            };

            // Connected peers, from the live snapshot's aggregate.
            let peers = stats
                .live
                .as_ref()
                .map(|l| l.snapshot.peer_stats.live as u32)
                .unwrap_or(0);
            // uploaded is monotonic; the delta over one poll gives up-speed.
            let up_speed = stats
                .uploaded_bytes
                .saturating_sub(last_uploaded)
                .saturating_mul(1000)
                / POLL_INTERVAL.as_millis() as u64;
            last_uploaded = stats.uploaded_bytes;
            let (seeders, leechers) = *scrape_state.lock().unwrap();
            let tick = TorrentTick {
                uploaded: stats.uploaded_bytes,
                up_speed,
                down_speed: down_bps,
                peers,
                seeders,
                leechers,
                status,
            };

            // total_bytes is 0 until a magnet resolves its metadata — report an
            // unknown total until then so the bar shows a resolving state.
            let total = (stats.total_bytes > 0).then_some(stats.total_bytes);
            progress(received, total, Some(tick));

            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// Build a librqbit add-source from the task's stored source string: a magnet or
/// http(s) URL is fetched by librqbit; anything else is treated as a path to a
/// local `.torrent` file.
fn torrent_input(source: &str) -> Result<AddTorrent<'_>, String> {
    if is_remote(source) {
        Ok(AddTorrent::from_url(source))
    } else {
        AddTorrent::from_local_filename(source)
            .map_err(|e| format!("couldn't read the torrent file: {e:#}"))
    }
}

/// Most segments a piece bar is downsampled to — enough detail for the UI without
/// shipping a per-piece array for a torrent with tens of thousands of pieces.
const PIECE_BUCKETS: usize = 200;

/// Whether piece `i` is set in a big-endian (Msb0) have-bitfield.
pub(crate) fn have_bit(bytes: &[u8], i: usize) -> bool {
    bytes
        .get(i / 8)
        .map(|b| (b >> (7 - (i % 8))) & 1 == 1)
        .unwrap_or(false)
}

/// Bucket a have-bitfield into ≤[`PIECE_BUCKETS`] segments, each the fraction of
/// pieces in its range that we hold (0.0–1.0).
pub(crate) fn bucket_haves(bytes: &[u8], total: usize) -> Vec<f32> {
    if total == 0 {
        return Vec::new();
    }
    let buckets = total.min(PIECE_BUCKETS);
    let mut have = vec![0u32; buckets];
    for i in 0..total {
        if have_bit(bytes, i) {
            have[i * buckets / total] += 1;
        }
    }
    (0..buckets)
        .map(|b| {
            let start = b * total / buckets;
            let end = (b + 1) * total / buckets;
            let size = (end - start).max(1) as f32;
            have[b] as f32 / size
        })
        .collect()
}

/// Distributed copies of the torrent in the swarm: the rarest piece's
/// availability, counting a piece we already hold as one copy.
pub(crate) fn distributed_copies(avail: &[u32], have: &[u8], total: usize) -> f64 {
    if total == 0 {
        return 0.0;
    }
    (0..total)
        .map(|i| avail.get(i).copied().unwrap_or(0) + have_bit(have, i) as u32)
        .min()
        .unwrap_or(0) as f64
}

/// Prune librqbit's session store and our resolved-`.torrent` cache down to the
/// torrents moin still tracks. librqbit persists a session entry plus a fastresume
/// `.bitv` for every torrent ever added and — because moin drives resume from its
/// own manifest with `dont_restore_persisted` — never removes them on its own, so
/// they pile up (a long-lived install accrues hundreds). `keep` is the set of info
/// hashes (lowercase hex) moin still has a task for; every other entry/file is
/// dropped. Runs at startup, before the session is built.
pub fn prune_store(data_dir: &Path, keep: &HashSet<String>) {
    let store = data_dir.join("torrent");
    let session = store.join("session");

    // Rewrite session.json to only the kept torrents. Parsing to a generic value
    // and back preserves every field librqbit relies on; we only drop whole
    // entries whose info hash moin no longer tracks.
    let db_path = session.join("session.json");
    if let Ok(bytes) = std::fs::read(&db_path) {
        if let Ok(mut db) = serde_json::from_slice::<serde_json::Value>(&bytes) {
            if let Some(torrents) = db.get_mut("torrents").and_then(|t| t.as_object_mut()) {
                let before = torrents.len();
                torrents.retain(|_, v| {
                    v.get("info_hash")
                        .and_then(|h| h.as_str())
                        .map(|h| keep.contains(&h.to_ascii_lowercase()))
                        .unwrap_or(false)
                });
                if torrents.len() != before {
                    if let Ok(out) = serde_json::to_vec(&db) {
                        let _ = std::fs::write(&db_path, out);
                    }
                }
            }
        }
    }

    // Delete orphan fastresume/`.torrent` files (session store) and our own resolved
    // `.torrent` cache (meta) whose info hash moin no longer tracks. A still-tracked
    // torrent re-resolves a missing cache on demand, so this only ever sheds cruft.
    for dir in [session, store.join("meta")] {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let ext = path.extension().and_then(|x| x.to_str()).unwrap_or("");
            if !matches!(ext, "bitv" | "torrent") {
                continue;
            }
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if !keep.contains(&stem) {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

/// Delete the persisted fastresume bitfield for an info hash, so the next add does
/// a full on-disk check instead of trusting the saved have-set — the mechanism
/// behind a force-recheck. A no-op if there's no bitfield (or the torrent is still
/// in the session holding it mmap'd; the caller detaches first).
pub fn clear_resume(data_dir: &Path, info_hash: &str) {
    let bitv = data_dir
        .join("torrent")
        .join("session")
        .join(format!("{info_hash}.bitv"));
    let _ = std::fs::remove_file(bitv);
}

/// The cache path for a resolved `.torrent`, under `<data>/torrent/meta`.
pub fn meta_path(data_dir: &Path, info_hash: &str) -> PathBuf {
    data_dir
        .join("torrent")
        .join("meta")
        .join(format!("{info_hash}.torrent"))
}

/// Build a magnet URI for a torrent from its `.torrent` bytes: info hash, display
/// name, and every tracker in the file. Stored as the task's source so a torrent
/// added from a `.torrent` file can still be re-fetched from the swarm after the
/// file (and our cached copy) are gone — the browser-capture path converts a
/// downloaded `.torrent` into a task and deletes the file, so without this a later
/// re-download from Archive has nothing to resolve.
pub fn magnet_from_bytes(bytes: &[u8], info_hash: &str, name: Option<&str>) -> String {
    build_magnet(info_hash, name, &trackers_from_bytes(bytes))
}

/// Build a magnet URI from an info hash, optional display name, and a tracker list
/// (each added as a `&tr=` param, in order). Empty tracker entries are skipped.
pub fn build_magnet(info_hash: &str, name: Option<&str>, trackers: &[String]) -> String {
    let mut magnet = format!("magnet:?xt=urn:btih:{info_hash}");
    if let Some(n) = name.filter(|s| !s.is_empty()) {
        magnet.push_str("&dn=");
        magnet.push_str(&magnet_encode(n));
    }
    for url in trackers {
        let url = url.trim();
        if !url.is_empty() {
            magnet.push_str("&tr=");
            magnet.push_str(&magnet_encode(url));
        }
    }
    magnet
}

/// Every tracker in a `.torrent`'s bytes — `announce` plus each announce-list
/// tier, de-duplicated in order. Empty on malformed bytes.
pub fn trackers_from_bytes(bytes: &[u8]) -> Vec<String> {
    let Ok(meta) = torrent_from_bytes::<ByteBufOwned>(bytes) else {
        return Vec::new();
    };
    let mut raw: Vec<&ByteBufOwned> = Vec::new();
    if let Some(a) = meta.announce.as_ref() {
        raw.push(a);
    }
    for tier in &meta.announce_list {
        raw.extend(tier.iter());
    }
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for a in raw {
        let url = String::from_utf8_lossy(a.as_ref());
        let url = url.trim();
        if !url.is_empty() && seen.insert(url.to_string()) {
            out.push(url.to_string());
        }
    }
    out
}

/// The tracker URLs declared in a magnet URI (its `tr=` params), in order. Empty
/// for a non-magnet or unparseable string.
pub fn trackers_from_magnet(source: &str) -> Vec<String> {
    Magnet::parse(source.trim())
        .map(|m| m.trackers)
        .unwrap_or_default()
}

/// The trackers a torrent source advertises: a magnet's `tr=` params, or a local
/// `.torrent` file's announce list. Empty for a remote URL (not fetched here) or
/// anything unreadable.
pub fn trackers_from_source(source: &str) -> Vec<String> {
    let source = source.trim();
    if source.to_ascii_lowercase().starts_with("magnet:") {
        trackers_from_magnet(source)
    } else if is_remote(source) {
        Vec::new()
    } else {
        std::fs::read(source)
            .ok()
            .map(|b| trackers_from_bytes(&b))
            .unwrap_or_default()
    }
}

/// Percent-encode a magnet parameter value, escaping everything outside the
/// RFC 3986 unreserved set so names and tracker URLs survive round-tripping.
fn magnet_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Parse `.torrent` bytes into our file model (torrent order preserved, all files
/// selected by default). Used to rebuild the file list when a task is created.
pub fn files_from_bytes(bytes: &[u8]) -> Result<Vec<TorrentFile>, String> {
    let meta = torrent_from_bytes::<ByteBufOwned>(bytes)
        .map_err(|e| format!("not a valid .torrent file: {e:#}"))?;
    files_from_info(&meta.info)
}

/// Build our file model from librqbit's parsed torrent info.
fn files_from_info(info: &TorrentMetaV1Info<ByteBufOwned>) -> Result<Vec<TorrentFile>, String> {
    let details = info
        .iter_file_details()
        .map_err(|e| format!("couldn't read the torrent's files: {e:#}"))?;
    let mut files = Vec::new();
    for d in details {
        let path = d.filename.to_vec().unwrap_or_default().join("/");
        files.push(TorrentFile {
            path,
            size: d.len,
            selected: true,
            received: 0,
        });
    }
    Ok(files)
}

/// The librqbit `only_files` list for a task's selection: `None` when every file
/// is selected (download everything), else the selected indices.
fn selected_indices(files: &[TorrentFile]) -> Option<Vec<usize>> {
    if files.is_empty() || files.iter().all(|f| f.selected) {
        None
    } else {
        Some(
            files
                .iter()
                .enumerate()
                .filter(|(_, f)| f.selected)
                .map(|(i, _)| i)
                .collect(),
        )
    }
}

/// Whether a source is a link librqbit resolves itself (magnet or http URL) as
/// opposed to a local `.torrent` path.
fn is_remote(source: &str) -> bool {
    let lower = source.trim_start().to_ascii_lowercase();
    lower.starts_with("magnet:") || lower.starts_with("http://") || lower.starts_with("https://")
}

/// What we can show about a torrent the moment it's added, before the swarm
/// delivers full metadata: a display name and info hash when the source carries
/// them (always for a `.torrent` file, usually for a magnet).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TorrentMeta {
    pub name: Option<String>,
    pub info_hash: Option<String>,
    pub source: TorrentSource,
}

/// Read what we can out of a magnet URI or a local `.torrent` file up front.
/// A bare info-hash magnet has no name yet — that's fine, it fills in once
/// metadata resolves.
pub fn parse_meta(source: &str) -> Result<TorrentMeta, String> {
    let source = source.trim();
    if source.is_empty() {
        return Err("no torrent given".to_string());
    }
    if is_remote(source) {
        // http(s) URLs to a .torrent are resolved at download time; only magnets
        // carry a name/hash we can read without fetching.
        if source.to_ascii_lowercase().starts_with("magnet:") {
            let magnet =
                Magnet::parse(source).map_err(|e| format!("invalid magnet link: {e:#}"))?;
            Ok(TorrentMeta {
                name: magnet.name.clone(),
                info_hash: magnet.as_id20().map(|h| h.as_string()),
                source: TorrentSource::Magnet,
            })
        } else {
            Ok(TorrentMeta {
                name: None,
                info_hash: None,
                source: TorrentSource::Magnet,
            })
        }
    } else {
        let bytes =
            std::fs::read(source).map_err(|e| format!("couldn't read the torrent file: {e}"))?;
        let meta = torrent_from_bytes::<ByteBufOwned>(&bytes)
            .map_err(|e| format!("not a valid .torrent file: {e:#}"))?;
        let name = meta
            .info
            .name
            .as_ref()
            .map(|n| String::from_utf8_lossy(&n.0).into_owned())
            .filter(|s| !s.is_empty());
        Ok(TorrentMeta {
            name,
            info_hash: Some(meta.info_hash.as_string()),
            source: TorrentSource::File,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_magnet_name_and_hash() {
        let magnet = "magnet:?xt=urn:btih:a621779b5e3d486e127c3efbca9b6f8d135f52e5&dn=Some+Torrent+Name&tr=udp://tracker.example:80";
        let meta = parse_meta(magnet).unwrap();
        assert_eq!(meta.source, TorrentSource::Magnet);
        assert_eq!(meta.name.as_deref(), Some("Some Torrent Name"));
        assert_eq!(
            meta.info_hash.as_deref(),
            Some("a621779b5e3d486e127c3efbca9b6f8d135f52e5")
        );
    }

    #[test]
    fn magnet_without_name_still_parses_its_hash() {
        let magnet = "magnet:?xt=urn:btih:a621779b5e3d486e127c3efbca9b6f8d135f52e5";
        let meta = parse_meta(magnet).unwrap();
        assert!(meta.name.is_none());
        assert!(meta.info_hash.is_some());
    }

    #[test]
    fn rejects_empty_and_garbage_sources() {
        assert!(parse_meta("   ").is_err());
        assert!(parse_meta("magnet:?xt=urn:btih:notahash").is_err());
    }
}
