//! The aria2c backend: a selectable alternative to the built-in engine for direct
//! HTTP downloads and torrents. moin runs one long-lived `aria2c --enable-rpc`
//! daemon and drives every transfer over JSON-RPC, so pause/resume/cancel/progress
//! and multi-connection splitting behave the same as the embedded backend from the
//! user's side.
//!
//! For **HTTP**, files line up with moin's own convention: aria2 writes to the same
//! `<dest>.part` the engine expects and we reuse [`http::finalize`] for the rename,
//! so the engine's cleanup and resume logic need no special cases. aria2's own
//! `.aria2` control sidecar is the one extra artifact, cleaned up here.
//!
//! For **torrents**, aria2 writes the real files straight into the task's output
//! folder (no `.part`), seeds with the configured ratio/time limits, and reports a
//! swarm tick plus a full live detail panel (files, peers, trackers, piece map,
//! availability) + live file re-selection and rate caps — parity with the built-in
//! engine. Only metadata resolution + the `.torrent` cache stay with the built-in
//! engine; aria2 downloads from the file it already resolved.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use super::backend::{
    Control, DownloadBackend, NetConfig, Outcome, ProgressFn, Signal, TorrentNet, TorrentTick,
    TransferOpts,
};
use super::fsattr;
use super::http;
use super::task::{PeerInfo, Task, TaskKind, TaskStatus, TorrentDetails, TorrentFile};
use super::tool::{new_command, Aria2Tool};
use super::torrent::{bucket_haves, distributed_copies, meta_path};

/// How often we poll aria2 for a task's progress.
const POLL: Duration = Duration::from_millis(300);
/// How long to wait for the RPC endpoint to come up after spawning the daemon.
const DAEMON_READY_TIMEOUT: Duration = Duration::from_secs(8);

pub struct Aria2Backend {
    tool: Arc<Aria2Tool>,
    /// Where the embedded engine caches resolved `.torrent` files — aria2 reads
    /// them to add a torrent without re-resolving a magnet from the swarm.
    data_dir: PathBuf,
    /// Latest torrent rate caps (from settings); applied per torrent download and
    /// pushed live to running torrents when settings change.
    net: StdMutex<TorrentNet>,
    /// Active torrents, info hash (lowercase hex) → aria2 GID, so the detail panel
    /// and live file/rate changes can find the download aria2 assigned.
    torrents: StdMutex<HashMap<String, String>>,
    /// A synchronously-readable copy of the current RPC handle (updated whenever the
    /// daemon (re)spawns), so `reconfigure` — a sync `&self` call — can push live
    /// rate changes to running torrents without touching the async daemon lock.
    last_rpc: StdMutex<Option<Rpc>>,
    daemon: Mutex<Option<Daemon>>,
}

/// The running aria2c process plus everything needed to talk to it.
struct Daemon {
    child: tokio::process::Child,
    rpc: Rpc,
}

/// A cheap, cloneable JSON-RPC handle — no lock held while a transfer polls.
#[derive(Clone)]
struct Rpc {
    client: reqwest::Client,
    endpoint: String,
    secret: String,
}

impl Aria2Backend {
    pub fn new(tool: Arc<Aria2Tool>, data_dir: PathBuf) -> Self {
        Self {
            tool,
            data_dir,
            net: StdMutex::new(default_net()),
            torrents: StdMutex::new(HashMap::new()),
            last_rpc: StdMutex::new(None),
            daemon: Mutex::new(None),
        }
    }

    /// The aria2 GID a torrent's download runs under, by info hash (lowercase hex).
    fn gid_for(&self, info_hash: &str) -> Option<String> {
        self.torrents
            .lock()
            .unwrap()
            .get(&info_hash.to_ascii_lowercase())
            .cloned()
    }

    /// Return a live RPC handle, (re)spawning the daemon if it isn't running.
    async fn rpc(&self) -> Result<Rpc, String> {
        let mut guard = self.daemon.lock().await;

        // Reap a daemon that has exited so we spawn a fresh one below.
        if let Some(d) = guard.as_mut() {
            if matches!(d.child.try_wait(), Ok(Some(_)) | Err(_)) {
                *guard = None;
            }
        }
        if let Some(d) = guard.as_ref() {
            return Ok(d.rpc.clone());
        }

        let bin = self
            .tool
            .binary()
            .ok_or_else(|| "aria2c isn't installed — set it up in Settings".to_string())?;
        let daemon = spawn_daemon(&bin).await?;
        let rpc = daemon.rpc.clone();
        *guard = Some(daemon);
        *self.last_rpc.lock().unwrap() = Some(rpc.clone());
        Ok(rpc)
    }
}

