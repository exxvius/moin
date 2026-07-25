//! moin's engine daemon, as a library.
//!
//! `main.rs` is a thin wrapper around [`run`]; the modules are public so the
//! integration tests can drive the real servers rather than a stand-in.

pub mod api;
pub mod extension;
pub mod hub;
pub mod paths;
