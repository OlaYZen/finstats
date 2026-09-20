//! Connections to the services around Jellyfin: Sonarr, Radarr, Seerr and the torrent clients.
//!
//! finstats only ever *reads* from them. Everything that makes that safe lives here:
//!
//! - **Own HTTP clients that follow no redirect.** reqwest drops `Authorization` and `Cookie` when a redirect
//!   leaves the host, but not `X-Api-Key`, and a 307 replays a POST body (a password) wherever it points.
//!   A redirect is therefore an answer ("enter that address instead"), never something to follow.
//! - **Secrets stay put.** `Service` is neither `Serialize` nor `Debug`, its JSON is written by hand, keys
//!   travel in headers and bodies and never in a URL, and an error says what happened (status, kind of
//!   failure) without repeating what the other side sent.
//! - Several connections of one kind are normal (a 4K Radarr, an anime Sonarr). An id is never reused, and
//!   whatever was read from a service belongs to that id: pointing a connection at another address throws
//!   its rows away, because ids from one instance mean nothing in the next.
//!
//! Only Jellyfin administrators may add or change a connection: it is a secret plus an address finstats will call.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use reqwest::header::{ACCEPT, HeaderMap, HeaderValue, LOCATION};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::JellyfinAdmin;
use crate::db::rusqlite::{Connection, OptionalExtension, params};
use crate::db;
use crate::state::{ApiError, ApiResult, App};

const MAX_SERVICES: usize = 20;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Sonarr,
    Radarr,
    Seerr,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Sonarr, Kind::Radarr, Kind::Seerr];

    pub fn key(self) -> &'static str {
        match self {
            Kind::Sonarr => "sonarr",
            Kind::Radarr => "radarr",
            Kind::Seerr => "seerr",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Sonarr => "Sonarr",
            Kind::Radarr => "Radarr",
            Kind::Seerr => "Seerr",
        }
    }

    pub fn from_key(key: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.key() == key)
    }

    fn example(self) -> &'static str {
        match self {
            Kind::Sonarr => "http://192.168.1.10:8989",
            Kind::Radarr => "http://192.168.1.10:7878",
            Kind::Seerr => "http://192.168.1.10:5055",
        }
    }

    fn what(self) -> &'static str {
        match self {
            Kind::Sonarr => "Series: what airs when, and what is downloading",
            Kind::Radarr => "Films: what is released when, and what is downloading",
            Kind::Seerr => "Requests: who asked for what (Seerr, Jellyseerr or Overseerr)",
        }
    }

    pub fn is_arr(self) -> bool {
        matches!(self, Kind::Sonarr | Kind::Radarr)
    }

}

// Deliberately not `Debug` and not `Serialize`: there is no way to print or send one by accident.
pub struct Service {
    pub id: i64,
    pub kind: Kind,
    pub name: String,
    pub url: String,
    pub username: Option<String>,
    secret: String,
    pub accept_invalid_certs: bool,
    pub enabled: bool,
}

impl Service {
    pub fn secret(&self) -> &str {
        &self.secret
    }
}

/// How a connection is doing. Kept in memory (the live loop asks every few seconds); the database only
/// hears about it when it changes, so that the last known state survives a restart.
#[derive(Clone, Default, PartialEq)]
pub struct Health {
    pub version: Option<String>,
    pub last_ok_at: Option<i64>,
    pub last_error: Option<String>,
}

pub struct Http {
    strict: reqwest::Client,
    /// For connections whose owner switched certificate checks off: a self-signed service on the LAN.
    lax: reqwest::Client,
}

impl Http {
    pub fn new() -> Self {
        let build = |lax: bool| {
            let mut headers = HeaderMap::new();
            headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
            reqwest::Client::builder()
                .default_headers(headers)
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(8))
                .timeout(Duration::from_secs(15))
                .pool_max_idle_per_host(2)
                .user_agent(concat!("finstats/", env!("CARGO_PKG_VERSION")))
                .danger_accept_invalid_certs(lax)
                .build()
                .expect("building http client")
        };
        Http { strict: build(false), lax: build(true) }
    }

    pub fn of(&self, svc: &Service) -> &reqwest::Client {
        if svc.accept_invalid_certs { &self.lax } else { &self.strict }
    }
}

