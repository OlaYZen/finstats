//! The torrent clients: qBittorrent, Transmission, Deluge.
//!
//! All three speak POST (a login, an RPC envelope), so "finstats only sends GET" cannot be the rule here.
//! The rule is `ALLOWED`: every call goes through `permit`, the list holds nothing that changes anything,
//! and a test keeps it that way. Deluge's `web.connect` is absent on purpose: a web UI that is not connected
//! to its daemon is reported, not repaired.
//!
//! Sessions (qBittorrent's `SID` cookie, Transmission's session id, Deluge's `_session_id`) are kept in
//! memory per connection and read from the response headers by hand; no cookie store is involved.

use anyhow::{Result, anyhow, bail};
use reqwest::header::{COOKIE, HeaderMap, REFERER, SET_COOKIE};
use serde_json::{Value, json};

use crate::services::{Kind, Service, explain, refuse};
use crate::state::App;

/// Every endpoint and RPC method finstats may call on a torrent client. All of them read.
pub const ALLOWED: [&str; 11] = [
    // qBittorrent Web API v2
    "auth/login", "app/version", "torrents/info", "transfer/info",
    // Transmission RPC
    "session-get", "session-stats", "torrent-get",
    // Deluge JSON-RPC
    "auth.login", "web.connected", "web.update_ui", "daemon.info",
];

fn permit(method: &str) -> Result<()> {
    if ALLOWED.contains(&method) { Ok(()) } else { bail!("finstats does not call `{method}` on a torrent client") }
}

const SESSION_HEADER: &str = "x-transmission-session-id";

fn session(app: &App, svc: &Service) -> Option<String> {
    app.client_sessions.lock().unwrap().get(&svc.id).cloned()
}

fn remember(app: &App, svc: &Service, value: Option<String>) {
    let mut map = app.client_sessions.lock().unwrap();
    match value {
        Some(v) => map.insert(svc.id, v),
        None => map.remove(&svc.id),
    };
}

/// `name=value` of the first cookie whose name contains `marker`.
fn cookie_from(headers: &HeaderMap, marker: &str) -> Option<String> {
    headers.get_all(SET_COOKIE).iter().filter_map(|v| v.to_str().ok()).find_map(|line| {
        let pair = line.split(';').next()?.trim();
        let (name, value) = pair.split_once('=')?;
        (name.contains(marker) && !value.is_empty()).then(|| pair.to_string())
    })
}

// ---------------------------------------------------------------- qBittorrent

async fn qb_login(app: &App, svc: &Service) -> Result<Option<String>> {
    permit("auth/login")?;
    let form = [("username", svc.username.as_deref().unwrap_or("")), ("password", svc.secret())];
    // qBittorrent refuses a login whose Referer is not itself.
    let resp = app.services_http.of(svc).post(format!("{}/api/v2/auth/login", svc.url)).header(REFERER, &svc.url).form(&form).send().await.map_err(|e| explain(e, svc))?;
    let status = resp.status();
    if status.as_u16() == 403 {
        bail!("qBittorrent has blocked this address after too many failed logins. Wait for the ban to pass (or restart qBittorrent), then check the user name and password");
    }
    if !status.is_success() {
        return Err(refuse(status, resp.headers(), svc));
    }
    let cookie = cookie_from(resp.headers(), "SID");
    let body = resp.text().await.unwrap_or_default();
    if body.trim().eq_ignore_ascii_case("fails.") {
        bail!("qBittorrent refused the user name or password");
    }
    // No cookie and no "Fails.": authentication is bypassed for this address. Fine.
    Ok(cookie)
}