#[async_trait::async_trait]
impl DownloadBackend for Aria2Backend {
    fn id(&self) -> &'static str {
        "aria2"
    }

    fn label(&self) -> &'static str {
        "aria2c"
    }

    fn supports(&self, kind: TaskKind) -> bool {
        // HTTP over addUri, torrents over addTorrent. Media is yt-dlp's job later.
        matches!(kind, TaskKind::Http | TaskKind::Torrent)
    }

    fn available(&self) -> bool {
        self.tool.is_available()
    }

    fn reconfigure(&self, net: NetConfig) {
        // Store the caps for the next add, and push them live to every running
        // torrent so a rate change takes effect without re-adding (matches the
        // built-in engine's live session ratelimits).
        *self.net.lock().unwrap() = net.torrent;
        let gids: Vec<String> = self.torrents.lock().unwrap().values().cloned().collect();
        let rpc = self.last_rpc.lock().unwrap().clone();
        let (Some(rpc), false) = (rpc, gids.is_empty()) else {
            return;
        };
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let caps = net.torrent;
            handle.spawn(async move {
                for gid in gids {
                    let opts = json!({
                        "max-download-limit": limit_str(caps.download_bps),
                        "max-upload-limit": limit_str(caps.upload_bps),
                    });
                    let _ = rpc.call("aria2.changeOption", vec![json!(gid), opts]).await;
                }
            });
        }
    }

    async fn torrent_details(&self, info_hash: &str) -> Option<Result<TorrentDetails, String>> {
        Some(self.details(info_hash).await)
    }

    async fn set_torrent_files(
        &self,
        info_hash: &str,
        selected: &[usize],
    ) -> Option<Result<(), String>> {
        Some(self.set_files(info_hash, selected).await)
    }

    async fn run(
        &self,
        task: Task,
        opts: TransferOpts,
        control: Control,
        progress: ProgressFn,
    ) -> Outcome {
        match task.kind {
            TaskKind::Http => self.run_http(task, opts, control, progress).await,
            TaskKind::Torrent => self.run_torrent(task, opts, control, progress).await,
            _ => Outcome::Failed("aria2c can't handle this source yet".to_string()),
        }
    }

    async fn shutdown(&self) {
        // Ask the daemon to exit cleanly so it saves each torrent's control file
        // (auto-save already runs every second, so this just tightens the window)
        // and no process lingers past moin. Then drop the child handle.
        let mut guard = self.daemon.lock().await;
        if let Some(daemon) = guard.take() {
            let _ = daemon.rpc.call("aria2.shutdown", vec![]).await;
            // Give it a beat to flush and exit before the handle drops (which would
            // kill_on_drop it); ignore errors, this is best-effort cleanup.
            let mut child = daemon.child;
            let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
        }
    }
}

impl Aria2Backend {
    /// Direct HTTP download over aria2's `addUri`, writing to moin's `.part`.
    async fn run_http(
        &self,
        task: Task,
        opts: TransferOpts,
        control: Control,
        progress: ProgressFn,
    ) -> Outcome {
        let rpc = match self.rpc().await {
            Ok(r) => r,
            Err(e) => return Outcome::Failed(e),
        };

        let part = task.part_path();
        let dest = task.dest.clone();
        let control_file = format!("{part}.aria2");

        // A leftover `.part` with no aria2 control file can't be resumed (it may be
        // a pre-sized segmented partial from the built-in backend). Clear it so
        // aria2 starts clean rather than treating padding as real data.
        clear_unresumable(&part, &control_file, &task.meta_path()).await;

        let (dir, out) = match split_dest(&part) {
            Some(v) => v,
            None => return Outcome::Failed("bad destination path".to_string()),
        };

        let conns = opts.connections.max(1).to_string();
        // aria2 rejects a min-split-size below 1 MiB, so hold that as the floor —
        // it's also the built-in engine's default, so both split the same way.
        let min_split = opts.min_split_size.max(1 << 20).to_string();
        let mut options = json!({
            "dir": dir,
            "out": out,
            "continue": "true",
            "split": conns,
            "max-connection-per-server": conns,
            "min-split-size": min_split,
            "max-tries": "5",
        });
        // Captured browser headers (Cookie, Referer, User-Agent…) go through as
        // aria2's `header` option — an array of raw "Name: Value" lines — so an
        // auth-gated link downloads the same way the built-in backend sends it.
        if !task.headers.is_empty() {
            let lines: Vec<String> = task
                .headers
                .iter()
                .map(|(name, value)| format!("{name}: {value}"))
                .collect();
            options["header"] = json!(lines);
        }
        let gid = match rpc
            .call("aria2.addUri", vec![json!([task.url]), options])
            .await
        {
            Ok(Value::String(g)) => g,
            Ok(_) => return Outcome::Failed("aria2 returned an unexpected reply".to_string()),
            Err(e) => return Outcome::Failed(format!("aria2 couldn't start the download: {e}")),
        };

        poll_to_completion(
            &rpc,
            &gid,
            &part,
            &dest,
            &control_file,
            opts.hide_part,
            &control,
            &progress,
        )
        .await
    }

