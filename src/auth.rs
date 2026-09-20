//! Sign-in with Jellyfin credentials.
//!
//! finstats never stores passwords. A login is forwarded to Jellyfin's
//! `AuthenticateByName`; on success we mint our own opaque session token (stored
//! hashed) and immediately end the Jellyfin session the check created.

use std::net::{IpAddr, SocketAddr};

use axum::extract::{ConnectInfo, FromRequestParts, State};
use axum::http::header::{COOKIE, SET_COOKIE};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use rand::RngCore;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::db::{self, rusqlite::OptionalExtension, rusqlite::params};
use crate::jellyfin::{self, AuthError};
use crate::state::{ApiError, ApiResult, App, JfConfig};

const COOKIE_NAME: &str = "finstats_session";
const SESSION_TTL_S: i64 = 30 * 86_400;
const MAX_ATTEMPTS: u32 = 10;
const ATTEMPT_WINDOW_S: i64 = 300;

/// Permission keys an administrator can grant. `sign_in` lets one user in while sign-in
/// for everyone is off; the rest widen what a signed-in user may see or do.
pub const SIGN_IN: &str = "sign_in";
pub const GRANTABLE: [&str; 5] = [SIGN_IN, "see_everyone", "see_network", "see_server", "manage"];

/// What this request may see and do. Jellyfin administrators always hold everything.
/// The year recap is not covered: it stays personal whatever is granted.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize)]
pub struct Perms {
    /// Other people's activity and statistics, the user list, every live stream.
    pub see_everyone: bool,
    /// IP addresses, device ids, local/remote.
    pub see_network: bool,
    /// The Server page, the server log, failed sign-ins and file paths.
    pub see_server: bool,
    /// Settings, tasks, imports and deleting plays.
    pub manage: bool,
}

impl Perms {
    pub const ALL: Perms = Perms { see_everyone: true, see_network: true, see_server: true, manage: true };

    pub fn from_keys<'a>(keys: impl IntoIterator<Item = &'a str>) -> Self {
        let mut p = Perms::default();
        for k in keys {
            match k {
                "see_everyone" => p.see_everyone = true,
                "see_network" => p.see_network = true,
                "see_server" => p.see_server = true,
                "manage" => p.manage = true,
                _ => {}
            }
        }
        p
    }
}

#[derive(Clone, Debug)]
pub struct AuthUser {
    pub id: String,
    pub name: String,
    /// A Jellyfin administrator. Only they can change who is allowed what.
    pub is_admin: bool,
    pub perms: Perms,
}

/// May change settings, run tasks, import and delete plays.
pub struct Manager(#[allow(dead_code)] pub AuthUser);
/// May see the server page and the server log.
pub struct ServerViewer(#[allow(dead_code)] pub AuthUser);
/// A Jellyfin administrator: the only one who may hand out permissions.
pub struct JellyfinAdmin(#[allow(dead_code)] pub AuthUser);

/// A non-admin's own grants, as stored. Administrators never have a row.
pub fn stored_grants(conn: &db::rusqlite::Connection, user_id: &str) -> anyhow::Result<Vec<String>> {
    let raw: Option<String> = conn.query_row("SELECT permissions FROM user_permissions WHERE user_id = ?1", [user_id], |r| r.get(0)).optional()?;
    Ok(raw.and_then(|r| serde_json::from_str(&r).ok()).unwrap_or_default())
}

/// `None` = this user may not sign in at all.
fn effective(is_admin: bool, grants: &[String], settings: &crate::state::Settings) -> Option<Perms> {
    if is_admin {
        return Some(Perms::ALL);
    }
    if !settings.allow_user_login && !grants.iter().any(|g| g == SIGN_IN) {
        return None;
    }
    Some(Perms::from_keys(grants.iter().chain(settings.default_permissions.iter()).map(String::as_str)))
}

fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

fn cookie_token(headers: &HeaderMap) -> Option<String> {
    headers.get_all(COOKIE).iter().filter_map(|v| v.to_str().ok()).flat_map(|v| v.split(';')).find_map(|kv| {
        let (k, v) = kv.trim().split_once('=')?;
        (k == COOKIE_NAME && !v.is_empty()).then(|| v.to_string())
    })
}

pub fn client_ip(app: &App, headers: &HeaderMap, peer: SocketAddr) -> IpAddr {
    if app.trust_proxy {
        if let Some(ip) = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .and_then(|v| v.trim().parse().ok())
        {
            return ip;
        }
    }
    peer.ip()
}

impl FromRequestParts<App> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, app: &App) -> Result<Self, Self::Rejection> {
        let unauthorized = || ApiError::new(StatusCode::UNAUTHORIZED, "Sign in to continue");
        let token = cookie_token(&parts.headers).ok_or_else(unauthorized)?;
        let hash = hash_token(&token);
        // Grants are read on every request, so a change by an administrator applies at once.
        let (mut user, grants) = app
            .db
            .call(move |c| {
                let user = c
                    .query_row(
                        "SELECT user_id, user_name, is_admin FROM sessions WHERE token_hash = ?1 AND expires_at > ?2",
                        params![hash, db::now()],
                        |r| Ok(AuthUser { id: r.get(0)?, name: r.get(1)?, is_admin: r.get::<_, i64>(2)? != 0, perms: Perms::default() }),
                    )
                    .optional()?;
                let grants = match &user {
                    Some(u) if !u.is_admin => stored_grants(c, &u.id)?,
                    _ => vec![],
                };
                Ok(user.map(|u| (u, grants)))
            })
            .await?
            .ok_or_else(unauthorized)?;
        user.perms = effective(user.is_admin, &grants, &app.settings()).ok_or_else(unauthorized)?;
        Ok(user)
    }
}

impl FromRequestParts<App> for Manager {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, app: &App) -> Result<Self, Self::Rejection> {
        let user = AuthUser::from_request_parts(parts, app).await?;
        if !user.perms.manage {
            return Err(ApiError::not_permitted("manage finstats"));
        }
        Ok(Manager(user))
    }
}