/// `GET /api/v2/<endpoint>`, signing in first when needed and once more when the session has expired.
pub async fn qb_get(app: &App, svc: &Service, endpoint: &'static str, query: &[(&str, String)]) -> Result<reqwest::Response> {
    permit(endpoint)?;
    for attempt in 0..2 {
        let cookie = match session(app, svc) {
            Some(c) => Some(c),
            None => {
                let fresh = qb_login(app, svc).await?;
                remember(app, svc, fresh.clone());
                fresh
            }
        };
        let mut req = app.services_http.of(svc).get(format!("{}/api/v2/{endpoint}", svc.url)).header(REFERER, &svc.url).query(query);
        if let Some(c) = &cookie {
            req = req.header(COOKIE, c);
        }
        let resp = req.send().await.map_err(|e| explain(e, svc))?;
        if resp.status().as_u16() == 403 && attempt == 0 && cookie.is_some() {
            remember(app, svc, None); // the session ran out
            continue;
        }
        if !resp.status().is_success() {
            return Err(refuse(resp.status(), resp.headers(), svc));
        }
        return Ok(resp);
    }
    bail!("qBittorrent keeps refusing the session")
}

// ---------------------------------------------------------------- Transmission

/// Where the RPC lives: people enter the host, the `/transmission` folder, or the whole thing.
fn transmission_rpc(base: &str) -> String {
    if base.ends_with("/rpc") {
        base.to_string()
    } else if base.ends_with("/transmission") {
        format!("{base}/rpc")
    } else {
        format!("{base}/transmission/rpc")
    }
}

pub async fn tr_call(app: &App, svc: &Service, method: &'static str, arguments: Value) -> Result<Value> {
    permit(method)?;
    let url = transmission_rpc(&svc.url);
    for _ in 0..2 {
        let mut req = app.services_http.of(svc).post(&url).header(SESSION_HEADER, session(app, svc).unwrap_or_default()).json(&json!({ "method": method, "arguments": arguments }));
        if svc.username.is_some() || !svc.secret().is_empty() {
            req = req.basic_auth(svc.username.as_deref().unwrap_or(""), Some(svc.secret()));
        }
        let resp = req.send().await.map_err(|e| explain(e, svc))?;
        // Its way of handing out a session: 409, with the id to come back with.
        if resp.status().as_u16() == 409 {
            let id = resp.headers().get(SESSION_HEADER).and_then(|v| v.to_str().ok()).map(str::to_string);
            if id.is_none() {
                bail!("{} did not answer like Transmission", svc.url);
            }
            remember(app, svc, id);
            continue;
        }
        if !resp.status().is_success() {
            return Err(refuse(resp.status(), resp.headers(), svc));
        }
        let body: Value = resp.json().await.map_err(|e| explain(e, svc))?;
        if body["result"].as_str() != Some("success") {
            bail!("Transmission could not answer `{method}`");
        }
        return Ok(body["arguments"].clone());
    }
    bail!("Transmission keeps asking for a new session")
}

// ---------------------------------------------------------------- Deluge

async fn dl_post(app: &App, svc: &Service, method: &'static str, params: Value) -> Result<(Value, Option<String>)> {
    permit(method)?;
    let mut req = app.services_http.of(svc).post(format!("{}/json", svc.url)).json(&json!({ "method": method, "params": params, "id": 1 }));
    if let Some(c) = session(app, svc) {
        req = req.header(COOKIE, c);
    }
    let resp = req.send().await.map_err(|e| explain(e, svc))?;
    if !resp.status().is_success() {
        return Err(refuse(resp.status(), resp.headers(), svc));
    }
    let cookie = cookie_from(resp.headers(), "_session_id");
    let body: Value = resp.json().await.map_err(|e| explain(e, svc))?;
    Ok((body, cookie))
}

async fn dl_login(app: &App, svc: &Service) -> Result<()> {
    let (body, cookie) = dl_post(app, svc, "auth.login", json!([svc.secret()])).await?;
    if body["result"].as_bool() != Some(true) {
        bail!("Deluge refused the password");
    }
    remember(app, svc, cookie);
    Ok(())
}

pub async fn dl_call(app: &App, svc: &Service, method: &'static str, params: Value) -> Result<Value> {
    for attempt in 0..2 {
        if session(app, svc).is_none() {
            dl_login(app, svc).await?;
        }
        let (body, _) = dl_post(app, svc, method, params.clone()).await?;
        if body["error"].is_null() {
            return Ok(body["result"].clone());
        }
        // Code 1: not signed in (any more).
        if body["error"]["code"].as_i64() == Some(1) && attempt == 0 {
            remember(app, svc, None);
            continue;
        }
        bail!("Deluge could not answer `{method}`");
    }
    bail!("Deluge keeps refusing the session")
}