// ---------------------------------------------------------------- addresses

/// What people type, made canonical, and nothing that could carry a secret or aim the request elsewhere:
/// no `user:password@`, no query, no fragment. A base path (`/sonarr`) stays.
pub fn clean_url(input: &str, kind: Kind) -> Result<String> {
    // Whatever scheme was typed must be http(s); a missing one is added below, and would otherwise hide another.
    let typed = input.trim().to_ascii_lowercase();
    if typed.contains("://") && !typed.starts_with("http://") && !typed.starts_with("https://") {
        bail!("The address must start with http:// or https://");
    }
    let url = crate::jellyfin::normalize_url_of(input, &format!("{}, like {}", kind.label(), kind.example()))?;
    let parsed = reqwest::Url::parse(&url).map_err(|_| anyhow!("That doesn't look like a valid URL"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        bail!("The address must start with http:// or https://");
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        bail!("Leave the user name and password out of the address; they have their own fields");
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        bail!("The address must not contain ? or #");
    }
    Ok(url)
}

/// Host, port and base path: when these change, it is another instance and its ids mean nothing here.
fn same_instance(a: &str, b: &str) -> bool {
    let key = |u: &str| reqwest::Url::parse(u).ok().map(|p| (p.host_str().map(str::to_ascii_lowercase), p.port_or_known_default(), p.path().trim_end_matches('/').to_string()));
    key(a).is_some() && key(a) == key(b)
}

// ---------------------------------------------------------------- talking

fn chain_mentions_certificate(e: &reqwest::Error) -> bool {
    let mut source: Option<&dyn std::error::Error> = Some(e);
    while let Some(s) = source {
        let text = s.to_string().to_ascii_lowercase();
        if text.contains("certificate") || text.contains("unknownissuer") || text.contains("self signed") || text.contains("self-signed") {
            return true;
        }
        source = s.source();
    }
    false
}

/// Why a request never got an answer, in words, and without the URL reqwest would print (it is in the sentence already).
pub fn explain(e: reqwest::Error, svc: &Service) -> anyhow::Error {
    let (label, url) = (svc.kind.label(), &svc.url);
    if chain_mentions_certificate(&e) {
        anyhow!("{url} presented a certificate finstats does not trust (self-signed, or from your own authority). Use its http:// address, or switch on “Accept a self-signed certificate” for this connection")
    } else if e.is_timeout() {
        anyhow!("{label} at {url} did not answer in time")
    } else if e.is_connect() {
        anyhow!("Could not connect to {url}. Is {label} running, and reachable from where finstats runs?")
    } else if e.is_decode() {
        anyhow!("{url} did not answer like {label}")
    } else {
        anyhow!("Could not reach {url}: {}", e.without_url())
    }
}

/// An answer that is not the one asked for. Says the status and what it usually means, never the body.
pub fn refuse(status: StatusCode, headers: &HeaderMap, svc: &Service) -> anyhow::Error {
    let (label, url) = (svc.kind.label(), &svc.url);
    if status.is_redirection() {
        let to: String = headers.get(LOCATION).and_then(|v| v.to_str().ok()).unwrap_or("another address").chars().filter(|c| !c.is_control()).take(200).collect();
        return anyhow!("{url} answers with a redirect to {to}. finstats follows no redirects, so that a key can never end up somewhere else: enter the final address, including any base path");
    }
    match (status.as_u16(), ()) {
        (401 | 403, _) => anyhow!("{label} refused the API key"),
        (404, _) => anyhow!("{url} answered 404 Not Found. Is the address right, including a base path like /{}?", svc.kind.key()),
        _ => anyhow!("{label} answered {status}"),
    }
}

/// `GET` on Sonarr, Radarr or Seerr. The key is a header; the query never carries it.
pub async fn get_json(app: &App, svc: &Service, path: &str, query: &[(&str, String)]) -> Result<Value> {
    let resp = app.services_http.of(svc).get(format!("{}{path}", svc.url)).header("X-Api-Key", svc.secret()).query(query).send().await.map_err(|e| explain(e, svc))?;
    if !resp.status().is_success() {
        return Err(refuse(resp.status(), resp.headers(), svc));
    }
    resp.json::<Value>().await.map_err(|e| explain(e, svc))
}

/// Bytes from Sonarr or Radarr (a poster). `None` for a 404 and for anything larger than `cap`.
pub async fn get_bytes(app: &App, svc: &Service, path: &str, cap: usize) -> Result<Option<Vec<u8>>> {
    let resp = app.services_http.of(svc).get(format!("{}{path}", svc.url)).header("X-Api-Key", svc.secret()).header(ACCEPT, "image/*").send().await.map_err(|e| explain(e, svc))?;
    if resp.status() == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !resp.status().is_success() {
        return Err(refuse(resp.status(), resp.headers(), svc));
    }
    if resp.content_length().is_some_and(|n| n as usize > cap) {
        return Ok(None);
    }
    let bytes = resp.bytes().await.map_err(|e| explain(e, svc))?;
    Ok((bytes.len() <= cap).then(|| bytes.to_vec()))
}

/// Does the address answer like the service it is meant to be, and does it accept the secret? → (what it is, its version)
pub async fn test(app: &App, svc: &Service) -> Result<(String, String)> {
    match svc.kind {
        Kind::Sonarr | Kind::Radarr => {
            let status = get_json(app, svc, "/api/v3/system/status", &[]).await?;
            let name = status["appName"].as_str().unwrap_or("");
            if !name.eq_ignore_ascii_case(svc.kind.label()) {
                bail!("{} answers, but as {}, not as {}", svc.url, if name.is_empty() { "something else" } else { name }, svc.kind.label());
            }
            Ok((name.to_string(), status["version"].as_str().unwrap_or("").to_string()))
        }
        Kind::Seerr => {
            // The status needs no key and says what this is; `auth/me` says whether the key is good.
            let status = get_json(app, svc, "/api/v1/status", &[]).await?;
            let Some(version) = status["version"].as_str() else { bail!("{} did not answer like Seerr", svc.url) };
            get_json(app, svc, "/api/v1/auth/me", &[]).await?;
            Ok(("Seerr".into(), version.to_string()))
        }
    }
}

// ---------------------------------------------------------------- the registry

fn read(r: &crate::db::rusqlite::Row) -> crate::db::rusqlite::Result<Option<(Service, Health)>> {
    let kind: String = r.get(1)?;
    let Some(kind) = Kind::from_key(&kind) else { return Ok(None) }; // a kind from a newer finstats
    Ok(Some((
        Service { id: r.get(0)?, kind, name: r.get(2)?, url: r.get(3)?, username: r.get(4)?, secret: r.get(5)?, accept_invalid_certs: r.get(6)?, enabled: r.get(7)? },
        Health { version: r.get(8)?, last_ok_at: r.get(9)?, last_error: r.get(10)? },
    )))
}

fn load(conn: &Connection) -> Result<Vec<(Service, Health)>> {
    let mut stmt = conn.prepare("SELECT id, kind, name, url, username, secret, accept_invalid_certs, enabled, version, last_ok_at, last_error FROM services ORDER BY id")?;
    let rows = stmt.query_map([], read)?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows.into_iter().flatten().collect())
}