impl FromRequestParts<App> for ServerViewer {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, app: &App) -> Result<Self, Self::Rejection> {
        let user = AuthUser::from_request_parts(parts, app).await?;
        if !user.perms.see_server {
            return Err(ApiError::not_permitted("see the server"));
        }
        Ok(ServerViewer(user))
    }
}

impl FromRequestParts<App> for JellyfinAdmin {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, app: &App) -> Result<Self, Self::Rejection> {
        let user = AuthUser::from_request_parts(parts, app).await?;
        if !user.is_admin {
            return Err(ApiError::forbidden());
        }
        Ok(JellyfinAdmin(user))
    }
}

fn session_cookie(token: &str, max_age: i64, secure: bool) -> HeaderValue {
    let mut c = format!("{COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}");
    if secure {
        c.push_str("; Secure");
    }
    HeaderValue::from_str(&c).expect("cookie is ascii")
}

fn is_https(headers: &HeaderMap) -> bool {
    headers.get("x-forwarded-proto").and_then(|v| v.to_str().ok()).is_some_and(|v| v.eq_ignore_ascii_case("https"))
}

async fn user_json(app: &App, id: &str, name: &str, is_admin: bool, perms: Perms) -> Value {
    let uid = id.to_string();
    let has_image = app
        .db
        .call(move |c| {
            Ok(c.query_row("SELECT image_tag IS NOT NULL FROM users WHERE id = ?1", [uid], |r| r.get::<_, bool>(0))
                .optional()?
                .unwrap_or(false))
        })
        .await
        .unwrap_or(false);
    // `features`: which optional pages have something behind them (a connected Sonarr, a Seerr, …).
    json!({ "id": id, "name": name, "is_admin": is_admin, "has_image": has_image, "permissions": perms, "features": crate::services::features(app) })
}

async fn start_session(
    app: &App,
    headers: &HeaderMap,
    ip: IpAddr,
    user_id: &str,
    user_name: &str,
    is_admin: bool,
    perms: Perms,
) -> ApiResult<Response> {
    let mut raw = [0u8; 32];
    rand::rng().fill_bytes(&mut raw);
    let token = hex::encode(raw);
    let hash = hash_token(&token);
    let ua = headers.get("user-agent").and_then(|v| v.to_str().ok()).map(|s| s.chars().take(200).collect::<String>());
    let (uid, uname, ip_s) = (user_id.to_string(), user_name.to_string(), ip.to_string());
    app.db
        .call(move |c| {
            let now = db::now();
            c.execute("DELETE FROM sessions WHERE expires_at <= ?1", [now])?;
            c.execute(
                "INSERT INTO sessions(token_hash, user_id, user_name, is_admin, created_at, expires_at, ip, user_agent)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![hash, uid, uname, is_admin, now, now + SESSION_TTL_S, ip_s, ua],
            )?;
            Ok(())
        })
        .await?;
    let body = Json(json!({ "user": user_json(app, user_id, user_name, is_admin, perms).await }));
    let mut resp = body.into_response();
    resp.headers_mut().insert(SET_COOKIE, session_cookie(&token, SESSION_TTL_S, is_https(headers)));
    Ok(resp)
}

