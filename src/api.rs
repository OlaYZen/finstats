//! Router, admin endpoints, image proxy and the embedded web UI.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Path, Query, Request, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, HOST, ORIGIN};
use axum::http::{HeaderValue, Method, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use futures_util::StreamExt;
use rust_embed::RustEmbed;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use tower_http::compression::CompressionLayer;
use tower_http::compression::predicate::{DefaultPredicate, NotForContentType, Predicate};

use crate::auth::{self, AuthUser, JellyfinAdmin, Manager};
use crate::state::{ApiError, ApiResult, App, Settings};
use crate::db::rusqlite::OptionalExtension;
use crate::{changelog, db, groups, import, pipeline, profile, recap, recent, security, services, stats, sync, timeline};

#[derive(RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/web"]
struct WebAssets;

pub fn router(app: App) -> Router {
    let api = Router::new()
        .route("/status", get(auth::status))
        .route("/setup/test", post(auth::setup_test))
        .route("/setup", post(auth::setup))
        .route("/auth/login", post(auth::login))
        .route("/auth/logout", post(auth::logout))
        .route("/auth/me", get(auth::me))
        .route("/summary", get(stats::summary))
        .route("/now-playing", get(stats::now_playing))
        .route("/stats/overview", get(stats::overview))
        .route("/stats/top", get(stats::top_handler))
        .route("/stats/heatmap", get(stats::heatmap_handler))
        .route("/stats/playback", get(stats::playback))
        .route("/stats/insights", get(stats::insights))
        .route("/stats/groups", get(groups::groups))
        .route("/library/insights", get(stats::library_insights))
        .route("/library/recent", get(recent::recently_added))
        .route("/server", get(stats::server))
        .route("/recap", get(recap::recap))
        .route("/changelog", get(changelog::changelog))
        .route("/activity", get(stats::activity))
        .route("/activity/{id}", get(stats::activity_detail))
        .route("/activity/{id}", delete(stats::activity_delete))
        .route("/users", get(stats::users))
        .route("/users/{id}", get(stats::user_detail))
        .route("/users/{id}/shows", get(profile::shows))
        .route("/users/{id}/timeline", get(timeline::timeline))
        .route("/me/seen", post(profile::mark_seen))
        .route("/libraries", get(stats::libraries))
        .route("/libraries/{id}", get(stats::library_detail))
        .route("/items/{id}", get(stats::item_detail))
        .route("/people/{id}", get(stats::person_detail))
        .route("/search", get(stats::search))
        .route("/events", get(stats::events))
        .route("/security", get(security::overview))
        .route("/security/alerts", get(security::alerts))
        .route("/security/alerts/resolve-all", post(security::resolve_all))
        .route("/security/alerts/{id}/resolve", post(security::resolve))
        .route("/security/alerts/{id}/reopen", post(security::reopen))
        .route("/security/database", post(security::download_database))
        .route("/img/item/{id}", get(item_image))
        .route("/img/user/{id}", get(user_image))
        .route("/img/arr/{service_id}/{media_id}", get(arr_image))
        .route("/upcoming", get(pipeline::upcoming))
        .route("/settings", get(get_settings).put(put_settings))
        .route("/permissions", get(get_permissions))
        .route("/permissions/defaults", axum::routing::put(put_default_permissions))
        .route("/permissions/users/{id}", axum::routing::put(put_user_permissions))
        .route("/services", get(services::list).post(services::create))
        .route("/services/test", post(services::test_connection))
        .route("/services/{id}", axum::routing::put(services::update).delete(services::remove))
        .route("/tasks", get(get_tasks))
        .route("/tasks/{id}/run", post(run_task))
        // Backups run to hundreds of MB and are streamed to disk, never buffered.
        .route("/import/jellystat", post(import_jellystat).layer(DefaultBodyLimit::disable()))
        .route("/backups", get(list_backups).post(create_backup))
        .route("/backups/restore", post(restore_upload).layer(DefaultBodyLimit::disable()))
        .route("/backups/{name}", get(download_backup).delete(delete_backup))
        .route("/backups/{name}/restore", post(restore_stored))
        .fallback(|| async { ApiError::not_found("Endpoint") })
        .layer(middleware::from_fn(same_origin));

    Router::new()
        .nest("/api", api)
        .fallback(static_handler)
        .layer(middleware::from_fn(security_headers))
        // A backup is gzip already. Compressing it again gains nothing, and the doubly-encoded stream
        // broke off in browsers: downloads failed without a word.
        .layer(CompressionLayer::new().gzip(true).compress_when(DefaultPredicate::new().and(NotForContentType::new("application/gzip"))))
        .with_state(app)
}

// ---------------------------------------------------------------- middleware

/// Browsers attach `Origin` to cross-site writes. Refuse any write whose origin is not us.
/// (SameSite=Lax cookies already cover this; this is the second lock.)
async fn same_origin(req: Request, next: Next) -> Response {
    if !matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS) {
        let headers = req.headers();
        if let (Some(origin), Some(host)) = (headers.get(ORIGIN).and_then(|v| v.to_str().ok()), headers.get(HOST).and_then(|v| v.to_str().ok())) {
            let forwarded = headers.get("x-forwarded-host").and_then(|v| v.to_str().ok());
            let origin_host = origin.split_once("://").map(|(_, h)| h).unwrap_or(origin);
            if origin_host != host && Some(origin_host) != forwarded {
                return ApiError::new(StatusCode::FORBIDDEN, "Cross-site request refused").into_response();
            }
        }
    }
    next.run(req).await
}

