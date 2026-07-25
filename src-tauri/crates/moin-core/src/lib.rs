//! moin's headless download engine.
//!
//! Deliberately free of any shell or transport types: no Tauri, no HTTP. Whatever
//! hosts it talks to [`engine::Engine`] and implements [`engine::Emitter`] to
//! receive events. The daemon is that host today; unit tests are another.

pub mod aria2;
pub mod backend;
pub mod category;
pub mod embedded;
pub mod endpoint;
pub mod engine;
pub mod events;
pub mod fsattr;
pub mod http;
pub mod scrape;
pub mod segmented;
pub mod settings;
pub mod store;
pub mod task;
pub mod tool;
pub mod torrent;
pub mod update;
pub mod watch;

// Torrent (librqbit) and media (yt-dlp) backends land in later phases, alongside
// aria2c — each just a new `impl DownloadBackend` registered in the engine.