/// Read the connections into memory: at start-up and after every change. Loops ask memory, not the database.
pub async fn reload(app: &App) -> Result<()> {
    let rows = app.db.call(|c| load(c)).await?;
    let mut health = app.service_health.write().unwrap();
    health.retain(|id, _| rows.iter().any(|(s, _)| s.id == *id));
    for (svc, stored) in &rows {
        health.entry(svc.id).or_insert_with(|| stored.clone());
    }
    *app.services.write().unwrap() = Arc::new(rows.into_iter().map(|(s, _)| Arc::new(s)).collect());
    Ok(())
}

pub fn all(app: &App) -> Arc<Vec<Arc<Service>>> {
    app.services.read().unwrap().clone()
}

pub fn enabled(app: &App, want: impl Fn(Kind) -> bool) -> Vec<Arc<Service>> {
    all(app).iter().filter(|s| s.enabled && want(s.kind)).cloned().collect()
}

/// Note how a request to a service went. Touches the database only when the picture changed.
pub async fn record(app: &App, id: i64, outcome: std::result::Result<Option<String>, String>) {
    let now = db::now();
    let (before, after) = {
        let mut map = app.service_health.write().unwrap();
        let h = map.entry(id).or_default();
        let before = h.clone();
        match outcome {
            Ok(version) => {
                h.last_ok_at = Some(now);
                h.last_error = None;
                if version.is_some() {
                    h.version = version;
                }
            }
            Err(message) => h.last_error = Some(message),
        }
        (before, h.clone())
    };
    // Every success moves `last_ok_at`; that alone is not worth a write more than once in a while.
    let changed = before.last_error != after.last_error || before.version != after.version || after.last_ok_at.unwrap_or(0) - before.last_ok_at.unwrap_or(0) >= 900;
    if changed {
        let _ = app.db.call(move |c| Ok(c.execute("UPDATE services SET version = ?1, last_ok_at = ?2, last_error = ?3 WHERE id = ?4", params![after.version, after.last_ok_at, after.last_error, id])?)).await;
    }
}