/// Sliding-window limiter against password guessing through finstats.
fn check_rate_limit(app: &App, ip: IpAddr) -> Result<(), ApiError> {
    let now = db::now();
    let mut map = app.login_attempts.lock().unwrap();
    map.retain(|_, (_, start)| now - *start < ATTEMPT_WINDOW_S);
    let entry = map.entry(ip).or_insert((0, now));
    if entry.0 >= MAX_ATTEMPTS {
        let wait = (ATTEMPT_WINDOW_S - (now - entry.1)).max(1);
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            format!("Too many sign-in attempts. Try again in {} minutes.", (wait + 59) / 60),
        ));
    }
    entry.0 += 1;
    Ok(())
}

fn clear_rate_limit(app: &App, ip: IpAddr) {
    app.login_attempts.lock().unwrap().remove(&ip);
}

#[derive(Deserialize)]
pub struct LoginBody {
    #[serde(default)]
    username: String,
    #[serde(default)]
    password: String,
}

pub async fn login(
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<LoginBody>,
) -> ApiResult<Response> {
    let jf = app
        .jellyfin()
        .ok_or_else(|| ApiError::new(StatusCode::CONFLICT, "finstats is not connected to Jellyfin yet"))?;
    let username = body.username.trim();
    if username.is_empty() {
        return Err(ApiError::bad_request("Enter your Jellyfin username"));
    }
    let ip = client_ip(&app, &headers, peer);
    check_rate_limit(&app, ip)?;

    let jf = app.jellyfin_anonymous(jf.base());
    let auth = match jf.authenticate(username, &body.password).await {
        Ok(a) => a,
        Err(AuthError::InvalidCredentials) => {
            return Err(ApiError::new(StatusCode::UNAUTHORIZED, "Wrong username or password"));
        }
        Err(AuthError::Other(e)) => {
            tracing::warn!("login could not be checked against Jellyfin: {e:#}");
            return Err(ApiError::new(StatusCode::BAD_GATEWAY, "Could not reach Jellyfin to check your sign-in"));
        }
    };
    jf.logout(&auth.access_token).await;

    let uid = auth.user_id.clone();
    let grants = if auth.is_admin { vec![] } else { app.db.call(move |c| stored_grants(c, &uid)).await? };
    let Some(perms) = effective(auth.is_admin, &grants, &app.settings()) else {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "You haven't been given access to finstats. A Jellyfin administrator can allow you in Settings.",
        ));
    };
    clear_rate_limit(&app, ip);
    tracing::info!("{} signed in from {ip}", auth.user_name);
    start_session(&app, &headers, ip, &auth.user_id, &auth.user_name, auth.is_admin, perms).await
}

pub async fn logout(State(app): State<App>, headers: HeaderMap) -> ApiResult<Response> {
    if let Some(token) = cookie_token(&headers) {
        let hash = hash_token(&token);
        app.db.call(move |c| Ok(c.execute("DELETE FROM sessions WHERE token_hash = ?1", [hash])?)).await?;
    }
    let mut resp = Json(json!({ "ok": true })).into_response();
    resp.headers_mut().insert(SET_COOKIE, session_cookie("", 0, is_https(&headers)));
    Ok(resp)
}

pub async fn me(State(app): State<App>, user: AuthUser) -> ApiResult {
    Ok(Json(json!({ "user": user_json(&app, &user.id, &user.name, user.is_admin, user.perms).await })))
}

// ---------------------------------------------------------------- first-run setup

pub async fn status(State(app): State<App>) -> Json<Value> {
    let cfg = app.config.read().unwrap().clone();
    Json(json!({
        "configured": cfg.is_some(),
        "version": env!("CARGO_PKG_VERSION"),
        "server_name": cfg.map(|c| c.server_name),
    }))
}

