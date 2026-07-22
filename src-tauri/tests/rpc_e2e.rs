//! End-to-end test for browser-capture: a cookie-gated origin server + moin's
//! loopback RPC + the engine + the embedded backend, all offline.
//!
//! It proves the whole chain a browser extension will drive: POST a URL with
//! captured headers to `/add`, and moin downloads it the way the browser would —
//! including that the download *fails* without the cookie, so the header
//! passthrough is load-bearing rather than incidental.

use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use moin_lib::core::engine::{Emitter, Engine};
use moin_lib::core::task::{Task, TaskProgress, TaskStatus};
use moin_lib::rpc;
use serde_json::json;
use std::sync::OnceLock;
use tiny_http::{Response, Server};
use tokio::runtime::{Handle, Runtime};

/// A single long-lived multi-thread runtime standing in for Tauri's, shared by
/// every test so the engine's spawned transfers have an executor to run on.
fn test_runtime() -> Handle {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| Runtime::new().unwrap()).handle().clone()
}

/// Bytes the origin serves once the right cookie is presented.
const PAYLOAD: &[u8] = b"the quick brown fox jumps over the lazy dog, repeatedly and at length.";
/// The cookie the origin demands — stand-in for a real session cookie.
const COOKIE: &str = "session=s3cr3t";

/// A no-op [`Emitter`]; the test inspects state through `engine.list()` instead.
struct NoopEmitter;
impl Emitter for NoopEmitter {
    fn added(&self, _t: &Task) {}
    fn progress(&self, _p: &TaskProgress) {}
    fn updated(&self, _t: &Task) {}
    fn removed(&self, _id: &str) {}
}

/// Grab a free loopback port by binding to :0 and reading it back.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Start an origin server that only serves [`PAYLOAD`] when the request carries
/// the expected `Cookie` header; otherwise it answers 403. Returns the file URL.
fn start_origin() -> String {
    let server = Server::http("127.0.0.1:0").unwrap();
    let port = server.server_addr().to_ip().unwrap().port();
    thread::spawn(move || {
        for request in server.incoming_requests() {
            let has_cookie = request
                .headers()
                .iter()
                .any(|h| h.field.equiv("Cookie") && h.value.as_str() == COOKIE);
            let response = if has_cookie {
                Response::from_data(PAYLOAD.to_vec())
            } else {
                Response::from_string("forbidden").with_status_code(403)
            };
            let _ = request.respond(response);
        }
    });
    format!("http://127.0.0.1:{port}/file.bin")
}

/// Build an engine in a fresh temp data dir with the RPC server bound to a known
/// free port, and start it. Returns the engine, its base URL, its token, and the
/// download directory the RPC server writes into.
fn start_moin() -> (Engine, String, String, PathBuf) {
    let base = std::env::temp_dir().join(format!("moin-e2e-{}", uuid_like()));
    let data_dir = base.join("data");
    let download_dir = base.join("downloads");
    std::fs::create_dir_all(&data_dir).unwrap();
    std::fs::create_dir_all(&download_dir).unwrap();

    let engine = Engine::new(data_dir, Arc::new(NoopEmitter)).unwrap();

    // Pin the RPC port to a free one so the test never collides with the default
    // (or a second test run). set_settings persists, but a throwaway data dir is
    // discarded at process end.
    let port = free_port();
    let mut settings = engine.settings();
    settings.rpc_port = port;
    settings.rpc_enabled = true;
    let token = settings.rpc_token.clone();
    engine.set_settings(settings);

    rpc::spawn(engine.clone(), download_dir.clone(), test_runtime());
    wait_for_ping(port);

    (
        engine,
        format!("http://127.0.0.1:{port}"),
        token,
        download_dir,
    )
}

/// A cheap unique-ish suffix without pulling uuid into the test.
fn uuid_like() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// Block until the RPC server answers `/ping`, so requests don't race the bind.
fn wait_for_ping(port: u16) {
    let url = format!("http://127.0.0.1:{port}/ping");
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if reqwest::blocking::get(&url).is_ok() {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!("RPC server never came up on port {port}");
}

/// Poll the engine until `id` reaches a terminal-ish state, returning the task.
fn wait_for_status(engine: &Engine, id: &str, want: TaskStatus) -> Task {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let task = engine
            .list()
            .into_iter()
            .find(|t| t.id == id)
            .expect("task should exist");
        if task.status == want {
            return task;
        }
        assert!(
            Instant::now() < deadline,
            "task stuck in {:?}, wanted {:?} (error: {:?})",
            task.status,
            want,
            task.error
        );
        thread::sleep(Duration::from_millis(50));
    }
}

/// POST `/add` and return (status_code, body).
fn post_add(base: &str, token: Option<&str>, body: serde_json::Value) -> (u16, String) {
    let client = reqwest::blocking::Client::new();
    let mut req = client.post(format!("{base}/add")).json(&body);
    if let Some(t) = token {
        req = req.header("Authorization", format!("Bearer {t}"));
    }
    let resp = req.send().unwrap();
    let status = resp.status().as_u16();
    (status, resp.text().unwrap())
}

#[test]
fn capture_with_cookie_downloads_the_file() {
    let (engine, base, token, download_dir) = start_moin();
    let url = start_origin();

    let (status, body) = post_add(
        &base,
        Some(&token),
        json!({ "url": url, "filename": "renamed.bin", "headers": { "Cookie": COOKIE } }),
    );
    assert_eq!(status, 200, "add should succeed: {body}");

    let task: Task = serde_json::from_str(&body).unwrap();
    let done = wait_for_status(&engine, &task.id, TaskStatus::Completed);

    let bytes = std::fs::read(&done.dest).unwrap();
    assert_eq!(
        bytes, PAYLOAD,
        "downloaded bytes should match what the origin served"
    );
    assert!(done.dest.starts_with(download_dir.to_str().unwrap()));
    // The captured filename overrides the URL-derived one.
    assert_eq!(
        done.filename, "renamed.bin",
        "should use the passed filename"
    );
}

#[test]
fn capture_without_cookie_fails() {
    let (engine, base, token, _dir) = start_moin();
    let url = start_origin();

    // Same URL, but no captured headers — the origin answers 403 and the download
    // fails, proving the passthrough is what makes the gated download work.
    let (status, body) = post_add(&base, Some(&token), json!({ "url": url }));
    assert_eq!(status, 200, "add itself still succeeds: {body}");

    let task: Task = serde_json::from_str(&body).unwrap();
    let failed = wait_for_status(&engine, &task.id, TaskStatus::Failed);
    assert!(
        failed.error.unwrap_or_default().contains("403"),
        "failure should mention the 403 from the origin"
    );
}

#[test]
fn add_without_token_is_rejected() {
    let (_engine, base, _token, _dir) = start_moin();
    let url = start_origin();

    let (status, _body) = post_add(&base, None, json!({ "url": url }));
    assert_eq!(status, 401, "missing token must be rejected");

    let (status, _body) = post_add(&base, Some("wrong-token"), json!({ "url": url }));
    assert_eq!(status, 401, "wrong token must be rejected");
}

#[test]
fn ping_is_unauthenticated() {
    let (_engine, base, _token, _dir) = start_moin();
    let resp = reqwest::blocking::get(format!("{base}/ping")).unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body = resp.text().unwrap();
    assert!(
        body.contains("\"app\":\"moin\""),
        "ping should identify moin: {body}"
    );
}
