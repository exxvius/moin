//! The control API every moin UI talks to.
//!
//! One POST route per engine command, named exactly as the old Tauri command was,
//! taking exactly the same JSON body — so a client is a thin swap of `invoke(name,
//! args)` for `POST /api/<name>`, and the two UIs can't drift in what they can ask
//! for. Handlers return the value on success, or `{"error": "…"}` with a non-2xx
//! status, mirroring what `invoke` resolved and rejected with.
//!
//! Plus `GET /events`: the one long-lived connection, carrying the engine's event
//! stream (see [`events`]) and doubling as the daemon's liveness signal — when the
//! last one closes, the daemon winds down.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::stream::Stream;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::error::RecvError;
use tower_http::cors::CorsLayer;

use moin_core::backend::BackendInfo;
use moin_core::category::Category;
use moin_core::engine::Engine;
use moin_core::events;
use moin_core::settings::Settings;
use moin_core::task::{Task, TaskProgress, TorrentDetails, TorrentPreview};
use moin_core::tool::ToolStatus;

use crate::hub::{ClientGuard, Event, Hub};

#[derive(Clone)]
pub struct Api {
    pub engine: Engine,
    pub hub: Arc<Hub>,
    /// Bearer token minted at daemon start (see `endpoint::Endpoint::token`).
    pub token: String,
    /// OS Downloads folder, the fallback when no download dir is configured.
    pub downloads: PathBuf,
}

impl Api {
    /// Where a new download lands: the user's override, else the OS Downloads
    /// folder. Mirrors what the Tauri command layer used to compute.
    fn download_dir(&self) -> PathBuf {
        match self.engine.settings().download_dir {
            Some(dir) => PathBuf::from(dir),
            None => self.downloads.clone(),
        }
    }
}

/// An engine error on its way back to a client.
pub struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({ "error": self.1 }))).into_response()
    }
}

/// Engine calls return `Result<_, String>`; a failure is the caller's problem far
/// more often than ours, so it reads as a 400.
impl From<String> for ApiError {
    fn from(message: String) -> Self {
        Self(StatusCode::BAD_REQUEST, message)
    }
}

type ApiResult<T> = Result<Json<T>, ApiError>;

pub fn router(api: Api) -> Router {
    let guarded = Router::new()
        .route("/api/app_info", post(app_info))
        .route("/api/check_update", post(check_update))
        .route("/api/add_download", post(add_download))
        .route("/api/prepare_torrent", post(prepare_torrent))
        .route("/api/add_torrent", post(add_torrent))
        .route("/api/merge_torrent_trackers", post(merge_torrent_trackers))
        .route("/api/torrent_details", post(torrent_details))
        .route("/api/set_torrent_files", post(set_torrent_files))
        .route("/api/list_downloads", post(list_downloads))
        .route("/api/pause_download", post(pause_download))
        .route("/api/resume_download", post(resume_download))
        .route("/api/start_seeding", post(start_seeding))
        .route("/api/force_start", post(force_start))
        .route("/api/force_recheck", post(force_recheck))
        .route("/api/cancel_download", post(cancel_download))
        .route("/api/remove_download", post(remove_download))
        .route("/api/delete_download", post(delete_download))
        .route("/api/remove_downloads", post(remove_downloads))
        .route("/api/delete_downloads", post(delete_downloads))
        .route("/api/retry_download", post(retry_download))
        .route("/api/forget_download", post(forget_download))
        .route("/api/get_settings", post(get_settings))
        .route("/api/set_settings", post(set_settings))
        .route("/api/patch_settings", post(patch_settings))
        .route("/api/list_backends", post(list_backends))
        .route("/api/regenerate_rpc_token", post(regenerate_rpc_token))
        .route("/api/tool_status", post(tool_status))
        .route("/api/download_tool", post(download_tool))
        .route("/api/set_tool_path", post(set_tool_path))
        .route("/api/list_categories", post(list_categories))
        .route("/api/suggest_category", post(suggest_category))
        .route("/api/create_category", post(create_category))
        .route("/api/update_category", post(update_category))
        .route("/api/delete_category", post(delete_category))
        .route("/api/reorder_categories", post(reorder_categories))
        .route("/api/move_to_category", post(move_to_category))
        .route("/api/default_download_dir", post(default_download_dir))
        .route("/api/category_folder", post(category_folder))
        .route("/events", get(events))
        .layer(middleware::from_fn_with_state(api.clone(), require_token));

    Router::new()
        // Unauthenticated so a client can tell "the engine is up" from "something
        // else has the port" before it has a token to offer. Reveals nothing but
        // identity and version.
        .route("/health", get(health))
        .merge(guarded)
        // The UI is a webview on a different origin, so it preflights. Token auth
        // means no cookies ride along, which is what makes a wildcard origin safe.
        .layer(CorsLayer::permissive())
        .with_state(api)
}