    /// Torrent download + seed over aria2's `addTorrent`. The real files land in the
    /// task's output folder; aria2 seeds until the ratio/time limit, then completes.
    async fn run_torrent(
        &self,
        task: Task,
        opts: TransferOpts,
        control: Control,
        progress: ProgressFn,
    ) -> Outcome {
        let rpc = match self.rpc().await {
            Ok(r) => r,
            Err(e) => return Outcome::Failed(e),
        };

        // aria2 downloads from the `.torrent` the embedded engine already resolved
        // and cached (or a local `.torrent` file the task points at). A bare magnet
        // with no cached metadata would drag in aria2's separate metadata phase, so
        // we require the resolved file instead — the add modal always caches it.
        let bytes = match self.torrent_bytes(&task).await {
            Some(b) => b,
            None => {
                return Outcome::Failed(
                    "couldn't read the torrent's metadata — re-add it so it resolves".to_string(),
                )
            }
        };

        let mut options = json!({
            "dir": task.dest.clone(),
            "continue": "true",
            "bt-save-metadata": "true",
            // Always set the ratio explicitly: aria2 defaults to 1.0, but moin's
            // "unlimited" means seed until stopped, which aria2 spells as 0.0.
            "seed-ratio": if opts.seed_ratio_limit > 0.0 {
                format!("{}", opts.seed_ratio_limit)
            } else {
                "0.0".to_string()
            },
        });
        // Only set a seed time when there's a limit: aria2 reads seed-time=0 as "do
        // not seed at all", which is the opposite of moin's "no time limit".
        if !opts.seed_time_limit.is_zero() {
            options["seed-time"] = json!(format!("{}", opts.seed_time_limit.as_secs_f64() / 60.0));
        }
        // Selected files as aria2's 1-based `select-file` list; omitted = all files.
        if let Some(list) = select_file(&task) {
            options["select-file"] = json!(list);
        }
        // Torrent rate caps from settings (per download, so HTTP over aria2 stays
        // unthrottled). Bytes/sec as a plain integer, which aria2 accepts.
        let net = *self.net.lock().unwrap();
        if let Some(bps) = net.download_bps {
            options["max-download-limit"] = json!(bps.get().to_string());
        }
        if let Some(bps) = net.upload_bps {
            options["max-upload-limit"] = json!(bps.get().to_string());
        }

        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let gid = match rpc
            .call("aria2.addTorrent", vec![json!(b64), json!([]), options])
            .await
        {
            Ok(Value::String(g)) => g,
            Ok(_) => return Outcome::Failed("aria2 returned an unexpected reply".to_string()),
            Err(e) => return Outcome::Failed(format!("aria2 couldn't add the torrent: {e}")),
        };

        // Register the GID so the detail panel + live file/rate changes can find it,
        // and clear it however this run ends.
        let hash = task.info_hash.as_ref().map(|h| h.to_ascii_lowercase());
        if let Some(h) = &hash {
            self.torrents.lock().unwrap().insert(h.clone(), gid.clone());
        }
        let outcome = poll_torrent(&rpc, &gid, &control, &progress).await;
        if let Some(h) = &hash {
            self.torrents.lock().unwrap().remove(h);
        }
        outcome
    }