async fn security_headers(req: Request, next: Next) -> Response {
    let mut resp = next.run(req).await;
    let h = resp.headers_mut();
    h.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    h.insert("x-frame-options", HeaderValue::from_static("DENY"));
    h.insert("referrer-policy", HeaderValue::from_static("same-origin"));
    h.insert(
        "content-security-policy",
        HeaderValue::from_static(
            "default-src 'self'; img-src 'self' data: blob:; style-src 'self'; script-src 'self'; \
             object-src 'none'; base-uri 'self'; frame-ancestors 'none'; form-action 'self'",
        ),
    );
    resp
}

// ---------------------------------------------------------------- web UI

async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if let Some(file) = WebAssets::get(path).filter(|_| !path.is_empty()) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        // Fonts never change; everything else revalidates so upgrades show up immediately.
        let cache = if path.starts_with("assets/fonts/") { "public, max-age=31536000, immutable" } else { "no-cache" };
        let etag = format!("\"{}\"", hex::encode(&file.metadata.sha256_hash()[..8]));
        return Response::builder()
            .header(CONTENT_TYPE, mime.as_ref())
            .header(CACHE_CONTROL, cache)
            .header("etag", etag)
            .body(Body::from(file.data.into_owned()))
            .unwrap();
    }
    if path.starts_with("assets/") {
        return StatusCode::NOT_FOUND.into_response();
    }
    // Client-side routes: /users/abc, /settings, …
    match WebAssets::get("index.html") {
        Some(index) => Response::builder()
            .header(CONTENT_TYPE, "text/html; charset=utf-8")
            .header(CACHE_CONTROL, "no-cache")
            .body(Body::from(index.data.into_owned()))
            .unwrap(),
        None => (StatusCode::NOT_FOUND, "finstats was built without its web UI").into_response(),
    }
}

// ---------------------------------------------------------------- images

const WIDTHS: [u32; 6] = [64, 96, 160, 300, 480, 1280];
const IMAGE_TTL: Duration = Duration::from_secs(7 * 86_400);
const MISSING_TTL: Duration = Duration::from_secs(86_400);

#[derive(Deserialize)]
struct ImageQuery {
    kind: Option<String>,
    w: Option<u32>,
}

fn sniff(bytes: &[u8]) -> &'static str {
    match bytes {
        [0xFF, 0xD8, ..] => "image/jpeg",
        [0x89, b'P', b'N', b'G', ..] => "image/png",
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => "image/webp",
        [b'G', b'I', b'F', ..] => "image/gif",
        _ => "application/octet-stream",
    }
}

fn image_response(bytes: Vec<u8>) -> Response {
    Response::builder()
        .header(CONTENT_TYPE, sniff(&bytes))
        .header(CACHE_CONTROL, "private, max-age=604800")
        .body(Body::from(bytes))
        .unwrap()
}