/// Reject anything without the daemon's bearer token.
async fn require_token(
    State(api): State<Api>,
    request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let presented = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    if presented != Some(api.token.as_str()) {
        return Err(ApiError(
            StatusCode::UNAUTHORIZED,
            "bad or missing token".to_string(),
        ));
    }
    Ok(next.run(request).await)
}

#[derive(Serialize)]
struct Health {
    app: &'static str,
    version: &'static str,
    pid: u32,
}

async fn health() -> Json<Health> {
    Json(Health {
        app: "moin-engine",
        version: env!("CARGO_PKG_VERSION"),
        pid: std::process::id(),
    })
}

// ---- Events -----------------------------------------------------------------

/// Per-client event stream.
///
/// Each connection gets its own coalescing pipeline: raw progress ticks are
/// deduped against what *this* client was last sent and buffered by task id
/// (latest wins), then flushed as one batch on the configured cadence. Keeping it
/// per-client is what makes a late joiner correct — it starts from a snapshot and
/// its own empty dedup map, rather than inheriting a state someone else built.
async fn events(State(api): State<Api>) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let (rx, guard) = api.hub.subscribe();
    let state = Stream_ {
        rx,
        _guard: guard,
        engine: api.engine.clone(),
        buf: HashMap::new(),
        last_sent: HashMap::new(),
        pending: VecDeque::new(),
        deadline: Instant::now() + api.engine.settings().progress_flush_interval(),
        sent_snapshot: false,
    };

    let stream = futures_util::stream::unfold(state, |mut s| async move {
        loop {
            if let Some(event) = s.pending.pop_front() {
                return Some((Ok(event), s));
            }

            // The snapshot always goes first, so the client never has to fetch the
            // list separately and race the stream.
            if !s.sent_snapshot {
                s.sent_snapshot = true;
                s.pending
                    .push_back(json_event(events::TASK_SNAPSHOT, &s.engine.list()));
                continue;
            }

            tokio::select! {
                incoming = s.rx.recv() => match incoming {
                    Ok(Event::Progress(p)) => {
                        // Unchanged since this client's last batch? Never send it.
                        if s.last_sent.get(&p.id) != Some(&p) {
                            s.buf.insert(p.id.clone(), p);
                        }
                    }
                    Ok(Event::Added(task)) => s.pending.push_back(json_event(events::TASK_ADDED, &task)),
                    Ok(Event::Updated(task)) => s.pending.push_back(json_event(events::TASK_UPDATED, &task)),
                    Ok(Event::Completed(task)) => {
                        s.pending.push_back(json_event(events::TASK_COMPLETED, &task))
                    }
                    Ok(Event::Removed(id)) => {
                        // Drop the dedup record so the map can't retain dead ids and
                        // a re-added id starts clean.
                        s.last_sent.remove(&id);
                        s.buf.remove(&id);
                        s.pending.push_back(json_event(events::TASK_REMOVED, &id));
                    }
                    Ok(Event::ToolProgress { received, total }) => s.pending.push_back(json_event(
                        events::TOOL_PROGRESS,
                        &serde_json::json!({ "received": received, "total": total }),
                    )),
                    Ok(Event::SettingsChanged(settings)) => {
                        s.pending.push_back(json_event(events::SETTINGS_CHANGED, &settings))
                    }
                    Ok(Event::CategoriesChanged(list)) => {
                        s.pending.push_back(json_event(events::CATEGORIES_CHANGED, &list))
                    }
                    // This client fell far enough behind that the channel dropped
                    // messages for it. It may have missed an add or a removal, so
                    // its view can't be trusted — resynchronise from a snapshot
                    // rather than carrying a hole forward.
                    Err(RecvError::Lagged(missed)) => {
                        tracing::warn!("a client lagged {missed} events; re-snapshotting");
                        s.buf.clear();
                        s.last_sent.clear();
                        s.pending.push_back(json_event(events::TASK_SNAPSHOT, &s.engine.list()));
                    }
                    // The engine is going away; end the stream.
                    Err(RecvError::Closed) => return None,
                },
                _ = tokio::time::sleep_until(s.deadline.into()) => {
                    // Re-read the cadence every flush so a settings change applies
                    // from the next tick without reconnecting.
                    s.deadline = Instant::now() + s.engine.settings().progress_flush_interval();
                    if !s.buf.is_empty() {
                        let batch: Vec<TaskProgress> = s.buf.drain().map(|(_, v)| v).collect();
                        for p in &batch {
                            s.last_sent.insert(p.id.clone(), p.clone());
                        }
                        s.pending.push_back(json_event(events::TASK_PROGRESS_BATCH, &batch));
                    }
                }
            }
        }
    });

    // A comment ping keeps intermediaries and the client's socket from calling an
    // idle stream dead.
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

