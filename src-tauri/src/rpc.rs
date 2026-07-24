//! The loopback RPC server: the browser extension's way into moin.
//!
//! moin's engine only ever took work through Tauri `invoke` from its own webview.
//! The companion browser extension lives in another process, so this tiny
//! HTTP/1.1 server (bound to `127.0.0.1` only) turns a POST into the same
//! `engine.add_http(...)` call the webview makes — the aria2-daemon pattern
//! pointed the other way.
//!
//! It runs on its own OS thread (tiny_http is blocking), but queuing a download
//! ends in `tokio::spawn` deep inside the engine, so the handler enters a tokio
//! runtime [`Handle`] first — the same runtime Tauri drives, so RPC-added and
//! webview-added downloads share one executor. Two routes: `GET /ping`
//! (unauthenticated, so the extension can detect moin is up) and `POST /add`
//! (bearer-token gated). The token is read live from settings, so regenerating it
//! takes effect immediately.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use tiny_http::{Header, Method, Request, Response, Server};
use tokio::runtime::Handle;

use crate::core::engine::Engine;

/// Cap on the request body we'll read. Cookie headers get large, but a few
/// hundred KiB is far more than any legitimate capture needs.
const MAX_BODY: u64 = 512 * 1024;

/// Start the RPC server if it's enabled in settings. Binds `127.0.0.1:<rpc_port>`
/// and serves on a dedicated thread; a bind failure is logged, not fatal (the app
/// runs fine without browser integration). `fallback_dir` is the OS Downloads
/// folder, used when no explicit download dir is set — mirroring the command layer.
/// `rt` is a handle to the tokio runtime the engine spawns transfers on.
pub fn spawn(engine: Engine, fallback_dir: PathBuf, rt: Handle) {
    let settings = engine.settings();
    if !settings.rpc_enabled {
        tracing::info!("browser-integration RPC disabled in settings");
        return;
    }
    let addr = format!("127.0.0.1:{}", settings.rpc_port);

    let server = match Server::http(&addr) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("browser-integration RPC couldn't bind {addr}: {e}");
            return;
        }
    };
    tracing::info!("browser-integration RPC listening on http://{addr}");

    std::thread::Builder::new()
        .name("moin-rpc".to_string())
        .spawn(move || {
            for request in server.incoming_requests() {
                handle(&engine, &fallback_dir, &rt, request);
            }
        })
        .ok();
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

/// Route one request. Every reply carries permissive CORS headers so the
/// extension's service worker can read it regardless of browser.
fn handle(engine: &Engine, fallback_dir: &Path, rt: &Handle, request: Request) {
    let method = request.method().clone();
    let url = request.url().to_string();
    let path = url.split('?').next().unwrap_or("");

    // CORS preflight — the extension may send Authorization + JSON, which trips a
    // preflight in some browsers. Answer it before anything else.
    if method == Method::Options {
        let _ = request.respond(cors(Response::empty(204)));
        return;
    }

    match (&method, path) {
        (Method::Get, "/ping") => {
            // Unauthenticated on purpose: lets the options page confirm "moin is
            // running here" before the token is entered. Reveals only name+version.
            let body = format!(
                r#"{{"app":"moin","version":"{}"}}"#,
                env!("CARGO_PKG_VERSION")
            );
            respond_json(request, 200, &body);
        }
        (Method::Post, "/add") => handle_add(engine, fallback_dir, rt, request),
        _ => respond_json(request, 404, r#"{"error":"not found"}"#),
    }
}

/// Handle `POST /add`: check the bearer token, parse the body, queue the download.
fn handle_add(engine: &Engine, fallback_dir: &Path, rt: &Handle, mut request: Request) {
    let settings = engine.settings();
    if !bearer_ok(&request, &settings.rpc_token) {
        respond_json(request, 401, r#"{"error":"bad or missing token"}"#);
        return;
    }

    // Read the body under a hard cap so a runaway client can't exhaust memory.
    let mut body = String::new();
    if request
        .as_reader()
        .take(MAX_BODY)
        .read_to_string(&mut body)
        .is_err()
    {
        respond_json(
            request,
            400,
            r#"{"error":"couldn't read the request body"}"#,
        );
        return;
    }

    let req: AddReq = match serde_json::from_str(&body) {
        Ok(r) => r,
        Err(e) => {
            let msg = format!(r#"{{"error":"bad JSON: {}"}}"#, escape(&e.to_string()));
            respond_json(request, 400, &msg);
            return;
        }
    };

    let dir = settings
        .download_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| fallback_dir.to_path_buf());

    // The extension doesn't choose a category, so run the same matching rules the
    // manual add does (as a browser capture), using the captured filename so a URL
    // with no obvious extension still categorizes. An explicit category still wins.
    let category = match req.category {
        Some(id) => Some(id),
        None => engine.categorize_capture(&req.url, req.filename.as_deref()),
    };

    // Queuing ends in `tokio::spawn` inside the engine, which needs a runtime in
    // scope — this OS thread has none of its own, so borrow the engine's.
    // A magnet link routes to the torrent engine instead of the HTTP path: added
    // with all files, the default folder, and the same auto-category rules. The
    // torrent then resolves its metadata as it downloads.
    let _guard = rt.enter();
    let result = if is_magnet(&req.url) {
        engine.add_torrent(req.url, dir, category, Vec::new(), None, Vec::new(), false)
    } else {
        engine.add_http(req.url, dir, category, req.headers, req.filename)
    };
    match result {
        Ok(task) => match serde_json::to_string(&task) {
            Ok(json) => respond_json(request, 200, &json),
            Err(_) => respond_json(request, 500, r#"{"error":"couldn't serialize the task"}"#),
        },
        Err(e) => {
            let msg = format!(r#"{{"error":"{}"}}"#, escape(&e));
            respond_json(request, 400, &msg);
        }
    }
}

/// Whether a captured URL is a magnet link (routed to the torrent engine).
fn is_magnet(url: &str) -> bool {
    url.trim_start().to_ascii_lowercase().starts_with("magnet:")
}

/// Whether the request carries `Authorization: Bearer <token>` matching `token`.
/// An empty configured token never matches, so a mis-seeded token can't open the
/// endpoint up.
fn bearer_ok(request: &Request, token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Authorization"))
        .map(|h| h.value.as_str())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|got| got == token)
        .unwrap_or(false)
}

/// Add CORS headers to a response. Token auth means no cookies are involved, so a
/// wildcard origin is safe.
fn cors<R: Read>(resp: Response<R>) -> Response<R> {
    resp.with_header(header("Access-Control-Allow-Origin", "*"))
        .with_header(header("Access-Control-Allow-Methods", "GET, POST, OPTIONS"))
        .with_header(header(
            "Access-Control-Allow-Headers",
            "authorization, content-type",
        ))
}

/// Send a JSON response with the given status, CORS headers included. Consumes the
/// request. Errors here are unrecoverable (the socket is gone), so they're dropped.
fn respond_json(request: Request, status: u16, body: &str) {
    let resp = cors(Response::from_string(body))
        .with_status_code(status)
        .with_header(header("Content-Type", "application/json"));
    let _ = request.respond(resp);
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes())
        .expect("static header name/value are always valid")
}

/// Minimal JSON string escaping for error messages we build by hand (the values
/// are error text, not attacker-controlled, but a stray quote would break the JSON).
fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}