/// Proxy + disk cache, so browsers never need to reach Jellyfin themselves (it is often
/// only reachable from the Docker network) and posters cost Jellyfin one resize each.
async fn cached_image(app: &App, cache_name: String, jf_path: String, width: u32) -> ApiResult<Response> {
    let jf = app.jellyfin();
    cached(app, cache_name, "Jellyfin", jf.map(|jf| async move { Ok(jf.image(&jf_path, width).await?.map(|(bytes, _)| bytes)) })).await
}

/// The disk cache around any image source: a fresh file is served as it is, a miss is remembered (as an empty
/// file) so grids of poster-less titles stay cheap, and when the source is down a stale poster beats a broken one.
/// `fetch` is `None` when there is nowhere to ask; it is only polled when the cache has no answer.
async fn cached<F>(app: &App, cache_name: String, source: &str, fetch: Option<F>) -> ApiResult<Response>
where
    F: std::future::Future<Output = anyhow::Result<Option<Vec<u8>>>>,
{
    let dir: PathBuf = app.data_dir.join("cache").join("img");
    let file = dir.join(&cache_name);
    if let Ok(meta) = tokio::fs::metadata(&file).await {
        let age = meta.modified().ok().and_then(|m| SystemTime::now().duration_since(m).ok()).unwrap_or(Duration::MAX);
        if meta.len() == 0 && age < MISSING_TTL {
            return Err(ApiError::not_found("Image"));
        }
        if meta.len() > 0 && age < IMAGE_TTL {
            if let Ok(bytes) = tokio::fs::read(&file).await {
                return Ok(image_response(bytes));
            }
        }
    }
    let fetch = fetch.ok_or_else(|| ApiError::not_found("Image"))?;
    let fetched = fetch.await;
    tokio::fs::create_dir_all(&dir).await.ok();
    match fetched {
        Ok(Some(bytes)) => {
            tokio::fs::write(&file, &bytes).await.ok();
            Ok(image_response(bytes))
        }
        Ok(None) => {
            tokio::fs::write(&file, b"").await.ok();
            Err(ApiError::not_found("Image"))
        }
        Err(e) => {
            if let Ok(bytes) = tokio::fs::read(&file).await {
                if !bytes.is_empty() {
                    return Ok(image_response(bytes));
                }
            }
            tracing::debug!("image fetch failed: {e:#}");
            Err(ApiError::new(StatusCode::BAD_GATEWAY, format!("Could not load the image from {source}")))
        }
    }
}

fn pick_width(w: Option<u32>) -> u32 {
    let want = w.unwrap_or(300);
    WIDTHS.iter().copied().find(|x| *x >= want).unwrap_or(1280)
}

fn valid_id(id: &str) -> Result<String, ApiError> {
    let id = db::norm_id(id);
    if id.len() == 32 && id.chars().all(|c| c.is_ascii_hexdigit()) { Ok(id) } else { Err(ApiError::not_found("Image")) }
}

async fn item_image(State(app): State<App>, _user: AuthUser, Path(id): Path<String>, Query(q): Query<ImageQuery>) -> ApiResult<Response> {
    let id = valid_id(&id)?;
    let width = pick_width(q.w);
    let (kind, jf_kind) = match q.kind.as_deref() {
        Some("backdrop") => ("backdrop", "Backdrop"),
        _ => ("primary", "Primary"),
    };
    cached_image(&app, format!("item-{id}-{kind}-{width}"), format!("/Items/{id}/Images/{jf_kind}"), width).await
}

async fn user_image(State(app): State<App>, _user: AuthUser, Path(id): Path<String>, Query(q): Query<ImageQuery>) -> ApiResult<Response> {
    let id = valid_id(&id)?;
    let width = pick_width(q.w.or(Some(96)));
    cached_image(&app, format!("user-{id}-{width}"), format!("/Users/{id}/Images/Primary"), width).await
}

