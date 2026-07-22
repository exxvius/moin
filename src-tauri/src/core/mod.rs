//! Headless download engine — deliberately free of any Tauri types so it can be
//! unit-tested without the shell. The Tauri layer talks to [`engine::Engine`]
//! and implements [`engine::Emitter`] to forward events to the UI.

pub mod backend;
pub mod embedded;
pub mod engine;
pub mod http;
pub mod settings;
pub mod store;
pub mod task;

// Torrent (librqbit) and media (yt-dlp) backends land in later phases, alongside
// aria2c — each just a new `impl DownloadBackend` registered in the engine.
