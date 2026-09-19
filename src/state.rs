use std::collections::{BTreeMap, HashMap};
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use anyhow::Result;
use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Notify;

use crate::db::{self, Db};
use crate::jellyfin::Jellyfin;

pub type App = Arc<AppState>;

pub struct AppState {
    pub db: Db,
    pub data_dir: PathBuf,
    pub http: reqwest::Client,
    pub device_id: String,
    pub trust_proxy: bool,
    pub config: RwLock<Option<JfConfig>>,
    pub settings: RwLock<Settings>,
    pub tasks: Tasks,
    pub live: RwLock<Vec<Value>>,
    pub collector: RwLock<CollectorStatus>,
    pub login_attempts: Mutex<HashMap<IpAddr, (u32, i64)>>,
    /// Wakes background loops when configuration or settings change.
    pub wake: Notify,
}

#[derive(Clone, Debug)]
pub struct JfConfig {
    pub url: String,
    pub api_key: String,
    pub server_name: String,
    pub server_version: String,
    /// Set when the connection comes from environment variables rather than the setup wizard.
    pub from_env: bool,
}

// `default`: settings saved by an older version simply lack the newer keys.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Re-read the library when Jellyfin's own scan task finishes instead of on a timer.
    pub follow_jellyfin_scan: bool,
    /// Every Jellyfin user may sign in (they then get `default_permissions`).
    pub allow_user_login: bool,
    /// Permissions every signed-in non-admin has; per-user grants add to these.
    pub default_permissions: Vec<String>,
    /// How often to ask Jellyfin for sessions while somebody is watching…
    pub active_interval_s: i64,
    /// …and while nobody is. A new play is noticed at most this late.
    pub idle_interval_s: i64,
    pub sync_interval_h: i64,
    pub merge_window_s: i64,
    pub min_play_s: i64,
    /// Different people starting the same title within this many seconds are watching together.
    pub group_window_s: i64,
    /// Ask a public "what is my IP" service for this network's address, so that plays from it count as local.
    pub public_ip_lookup: bool,
    /// More addresses that count as home, added by hand.
    pub home_addresses: Vec<String>,
    /// Write a backup this often, in days. 0 turns automatic backups off.
    pub backup_every_d: i64,
    /// How many backups to keep; the oldest go first.
    pub backup_keep: i64,
}

impl Default for Settings {
    fn default() -> Self {
        Self { follow_jellyfin_scan: true, allow_user_login: false, default_permissions: vec![], active_interval_s: 1, idle_interval_s: 5, sync_interval_h: 6, merge_window_s: 600, min_play_s: 0, group_window_s: 60, public_ip_lookup: true, home_addresses: vec![], backup_every_d: 7, backup_keep: 5 }
    }
}

impl Settings {
    pub fn load(conn: &db::rusqlite::Connection) -> Result<Self> {
        Ok(match db::get_setting(conn, "settings")? {
            Some(raw) => serde_json::from_str(&raw).unwrap_or_default(),
            None => Self::default(),
        })
    }