fn ensure_unconfigured(app: &App) -> Result<(), ApiError> {
    if app.is_configured() {
        return Err(ApiError::new(StatusCode::CONFLICT, "finstats is already set up"));
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct SetupTestBody {
    #[serde(default)]
    url: String,
}

pub async fn setup_test(State(app): State<App>, Json(body): Json<SetupTestBody>) -> ApiResult {
    ensure_unconfigured(&app)?;
    let url = jellyfin::normalize_url(&body.url).map_err(|e| ApiError::bad_request(e.to_string()))?;
    let info = app
        .jellyfin_anonymous(&url)
        .public_info()
        .await
        .map_err(|e| ApiError::new(StatusCode::BAD_GATEWAY, e.to_string()))?;
    Ok(Json(json!({ "server_name": info.server_name, "version": info.version, "id": info.id, "url": url })))
}

#[derive(Deserialize)]
pub struct SetupBody {
    #[serde(default)]
    url: String,
    #[serde(default)]
    username: String,
    #[serde(default)]
    password: String,
}

pub async fn setup(
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<SetupBody>,
) -> ApiResult<Response> {
    ensure_unconfigured(&app)?;
    let ip = client_ip(&app, &headers, peer);
    check_rate_limit(&app, ip)?;
    let url = jellyfin::normalize_url(&body.url).map_err(|e| ApiError::bad_request(e.to_string()))?;
    let jf = app.jellyfin_anonymous(&url);
    let info = jf.public_info().await.map_err(|e| ApiError::new(StatusCode::BAD_GATEWAY, e.to_string()))?;

    let auth = match jf.authenticate(body.username.trim(), &body.password).await {
        Ok(a) => a,
        Err(AuthError::InvalidCredentials) => {
            return Err(ApiError::new(StatusCode::UNAUTHORIZED, "Wrong username or password"));
        }
        Err(AuthError::Other(e)) => return Err(ApiError::new(StatusCode::BAD_GATEWAY, e.to_string())),
    };
    if !auth.is_admin {
        jf.logout(&auth.access_token).await;
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "That account is not a Jellyfin administrator. Set finstats up with an administrator account.",
        ));
    }
    let key = jf.ensure_api_key(&auth.access_token).await;
    jf.logout(&auth.access_token).await;
    let api_key = key.map_err(|e| ApiError::new(StatusCode::BAD_GATEWAY, e.to_string()))?;

    // Re-check under the lock-free flag: two people finishing the wizard at once.
    ensure_unconfigured(&app)?;
    let cfg = JfConfig {
        url,
        api_key,
        server_name: info.server_name.unwrap_or_else(|| "Jellyfin".into()),
        server_version: info.version.unwrap_or_default(),
        from_env: false,
    };
    let stored = cfg.clone();
    app.db
        .call(move |c| {
            db::set_setting(c, "jellyfin_url", &stored.url)?;
            db::set_setting(c, "jellyfin_api_key", &stored.api_key)?;
            db::set_setting(c, "server_name", &stored.server_name)?;
            db::set_setting(c, "server_version", &stored.server_version)?;
            Ok(())
        })
        .await?;
    *app.config.write().unwrap() = Some(cfg);
    clear_rate_limit(&app, ip);
    tracing::info!("setup completed by {}", auth.user_name);
    app.wake.notify_waiters();

    start_session(&app, &headers, ip, &auth.user_id, &auth.user_name, true, Perms::ALL).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Settings;

    fn grants(keys: &[&str]) -> Vec<String> {
        keys.iter().map(|k| k.to_string()).collect()
    }

    #[test]
    fn administrators_hold_everything_and_need_no_grant() {
        assert_eq!(effective(true, &[], &Settings::default()), Some(Perms::ALL));
    }

    #[test]
    fn nobody_else_gets_in_unless_allowed() {
        let closed = Settings::default();
        assert_eq!(effective(false, &[], &closed), None);
        // Holding a viewing permission is not an invitation: sign-in is its own grant.
        assert_eq!(effective(false, &grants(&["see_everyone"]), &closed), None);
        assert_eq!(effective(false, &grants(&["sign_in"]), &closed), Some(Perms::default()));
    }

    #[test]
    fn defaults_and_personal_grants_add_up() {
        let open = Settings { allow_user_login: true, default_permissions: grants(&["see_everyone"]), ..Settings::default() };
        assert_eq!(effective(false, &[], &open), Some(Perms { see_everyone: true, ..Perms::default() }));
        let p = effective(false, &grants(&["see_network", "bogus"]), &open).unwrap();
        assert_eq!(p, Perms { see_everyone: true, see_network: true, ..Perms::default() });
        assert!(!p.manage && !p.see_server);
    }

    #[test]
    fn unknown_default_permissions_are_rejected() {
        let s = Settings { default_permissions: grants(&["see_everyone", "root"]), ..Settings::default() };
        assert!(s.validate().is_err());
        assert!(Settings { default_permissions: grants(&["manage"]), ..Settings::default() }.validate().is_ok());
    }
}