    /// The `.torrent` bytes for a task: the embedded engine's cached copy first
    /// (keyed by info hash), else a local `.torrent` the task's source points at.
    async fn torrent_bytes(&self, task: &Task) -> Option<Vec<u8>> {
        if let Some(hash) = &task.info_hash {
            if let Ok(bytes) = tokio::fs::read(meta_path(&self.data_dir, hash)).await {
                return Some(bytes);
            }
        }
        if !is_magnet(&task.url) {
            return tokio::fs::read(&task.url).await.ok();
        }
        None
    }

    /// Live detail for an active aria2 torrent: files (with progress + selection),
    /// connected peers, trackers, the piece-have bar, and swarm availability — the
    /// same shape the built-in engine reports, assembled from aria2's RPC.
    async fn details(&self, info_hash: &str) -> Result<TorrentDetails, String> {
        let gid = self
            .gid_for(info_hash)
            .ok_or_else(|| "torrent isn't active".to_string())?;
        let rpc = self.rpc().await?;

        let status = rpc
            .call(
                "aria2.tellStatus",
                vec![
                    json!(gid),
                    json!(["bitfield", "numPieces", "bittorrent", "dir"]),
                ],
            )
            .await?;
        let files_raw = rpc.call("aria2.getFiles", vec![json!(gid)]).await?;
        // getPeers fails for a metadata-only phase; treat that as "no peers yet".
        let peers_raw = rpc
            .call("aria2.getPeers", vec![json!(gid)])
            .await
            .unwrap_or(Value::Null);

        let dir = status.get("dir").and_then(Value::as_str).unwrap_or("");
        let files = build_files(&files_raw, dir);
        let (peers, peer_bitfields) = build_peers(&peers_raw);
        let trackers = build_trackers(&status);

        let total_pieces = num(&status, "numPieces") as usize;
        let have = status
            .get("bitfield")
            .and_then(Value::as_str)
            .map(hex_to_bytes)
            .unwrap_or_default();
        let pieces = bucket_haves(&have, total_pieces);
        let avail = piece_availability(&peer_bitfields, total_pieces);
        let availability = distributed_copies(&avail, &have, total_pieces);

        Ok(TorrentDetails {
            files,
            peers,
            trackers,
            pieces,
            availability,
        })
    }

    /// Change which files an active aria2 torrent downloads, live, via aria2's
    /// `select-file` (1-based indices). An empty selection leaves it unchanged —
    /// aria2 can't have zero selected files.
    async fn set_files(&self, info_hash: &str, selected: &[usize]) -> Result<(), String> {
        if selected.is_empty() {
            return Ok(());
        }
        let gid = self
            .gid_for(info_hash)
            .ok_or_else(|| "torrent isn't active".to_string())?;
        let rpc = self.rpc().await?;
        let list = selected
            .iter()
            .map(|i| (i + 1).to_string())
            .collect::<Vec<_>>()
            .join(",");
        rpc.call(
            "aria2.changeOption",
            vec![json!(gid), json!({ "select-file": list })],
        )
        .await?;
        Ok(())
    }
}

