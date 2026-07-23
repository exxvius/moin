//! The built-in backend: reqwest for HTTP and librqbit for BitTorrent. It's
//! always available and needs no external tools.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use super::backend::{Control, DownloadBackend, NetConfig, Outcome, ProgressFn, TransferOpts};
use super::http;
use super::task::{Task, TaskKind};
use super::torrent::TorrentEngine;

pub struct EmbeddedBackend {
    /// Behind a mutex so a connect-timeout change rebuilds it in place. Cloning a
    /// `reqwest::Client` is cheap (it's `Arc` inside), so `run` takes a clone and
    /// drops the lock before doing any I/O.
    client: Mutex<reqwest::Client>,
    /// The shared torrent session, built lazily on the first magnet/.torrent.
    torrent: TorrentEngine,
}

impl EmbeddedBackend {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            client: Mutex::new(build_client(None)),
            torrent: TorrentEngine::new(data_dir),
        }
    }
}

/// Build the HTTP client, applying `connect_timeout` when set.
fn build_client(connect_timeout: Option<Duration>) -> reqwest::Client {
    let mut builder =
        reqwest::Client::builder().user_agent(concat!("moin/", env!("CARGO_PKG_VERSION")));
    if let Some(timeout) = connect_timeout {
        builder = builder.connect_timeout(timeout);
    }
    builder.build().unwrap_or_default()
}

#[async_trait::async_trait]
impl DownloadBackend for EmbeddedBackend {
    fn id(&self) -> &'static str {
        "embedded"
    }

    fn label(&self) -> &'static str {
        "Built-in"
    }

    fn supports(&self, kind: TaskKind) -> bool {
        // HTTP over reqwest, torrents over librqbit; media is yt-dlp's job later.
        matches!(kind, TaskKind::Http | TaskKind::Torrent)
    }

    fn reconfigure(&self, net: NetConfig) {
        *self.client.lock().unwrap() = build_client(net.connect_timeout);
    }

    async fn run(
        &self,
        task: Task,
        opts: TransferOpts,
        control: Control,
        progress: ProgressFn,
    ) -> Outcome {
        match task.kind {
            TaskKind::Http => {
                let client = self.client.lock().unwrap().clone();
                let headers = http::build_headers(&task.headers);
                http::download(
                    &client,
                    &task.url,
                    &headers,
                    &task.part_path(),
                    &task.dest,
                    &task.meta_path(),
                    opts.connections.max(1),
                    opts.min_split_size,
                    opts.hide_part,
                    opts.stall_timeout,
                    &control,
                    &progress,
                )
                .await
            }
            TaskKind::Torrent => self.torrent.download(&task, &control, &progress).await,
            _ => Outcome::Failed("the built-in backend can't handle this source yet".to_string()),
        }
    }
}