/// Everything the stream carries is JSON; a serialization failure here would mean
/// a type that can't round-trip, which is a bug rather than a runtime condition.
fn json_event<T: Serialize>(name: &str, payload: &T) -> SseEvent {
    SseEvent::default()
        .event(name)
        .data(serde_json::to_string(payload).unwrap_or_else(|_| "null".to_string()))
}

use std::convert::Infallible;

/// The per-connection pipeline state. Named awkwardly because it's only ever the
/// seed for the `unfold` above.
struct Stream_ {
    rx: tokio::sync::broadcast::Receiver<Event>,
    _guard: ClientGuard,
    engine: Engine,
    buf: HashMap<String, TaskProgress>,
    last_sent: HashMap<String, TaskProgress>,
    pending: VecDeque<SseEvent>,
    deadline: Instant,
    sent_snapshot: bool,
}

// ---- Commands ---------------------------------------------------------------

#[derive(Serialize)]
struct AppInfo {
    name: &'static str,
    version: &'static str,
}

async fn app_info() -> Json<AppInfo> {
    Json(AppInfo {
        name: "moin",
        version: env!("CARGO_PKG_VERSION"),
    })
}

async fn check_update() -> ApiResult<moin_core::update::UpdateInfo> {
    Ok(Json(
        moin_core::update::check(env!("CARGO_PKG_VERSION")).await?,
    ))
}

#[derive(Deserialize)]
struct AddDownload {
    url: String,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    headers: Option<BTreeMap<String, String>>,
    #[serde(default)]
    filename: Option<String>,
}

async fn add_download(State(api): State<Api>, Json(req): Json<AddDownload>) -> ApiResult<Task> {
    let dir = api.download_dir();
    Ok(Json(api.engine.add_http(
        req.url,
        dir,
        req.category,
        req.headers.unwrap_or_default(),
        req.filename,
    )?))
}

#[derive(Deserialize)]
struct Source {
    source: String,
}

async fn prepare_torrent(
    State(api): State<Api>,
    Json(req): Json<Source>,
) -> ApiResult<TorrentPreview> {
    let dir = api.download_dir();
    Ok(Json(api.engine.prepare_torrent(req.source, dir).await?))
}

#[derive(Deserialize)]
struct AddTorrent {
    source: String,
    dir: String,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    selected: Vec<usize>,
    #[serde(default)]
    folder: Option<String>,
    #[serde(default)]
    renames: Vec<String>,
}

async fn add_torrent(State(api): State<Api>, Json(req): Json<AddTorrent>) -> ApiResult<Task> {
    Ok(Json(api.engine.add_torrent(
        req.source,
        req.dir.into(),
        req.category,
        req.selected,
        req.folder,
        req.renames,
        false,
    )?))
}

async fn merge_torrent_trackers(
    State(api): State<Api>,
    Json(req): Json<Source>,
) -> ApiResult<Task> {
    Ok(Json(api.engine.merge_torrent_trackers(req.source).await?))
}