/// The tasks that read from these services. `sync::spawn` has its own list, and insists on a Jellyfin connection.
pub const TASKS: [&str; 3] = ["sync_upcoming", "sync_requests", "sync_grabs"];

/// Runs a task unless it is already running (or is not one of ours). Returns false in that case.
pub fn spawn(app: &App, id: &str) -> bool {
    let Some(id) = TASKS.into_iter().find(|t| *t == id) else { return false };
    if !app.tasks.try_start(id, "Starting…") {
        return false;
    }
    let app = app.clone();
    tokio::spawn(async move {
        let outcome = match id {
            "sync_upcoming" => crate::arr::sync_upcoming(&app).await,
            "sync_requests" => crate::seerr::sync_requests(&app).await,
            "sync_grabs" => crate::arr::sync_grabs(&app).await,
            other => Err(anyhow!("unknown task {other}")),
        };
        app.tasks.finish(id, outcome.map(|m| (m, None)));
    });
    true
}

/// With the other small, regular jobs: is every connection still answering? This is what keeps the status
/// under Settings → Connections true for a service nothing else has asked lately.
pub async fn check_all(app: &App) {
    for svc in enabled(app, |_| true) {
        let outcome = test(app, &svc).await;
        if let Err(e) = &outcome {
            tracing::debug!("{} ({}) is not answering: {e}", svc.name, svc.kind.label());
        }
        record(app, svc.id, outcome.map(|(_, version)| Some(version)).map_err(|e| e.to_string())).await;
    }
}

/// What the UI may build on. Says nothing about which services, only which kinds of page have something behind them.
pub fn features(app: &App) -> Value {
    let list = all(app);
    let has = |want: fn(Kind) -> bool| list.iter().any(|s| s.enabled && want(s.kind));
    json!({ "upcoming": has(Kind::is_arr), "requests": has(|k| k == Kind::Seerr), "downloads": has(Kind::is_arr) })
}

// ---------------------------------------------------------------- API (Jellyfin administrators)

fn service_json(svc: &Service, health: Option<&Health>) -> Value {
    let h = health.cloned().unwrap_or_default();
    json!({
        "id": svc.id, "kind": svc.kind.key(), "label": svc.kind.label(), "name": svc.name, "url": svc.url,
        "has_secret": !svc.secret.is_empty(), "accept_invalid_certs": svc.accept_invalid_certs, "enabled": svc.enabled,
        "version": h.version, "last_ok_at": h.last_ok_at, "last_error": h.last_error,
    })
}

fn list_json(app: &App) -> Value {
    let health = app.service_health.read().unwrap();
    let services: Vec<Value> = all(app).iter().map(|s| service_json(s, health.get(&s.id))).collect();
    let kinds: Vec<Value> = Kind::ALL
        .into_iter()
        .map(|k| json!({ "key": k.key(), "label": k.label(), "what": k.what(), "example": k.example() }))
        .collect();
    json!({ "services": services, "kinds": kinds })
}

// No `Debug`: it holds a secret on its way in.
#[derive(Deserialize)]
pub struct ServiceBody {
    /// For a test while editing: use what is stored for whatever is left out.
    id: Option<i64>,
    kind: Option<String>,
    name: Option<String>,
    url: Option<String>,
    secret: Option<String>,
    accept_invalid_certs: Option<bool>,
    enabled: Option<bool>,
}