/// The poster of a title that is not in the library yet: only Sonarr or Radarr has it. Both path segments
/// are numbers, the upstream path is a constant, and only posters of titles finstats itself has listed are
/// served, so this is no window into everything Sonarr and Radarr know.
async fn arr_image(State(app): State<App>, _user: AuthUser, Path((service_id, media_id)): Path<(i64, i64)>, Query(q): Query<ImageQuery>) -> ApiResult<Response> {
    let width = if q.w.unwrap_or(250) <= 250 { 250 } else { 500 };
    let svc = services::all(&app).iter().find(|s| s.id == service_id && s.enabled && s.kind.is_arr()).cloned().ok_or_else(|| ApiError::not_found("Image"))?;
    if media_id <= 0 || !app.db.call(move |c| pipeline::poster_is_listed(c, service_id, media_id)).await? {
        return Err(ApiError::not_found("Image"));
    }
    let worker = app.clone();
    cached(&app, format!("arr-{service_id}-{media_id}-{width}"), svc.kind.label(), Some(async move {
        let bytes = services::get_bytes(&worker, &svc, &crate::arr::poster_path(media_id, width), 6 * 1024 * 1024).await?;
        // Whatever comes back is only passed on if it is an image.
        Ok(bytes.filter(|b| sniff(b) != "application/octet-stream"))
    }))
    .await
}