#[derive(Deserialize)]
struct Id {
    id: String,
}

async fn torrent_details(State(api): State<Api>, Json(req): Json<Id>) -> ApiResult<TorrentDetails> {
    Ok(Json(api.engine.torrent_details(&req.id).await?))
}

#[derive(Deserialize)]
struct SetTorrentFiles {
    id: String,
    selected: Vec<usize>,
}

async fn set_torrent_files(
    State(api): State<Api>,
    Json(req): Json<SetTorrentFiles>,
) -> ApiResult<()> {
    Ok(Json(
        api.engine.set_torrent_files(&req.id, req.selected).await?,
    ))
}

async fn list_downloads(State(api): State<Api>) -> Json<Vec<Task>> {
    Json(api.engine.list())
}

async fn pause_download(State(api): State<Api>, Json(req): Json<Id>) -> ApiResult<()> {
    Ok(Json(api.engine.pause(&req.id)?))
}

async fn resume_download(State(api): State<Api>, Json(req): Json<Id>) -> ApiResult<()> {
    Ok(Json(api.engine.resume(&req.id)?))
}

async fn start_seeding(State(api): State<Api>, Json(req): Json<Id>) -> ApiResult<()> {
    Ok(Json(api.engine.start_seeding(&req.id)?))
}

async fn force_start(State(api): State<Api>, Json(req): Json<Id>) -> ApiResult<()> {
    Ok(Json(api.engine.force_start(&req.id)?))
}

async fn force_recheck(State(api): State<Api>, Json(req): Json<Id>) -> ApiResult<()> {
    Ok(Json(api.engine.force_recheck(&req.id).await?))
}

async fn cancel_download(State(api): State<Api>, Json(req): Json<Id>) -> ApiResult<()> {
    Ok(Json(api.engine.cancel(&req.id).await?))
}

async fn remove_download(State(api): State<Api>, Json(req): Json<Id>) -> ApiResult<()> {
    Ok(Json(api.engine.remove(&req.id).await?))
}

async fn delete_download(State(api): State<Api>, Json(req): Json<Id>) -> ApiResult<()> {
    Ok(Json(api.engine.delete(&req.id).await?))
}

#[derive(Deserialize)]
struct Ids {
    ids: Vec<String>,
}

async fn remove_downloads(State(api): State<Api>, Json(req): Json<Ids>) -> ApiResult<()> {
    Ok(Json(api.engine.archive_many(req.ids, false).await?))
}

async fn delete_downloads(State(api): State<Api>, Json(req): Json<Ids>) -> ApiResult<()> {
    Ok(Json(api.engine.archive_many(req.ids, true).await?))
}

async fn retry_download(State(api): State<Api>, Json(req): Json<Id>) -> Json<()> {
    api.engine.retry(&req.id);
    Json(())
}

async fn forget_download(State(api): State<Api>, Json(req): Json<Id>) -> Json<()> {
    api.engine.forget(&req.id);
    Json(())
}

async fn get_settings(State(api): State<Api>) -> Json<Settings> {
    Json(api.engine.settings())
}

#[derive(Deserialize)]
struct SetSettings {
    settings: Settings,
}

/// Replace the whole settings object. Kept for anything that genuinely holds a
/// complete, current copy — but prefer `patch_settings`, which can't clobber a
/// field another client changed in the meantime.
async fn set_settings(State(api): State<Api>, Json(req): Json<SetSettings>) -> Json<()> {
    api.engine.set_settings(req.settings);
    api.hub
        .publish(Event::SettingsChanged(api.engine.settings()));
    Json(())
}