/// The connection a request describes: its fields where given, the stored ones otherwise.
fn describe(body: ServiceBody, stored: Option<&Service>, taken: &[String]) -> std::result::Result<Service, ApiError> {
    let kind = match (stored, body.kind.as_deref()) {
        (Some(s), _) => s.kind, // a connection never changes its kind
        (None, Some(k)) => Kind::from_key(k).ok_or_else(|| ApiError::bad_request(format!("Unknown kind of service `{k}`")))?,
        (None, None) => return Err(ApiError::bad_request("Say which kind of service this is")),
    };
    let url = match (body.url.as_deref(), stored) {
        (Some(u), _) => clean_url(u, kind).map_err(|e| ApiError::bad_request(format!("{e}")))?,
        (None, Some(s)) => s.url.clone(),
        (None, None) => return Err(ApiError::bad_request(format!("Enter the address of {}", kind.label()))),
    };
    let secret = match (body.secret.filter(|s| !s.is_empty()), stored) {
        (Some(s), _) => s,
        (None, Some(s)) => s.secret.clone(),
        (None, None) => return Err(ApiError::bad_request("Enter the API key")),
    };
    if secret.len() > 512 || secret.chars().any(char::is_control) {
        return Err(ApiError::bad_request("That does not look like a key or password"));
    }
    // Nothing finstats connects to needs a user name any more; the column stays for databases that have one.
    let username = stored.and_then(|s| s.username.clone());
    let wanted: String = body.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).map(|n| n.chars().filter(|c| !c.is_control()).take(60).collect()).unwrap_or_default();
    let name = if !wanted.is_empty() {
        wanted
    } else if let Some(s) = stored {
        s.name.clone()
    } else {
        // "Radarr", then "Radarr 2": several of a kind are normal.
        (1..).map(|n| if n == 1 { kind.label().to_string() } else { format!("{} {n}", kind.label()) }).find(|n| !taken.contains(n)).unwrap_or_default()
    };
    Ok(Service {
        id: stored.map(|s| s.id).unwrap_or(0),
        kind,
        name,
        url,
        username,
        secret,
        accept_invalid_certs: body.accept_invalid_certs.or(stored.map(|s| s.accept_invalid_certs)).unwrap_or(false),
        enabled: body.enabled.or(stored.map(|s| s.enabled)).unwrap_or(true),
    })
}

fn find(app: &App, id: i64) -> std::result::Result<Arc<Service>, ApiError> {
    all(app).iter().find(|s| s.id == id).cloned().ok_or_else(|| ApiError::not_found("Connection"))
}

fn names(app: &App) -> Vec<String> {
    all(app).iter().map(|s| s.name.clone()).collect()
}

fn unreachable_service(e: anyhow::Error) -> ApiError {
    ApiError::new(StatusCode::BAD_GATEWAY, format!("{e}"))
}

pub async fn list(State(app): State<App>, JellyfinAdmin(_): JellyfinAdmin) -> ApiResult {
    Ok(Json(list_json(&app)))
}

pub async fn test_connection(State(app): State<App>, JellyfinAdmin(_): JellyfinAdmin, Json(body): Json<ServiceBody>) -> ApiResult {
    let stored = match body.id {
        Some(id) => Some(find(&app, id)?),
        None => None,
    };
    let svc = describe(body, stored.as_deref(), &[])?;
    let (what, version) = test(&app, &svc).await.map_err(unreachable_service)?;
    Ok(Json(json!({ "ok": true, "app": what, "version": version })))
}

pub async fn create(State(app): State<App>, JellyfinAdmin(_): JellyfinAdmin, Json(body): Json<ServiceBody>) -> ApiResult {
    if all(&app).len() >= MAX_SERVICES {
        return Err(ApiError::bad_request(format!("At most {MAX_SERVICES} connections")));
    }
    let mut body = body;
    body.id = None;
    let svc = describe(body, None, &names(&app))?;
    // Only what answers is saved: a typo in a key is found now, not in a log next week.
    let (_, version) = test(&app, &svc).await.map_err(unreachable_service)?;
    let now = db::now();
    app.db
        .call(move |c| {
            Ok(c.execute(
                "INSERT INTO services(kind, name, url, username, secret, accept_invalid_certs, enabled, created_at, version, last_ok_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?8)",
                params![svc.kind.key(), svc.name, svc.url, svc.username, svc.secret, svc.accept_invalid_certs, svc.enabled, now, version],
            )?)
        })
        .await?;
    changed(&app).await?;
    Ok(Json(list_json(&app)))
}

