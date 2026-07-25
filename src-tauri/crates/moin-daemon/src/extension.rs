//! The browser extension's way in.
//!
//! Kept as a separate listener on its own user-configurable port, exactly as it
//! was before the daemon existed: the extension is already deployed against this
//! contract (`GET /ping`, `POST /add`, bearer `rpc_token`, port from settings),
//! and none of that should shift just because the engine moved house.
//!
//! It stays separate from the control API for a practical reason too: the control
//! port is fixed, because binding it is how two UIs agree on who owns the engine.
//! The extension port is the user's to change, and a client that can't find a
//! moved port has no way back in.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tower_http::cors::CorsLayer;

use moin_core::engine::Engine;
use moin_core::task::Task;

#[derive(Clone)]
struct Ext {
    engine: Engine,
    /// OS Downloads folder, used when no explicit download dir is set.
    fallback_dir: PathBuf,
}

/// Bind and serve, if browser integration is switched on. A bind failure is
/// logged rather than fatal — the app is perfectly usable without it.
pub async fn serve(engine: Engine, fallback_dir: PathBuf, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    let settings = engine.settings();
    if !settings.rpc_enabled {
        tracing::info!("browser-integration RPC disabled in settings");
        return;
    }
    let addr = SocketAddr::from(([127, 0, 0, 1], settings.rpc_port));
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!("browser-integration RPC couldn't bind {addr}: {e}");
            return;
        }
    };
    tracing::info!("browser-integration RPC listening on http://{addr}");

    let app = Router::new()
        .route("/ping", get(ping))
        .route("/add", post(add))
        // Token auth means no cookies are involved, so a wildcard origin is safe —
        // and the extension's service worker needs it whatever the browser.
        .layer(CorsLayer::permissive())
        .with_state(Ext {
            engine,
            fallback_dir,
        });

    let served = axum::serve(listener, app).with_graceful_shutdown(async move {
        let _ = shutdown.changed().await;
    });
    if let Err(e) = served.await {
        tracing::warn!("browser-integration RPC stopped: {e}");
    }
}

#[derive(Serialize)]
struct Ping {
    app: &'static str,
    version: &'static str,
}

/// Unauthenticated on purpose: it lets the extension's options page confirm "moin
/// is running here" before a token has been entered. Reveals only name + version.
/// The shape is part of the extension's contract — don't change it.
async fn ping() -> Json<Ping> {
    Json(Ping {
        app: "moin",
        version: env!("CARGO_PKG_VERSION"),
    })
}

/// The JSON body of a `POST /add`. Only `url` is required; the rest refine where
/// and how the download runs.
#[derive(Deserialize)]
struct AddReq {
    url: String,
    #[serde(default)]
    category: Option<String>,
    /// The name the browser would have used (a link's `download` attribute, its
    /// chosen filename, or a parsed `Content-Disposition`). Sanitized engine-side.
    #[serde(default)]
    filename: Option<String>,
    /// Captured request headers (Cookie, Referer, User-Agent…) so an auth-gated
    /// link downloads the same way the browser would.
    #[serde(default)]
    headers: BTreeMap<String, String>,
}

struct ExtError(StatusCode, String);

impl IntoResponse for ExtError {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({ "error": self.1 }))).into_response()
    }
}

async fn add(
    State(ext): State<Ext>,
    headers: axum::http::HeaderMap,
    body: String,
) -> Result<Json<Task>, ExtError> {
    let settings = ext.engine.settings();
    // An empty configured token never matches, so a mis-seeded token can't open
    // the endpoint up.
    let presented = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    if settings.rpc_token.is_empty() || presented != Some(settings.rpc_token.as_str()) {
        return Err(ExtError(
            StatusCode::UNAUTHORIZED,
            "bad or missing token".to_string(),
        ));
    }

    let req: AddReq = serde_json::from_str(&body)
        .map_err(|e| ExtError(StatusCode::BAD_REQUEST, format!("bad JSON: {e}")))?;

    let dir = settings
        .download_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| ext.fallback_dir.clone());

    // The extension doesn't choose a category, so run the same matching rules the
    // manual add does (as a browser capture), using the captured filename so a URL
    // with no obvious extension still categorizes. An explicit category still wins.
    let category = match req.category {
        Some(id) => Some(id),
        None => ext
            .engine
            .categorize_capture(&req.url, req.filename.as_deref()),
    };

    // A magnet link routes to the torrent engine instead of the HTTP path: added
    // with all files, the default folder, and the same auto-category rules. The
    // torrent then resolves its metadata as it downloads.
    let result = if is_magnet(&req.url) {
        ext.engine
            .add_torrent(req.url, dir, category, Vec::new(), None, Vec::new(), false)
    } else {
        ext.engine
            .add_http(req.url, dir, category, req.headers, req.filename)
    };
    result
        .map(Json)
        .map_err(|e| ExtError(StatusCode::BAD_REQUEST, e))
}

/// Whether a captured URL is a magnet link (routed to the torrent engine).
fn is_magnet(url: &str) -> bool {
    url.trim_start().to_ascii_lowercase().starts_with("magnet:")
}