/// Merge a partial settings object into the current one.
///
/// This is what makes two open UIs safe. A whole-object save races: window A
/// loads settings, window B changes the seed ratio, then A saves its now-stale
/// copy and silently reverts B. Merging server-side against the *current* value
/// means each client only ever writes the fields it actually touched.
async fn patch_settings(
    State(api): State<Api>,
    Json(patch): Json<serde_json::Value>,
) -> ApiResult<Settings> {
    let current = serde_json::to_value(api.engine.settings())
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let serde_json::Value::Object(mut merged) = current else {
        return Err(ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "settings didn't serialize to an object".to_string(),
        ));
    };
    let serde_json::Value::Object(changes) = patch else {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "expected an object of settings fields".to_string(),
        ));
    };
    for (key, value) in changes {
        if !merged.contains_key(&key) {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                format!("unknown setting: {key}"),
            ));
        }
        merged.insert(key, value);
    }
    let next: Settings = serde_json::from_value(serde_json::Value::Object(merged))
        .map_err(|e| ApiError(StatusCode::BAD_REQUEST, e.to_string()))?;
    api.engine.set_settings(next);
    let saved = api.engine.settings();
    api.hub.publish(Event::SettingsChanged(saved.clone()));
    Ok(Json(saved))
}

async fn list_backends(State(api): State<Api>) -> Json<Vec<BackendInfo>> {
    Json(api.engine.backends())
}

async fn regenerate_rpc_token(State(api): State<Api>) -> Json<String> {
    let token = api.engine.regenerate_rpc_token();
    api.hub
        .publish(Event::SettingsChanged(api.engine.settings()));
    Json(token)
}

async fn tool_status(State(api): State<Api>) -> Json<ToolStatus> {
    Json(api.engine.tool_status().await)
}

async fn download_tool(State(api): State<Api>) -> ApiResult<ToolStatus> {
    let hub = api.hub.clone();
    Ok(Json(
        api.engine
            .install_tool(move |received, total| {
                hub.publish(Event::ToolProgress { received, total });
            })
            .await?,
    ))
}

#[derive(Deserialize)]
struct SetToolPath {
    #[serde(default)]
    path: Option<String>,
}

async fn set_tool_path(State(api): State<Api>, Json(req): Json<SetToolPath>) -> Json<ToolStatus> {
    Json(api.engine.set_tool_path(req.path).await)
}

async fn list_categories(State(api): State<Api>) -> Json<Vec<Category>> {
    Json(api.engine.categories())
}

#[derive(Deserialize)]
struct Url {
    url: String,
}

async fn suggest_category(State(api): State<Api>, Json(req): Json<Url>) -> Json<Option<String>> {
    Json(api.engine.suggest_category(&req.url))
}

#[derive(Deserialize)]
struct CategoryBody {
    category: Category,
}

/// The category mutators return the whole list so the calling client can replace
/// its copy in one step, and also broadcast it so every *other* client stays level.
fn categories_changed(api: &Api, list: Vec<Category>) -> Json<Vec<Category>> {
    api.hub.publish(Event::CategoriesChanged(list.clone()));
    Json(list)
}

async fn create_category(
    State(api): State<Api>,
    Json(req): Json<CategoryBody>,
) -> Json<Vec<Category>> {
    let list = api.engine.create_category(req.category);
    categories_changed(&api, list)
}

async fn update_category(
    State(api): State<Api>,
    Json(req): Json<CategoryBody>,
) -> Json<Vec<Category>> {
    let list = api.engine.update_category(req.category);
    categories_changed(&api, list)
}

async fn delete_category(State(api): State<Api>, Json(req): Json<Id>) -> Json<Vec<Category>> {
    let list = api.engine.delete_category(&req.id);
    categories_changed(&api, list)
}

async fn reorder_categories(State(api): State<Api>, Json(req): Json<Ids>) -> Json<Vec<Category>> {
    let list = api.engine.reorder_categories(req.ids);
    categories_changed(&api, list)
}

#[derive(Deserialize)]
struct MoveToCategory {
    ids: Vec<String>,
    #[serde(default)]
    category: Option<String>,
}

async fn move_to_category(State(api): State<Api>, Json(req): Json<MoveToCategory>) -> Json<()> {
    let dir = api.download_dir();
    api.engine.move_to_category(req.ids, req.category, dir);
    Json(())
}

async fn default_download_dir(State(api): State<Api>) -> Json<String> {
    Json(api.download_dir().to_string_lossy().into_owned())
}

#[derive(Deserialize)]
struct CategoryFolder {
    #[serde(default)]
    category: Option<String>,
}

async fn category_folder(State(api): State<Api>, Json(req): Json<CategoryFolder>) -> Json<String> {
    let dir = api.download_dir();
    Json(api.engine.category_folder(req.category, dir))
}