/// Drop cached images nobody has refreshed in a month.
pub async fn prune_image_cache(app: App) {
    loop {
        let dir = app.data_dir.join("cache").join("img");
        let _ = tokio::task::spawn_blocking(move || {
            let Ok(entries) = std::fs::read_dir(&dir) else { return };
            for e in entries.flatten() {
                let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|m| SystemTime::now().duration_since(m).ok()).is_some_and(|a| a > Duration::from_secs(30 * 86_400));
                if old {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        })
        .await;
        tokio::time::sleep(Duration::from_secs(86_400)).await;
    }
}

// ---------------------------------------------------------------- settings & tasks

fn settings_json(app: &App) -> Value {
    let cfg = app.config.read().unwrap().clone();
    let mut v = serde_json::to_value(app.settings()).unwrap_or_else(|_| json!({}));
    if let (Some(obj), Some(c)) = (v.as_object_mut(), cfg) {
        obj.insert("jellyfin_url".into(), json!(c.url));
        obj.insert("server_name".into(), json!(c.server_name));
        obj.insert("server_version".into(), json!(c.server_version));
        obj.insert("configured_by_env".into(), json!(c.from_env));
    }
    v
}

/// The settings plus what is only read: the home addresses finstats knows, and who it would ask.
async fn settings_response(app: &App) -> ApiResult {
    let mut v = settings_json(app);
    let known = app.db.call(|c| crate::network::list(c)).await?;
    if let Some(obj) = v.as_object_mut() {
        obj.insert("known_home_addresses".into(), json!(known));
        obj.insert("public_ip_services".into(), json!(crate::network::services()));
        obj.insert("geoip".into(), security::status_json(app));
    }
    Ok(Json(v))
}

async fn get_settings(State(app): State<App>, Manager(_): Manager) -> ApiResult {
    settings_response(&app).await
}

async fn put_settings(State(app): State<App>, Manager(user): Manager, Json(patch): Json<Value>) -> ApiResult {
    let mut merged = serde_json::to_value(app.settings()).map_err(anyhow::Error::from)?;
    let (Some(target), Some(patch)) = (merged.as_object_mut(), patch.as_object()) else {
        return Err(ApiError::bad_request("Expected a JSON object"));
    };
    // Who gets in and what everyone may see is an administrator's call: a manager must not be able to promote themselves.
    if !user.is_admin && ACCESS_KEYS.iter().any(|k| patch.contains_key(*k)) {
        return Err(ApiError::forbidden());
    }
    for (k, v) in patch {
        if target.contains_key(k) {
            target.insert(k.clone(), v.clone());
        }
    }
    let next: Settings = serde_json::from_value(merged).map_err(|e| ApiError::bad_request(format!("Invalid settings: {e}")))?;
    next.validate().map_err(ApiError::bad_request)?;
    let raw = serde_json::to_string(&next).map_err(anyhow::Error::from)?;
    let regroup = (next.group_window_s != app.settings().group_window_s).then_some(next.group_window_s);
    let before = app.settings();
    let homes = (next.home_addresses != before.home_addresses).then(|| next.home_addresses.clone());
    let homes_changed = homes.is_some();
    let lookup_switched_on = next.public_ip_lookup && !before.public_ip_lookup;
    let geoip_switched_on = next.geoip_download && !before.geoip_download;
    let rules_changed = (next.travel_speed_kmh, next.travel_min_km) != (before.travel_speed_kmh, before.travel_min_km);
    app.db
        .call(move |c| {
            db::set_setting(c, "settings", &raw)?;
            // A different window means different groups, for the whole history.
            if let Some(window) = regroup {
                groups::detect(c, window, None)?;
            }
            // Home addresses decide which plays were local, for the whole history.
            if let Some(list) = &homes {
                crate::network::set_manual(c, list)?;
                crate::network::reclassify(c)?;
            }
            Ok(())
        })
        .await?;
    *app.settings.write().unwrap() = next;
    app.wake.notify_waiters();
    if lookup_switched_on {
        crate::network::refresh(&app).await;
    }
    if geoip_switched_on {
        crate::geo::refresh(&app).await;
    }
    if rules_changed || homes_changed {
        security::check(&app, None).await;
    }
    settings_response(&app).await
}

async fn get_tasks(State(app): State<App>, Manager(_): Manager) -> ApiResult {
    let data_dir = app.data_dir.clone();
    let dbinfo = app
        .db
        .call(move |c| {
            let (plays, oldest): (i64, Option<i64>) = c.query_row("SELECT COUNT(*), MIN(started_at) FROM playbacks", [], |r| Ok((r.get(0)?, r.get(1)?)))?;
            let items: i64 = c.query_row("SELECT COUNT(*) FROM items WHERE removed = 0", [], |r| r.get(0))?;
            let size: u64 = ["finstats.db", "finstats.db-wal"].iter().filter_map(|f| std::fs::metadata(data_dir.join(f)).ok()).map(|m| m.len()).sum();
            Ok(json!({ "size_bytes": size, "plays": plays, "items": items, "oldest_play_at": oldest }))
        })
        .await?;
    let collector = app.collector.read().unwrap().clone();
    Ok(Json(json!({ "tasks": app.tasks.snapshot(), "collector": collector, "db": dbinfo })))
}

/// Every task that can be started by hand. `Tasks::try_start` panics on an id it does not know, so a test
/// holds this list against `TASK_IDS`.
const RUNNABLE: [&str; 6] = ["sync_users", "sync_libraries", "sync_events", "sync_server", "sync_userdata", "sync_upcoming"];

async fn run_task(State(app): State<App>, Manager(_): Manager, Path(id): Path<String>) -> ApiResult<Response> {
    let Some(id) = RUNNABLE.into_iter().find(|t| *t == id) else { return Err(ApiError::not_found("Task")) };
    // One way in: the Jellyfin reads, then the ones that read from connected services.
    if !sync::spawn(&app, id) && !services::spawn(&app, id) {
        return Err(ApiError::new(StatusCode::CONFLICT, "That task is already running"));
    }
    Ok((StatusCode::ACCEPTED, Json(json!({ "ok": true }))).into_response())
}

// ---------------------------------------------------------------- permissions

const ACCESS_KEYS: [&str; 2] = ["allow_user_login", "default_permissions"];

const PERMISSION_INFO: [(&str, &str, &str); 5] = [
    ("sign_in", "Sign in", "May use finstats and sees their own statistics and recap."),
    ("see_everyone", "See everyone's activity", "Other people's statistics and history, the Users page and every live stream."),
    ("see_network", "See network details", "IP addresses, device ids and whether a play was local or remote."),
    ("see_server", "See the server", "The Server page, the server log, failed sign-ins and file paths."),
    ("manage", "Manage finstats", "Settings, tasks, the Jellystat import and deleting plays. Cannot change permissions."),
];

#[derive(Deserialize)]
struct PermissionsBody {
    #[serde(default)]
    permissions: Vec<String>,
}

fn clean_permissions(list: Vec<String>) -> Result<Vec<String>, ApiError> {
    if let Some(bad) = list.iter().find(|p| !auth::GRANTABLE.contains(&p.as_str())) {
        return Err(ApiError::bad_request(format!("Unknown permission `{bad}`")));
    }
    // Stored in a fixed order, without duplicates.
    Ok(auth::GRANTABLE.iter().filter(|g| list.iter().any(|p| p == *g)).map(|g| g.to_string()).collect())
}

fn defaults_as_keys(s: &Settings) -> Vec<String> {
    let mut out: Vec<String> = if s.allow_user_login { vec![auth::SIGN_IN.to_string()] } else { vec![] };
    out.extend(s.default_permissions.iter().cloned());
    out
}

async fn get_permissions(State(app): State<App>, JellyfinAdmin(_): JellyfinAdmin) -> ApiResult {
    let users = app
        .db
        .call(|c| {
            let mut stmt = c.prepare(
                "SELECT u.id, u.name, u.is_admin, u.is_disabled, u.removed, (u.image_tag IS NOT NULL), COALESCE(p.permissions, '[]')
                 FROM users u LEFT JOIN user_permissions p ON p.user_id = u.id
                 WHERE u.removed = 0 ORDER BY u.is_admin DESC, u.name COLLATE NOCASE",
            )?;
            let rows = stmt
                .query_map([], |r| {
                    let raw: String = r.get(6)?;
                    Ok(json!({
                        "id": r.get::<_, String>(0)?, "name": r.get::<_, String>(1)?, "is_admin": r.get::<_, bool>(2)?,
                        "is_disabled": r.get::<_, bool>(3)?, "has_image": r.get::<_, bool>(5)?,
                        "permissions": serde_json::from_str::<Value>(&raw).unwrap_or_else(|_| json!([])),
                    }))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?;
    let available: Vec<Value> = PERMISSION_INFO.iter().map(|(key, label, description)| json!({ "key": key, "label": label, "description": description })).collect();
    Ok(Json(json!({ "available": available, "defaults": defaults_as_keys(&app.settings()), "users": users })))
}

async fn put_default_permissions(State(app): State<App>, JellyfinAdmin(_): JellyfinAdmin, Json(body): Json<PermissionsBody>) -> ApiResult {
    let keys = clean_permissions(body.permissions)?;
    let mut next = app.settings();
    next.allow_user_login = keys.iter().any(|k| k == auth::SIGN_IN);
    next.default_permissions = keys.into_iter().filter(|k| k != auth::SIGN_IN).collect();
    let raw = serde_json::to_string(&next).map_err(anyhow::Error::from)?;
    app.db.call(move |c| db::set_setting(c, "settings", &raw)).await?;
    *app.settings.write().unwrap() = next.clone();
    Ok(Json(json!({ "defaults": defaults_as_keys(&next) })))
}

async fn put_user_permissions(State(app): State<App>, JellyfinAdmin(_): JellyfinAdmin, Path(id): Path<String>, Json(body): Json<PermissionsBody>) -> ApiResult {
    let id = db::norm_id(&id);
    let keys = clean_permissions(body.permissions)?;
    let stored = keys.clone();
    let found = app
        .db
        .call(move |c| {
            let is_admin: Option<bool> = c.query_row("SELECT is_admin FROM users WHERE id = ?1", [&id], |r| r.get(0)).optional()?;
            let Some(is_admin) = is_admin else { return Ok(None) };
            if is_admin {
                return Ok(Some(false));
            }
            if stored.is_empty() {
                c.execute("DELETE FROM user_permissions WHERE user_id = ?1", [&id])?;
            } else {
                c.execute(
                    "INSERT INTO user_permissions(user_id, permissions, updated_at) VALUES (?1, ?2, ?3)
                     ON CONFLICT(user_id) DO UPDATE SET permissions = excluded.permissions, updated_at = excluded.updated_at",
                    db::rusqlite::params![id, serde_json::to_string(&stored)?, db::now()],
                )?;
            }
            Ok(Some(true))
        })
        .await?;
    match found {
        None => Err(ApiError::not_found("User")),
        Some(false) => Err(ApiError::bad_request("Jellyfin administrators already have every permission")),
        Some(true) => Ok(Json(json!({ "permissions": keys }))),
    }
}

// ---------------------------------------------------------------- Jellystat import

// ---------------------------------------------------------------- backups
// Jellyfin administrators only: a backup is everyone's history, and restoring one can bring permissions back.

#[derive(Deserialize)]
struct RestoreQuery {
    /// Also restore settings and permissions (default: yes).
    settings: Option<bool>,
}

fn backup_path(app: &App, name: &str) -> ApiResult<PathBuf> {
    if !crate::backup::valid_name(name) {
        return Err(ApiError::not_found("Backup"));
    }
    let path = crate::backup::dir(&app.data_dir).join(name);
    if path.is_file() { Ok(path) } else { Err(ApiError::not_found("Backup")) }
}

async fn list_backups(State(app): State<App>, JellyfinAdmin(_): JellyfinAdmin) -> ApiResult {
    let dir = crate::backup::dir(&app.data_dir);
    let backups = tokio::task::spawn_blocking(move || crate::backup::list(&dir)).await.map_err(anyhow::Error::from)?;
    let s = app.settings();
    let next_at = (s.backup_every_d > 0).then(|| backups.first().and_then(|b| b["created_at"].as_i64()).map(|at| at + s.backup_every_d * 86_400)).flatten();
    Ok(Json(json!({ "backups": backups, "every_d": s.backup_every_d, "keep": s.backup_keep, "next_at": next_at })))
}

async fn create_backup(State(app): State<App>, JellyfinAdmin(_): JellyfinAdmin) -> ApiResult<Response> {
    if !sync::run_backup(&app, false) {
        return Err(ApiError::new(StatusCode::CONFLICT, "A backup is already being written"));
    }
    Ok((StatusCode::ACCEPTED, Json(json!({ "ok": true }))).into_response())
}

async fn download_backup(State(app): State<App>, JellyfinAdmin(_): JellyfinAdmin, Path(name): Path<String>) -> ApiResult<Response> {
    let path = backup_path(&app, &name)?;
    let file = tokio::fs::File::open(&path).await.map_err(anyhow::Error::from)?;
    let len = file.metadata().await.map_err(anyhow::Error::from)?.len();
    // Streamed from disk in 64 KB pieces: a large history never has to fit in memory.
    let stream = futures_util::stream::unfold(file, |mut f| async move {
        let mut buf = vec![0u8; 64 * 1024];
        match tokio::io::AsyncReadExt::read(&mut f, &mut buf).await {
            Ok(0) => None,
            Ok(n) => { buf.truncate(n); Some((Ok::<_, std::io::Error>(buf), f)) }
            Err(e) => Some((Err(e), f)),
        }
    });
    Ok(Response::builder()
        .header(CONTENT_TYPE, "application/gzip")
        .header("content-length", len)
        .header("content-disposition", format!("attachment; filename=\"{name}\""))
        .header(CACHE_CONTROL, "no-store")
        .body(Body::from_stream(stream))
        .unwrap())
}

async fn delete_backup(State(app): State<App>, JellyfinAdmin(_): JellyfinAdmin, Path(name): Path<String>) -> ApiResult {
    let path = backup_path(&app, &name)?;
    tokio::fs::remove_file(&path).await.map_err(anyhow::Error::from)?;
    Ok(Json(json!({ "ok": true })))
}

/// Run a restore in the background, then load what it may have changed (the settings) into the running app.
fn spawn_restore(app: &App, path: PathBuf, with_settings: bool, remove_after: bool) {
    let worker = app.clone();
    tokio::task::spawn_blocking(move || {
        let outcome = crate::backup::restore(&worker.db, &path, with_settings, Some((&worker.tasks, "restore")));
        if remove_after {
            let _ = std::fs::remove_file(&path);
        }
        if outcome.is_ok() {
            if let Ok(conn) = worker.db.conn() {
                if let Ok(loaded) = Settings::load(&conn) {
                    *worker.settings.write().unwrap() = loaded;
                }
            }
            worker.wake.notify_waiters();
        }
        worker.tasks.finish(
            "restore",
            outcome.map(|r| (format!("Restored {} plays ({} already present)", r.plays_imported, r.plays_skipped), serde_json::to_value(&r).ok())),
        );
    });
}

async fn restore_stored(State(app): State<App>, JellyfinAdmin(_): JellyfinAdmin, Path(name): Path<String>, Query(q): Query<RestoreQuery>) -> ApiResult<Response> {
    let path = backup_path(&app, &name)?;
    if !app.tasks.try_start("restore", "Reading backup") {
        return Err(ApiError::new(StatusCode::CONFLICT, "A restore is already running"));
    }
    spawn_restore(&app, path, q.settings.unwrap_or(true), false);
    Ok((StatusCode::ACCEPTED, Json(json!({ "ok": true }))).into_response())
}

async fn restore_upload(State(app): State<App>, JellyfinAdmin(_): JellyfinAdmin, Query(q): Query<RestoreQuery>, req: Request) -> ApiResult<Response> {
    if !app.tasks.try_start("restore", "Receiving backup") {
        return Err(ApiError::new(StatusCode::CONFLICT, "A restore is already running"));
    }
    let path = app.data_dir.join("restore-upload.tmp");
    let received = async {
        let mut file = tokio::fs::File::create(&path).await?;
        let mut stream = req.into_body().into_data_stream();
        let mut total = 0u64;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| anyhow::anyhow!("The upload was interrupted: {e}"))?;
            total += chunk.len() as u64;
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        anyhow::Ok(total)
    }
    .await;
    match received {
        Ok(n) if n > 0 => {}
        other => {
            let _ = tokio::fs::remove_file(&path).await;
            let msg = other.map(|_| "The uploaded file was empty".to_string()).unwrap_or_else(|e| format!("{e:#}"));
            app.tasks.finish("restore", Err(anyhow::anyhow!(msg.clone())));
            return Err(ApiError::bad_request(msg));
        }
    }
    spawn_restore(&app, path, q.settings.unwrap_or(true), true);
    Ok((StatusCode::ACCEPTED, Json(json!({ "ok": true }))).into_response())
}

async fn import_jellystat(State(app): State<App>, Manager(_): Manager, req: Request) -> ApiResult<Response> {
    if !app.tasks.try_start("import", "Receiving backup") {
        return Err(ApiError::new(StatusCode::CONFLICT, "An import is already running"));
    }
    let path = app.data_dir.join("jellystat-upload.tmp");
    let received = async {
        let mut file = tokio::fs::File::create(&path).await?;
        let mut stream = req.into_body().into_data_stream();
        let mut total = 0u64;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| anyhow::anyhow!("The upload was interrupted: {e}"))?;
            total += chunk.len() as u64;
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        anyhow::Ok(total)
    }
    .await;

    let total = match received {
        Ok(0) => {
            let _ = tokio::fs::remove_file(&path).await;
            app.tasks.finish("import", Err(anyhow::anyhow!("The uploaded file was empty")));
            return Err(ApiError::bad_request("The uploaded file was empty"));
        }
        Ok(n) => n,
        Err(e) => {
            let _ = tokio::fs::remove_file(&path).await;
            let msg = format!("{e:#}");
            app.tasks.finish("import", Err(e));
            return Err(ApiError::bad_request(msg));
        }
    };
    tracing::info!("received Jellystat backup ({:.1} MB)", total as f64 / 1e6);

    let worker = app.clone();
    tokio::task::spawn_blocking(move || {
        let outcome = import::run(&worker.db, &path, Some(&worker.tasks));
        let _ = std::fs::remove_file(&path);
        worker.tasks.finish(
            "import",
            outcome.map(|r| {
                let msg = format!("Imported {} plays ({} already present)", r.plays_imported, r.plays_skipped);
                (msg, serde_json::to_value(&r).ok())
            }),
        );
    });
    Ok((StatusCode::ACCEPTED, Json(json!({ "ok": true }))).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_task_that_can_be_started_is_known_and_has_exactly_one_runner() {
        for id in RUNNABLE {
            assert!(crate::state::TASK_IDS.contains(&id), "`{id}` would panic in Tasks::try_start");
            assert_eq!(sync::TASKS.contains(&id) as u8 + services::TASKS.contains(&id) as u8, 1, "`{id}` needs one runner");
        }
        for id in sync::TASKS.iter().chain(services::TASKS.iter()) {
            assert!(RUNNABLE.contains(id), "`{id}` cannot be started by hand");
        }
    }
}
