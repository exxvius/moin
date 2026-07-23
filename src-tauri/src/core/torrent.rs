//! The embedded BitTorrent engine: an in-process librqbit [`Session`] shared by
//! every torrent task. Where the HTTP path owns a byte loop, a torrent runs
//! inside librqbit's own session — so this drives it by *observing* a handle
//! (poll its stats, map them onto progress + an [`Outcome`]) and nudging it
//! through [`Control`], much closer to the aria2 daemon pattern than to
//! `http::download`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use librqbit::{
    torrent_from_bytes, AddTorrent, AddTorrentOptions, ByteBufOwned, Magnet, Session,
    SessionOptions,
};
use tokio::sync::OnceCell;

use super::backend::{Control, Outcome, ProgressFn, Signal};
use super::task::{Task, TorrentSource};

/// How often we poll a torrent's stats into progress + re-check the control.
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Default range of TCP ports to accept incoming peers on. A configurable listen
/// port is a later milestone; a fixed range here just means the swarm can reach
/// us, which materially helps download speed.
const LISTEN_PORTS: std::ops::Range<u16> = 4240..4260;

/// Owns the lazily-built shared session. Building a session binds a listen port
/// and starts DHT, so it's deferred until the first torrent actually needs it —
/// users who only ever download over HTTP never pay for it.
pub struct TorrentEngine {
    data_dir: PathBuf,
    session: OnceCell<Arc<Session>>,
}

impl TorrentEngine {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            session: OnceCell::new(),
        }
    }

    /// The shared session, built on first use. librqbit keeps its own state
    /// (DHT cache, fastresume) under `<data>/torrent`.
    async fn session(&self) -> Result<Arc<Session>, String> {
        self.session
            .get_or_try_init(|| async {
                let store_dir = self.data_dir.join("torrent");
                let opts = SessionOptions {
                    // Restore piece state quickly after a restart / resume.
                    fastresume: true,
                    listen_port_range: Some(LISTEN_PORTS),
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

    /// Drive one torrent task to a terminal [`Outcome`]. `task.dest` is the output
    /// folder; librqbit manages its own partials + fastresume inside it.
    pub async fn download(&self, task: &Task, control: &Control, progress: &ProgressFn) -> Outcome {
        let session = match self.session().await {
            Ok(s) => s,
            Err(e) => return Outcome::Failed(e),
        };

        let add = match torrent_input(&task.url) {
            Ok(add) => add,
            Err(e) => return Outcome::Failed(e),
        };
        let opts = AddTorrentOptions {
            output_folder: Some(task.dest.clone()),
            // Write over existing partials so a resume picks up where it left off.
            overwrite: true,
            ..Default::default()
        };

        let handle = match session.add_torrent(add, Some(opts)).await {
            Ok(resp) => match resp.into_handle() {
                Some(h) => h,
                None => return Outcome::Failed("torrent produced no handle".to_string()),
            },
            Err(e) => return Outcome::Failed(format!("couldn't add the torrent: {e:#}")),
        };

        // A resume re-adds an already-managed torrent that we left paused; wake it.
        if handle.is_paused() {
            let _ = session.unpause(&handle).await;
        }

        loop {
            match control.signal() {
                Signal::Pause => {
                    let _ = session.pause(&handle).await;
                    return Outcome::Paused;
                }
                Signal::Cancel => {
                    let _ = session.delete(handle.id().into(), true).await;
                    return Outcome::Canceled;
                }
                Signal::Run => {}
            }

            let stats = handle.stats();
            if let librqbit::TorrentStatsState::Error = stats.state {
                let msg = stats.error.unwrap_or_else(|| "torrent error".to_string());
                return Outcome::Failed(msg);
            }

            // total_bytes is 0 until a magnet resolves its metadata — report an
            // unknown total until then so the bar shows a resolving state.
            let total = (stats.total_bytes > 0).then_some(stats.total_bytes);
            progress(stats.progress_bytes, total);

            if stats.finished && stats.total_bytes > 0 {
                // Seeding is a later milestone; for now stop uploading and settle
                // as complete once the payload is on disk.
                let _ = session.pause(&handle).await;
                progress(stats.total_bytes, total);
                return Outcome::Completed;
            }

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