pub async fn update(State(app): State<App>, JellyfinAdmin(_): JellyfinAdmin, Path(id): Path<i64>, Json(body): Json<ServiceBody>) -> ApiResult {
    let stored = find(&app, id)?;
    let taken: Vec<String> = names(&app).into_iter().filter(|n| *n != stored.name).collect();
    let svc = describe(body, Some(&stored), &taken)?;
    let reconnect = svc.url != stored.url || svc.secret != stored.secret || svc.username != stored.username || svc.accept_invalid_certs != stored.accept_invalid_certs;
    let moved = !same_instance(&svc.url, &stored.url);
    let version = if reconnect && svc.enabled { Some(test(&app, &svc).await.map_err(unreachable_service)?.1) } else { None };
    let now = db::now();
    app.db
        .call(move |c| {
            let tx = c.transaction()?;
            tx.execute(
                "UPDATE services SET name = ?1, url = ?2, username = ?3, secret = ?4, accept_invalid_certs = ?5, enabled = ?6 WHERE id = ?7",
                params![svc.name, svc.url, svc.username, svc.secret, svc.accept_invalid_certs, svc.enabled, id],
            )?;
            if let Some(v) = &version {
                tx.execute("UPDATE services SET version = ?1, last_ok_at = ?2, last_error = NULL WHERE id = ?3", params![v, now, id])?;
            }
            if moved {
                forget(&tx, id)?;
            }
            tx.commit()?;
            Ok(())
        })
        .await?;
    app.service_health.write().unwrap().remove(&id);
    changed(&app).await?;
    Ok(Json(list_json(&app)))
}

pub async fn remove(State(app): State<App>, JellyfinAdmin(_): JellyfinAdmin, Path(id): Path<i64>) -> ApiResult {
    find(&app, id)?;
    // Whatever was read from it goes with it (ON DELETE CASCADE).
    app.db.call(move |c| Ok(c.execute("DELETE FROM services WHERE id = ?1", [id])?)).await?;
    changed(&app).await?;
    Ok(Json(list_json(&app)))
}

/// Everything read from a connection, thrown away: it now points at another instance.
fn forget(conn: &Connection, id: i64) -> Result<()> {
    for table in CHILD_TABLES {
        let exists: Option<i64> = conn.query_row("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1", [table], |r| r.get(0)).optional()?;
        if exists.is_some() {
            conn.execute(&format!("DELETE FROM {table} WHERE service_id = ?1"), [id])?;
        }
    }
    conn.execute("DELETE FROM settings WHERE key LIKE ?1", [format!("service:{id}:%")])?;
    Ok(())
}

/// Tables whose rows belong to one connection (`service_id … REFERENCES services ON DELETE CASCADE`).
const CHILD_TABLES: [&str; 3] = ["upcoming", "requests", "grabs"];

