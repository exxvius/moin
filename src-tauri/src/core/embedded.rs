//! The built-in backend: reqwest for HTTP today, librqbit for BitTorrent once
//! the torrent phase lands. It's always available and needs no external tools.

use super::backend::{Control, DownloadBackend, Outcome, ProgressFn, TransferOpts};
use super::http;
use super::task::{Task, TaskKind};

pub struct EmbeddedBackend {
    client: reqwest::Client,
}

impl EmbeddedBackend {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .user_agent(concat!("moin/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default();
        Self { client }
    }
}

impl Default for EmbeddedBackend {
    fn default() -> Self {
        Self::new()
    }
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
        // Torrent arrives with librqbit in a later phase; media is yt-dlp's job.
        matches!(kind, TaskKind::Http)
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
                http::download(
                    &self.client,
                    &task.url,
                    &task.part_path(),
                    &task.dest,
                    &task.meta_path(),
                    opts.connections.max(1),
                    &control,
                    &progress,
                )
                .await
            }
            _ => Outcome::Failed("the built-in backend can't handle this source yet".to_string()),
        }
    }
}