/// Drive one aria2 torrent GID to a terminal [`Outcome`], reporting a swarm tick and
/// honoring pause/cancel. A finished torrent keeps seeding (aria2 stays `active`
/// with the ratio/time limit) until aria2 reports `complete` or moin stops it.
async fn poll_torrent(rpc: &Rpc, gid: &str, control: &Control, progress: &ProgressFn) -> Outcome {
    loop {
        let status = match rpc
            .call(
                "aria2.tellStatus",
                vec![
                    json!(gid),
                    json!([
                        "status",
                        "completedLength",
                        "totalLength",
                        "uploadLength",
                        "uploadSpeed",
                        "connections",
                        "numSeeders",
                        "errorMessage"
                    ]),
                ],
            )
            .await
        {
            Ok(v) => v,
            Err(e) => return Outcome::Failed(format!("lost contact with aria2: {e}")),
        };

        let completed = num(&status, "completedLength");
        let total = num(&status, "totalLength");
        // The selected files are done once we've fetched their whole size.
        let finished = total > 0 && completed >= total;

        match control.signal() {
            Signal::Pause => {
                stop(rpc, gid).await;
                // Pausing a finished torrent stops seeding → done; otherwise parked.
                return if finished {
                    Outcome::Completed
                } else {
                    Outcome::Paused
                };
            }
            Signal::Cancel => {
                stop(rpc, gid).await;
                return Outcome::Canceled;
            }
            Signal::Run => {}
        }

        let state = status.get("status").and_then(Value::as_str).unwrap_or("");
        match state {
            "complete" => {
                let _ = rpc
                    .call("aria2.removeDownloadResult", vec![json!(gid)])
                    .await;
                return Outcome::Completed;
            }
            "error" => {
                let msg = status
                    .get("errorMessage")
                    .and_then(Value::as_str)
                    .filter(|m| !m.is_empty())
                    .unwrap_or("torrent failed")
                    .to_string();
                let _ = rpc
                    .call("aria2.removeDownloadResult", vec![json!(gid)])
                    .await;
                return Outcome::Failed(msg);
            }
            "removed" => return Outcome::Canceled,
            _ => {}
        }

        let peers = num(&status, "connections") as u32;
        let seeders = num(&status, "numSeeders") as u32;
        let tick = TorrentTick {
            uploaded: num(&status, "uploadLength"),
            up_speed: num(&status, "uploadSpeed"),
            down_speed: num(&status, "downloadSpeed"),
            peers,
            seeders,
            leechers: peers.saturating_sub(seeders),
            status: if finished {
                TaskStatus::Seeding
            } else {
                TaskStatus::Downloading
            },
        };
        progress(completed, (total > 0).then_some(total), Some(tick));
        tokio::time::sleep(POLL).await;
    }
}

/// aria2's 1-based `select-file` value for a partial selection, or `None` when
/// every file is picked (download the whole torrent).
fn select_file(task: &Task) -> Option<String> {
    if task.files.is_empty() || task.files.iter().all(|f| f.selected) {
        return None;
    }
    let list = task
        .files
        .iter()
        .enumerate()
        .filter(|(_, f)| f.selected)
        .map(|(i, _)| (i + 1).to_string())
        .collect::<Vec<_>>()
        .join(",");
    (!list.is_empty()).then_some(list)
}

/// Whether a source string is a magnet link (aria2 would resolve its metadata
/// itself, which we avoid by using the embedded engine's cached `.torrent`).
fn is_magnet(source: &str) -> bool {
    source
        .trim_start()
        .to_ascii_lowercase()
        .starts_with("magnet:")
}

/// The rate caps aria2 starts with before settings are pushed in via `reconfigure`.
fn default_net() -> TorrentNet {
    TorrentNet {
        listen_port: 4240,
        dht: true,
        upnp: true,
        download_bps: None,
        upload_bps: None,
    }
}

/// A rate cap as aria2's `max-*-limit` wants it: the byte count, or `"0"` (aria2's
/// "no limit") when unset.
fn limit_str(bps: Option<std::num::NonZeroU32>) -> String {
    bps.map(|b| b.get().to_string())
        .unwrap_or_else(|| "0".to_string())
}

/// Parse aria2's hex bitfield string into raw bytes (big-endian, Msb0 — the same
/// layout the piece-bar helpers expect).
fn hex_to_bytes(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .filter_map(|i| u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok())
        .collect()
}