/// A web UI that is not connected to a daemon knows about no torrents. Saying so is all finstats does about it.
async fn dl_connected(app: &App, svc: &Service) -> Result<()> {
    if dl_call(app, svc, "web.connected", json!([])).await?.as_bool() != Some(true) {
        bail!("Deluge’s web page is not connected to its daemon. Open Deluge in a browser, connect it under Connection Manager, and try again");
    }
    Ok(())
}

// ---------------------------------------------------------------- connection test

pub async fn test(app: &App, svc: &Service) -> Result<(String, String)> {
    remember(app, svc, None); // a test always signs in afresh: it is the credentials that are being tested
    let version = match svc.kind {
        Kind::QBittorrent => {
            let text = qb_get(app, svc, "app/version", &[]).await?.text().await.unwrap_or_default();
            let v = text.trim();
            if v.is_empty() || v.len() > 40 || v.contains('<') {
                bail!("{} did not answer like qBittorrent", svc.url);
            }
            v.trim_start_matches('v').to_string()
        }
        Kind::Transmission => tr_call(app, svc, "session-get", json!({ "fields": ["version"] })).await?["version"].as_str().map(str::to_string).ok_or_else(|| anyhow!("{} did not answer like Transmission", svc.url))?,
        Kind::Deluge => {
            dl_connected(app, svc).await?;
            dl_call(app, svc, "daemon.info", json!([])).await.ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
        }
        _ => bail!("not a torrent client"),
    };
    Ok((svc.kind.label().to_string(), version))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_finstats_may_call_changes_anything() {
        // Words that start, stop, add, remove, move or configure something in any of the three clients.
        let forbidden = ["add", "delete", "remove", "pause", "resume", "start", "stop", "set", "move", "rename", "recheck", "reannounce", "connect", "disconnect",
            "shutdown", "create", "edit", "toggle", "increase", "decrease", "top", "bottom", "queue", "upload", "download_torrent", "logout", "prefs"];
        for method in ALLOWED {
            let tail = method.rsplit(['/', '.', '-']).next().unwrap();
            assert!(!forbidden.contains(&tail), "`{method}` looks like it changes something");
        }
        assert!(!ALLOWED.contains(&"web.connect"));
        assert!(permit("torrents/delete").is_err() && permit("torrent-remove").is_err() && permit("core.pause_torrent").is_err());
        assert!(permit("torrents/info").is_ok());
    }

    #[test]
    fn the_rpc_address_is_found_wherever_the_owner_pointed() {
        assert_eq!(transmission_rpc("http://nas:9091"), "http://nas:9091/transmission/rpc");
        assert_eq!(transmission_rpc("http://nas:9091/transmission"), "http://nas:9091/transmission/rpc");
        assert_eq!(transmission_rpc("https://media.example/tr/rpc"), "https://media.example/tr/rpc");
    }

    #[test]
    fn a_session_cookie_is_read_without_its_attributes() {
        let mut h = HeaderMap::new();
        h.append(SET_COOKIE, "theme=dark; Path=/".parse().unwrap());
        h.append(SET_COOKIE, "QBT_SID_8080=abc123; HttpOnly; SameSite=Strict; path=/".parse().unwrap());
        assert_eq!(cookie_from(&h, "SID").as_deref(), Some("QBT_SID_8080=abc123"));
        assert_eq!(cookie_from(&h, "_session_id"), None);
        let mut d = HeaderMap::new();
        d.append(SET_COOKIE, "_session_id=f00; Expires=Sun, 20 Sep 2026 18:00:00 GMT; Path=/json".parse().unwrap());
        assert_eq!(cookie_from(&d, "_session_id").as_deref(), Some("_session_id=f00"));
    }
}