async fn changed(app: &App) -> Result<()> {
    reload(app).await?;
    app.wake.notify_waiters();
    app.downloads_wake.notify_waiters();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service(kind: Kind, url: &str) -> Service {
        Service { id: 1, kind, name: kind.label().into(), url: url.into(), username: None, secret: "hunter2-secret-key".into(), accept_invalid_certs: false, enabled: true }
    }

    #[test]
    fn an_address_keeps_its_base_path_and_carries_nothing_else() {
        assert_eq!(clean_url("192.168.1.10:8989/", Kind::Sonarr).unwrap(), "http://192.168.1.10:8989");
        assert_eq!(clean_url(" https://media.example/sonarr/ ", Kind::Sonarr).unwrap(), "https://media.example/sonarr");
        assert_eq!(clean_url("http://nas:7878/web/", Kind::Radarr).unwrap(), "http://nas:7878");
        for bad in ["http://admin:secret@nas:8989", "http://nas:8989/?apikey=abc", "http://nas:8989/#x", "ftp://nas", "file:///etc/passwd", ""] {
            assert!(clean_url(bad, Kind::Sonarr).is_err(), "{bad} was accepted");
        }
    }

    #[test]
    fn another_host_port_or_base_path_is_another_instance() {
        assert!(same_instance("http://nas:8989", "http://NAS:8989/"));
        assert!(same_instance("https://media.example", "https://media.example:443"));
        assert!(!same_instance("http://nas:8989", "http://nas:8990"));
        assert!(!same_instance("http://nas/sonarr", "http://nas/sonarr4k"));
        assert!(!same_instance("http://nas:8989", "http://other:8989"));
    }

    #[test]
    fn what_the_ui_gets_never_contains_the_secret() {
        let svc = service(Kind::Radarr, "http://nas:7878");
        let text = service_json(&svc, None).to_string();
        assert!(!text.contains("hunter2"), "{text}");
        assert!(text.contains("\"has_secret\":true"));
    }

    #[test]
    fn an_answer_that_is_not_ours_is_explained_without_repeating_it() {
        let svc = service(Kind::Sonarr, "http://nas:8989");
        let mut headers = HeaderMap::new();
        headers.insert(LOCATION, HeaderValue::from_static("/login?returnUrl=%2F"));
        let text = refuse(StatusCode::FOUND, &headers, &svc).to_string();
        assert!(text.contains("redirect to /login") && text.contains("follows no redirects"), "{text}");
        assert!(refuse(StatusCode::UNAUTHORIZED, &HeaderMap::new(), &svc).to_string().contains("refused the API key"));
        assert!(refuse(StatusCode::NOT_FOUND, &HeaderMap::new(), &svc).to_string().contains("/sonarr"));
        for e in [StatusCode::FOUND, StatusCode::UNAUTHORIZED, StatusCode::BAD_GATEWAY] {
            assert!(!refuse(e, &headers, &svc).to_string().contains("hunter2"));
        }
    }

    #[test]
    fn a_second_connection_of_a_kind_gets_its_own_name() {
        let body = |name: Option<&str>| ServiceBody { id: None, kind: Some("radarr".into()), name: name.map(str::to_string), url: Some("nas:7878".into()), secret: Some("k".into()), accept_invalid_certs: None, enabled: None };
        let first = describe(body(None), None, &[]).ok().unwrap();
        assert_eq!((first.name.as_str(), first.enabled, first.accept_invalid_certs), ("Radarr", true, false));
        assert_eq!(describe(body(None), None, &["Radarr".into()]).ok().unwrap().name, "Radarr 2");
        assert_eq!(describe(body(Some("  4K  ")), None, &[]).ok().unwrap().name, "4K");
        // Editing without retyping the key keeps the key, and the kind never changes.
        let edited = describe(ServiceBody { id: Some(1), kind: Some("sonarr".into()), name: None, url: None, secret: None, accept_invalid_certs: Some(true), enabled: None }, Some(&first), &[]).ok().unwrap();
        assert!(edited.kind == Kind::Radarr && edited.secret() == "k" && edited.accept_invalid_certs);
        assert!(describe(ServiceBody { id: None, kind: Some("radarr".into()), name: None, url: Some("nas".into()), secret: None, accept_invalid_certs: None, enabled: None }, None, &[]).is_err());
    }

    /// The reason this module has its own client: a redirect must not be followed, or the key goes along.
    #[tokio::test]
    async fn a_redirect_is_reported_and_the_key_goes_nowhere_else() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let elsewhere = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = elsewhere.local_addr().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 2048];
            let _ = sock.read(&mut buf).await;
            let _ = sock.write_all(format!("HTTP/1.1 307 Temporary Redirect\r\nLocation: http://{target}/api/v3/system/status\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await;
        });
        let svc = service(Kind::Sonarr, &format!("http://{addr}"));
        let http = Http::new();
        let resp = http.of(&svc).get(format!("{}/api/v3/system/status", svc.url)).header("X-Api-Key", svc.secret()).send().await.unwrap();
        assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT);
        assert!(refuse(resp.status(), resp.headers(), &svc).to_string().contains("redirect"));
        // Nobody ever knocks on the other door.
        assert!(tokio::time::timeout(Duration::from_millis(300), elsewhere.accept()).await.is_err(), "the redirect was followed");
    }
}