/// Build the file model from aria2's `getFiles`, making each path relative to the
/// torrent's output folder (aria2 reports absolute paths).
fn build_files(v: &Value, dir: &str) -> Vec<TorrentFile> {
    // Normalize separators before stripping — aria2 can report the dir and the file
    // path with different slashes on Windows, which would defeat a raw prefix match.
    let dir = dir.replace('\\', "/");
    v.as_array()
        .map(|arr| {
            arr.iter()
                .map(|f| {
                    let abs = f
                        .get("path")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .replace('\\', "/");
                    let rel = abs
                        .strip_prefix(&dir)
                        .unwrap_or(&abs)
                        .trim_start_matches('/')
                        .to_string();
                    TorrentFile {
                        path: rel,
                        size: num(f, "length"),
                        selected: f.get("selected").and_then(Value::as_str) == Some("true"),
                        received: num(f, "completedLength"),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Build the peer list from aria2's `getPeers`, returning the display rows and each
/// peer's have-bitfield (used to compute swarm availability).
fn build_peers(v: &Value) -> (Vec<PeerInfo>, Vec<Vec<u8>>) {
    let mut peers = Vec::new();
    let mut bitfields = Vec::new();
    let Some(arr) = v.as_array() else {
        return (peers, bitfields);
    };
    for p in arr {
        let ip = p.get("ip").and_then(Value::as_str).unwrap_or("");
        let port = p.get("port").and_then(Value::as_str).unwrap_or("");
        let is_seed = p.get("seeder").and_then(Value::as_str) == Some("true");
        let choking = p.get("peerChoking").and_then(Value::as_str) == Some("true");
        let state = if is_seed {
            "seed"
        } else if choking {
            "choked"
        } else {
            "active"
        };
        peers.push(PeerInfo {
            addr: format!("{ip}:{port}"),
            state: state.to_string(),
            // aria2 doesn't expose per-peer total bytes over RPC, only live speed.
            downloaded: 0,
        });
        if let Some(bf) = p.get("bitfield").and_then(Value::as_str) {
            bitfields.push(hex_to_bytes(bf));
        }
    }
    (peers, bitfields)
}

/// Flatten a torrent's announce list (from `tellStatus`'s `bittorrent.announceList`)
/// into a sorted, de-duplicated tracker URL list.
fn build_trackers(status: &Value) -> Vec<String> {
    let mut trackers: Vec<String> = status
        .get("bittorrent")
        .and_then(|b| b.get("announceList"))
        .and_then(Value::as_array)
        .map(|tiers| {
            tiers
                .iter()
                .filter_map(Value::as_array)
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    trackers.sort();
    trackers.dedup();
    trackers
}

/// Per-piece availability across the connected peers: how many hold each piece.
/// Feeds [`distributed_copies`] for the swarm "availability" figure.
fn piece_availability(bitfields: &[Vec<u8>], total: usize) -> Vec<u32> {
    let mut avail = vec![0u32; total];
    for bf in bitfields {
        for (i, a) in avail.iter_mut().enumerate() {
            if super::torrent::have_bit(bf, i) {
                *a += 1;
            }
        }
    }
    avail
}

/// Drive one aria2 GID to a terminal [`Outcome`], reporting progress and honoring
/// the supervisor's pause/cancel signals.
#[allow(clippy::too_many_arguments)]
async fn poll_to_completion(
    rpc: &Rpc,
    gid: &str,
    part: &str,
    dest: &str,
    control_file: &str,
    hide_part: bool,
    control: &Control,
    progress: &ProgressFn,
) -> Outcome {
    // Hide the .part + control file once aria2 has created them (checked each
    // poll until it lands). `finalize` clears the attribute on the finished file.
    let mut hidden_done = false;
    loop {
        if hide_part && !hidden_done && tokio::fs::metadata(part).await.is_ok() {
            fsattr::set_hidden(part, true);
            fsattr::set_hidden(control_file, true);
            hidden_done = true;
        }
        // React to pause/cancel first so a remove releases the file promptly. Both
        // stop the aria2 download; pause keeps the partial for resume, cancel lets
        // the engine drop it (and we drop aria2's control sidecar).
        match control.signal() {
            Signal::Pause => {
                stop(rpc, gid).await;
                return Outcome::Paused;
            }
            Signal::Cancel => {
                stop(rpc, gid).await;
                let _ = tokio::fs::remove_file(control_file).await;
                return Outcome::Canceled;
            }
            Signal::Run => {}
        }

        let status = match rpc
            .call(
                "aria2.tellStatus",
                vec![
                    json!(gid),
                    json!(["status", "completedLength", "totalLength", "errorMessage"]),
                ],
            )
            .await
        {
            Ok(v) => v,
            Err(e) => return Outcome::Failed(format!("lost contact with aria2: {e}")),
        };

        let completed = num(&status, "completedLength");
        let total = num(&status, "totalLength");
        progress(completed, (total > 0).then_some(total), None);

        match status.get("status").and_then(Value::as_str).unwrap_or("") {
            "complete" => {
                let _ = rpc
                    .call("aria2.removeDownloadResult", vec![json!(gid)])
                    .await;
                let _ = tokio::fs::remove_file(control_file).await;
                return http::finalize(part, dest).await;
            }
            "error" => {
                let msg = status
                    .get("errorMessage")
                    .and_then(Value::as_str)
                    .filter(|m| !m.is_empty())
                    .unwrap_or("download failed")
                    .to_string();
                let _ = rpc
                    .call("aria2.removeDownloadResult", vec![json!(gid)])
                    .await;
                let _ = tokio::fs::remove_file(control_file).await;
                return Outcome::Failed(msg);
            }
            "removed" => return Outcome::Canceled,
            // active / waiting / paused: keep polling.
            _ => tokio::time::sleep(POLL).await,
        }
    }
}

/// Stop an aria2 download and clear it from the stopped list. Aborting flushes the
/// control file, so a paused transfer can resume; the on-disk `.part` is untouched.
async fn stop(rpc: &Rpc, gid: &str) {
    let _ = rpc.call("aria2.remove", vec![json!(gid)]).await;
    let _ = rpc
        .call("aria2.removeDownloadResult", vec![json!(gid)])
        .await;
}

impl Rpc {
    /// One JSON-RPC call, secret-prefixed as aria2 expects. Returns the `result`
    /// field or the RPC error message.
    async fn call(&self, method: &str, mut params: Vec<Value>) -> Result<Value, String> {
        let mut full = Vec::with_capacity(params.len() + 1);
        full.push(json!(format!("token:{}", self.secret)));
        full.append(&mut params);
        let body = json!({
            "jsonrpc": "2.0",
            "id": "moin",
            "method": method,
            "params": full,
        });

        let resp = self
            .client
            .post(&self.endpoint)
            .json(&body)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let value: Value = resp.json().await.map_err(|e| e.to_string())?;
        if let Some(err) = value.get("error") {
            let msg = err
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("RPC error");
            return Err(msg.to_string());
        }
        Ok(value.get("result").cloned().unwrap_or(Value::Null))
    }
}

/// Spawn `aria2c` with RPC enabled on a free loopback port and wait for it to
/// answer. The daemon serves every aria2-backed download; moin's own concurrency
/// limit governs how many run at once, so aria2's is set out of the way.
async fn spawn_daemon(bin: &Path) -> Result<Daemon, String> {
    let port = free_port()?;
    let secret = uuid::Uuid::new_v4().to_string();

    let child = new_command(bin)
        .arg("--enable-rpc")
        .arg("--rpc-listen-all=false")
        .arg(format!("--rpc-listen-port={port}"))
        .arg(format!("--rpc-secret={secret}"))
        .arg("--continue=true")
        .arg("--auto-file-renaming=false")
        .arg("--allow-overwrite=true")
        .arg("--auto-save-interval=1")
        .arg("--max-concurrent-downloads=64")
        .arg("--quiet=true")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("couldn't start aria2c: {e}"))?;

    let rpc = Rpc {
        client: reqwest::Client::new(),
        endpoint: format!("http://127.0.0.1:{port}/jsonrpc"),
        secret,
    };

    // Poll getVersion until the RPC server is up.
    let deadline = tokio::time::Instant::now() + DAEMON_READY_TIMEOUT;
    loop {
        if rpc.call("aria2.getVersion", vec![]).await.is_ok() {
            return Ok(Daemon { child, rpc });
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("aria2c started but its RPC never came up".to_string());
        }
        tokio::time::sleep(Duration::from_millis(120)).await;
    }
}

/// Grab a free loopback TCP port by binding to :0 and reading it back.
fn free_port() -> Result<u16, String> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    listener
        .local_addr()
        .map(|a| a.port())
        .map_err(|e| e.to_string())
}

/// Split a `.part` path into (directory, filename) strings for aria2's `dir`/`out`.
fn split_dest(part: &str) -> Option<(String, String)> {
    let path = PathBuf::from(part);
    let dir = path.parent()?.to_string_lossy().into_owned();
    let out = path.file_name()?.to_string_lossy().into_owned();
    Some((dir, out))
}

/// Drop a `.part` (and its segmented `.meta`) that has no aria2 control file, so a
/// partial written by a different backend can't be mistaken for a resumable one.
async fn clear_unresumable(part: &str, control_file: &str, meta: &str) {
    let has_part = tokio::fs::metadata(part).await.is_ok();
    let has_control = tokio::fs::metadata(control_file).await.is_ok();
    if has_part && !has_control {
        let _ = tokio::fs::remove_file(part).await;
        let _ = tokio::fs::remove_file(meta).await;
    }
}

/// Read an aria2 numeric field (they come back as decimal strings) as `u64`.
fn num(status: &Value, key: &str) -> u64 {
    status
        .get(key)
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}
