//! Minimal Jellyfin API client — only the endpoints finstats needs.

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};

pub const APP_NAME: &str = "finstats";

#[derive(Clone)]
pub struct Jellyfin {
    http: Client,
    base: String,
    token: Option<String>,
    device_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PublicInfo {
    pub server_name: Option<String>,
    pub version: Option<String>,
    pub id: Option<String>,
}

#[derive(Debug)]
pub struct AuthResult {
    pub user_id: String,
    pub user_name: String,
    pub is_admin: bool,
    pub access_token: String,
}

#[derive(Debug)]
pub enum AuthError {
    InvalidCredentials,
    Other(anyhow::Error),
}

/// Jellyfin 10.x answers in PascalCase by default, newer servers in camelCase.
/// Asking for the profile explicitly gets the same shape from both.
const ACCEPT_PASCAL: &str = r#"application/json; profile="PascalCase""#;

pub fn http_client() -> Client {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(reqwest::header::ACCEPT, reqwest::header::HeaderValue::from_static(ACCEPT_PASCAL));
    Client::builder()
        .default_headers(headers)
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(60))
        .pool_max_idle_per_host(2)
        .user_agent(concat!("finstats/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("building http client")
}

/// Accepts what people actually type: missing scheme, trailing slash, a pasted `/web/` path.
pub fn normalize_url(input: &str) -> Result<String> {
    normalize_url_of(input, "your Jellyfin server")
}

/// The same forgiving reading for any service a person types the address of.
pub fn normalize_url_of(input: &str, what: &str) -> Result<String> {
    let mut s = input.trim().to_string();
    if s.is_empty() {
        bail!("Enter the address of {what}");
    }
    if !s.starts_with("http://") && !s.starts_with("https://") {
        s = format!("http://{s}");
    }
    if let Some(i) = s.find("/web") {
        if s[i..].starts_with("/web/") || s.ends_with("/web") {
            s.truncate(i);
        }
    }
    while s.ends_with('/') {
        s.pop();
    }
    reqwest::Url::parse(&s).map_err(|_| anyhow!("That doesn't look like a valid URL"))?;
    Ok(s)
}

/// `http://box:8096` → `ws://box:8096/socket`, `https://media.example/jf` → `wss://media.example/jf/socket`.
/// A base behind a reverse proxy keeps its path, which is why this is a swap on the already-normalised
/// string and not `Url::set_scheme` (which refuses http → ws).
pub fn socket_url_of(base: &str) -> Result<String> {
    let base = base.trim_end_matches('/');
    let ws = match base.split_once("://") {
        Some(("http", rest)) => format!("ws://{rest}"),
        Some(("https", rest)) => format!("wss://{rest}"),
        _ => bail!("{base} is not an http address"),
    };
    Ok(format!("{ws}/socket"))
}

/// Percent-encoding for the one query fallback below: everything but the unreserved set.
fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

impl Jellyfin {
    pub fn new(http: Client, base: &str, token: Option<String>, device_id: &str) -> Self {
        Self { http, base: base.trim_end_matches('/').to_string(), token, device_id: device_id.to_string() }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    fn auth_header(&self, token: Option<&str>) -> String {
        let mut h = format!(
            r#"MediaBrowser Client="{APP_NAME}", Device="{APP_NAME}", DeviceId="{}", Version="{}""#,
            self.device_id,
            env!("CARGO_PKG_VERSION")
        );
        if let Some(t) = token {
            h.push_str(&format!(r#", Token="{t}""#));
        }
        h
    }

    /// What `socket.rs` needs to open `/socket`: the URL and the same `Authorization` every read carries.
    /// The header, not `?api_key=`, so the key stays out of proxy logs and out of any message naming the URL.
    pub fn socket_handshake(&self) -> Result<(String, String)> {
        Ok((socket_url_of(&self.base)?, self.auth_header(self.token.as_deref())))
    }

    /// The same URL with the key in the query, for the one server that refuses the header on an upgrade.
    /// Never logged, never stored: it is built at the moment of the retry and dropped with it.
    pub fn socket_url_with_key(&self) -> Result<String> {
        let url = socket_url_of(&self.base)?;
        Ok(match self.token.as_deref() {
            Some(t) => format!("{url}?api_key={}&deviceId={}", urlencode(t), urlencode(&self.device_id)),
            None => url,
        })
    }

    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.http.get(format!("{}{}", self.base, path)).header("Authorization", self.auth_header(self.token.as_deref()))
    }

    async fn get_json(&self, path: &str, query: &[(&str, String)]) -> Result<Value> {
        let resp = self.get(path).query(query).send().await.with_context(|| format!("GET {path}"))?;
        let status = resp.status();
        if !status.is_success() {
            bail!("Jellyfin answered {status} for {path}");
        }
        resp.json().await.with_context(|| format!("decoding {path}"))
    }

    pub async fn public_info(&self) -> Result<PublicInfo> {
        let resp = self
            .http
            .get(format!("{}/System/Info/Public", self.base))
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    anyhow!("Timed out reaching {}", self.base)
                } else if e.is_connect() {
                    anyhow!("Could not connect to {}", self.base)
                } else {
                    anyhow!("Could not reach {}: {e}", self.base)
                }
            })?;
        if !resp.status().is_success() {
            bail!("{} answered {} — is this a Jellyfin server?", self.base, resp.status());
        }
        let info: PublicInfo =
            resp.json().await.map_err(|_| anyhow!("{} did not answer like a Jellyfin server", self.base))?;
        if info.id.is_none() && info.version.is_none() {
            bail!("{} did not answer like a Jellyfin server", self.base);
        }
        Ok(info)
    }

    pub async fn authenticate(&self, username: &str, password: &str) -> Result<AuthResult, AuthError> {
        let resp = self
            .http
            .post(format!("{}/Users/AuthenticateByName", self.base))
            .header("Authorization", self.auth_header(None))
            .json(&json!({ "Username": username, "Pw": password }))
            .send()
            .await
            .map_err(|e| AuthError::Other(anyhow!("Could not reach Jellyfin: {e}")))?;
        match resp.status() {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => return Err(AuthError::InvalidCredentials),
            s if !s.is_success() => return Err(AuthError::Other(anyhow!("Jellyfin answered {s}"))),
            _ => {}
        }
        let v: Value = resp.json().await.map_err(|e| AuthError::Other(e.into()))?;
        let user = &v["User"];
        Ok(AuthResult {
            user_id: crate::db::norm_id(user["Id"].as_str().unwrap_or_default()),
            user_name: user["Name"].as_str().unwrap_or(username).to_string(),
            is_admin: user["Policy"]["IsAdministrator"].as_bool().unwrap_or(false),
            access_token: v["AccessToken"].as_str().unwrap_or_default().to_string(),
        })
    }

    /// End the Jellyfin session created by `authenticate` so logins don't pile up as devices.
    pub async fn logout(&self, token: &str) {
        let _ = self
            .http
            .post(format!("{}/Sessions/Logout", self.base))
            .header("Authorization", self.auth_header(Some(token)))
            .send()
            .await;
    }

    /// Create (or reuse) an API key named after the app, using an admin's access token.
    pub async fn ensure_api_key(&self, admin_token: &str) -> Result<String> {
        let find = |v: &Value| -> Option<String> {
            v["Items"]
                .as_array()?
                .iter()
                .filter(|k| k["AppName"].as_str() == Some(APP_NAME))
                .filter_map(|k| k["AccessToken"].as_str())
                .next_back()
                .map(str::to_string)
        };
        let list = || async {
            let r = self
                .http
                .get(format!("{}/Auth/Keys", self.base))
                .header("Authorization", self.auth_header(Some(admin_token)))
                .send()
                .await?;
            if !r.status().is_success() {
                bail!("Jellyfin refused to list API keys ({})", r.status());
            }
            Ok(r.json::<Value>().await?)
        };
        if let Some(k) = find(&list().await?) {
            return Ok(k);
        }
        let r = self
            .http
            .post(format!("{}/Auth/Keys", self.base))
            .query(&[("app", APP_NAME)])
            .header("Authorization", self.auth_header(Some(admin_token)))
            .send()
            .await?;
        if !r.status().is_success() {
            bail!("Jellyfin refused to create an API key ({})", r.status());
        }
        find(&list().await?).ok_or_else(|| anyhow!("Created an API key but could not read it back"))
    }

    pub async fn system_info(&self) -> Result<Value> {
        self.get_json("/System/Info", &[]).await
    }

    pub async fn sessions(&self) -> Result<Vec<Value>> {
        let resp = self
            .get("/Sessions")
            .query(&[("ActiveWithinSeconds", "300")])
            .timeout(Duration::from_secs(15))
            .send()
            .await?;
        if !resp.status().is_success() {
            bail!("Jellyfin answered {} for /Sessions", resp.status());
        }
        Ok(resp.json().await?)
    }

    pub async fn users(&self) -> Result<Vec<Value>> {
        match self.get_json("/Users", &[]).await? {
            Value::Array(a) => Ok(a),
            _ => bail!("unexpected /Users response"),
        }
    }

    pub async fn virtual_folders(&self) -> Result<Vec<Value>> {
        match self.get_json("/Library/VirtualFolders", &[]).await? {
            Value::Array(a) => Ok(a),
            _ => bail!("unexpected /Library/VirtualFolders response"),
        }
    }

    /// One page of a library's items. Returns (items, total).
    pub async fn items_page(&self, library_id: &str, start: usize, limit: usize) -> Result<(Vec<Value>, usize)> {
        let q = [
            ("ParentId", library_id.to_string()),
            ("Recursive", "true".into()),
            (
                "IncludeItemTypes",
                "Movie,Series,Season,Episode,Audio,MusicAlbum,MusicVideo,Video,Book,AudioBook".into(),
            ),
            ("Fields", "Genres,DateCreated,MediaSources,Path,Overview,OriginalTitle,ProviderIds,Studios".into()),
            // Missing and unaired episodes exist in Jellyfin as virtual items without a file. They are
            // not part of the library as far as statistics go.
            ("ExcludeLocationTypes", "Virtual".into()),
            // With "group movies into collections" on, Jellyfin answers with the BoxSet *instead of* the
            // films inside it. Everything in a collection would be missing, and then flagged as removed.
            ("CollapseBoxSetItems", "false".into()),
            ("EnableUserData", "false".into()),
            ("EnableImageTypes", "Primary,Backdrop".into()),
            ("ImageTypeLimit", "1".into()),
            ("SortBy", "DateCreated,SortName".into()),
            ("SortOrder", "Ascending".into()),
            ("StartIndex", start.to_string()),
            ("Limit", limit.to_string()),
            ("EnableTotalRecordCount", (start == 0).to_string()),
        ];
        let resp = self.get("/Items").query(&q).timeout(Duration::from_secs(180)).send().await?;
        if !resp.status().is_success() {
            bail!("Jellyfin answered {} for /Items", resp.status());
        }
        let mut v: Value = resp.json().await?;
        let total = v["TotalRecordCount"].as_u64().unwrap_or(0) as usize;
        let items = match v["Items"].take() {
            Value::Array(a) => a,
            _ => vec![],
        };
        Ok((items, total))
    }

    /// One page of a library's films and shows with their cast and crew, and nothing else. Asked
    /// separately from `items_page`: people on every episode would multiply that read for no gain.
    pub async fn people_page(&self, library_id: &str, start: usize, limit: usize) -> Result<Vec<Value>> {
        let q = [
            ("ParentId", library_id.to_string()),
            ("Recursive", "true".into()),
            ("IncludeItemTypes", "Movie,Series".into()),
            ("Fields", "People".into()),
            ("ExcludeLocationTypes", "Virtual".into()),
            ("CollapseBoxSetItems", "false".into()),
            ("EnableUserData", "false".into()),
            ("EnableImages", "false".into()),
            ("SortBy", "DateCreated,SortName".into()),
            ("SortOrder", "Ascending".into()),
            ("StartIndex", start.to_string()),
            ("Limit", limit.to_string()),
            ("EnableTotalRecordCount", "false".into()),
        ];
        let resp = self.get("/Items").query(&q).timeout(Duration::from_secs(180)).send().await?;
        if !resp.status().is_success() {
            bail!("Jellyfin answered {} for /Items", resp.status());
        }
        let mut v: Value = resp.json().await?;
        Ok(match v["Items"].take() {
            Value::Array(a) => a,
            _ => vec![],
        })
    }

    async fn get_array(&self, path: &str, query: &[(&str, String)]) -> Result<Vec<Value>> {
        match self.get_json(path, query).await? {
            Value::Array(a) => Ok(a),
            mut v => match v["Items"].take() {
                Value::Array(a) => Ok(a),
                _ => bail!("unexpected response for {path}"),
            },
        }
    }

    /// Every device that has ever signed in, not just the ones with a session right now.
    pub async fn devices(&self) -> Result<Vec<Value>> {
        self.get_array("/Devices", &[]).await
    }

    pub async fn plugins(&self) -> Result<Vec<Value>> {
        self.get_array("/Plugins", &[]).await
    }

    pub async fn scheduled_tasks(&self) -> Result<Vec<Value>> {
        self.get_array("/ScheduledTasks", &[("isHidden", "false".into())]).await
    }

    /// Every task, the ones Jellyfin hides in its own dashboard included: "what is running right now"
    /// must not leave something out because Jellyfin does not usually draw it.
    pub async fn scheduled_tasks_all(&self) -> Result<Vec<Value>> {
        self.get_array("/ScheduledTasks", &[]).await
    }

    /// Jellyfin's own "Scan Media Library" task: (is it running right now, when it last finished).
    /// `None` when the server does not list such a task.
    pub async fn library_scan_status(&self) -> Result<Option<(bool, Option<i64>)>> {
        let tasks = self.scheduled_tasks().await?;
        Ok(tasks.iter().find(|t| t["Key"].as_str() == Some("RefreshLibrary") || t["Name"].as_str() == Some("Scan Media Library")).map(|t| {
            let running = t["State"].as_str().is_some_and(|s| s != "Idle");
            let finished = t["LastExecutionResult"]["EndTimeUtc"].as_str().and_then(crate::db::parse_ts);
            (running, finished)
        }))
    }

    /// Disk usage per library and system folder. Only exists on Jellyfin 10.11+.
    pub async fn storage(&self) -> Option<Value> {
        self.get_json("/System/Info/Storage", &[]).await.ok()
    }

    /// One page of a user's items matching a Jellyfin filter (`IsPlayed`, `IsFavorite`), with their UserData.
    pub async fn user_items_page(&self, user_id: &str, filter: &str, types: &str, start: usize, limit: usize) -> Result<Vec<Value>> {
        let q = [
            ("userId", user_id.to_string()),
            ("Recursive", "true".into()),
            ("Filters", filter.to_string()),
            ("IncludeItemTypes", types.to_string()),
            ("EnableUserData", "true".into()),
            ("EnableImages", "false".into()),
            ("EnableTotalRecordCount", "false".into()),
            ("StartIndex", start.to_string()),
            ("Limit", limit.to_string()),
        ];
        let resp = self.get("/Items").query(&q).timeout(Duration::from_secs(120)).send().await?;
        if !resp.status().is_success() {
            bail!("Jellyfin answered {} for a user's items", resp.status());
        }
        let mut v: Value = resp.json().await?;
        Ok(match v["Items"].take() {
            Value::Array(a) => a,
            _ => vec![],
        })
    }

    pub async fn activity_log(&self, start: usize, limit: usize, min_date: Option<&str>) -> Result<(Vec<Value>, usize)> {
        let mut q = vec![("startIndex", start.to_string()), ("limit", limit.to_string())];
        if let Some(d) = min_date {
            q.push(("minDate", d.to_string()));
        }
        let mut v = self.get_json("/System/ActivityLog/Entries", &q).await?;
        let total = v["TotalRecordCount"].as_u64().unwrap_or(0) as usize;
        let items = match v["Items"].take() {
            Value::Array(a) => a,
            _ => vec![],
        };
        Ok((items, total))
    }

    /// Fetch an image. `Ok(None)` when Jellyfin has none.
    pub async fn image(&self, path: &str, width: u32) -> Result<Option<(Vec<u8>, String)>> {
        let resp = self
            .get(path)
            .header(reqwest::header::ACCEPT, "image/*")
            .query(&[("fillWidth", width.to_string()), ("quality", "90".into())])
            .timeout(Duration::from_secs(20))
            .send()
            .await?;
        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            bail!("Jellyfin answered {} for image", resp.status());
        }
        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("image/jpeg")
            .to_string();
        Ok(Some((resp.bytes().await?.to_vec(), ct)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_socket_url_keeps_the_host_the_port_and_the_path_behind_a_proxy() {
        assert_eq!(socket_url_of("http://box:8096").unwrap(), "ws://box:8096/socket");
        assert_eq!(socket_url_of("https://media.example").unwrap(), "wss://media.example/socket");
        assert_eq!(socket_url_of("https://media.example/jellyfin").unwrap(), "wss://media.example/jellyfin/socket");
        assert_eq!(socket_url_of("http://192.168.1.10:8096/").unwrap(), "ws://192.168.1.10:8096/socket");
        assert!(socket_url_of("box:8096").is_err(), "an address without a scheme never reaches here");
    }

    #[test]
    fn the_key_in_a_query_is_encoded() {
        assert_eq!(urlencode("abc123"), "abc123");
        assert_eq!(urlencode("a b&c=d"), "a%20b%26c%3Dd");
    }
}