    pub fn validate(&self) -> Result<(), String> {
        let check = |name: &str, v: i64, lo: i64, hi: i64| {
            if (lo..=hi).contains(&v) { Ok(()) } else { Err(format!("{name} must be between {lo} and {hi}")) }
        };
        if let Some(bad) = self.default_permissions.iter().find(|p| !crate::auth::GRANTABLE.contains(&p.as_str())) {
            return Err(format!("unknown permission `{bad}`"));
        }
        if let Some(bad) = self.home_addresses.iter().find(|a| crate::network::canonical(a).is_none()) {
            return Err(format!("`{bad}` is not an IP address"));
        }
        if self.home_addresses.len() > 50 {
            return Err("at most 50 home addresses".into());
        }
        check("backup_every_d", self.backup_every_d, 0, 365)?;
        check("backup_keep", self.backup_keep, 1, 100)?;
        check("active_interval_s", self.active_interval_s, 1, 60)?;
        check("idle_interval_s", self.idle_interval_s, 1, 60)?;
        check("sync_interval_h", self.sync_interval_h, 1, 168)?;
        check("merge_window_s", self.merge_window_s, 0, 86_400)?;
        check("group_window_s", self.group_window_s, 5, 600)?;
        check("min_play_s", self.min_play_s, 0, 3_600)
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct CollectorStatus {
    pub connected: bool,
    pub last_poll_at: i64,
    pub active_sessions: usize,
    pub error: Option<String>,
}

impl AppState {
    pub fn jellyfin(&self) -> Option<Jellyfin> {
        let cfg = self.config.read().unwrap();
        cfg.as_ref().map(|c| Jellyfin::new(self.http.clone(), &c.url, Some(c.api_key.clone()), &self.device_id))
    }

    pub fn jellyfin_anonymous(&self, url: &str) -> Jellyfin {
        Jellyfin::new(self.http.clone(), url, None, &self.device_id)
    }

    pub fn settings(&self) -> Settings {
        self.settings.read().unwrap().clone()
    }

    pub fn is_configured(&self) -> bool {
        self.config.read().unwrap().is_some()
    }
}

// ---------------------------------------------------------------- tasks

#[derive(Clone, Debug, Serialize)]
pub struct TaskState {
    pub id: &'static str,
    pub state: &'static str,
    pub message: Option<String>,
    pub progress: Option<f64>,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub error: Option<String>,
    pub result: Option<Value>,
}

#[derive(Clone)]
pub struct Tasks(Arc<Mutex<BTreeMap<&'static str, TaskState>>>);

pub const TASK_IDS: [&str; 8] = ["sync_users", "sync_libraries", "sync_events", "sync_server", "sync_userdata", "import", "backup", "restore"];

impl Tasks {
    pub fn new() -> Self {
        let map = TASK_IDS
            .iter()
            .map(|id| {
                (*id, TaskState { id, state: "idle", message: None, progress: None, started_at: None, finished_at: None, error: None, result: None })
            })
            .collect();
        Tasks(Arc::new(Mutex::new(map)))
    }

    /// Returns false when the task is already running.
    pub fn try_start(&self, id: &'static str, message: &str) -> bool {
        let mut map = self.0.lock().unwrap();
        let t = map.get_mut(id).expect("unknown task");
        if t.state == "running" {
            return false;
        }
        *t = TaskState {
            id,
            state: "running",
            message: Some(message.to_string()),
            progress: None,
            started_at: Some(db::now()),
            finished_at: None,
            error: None,
            result: None,
        };
        true
    }

    pub fn update(&self, id: &'static str, message: impl Into<String>, progress: Option<f64>) {
        if let Some(t) = self.0.lock().unwrap().get_mut(id) {
            t.message = Some(message.into());
            t.progress = progress.map(|p| p.clamp(0.0, 1.0));
        }
    }

    pub fn finish(&self, id: &'static str, outcome: Result<(String, Option<Value>)>) {
        if let Some(t) = self.0.lock().unwrap().get_mut(id) {
            t.finished_at = Some(db::now());
            t.progress = None;
            match outcome {
                Ok((msg, result)) => {
                    t.state = "ok";
                    t.message = Some(msg);
                    t.result = result;
                }
                Err(e) => {
                    tracing::error!("task {id} failed: {e:#}");
                    t.state = "error";
                    t.error = Some(format!("{e:#}"));
                    t.message = Some("Failed".into());
                }
            }
        }
    }

    pub fn snapshot(&self) -> Vec<TaskState> {
        self.0.lock().unwrap().values().cloned().collect()
    }
}

// ---------------------------------------------------------------- errors

pub struct ApiError(pub StatusCode, pub String);

impl ApiError {
    pub fn new(status: StatusCode, msg: impl Into<String>) -> Self {
        ApiError(status, msg.into())
    }
    pub fn bad_request(msg: impl Into<String>) -> Self {
        ApiError(StatusCode::BAD_REQUEST, msg.into())
    }
    pub fn not_found(what: &str) -> Self {
        ApiError(StatusCode::NOT_FOUND, format!("{what} not found"))
    }
    pub fn forbidden() -> Self {
        ApiError(StatusCode::FORBIDDEN, "Only Jellyfin administrators can do this".into())
    }
    pub fn not_permitted(what: &str) -> Self {
        ApiError(StatusCode::FORBIDDEN, format!("You don't have permission to {what}. A Jellyfin administrator can grant it in Settings."))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        tracing::error!("request failed: {e:#}");
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong on the server. Check the finstats log.".into())
    }
}

impl From<db::rusqlite::Error> for ApiError {
    fn from(e: db::rusqlite::Error) -> Self {
        anyhow::Error::from(e).into()
    }
}

pub type ApiResult<T = Json<Value>> = Result<T, ApiError>;
