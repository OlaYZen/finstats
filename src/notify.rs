//! What finstats has to say, and where it says it.
//!
//! Everything finstats works out is otherwise only visible on a page somebody has to remember to open.
//! This is the one way out: an *event* is raised where the thing is noticed, and every *destination*
//! that asked for that kind of event is sent a message about it.
//!
//! This is also the first thing finstats ever sends anywhere, so the rules are narrow and every one of
//! them is a test rather than a habit:
//!
//! - **Nothing leaves unless it was asked for.** A destination is a row somebody entered, with the kinds
//!   of event it wants ticked. No destination, no request; an unticked kind, no request.
//! - **Addresses and places stay behind a switch.** An event carries two bags: `data`, which any
//!   destination may be told, and `private` — IP addresses, coordinates — which is only rendered for a
//!   destination whose owner switched "include addresses" on. `message()` is the only thing that can
//!   reach either, so the rule holds by construction.
//! - **An event is written once.** `raise` is `INSERT OR IGNORE` on `dedupe`, exactly like
//!   `security_alerts`, so deriving the same thing twice sends nothing twice. Delivery is a separate row
//!   per destination with its own attempts and clock: a webhook that is down delays nothing else.
//! - **Nothing old is ever sent.** Something found more than `HISTORIC_S` after it happened is recorded
//!   and marked `historic`, and a destination never hears about anything that happened before it existed.
//!   Between them, adding a destination cannot flood it with a year of history.
//! - **A personal destination carries exactly what its owner can see in the app** (`Need`, checked against
//!   the same `Perms` every page is checked against), and may only point at a public host: it is somebody
//!   else's address that finstats would be making requests to.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use axum::Json;
use axum::extract::{Path, Query, State};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::{AuthUser, Perms};
use crate::channels::{self, Channel};
use crate::db::rusqlite::{Connection, OptionalExtension, params};
use crate::db;
use crate::state::{ApiError, ApiResult, App, Settings};

/// Found this long after it happened, an event is history: it is recorded, and nobody is told.
pub const HISTORIC_S: i64 = 6 * 3_600;
/// At most this many destinations in total, and this many belonging to one person.
const MAX_TARGETS: usize = 20;
const MAX_OWN_TARGETS: usize = 5;
/// A destination is sent at most this many messages a minute; the rest wait for the next one.
const PER_MINUTE: usize = 20;
/// How many due deliveries one pass takes on.
const BATCH: usize = 20;
/// How long the history of what was sent is kept. Long enough to answer "did I hear about that?", and
/// short enough that a server where every play is announced does not keep a row for each one for ever.
/// Nothing depends on an old row: every source either re-derives only the last few hours or raises an
/// event once, when the thing itself is first written down.
const KEEP_S: i64 = 30 * 86_400;

// ---------------------------------------------------------------- the catalogue

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Security,
    Housekeeping,
    Library,
    Playback,
}

impl Group {
    pub const ALL: [Group; 4] = [Group::Security, Group::Housekeeping, Group::Library, Group::Playback];

    pub fn key(self) -> &'static str {
        match self {
            Group::Security => "security",
            Group::Housekeeping => "housekeeping",
            Group::Library => "library",
            Group::Playback => "playback",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Group::Security => "Security",
            Group::Housekeeping => "Housekeeping",
            Group::Library => "Library",
            Group::Playback => "Playback",
        }
    }
}

/// Who may be told about an event of this kind on a destination of their own. A destination belonging to
/// the server itself is not filtered: only a Jellyfin administrator can add one, and they see everything.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Need {
    /// Nobody in particular is in it.
    Anyone,
    /// About a person: their own always, somebody else's needs `see_everyone`.
    Person,
    /// About a person *and* where they were: `see_network` as well, which is `security::gate`'s rule.
    Place,
    /// About the server itself: `see_server`.
    Server,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Travel,
    NewCountry,
    FailedSignIns,
    TaskFailed,
    BackupFailed,
    ServiceDown,
    ServiceBack,
    NewItems,
    RequestAvailable,
    PlayStarted,
    PlayStopped,
    /// The message the Test button sends. Never ticked, never fanned out: it goes to one destination.
    Test,
}

impl Kind {
    pub const ALL: [Kind; 12] = [
        Kind::Travel, Kind::NewCountry, Kind::FailedSignIns, Kind::TaskFailed, Kind::BackupFailed, Kind::ServiceDown,
        Kind::ServiceBack, Kind::NewItems, Kind::RequestAvailable, Kind::PlayStarted, Kind::PlayStopped, Kind::Test,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Kind::Travel => "travel",
            Kind::NewCountry => "new_country",
            Kind::FailedSignIns => "failed_sign_ins",
            Kind::TaskFailed => "task_failed",
            Kind::BackupFailed => "backup_failed",
            Kind::ServiceDown => "service_down",
            Kind::ServiceBack => "service_back",
            Kind::NewItems => "new_items",
            Kind::RequestAvailable => "request_available",
            Kind::PlayStarted => "play_started",
            Kind::PlayStopped => "play_stopped",
            Kind::Test => "test",
        }
    }

    pub fn from_key(key: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.key() == key)
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Travel => "Impossible travel",
            Kind::NewCountry => "A new country",
            Kind::FailedSignIns => "Failed sign-ins",
            Kind::TaskFailed => "A task failed",
            Kind::BackupFailed => "A backup failed",
            Kind::ServiceDown => "Something stopped answering",
            Kind::ServiceBack => "Something is answering again",
            Kind::NewItems => "New in the library",
            Kind::RequestAvailable => "A request is watchable",
            Kind::PlayStarted => "Somebody started watching",
            Kind::PlayStopped => "Somebody stopped watching",
            Kind::Test => "Test message",
        }
    }

    /// One line, in the owner's words, for the list of things a destination can be told about.
    pub fn what(self) -> &'static str {
        match self {
            Kind::Travel => "Two sightings of one account that no flight connects",
            Kind::NewCountry => "The first time an account is seen in a country",
            Kind::FailedSignIns => "Several failed sign-ins for one account, or from one address, in a row",
            Kind::TaskFailed => "A sync, an import or a read of a connected service ended in an error",
            Kind::BackupFailed => "A backup could not be written",
            Kind::ServiceDown => "Jellyfin, Sonarr, Radarr or Seerr stopped answering",
            Kind::ServiceBack => "…and when it answers again",
            Kind::NewItems => "Titles added to the library, folded per show and day",
            Kind::RequestAvailable => "Something somebody asked for in Seerr can now be watched",
            Kind::PlayStarted => "A play begins: the title, the person and the device",
            Kind::PlayStopped => "A play ends, with how much of it was watched",
            Kind::Test => "Sent by the Test button, and by nothing else",
        }
    }

    pub fn group(self) -> Option<Group> {
        Some(match self {
            Kind::Travel | Kind::NewCountry | Kind::FailedSignIns => Group::Security,
            Kind::TaskFailed | Kind::BackupFailed | Kind::ServiceDown | Kind::ServiceBack => Group::Housekeeping,
            Kind::NewItems | Kind::RequestAvailable => Group::Library,
            Kind::PlayStarted | Kind::PlayStopped => Group::Playback,
            Kind::Test => return None,
        })
    }

    pub fn severity(self) -> &'static str {
        match self {
            Kind::Travel => ALERT,
            Kind::FailedSignIns | Kind::TaskFailed | Kind::BackupFailed | Kind::ServiceDown => WARN,
            _ => INFO,
        }
    }

    pub fn need(self) -> Need {
        match self {
            Kind::Travel | Kind::NewCountry | Kind::FailedSignIns => Need::Place,
            Kind::TaskFailed | Kind::BackupFailed | Kind::ServiceDown | Kind::ServiceBack => Need::Server,
            Kind::RequestAvailable | Kind::PlayStarted | Kind::PlayStopped => Need::Person,
            Kind::NewItems | Kind::Test => Need::Anyone,
        }
    }

    /// Everything a destination can ask for. The test message is not on the list: it is sent by hand.
    pub fn tickable() -> impl Iterator<Item = Kind> {
        Kind::ALL.into_iter().filter(|k| k.group().is_some())
    }

    /// What a new destination starts with ticked. The chatty one is left off on purpose.
    pub fn default_events() -> Vec<String> {
        Kind::tickable().filter(|k| k.group() != Some(Group::Playback)).map(|k| k.key().to_string()).collect()
    }
}

pub const INFO: &str = "info";
pub const WARN: &str = "warn";
pub const ALERT: &str = "alert";
pub const SEVERITIES: [&str; 3] = [INFO, WARN, ALERT];

fn rank(severity: &str) -> usize {
    SEVERITIES.iter().position(|s| *s == severity).unwrap_or(0)
}

// ---------------------------------------------------------------- an event

/// Something worth telling somebody about. `data` is what any destination may be told; `private` holds
/// the addresses and coordinates that only a destination with "include addresses" ever sees.
pub struct Event {
    pub kind: Kind,
    pub at: i64,
    pub dedupe: String,
    pub severity: &'static str,
    pub user_id: Option<String>,
    pub user_name: Option<String>,
    pub title: String,
    pub body: String,
    /// A path inside finstats (`/security`), joined with `public_url` when a message is built.
    pub link: Option<String>,
    data: Vec<(String, String)>,
    private: Vec<(String, String)>,
}

impl Event {
    pub fn new(kind: Kind, dedupe: impl Into<String>, title: impl Into<String>, body: impl Into<String>) -> Self {
        Event {
            kind,
            at: db::now(),
            dedupe: dedupe.into(),
            severity: kind.severity(),
            user_id: None,
            user_name: None,
            title: title.into(),
            body: body.into(),
            link: None,
            data: vec![],
            private: vec![],
        }
    }

    /// When the thing itself happened, if that is not now.
    pub fn at(mut self, at: i64) -> Self {
        self.at = at;
        self
    }

    pub fn about(mut self, user_id: impl Into<String>, user_name: impl Into<String>) -> Self {
        self.user_id = Some(user_id.into());
        self.user_name = Some(user_name.into());
        self
    }

    pub fn link(mut self, path: impl Into<String>) -> Self {
        self.link = Some(path.into());
        self
    }

    pub fn severity(mut self, severity: &'static str) -> Self {
        self.severity = severity;
        self
    }

    /// A line every destination may be told.
    pub fn field(mut self, label: &str, value: impl Into<String>) -> Self {
        self.data.push((label.to_string(), value.into()));
        self
    }

    /// A line that names an address or a place: only for a destination that asked for them.
    pub fn private_field(mut self, label: &str, value: impl Into<String>) -> Self {
        self.private.push((label.to_string(), value.into()));
        self
    }
}

/// One event as it was written down, which is what a message is built from.
pub struct Stored {
    pub id: i64,
    pub kind: Kind,
    pub severity: String,
    pub at: i64,
    pub user_name: Option<String>,
    pub title: String,
    pub body: String,
    pub link: Option<String>,
    data: Vec<(String, String)>,
    private: Vec<(String, String)>,
}

fn pairs_json(list: &[(String, String)]) -> String {
    Value::Array(list.iter().map(|(k, v)| json!([k, v])).collect()).to_string()
}

fn pairs_of(raw: &str) -> Vec<(String, String)> {
    serde_json::from_str::<Value>(raw)
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|p| Some((p.get(0)?.as_str()?.to_string(), p.get(1)?.as_str()?.to_string())))
        .collect()
}

/// A message on its way out, the same for every kind of destination; `channels.rs` decides how each one
/// carries it. The only place `private` is ever read, and only when the destination asked for it.
pub struct Message {
    pub kind: Kind,
    pub at: i64,
    pub title: String,
    pub body: String,
    pub link: Option<String>,
    pub severity: String,
    /// Whose it is, by name. A destination is told who, never the Jellyfin id.
    pub user: Option<String>,
    pub fields: Vec<(String, String)>,
}

impl Message {
    /// Title, body and every field as one piece of text, for the destinations that take only that.
    pub fn text(&self) -> String {
        let mut out = self.body.clone();
        for (label, value) in &self.fields {
            out.push_str(&format!("\n{label}: {value}"));
        }
        if let Some(link) = &self.link {
            out.push_str(&format!("\n{link}"));
        }
        out
    }
}

/// What one event says. `with_addresses` is the per-destination switch: without it, nothing an address or
/// a coordinate was put in can appear, because `private` is not read at all.
pub fn message(e: &Stored, with_addresses: bool, public_url: Option<&str>) -> Message {
    let mut fields = e.data.clone();
    if with_addresses {
        fields.extend(e.private.iter().cloned());
    }
    let link = e.link.as_ref().and_then(|path| {
        let base = public_url?.trim_end_matches('/');
        (!base.is_empty()).then(|| format!("{base}{path}"))
    });
    Message { kind: e.kind, at: e.at, title: e.title.clone(), body: e.body.clone(), link, severity: e.severity.clone(), user: e.user_name.clone(), fields }
}

// ---------------------------------------------------------------- destinations

// Deliberately neither `Debug` nor `Serialize`: a Discord webhook URL *is* its credential, so the address
// is as much a secret as the key beside it and there must be no way to print or send one by accident.
pub struct Target {
    pub id: i64,
    pub channel: Channel,
    pub name: String,
    url: String,
    secret: String,
    pub topic: Option<String>,
    /// What this kind of destination needs beyond the three above: only mail has any (`from`, `username`).
    /// Not a secret — the password is `secret` — but not something a page needs either.
    pub options: BTreeMap<String, String>,
    /// `None` = the server's own destination; otherwise the person it belongs to.
    pub owner_id: Option<String>,
    pub events: Vec<String>,
    pub with_addresses: bool,
    pub min_severity: String,
    pub accept_invalid_certs: bool,
    pub enabled: bool,
    pub created_at: i64,
}

impl Target {
    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn secret(&self) -> &str {
        &self.secret
    }

    /// One of the extra fields this kind of destination asked for, or "" when it was never given.
    pub fn option(&self, key: &str) -> &str {
        self.options.get(key).map(String::as_str).unwrap_or_default()
    }

    pub fn wants(&self, kind: Kind) -> bool {
        self.events.iter().any(|e| e == kind.key())
    }

    /// Host and port only, and the last thing that could be read as a name — never the token. Which
    /// topic, which chat and which mailbox are shown because otherwise two destinations of a kind read
    /// the same; a Pushover user key is not, because it is half of what it takes to send.
    pub fn shown(&self) -> String {
        let host = crate::outbound::host_of(&self.url);
        let topic = self.topic.as_deref().filter(|t| !t.is_empty());
        match (topic, self.channel) {
            (Some(to), Channel::Email) => format!("{to} via {host}"),
            (Some(topic), Channel::Ntfy | Channel::Telegram) => format!("{host}/{topic}"),
            _ => format!("{host}/…"),
        }
    }
}

fn read_target(r: &crate::db::rusqlite::Row) -> crate::db::rusqlite::Result<Option<Target>> {
    let channel: String = r.get(1)?;
    let Some(channel) = Channel::from_key(&channel) else { return Ok(None) }; // a kind from a newer finstats
    let events: String = r.get(7)?;
    Ok(Some(Target {
        id: r.get(0)?,
        channel,
        name: r.get(2)?,
        url: r.get(3)?,
        secret: r.get(4)?,
        topic: r.get(5)?,
        owner_id: r.get(6)?,
        events: serde_json::from_str(&events).unwrap_or_default(),
        with_addresses: r.get(8)?,
        min_severity: r.get(9)?,
        accept_invalid_certs: r.get(10)?,
        enabled: r.get(11)?,
        created_at: r.get(12)?,
        options: serde_json::from_str(&r.get::<_, String>(13)?).unwrap_or_default(),
    }))
}

const TARGET_COLS: &str = "id, kind, name, url, secret, topic, owner_id, events, with_addresses, min_severity, accept_invalid_certs, enabled, created_at, options";

fn load(conn: &Connection) -> Result<Vec<Target>> {
    let mut stmt = conn.prepare(&format!("SELECT {TARGET_COLS} FROM notify_targets ORDER BY id"))?;
    let rows = stmt.query_map([], read_target)?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows.into_iter().flatten().collect())
}

/// Read the destinations into memory: at start-up and after every change. The bus asks memory, never the
/// database, because `raise` is called from inside transactions that are already holding a connection.
pub async fn reload(app: &App) -> Result<()> {
    let rows = app.db.call(|c| load(c)).await?;
    *app.notify_targets.write().unwrap() = Arc::new(rows.into_iter().map(Arc::new).collect());
    Ok(())
}

pub fn all(app: &App) -> Arc<Vec<Arc<Target>>> {
    app.notify_targets.read().unwrap().clone()
}

// ---------------------------------------------------------------- who gets what

/// Everything the fan-out needs besides the event itself: the destinations, and what each personal one's
/// owner is allowed to see. Built once per scan, so a whole security rescan costs one read.
pub struct Fanout {
    pub targets: Arc<Vec<Arc<Target>>>,
    /// The owner of a personal destination → what they may see; absent when they may not sign in at all.
    pub perms: HashMap<String, Perms>,
}

impl Fanout {
    /// Is there any destination at all that would want this? An install with no destinations (most of
    /// them) must not be writing a row every time a play starts, so nothing is recorded that nothing
    /// wants. A destination added later is only ever told about what happens after it exists anyway.
    pub fn anyone_wants(&self, kind: Kind) -> bool {
        self.targets.iter().any(|t| t.enabled && t.wants(kind))
    }

    pub fn of(conn: &Connection, app: &App) -> Result<Fanout> {
        Fanout::build(conn, all(app), &app.settings())
    }

    pub fn build(conn: &Connection, targets: Arc<Vec<Arc<Target>>>, settings: &Settings) -> Result<Fanout> {
        let mut perms = HashMap::new();
        for owner in targets.iter().filter_map(|t| t.owner_id.clone()) {
            if perms.contains_key(&owner) {
                continue;
            }
            let is_admin: Option<bool> =
                conn.query_row("SELECT is_admin FROM users WHERE id = ?1", [&owner], |r| r.get(0)).optional()?;
            let Some(is_admin) = is_admin else { continue };
            let grants = crate::auth::stored_grants(conn, &owner)?;
            if let Some(p) = crate::auth::effective(is_admin, &grants, settings) {
                perms.insert(owner, p);
            }
        }
        Ok(Fanout { targets, perms })
    }
}

/// May this destination be told about this event? Pure, and the whole rule in one place: the same event,
/// destination and permissions always decide the same way.
pub fn wanted_by(t: &Target, kind: Kind, severity: &str, at: i64, about: Option<&str>, owner: Option<Perms>) -> bool {
    if !t.enabled || !t.wants(kind) || rank(severity) < rank(&t.min_severity) {
        return false;
    }
    // Nothing that happened before a destination existed: adding one is not a way to be sent the year.
    if at < t.created_at {
        return false;
    }
    let Some(owner_id) = &t.owner_id else { return true }; // the server's own destination
    let Some(perms) = owner else { return false }; // somebody who may not even sign in
    let own = about.is_some_and(|u| u == owner_id);
    match kind.need() {
        Need::Anyone => true,
        Need::Person => own || perms.see_everyone,
        Need::Place => own || (perms.see_everyone && perms.see_network),
        Need::Server => perms.see_server,
    }
}

// ---------------------------------------------------------------- raising one

/// Write an event down and queue it for every destination that wants it. `false` when it was already
/// known — which is the normal answer, because most sources re-derive the same events every pass.
pub fn raise_in(conn: &Connection, f: &Fanout, e: &Event) -> Result<bool> {
    if !f.anyone_wants(e.kind) {
        return Ok(false); // nobody is listening: nothing to write down
    }
    let now = db::now();
    let historic = now - e.at > HISTORIC_S;
    let inserted = conn.prepare_cached(
        "INSERT OR IGNORE INTO notify_events(kind, severity, at, created_at, dedupe, user_id, user_name, title, body, link, data, private, historic)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
    )?
    .execute(params![
        e.kind.key(), e.severity, e.at, now, e.dedupe, e.user_id, e.user_name, e.title, e.body, e.link,
        pairs_json(&e.data), pairs_json(&e.private), historic
    ])?;
    if inserted == 0 {
        return Ok(false);
    }
    if historic {
        return Ok(true); // recorded, and nobody is told: it is history, not news
    }
    let event_id = conn.last_insert_rowid();
    let mut queue = conn.prepare_cached("INSERT OR IGNORE INTO notify_deliveries(event_id, target_id, state, next_at) VALUES (?1, ?2, 'queued', ?3)")?;
    for t in f.targets.iter() {
        let owner = t.owner_id.as_ref().and_then(|o| f.perms.get(o)).copied();
        if wanted_by(t, e.kind, e.severity, e.at, e.user_id.as_deref(), owner) {
            queue.execute(params![event_id, t.id, now])?;
        }
    }
    Ok(true)
}

/// The same, for the callers that are not already holding a connection. Never fails a caller: a
/// notification that cannot be written is a line in the log, not a reason for the thing itself to fail.
pub async fn raise(app: &App, e: Event) -> bool {
    // Asked of memory before the database is touched at all: on an install with no destinations this is
    // the whole cost of a play beginning.
    if !all(app).iter().any(|t| t.enabled && t.wants(e.kind)) {
        return false;
    }
    let app2 = app.clone();
    let done = app.db.call(move |c| {
        let f = Fanout::of(c, &app2)?;
        raise_in(c, &f, &e)
    }).await;
    match done {
        Ok(true) => {
            app.notify_wake.notify_one();
            true
        }
        Ok(false) => false,
        Err(err) => {
            tracing::warn!("could not write a notification: {err:#}");
            false
        }
    }
}

// ---------------------------------------------------------------- the small ones, raised from anywhere

/// One a day per thing that went wrong: a job that fails every quarter of an hour must not be a message
/// every quarter of an hour, and the Tasks card says the rest.
fn daily(prefix: &str, what: &str) -> String {
    format!("notify:{prefix}:{what}:{}", db::now() / 86_400)
}

/// What a background job is called, for somebody who never reads the log. The Tasks card in Settings
/// says the same things in the same words.
fn task_label(id: &str) -> &str {
    match id {
        "sync_users" => "Reading the users",
        "sync_libraries" => "Reading the library",
        "sync_events" => "Reading the server log",
        "sync_server" => "Reading the server details",
        "sync_userdata" => "Reading watched and favourites",
        "sync_upcoming" => "Reading the Sonarr and Radarr calendars",
        "sync_requests" => "Reading the requests from Seerr",
        "sync_grabs" => "Reading the download history",
        "import" => "The Jellystat import",
        "backup" => "Writing a backup",
        "geoip" => "The geolocation database",
        other => other,
    }
}

/// A background job ended in an error.
pub async fn task_failed(app: &App, id: &str, error: &str) {
    let event = Event::new(
        Kind::TaskFailed,
        daily("task", id),
        format!("{} failed", task_label(id)),
        format!("{} ended in an error. finstats will try again on its own schedule.", task_label(id)),
    )
    .field("Job", id.to_string())
    .field("Error", error.chars().take(400).collect::<String>())
    .link("/settings");
    raise(app, event).await;
}

/// A backup could not be written. The one kind of failure that quietly costs you everything.
pub async fn backup_failed(app: &App, error: &str) {
    let event = Event::new(
        Kind::BackupFailed,
        daily("backup", "write"),
        "A backup could not be written",
        "finstats could not write its automatic backup. Check the finstats log and the data folder.",
    )
    .field("Error", error.chars().take(400).collect::<String>())
    .link("/settings");
    raise(app, event).await;
}

/// Something finstats reads from stopped answering, or started again. Both are worth saying once.
pub async fn service_state(app: &App, name: &str, what: &str, down: Option<&str>) {
    let event = match down {
        Some(error) => Event::new(
            Kind::ServiceDown,
            daily("down", name),
            format!("{name} is not answering"),
            format!("finstats cannot read from {name} ({what})."),
        )
        .field("Error", error.chars().take(400).collect::<String>()),
        None => Event::new(
            Kind::ServiceBack,
            daily("back", name),
            format!("{name} is answering again"),
            format!("finstats can read from {name} ({what}) again."),
        ),
    };
    raise(app, event.field("Connection", name.to_string()).link("/settings")).await;
}

// ---------------------------------------------------------------- sending

/// How long to wait before trying a delivery again, by how many attempts have already failed.
/// `None` gives up: five attempts over about eight hours is long enough for anything that is coming back.
pub fn backoff(attempts: i64) -> Option<i64> {
    match attempts {
        1 => Some(30),
        2 => Some(120),
        3 => Some(600),
        4 => Some(3_600),
        _ => None,
    }
}

/// One delivery waiting to go out, with the event it is about.
pub struct Due {
    pub event: Stored,
    pub target_id: i64,
    pub attempts: i64,
}

fn due(conn: &Connection, now: i64, limit: usize) -> Result<Vec<Due>> {
    let mut stmt = conn.prepare_cached(
        "SELECT e.id, e.kind, e.severity, e.at, e.user_name, e.title, e.body, e.link, e.data, e.private, d.target_id, d.attempts
         FROM notify_deliveries d JOIN notify_events e ON e.id = d.event_id
         WHERE d.state = 'queued' AND d.next_at <= ?1 ORDER BY d.next_at, d.event_id LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![now, limit as i64], |r| {
            let kind: String = r.get(1)?;
            let (data, private): (String, String) = (r.get(8)?, r.get(9)?);
            Ok((
                Due {
                    event: Stored {
                        id: r.get(0)?,
                        kind: Kind::from_key(&kind).unwrap_or(Kind::Test),
                        severity: r.get(2)?,
                        at: r.get(3)?,
                        user_name: r.get(4)?,
                        title: r.get(5)?,
                        body: r.get(6)?,
                        link: r.get(7)?,
                        data: pairs_of(&data),
                        private: pairs_of(&private),
                    },
                    target_id: r.get(10)?,
                    attempts: r.get(11)?,
                },
                kind,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    // A kind this version does not know (a database from a newer finstats) is left in the queue untouched.
    Ok(rows.into_iter().filter(|(_, key)| Kind::from_key(key).is_some()).map(|(d, _)| d).collect())
}

/// When the next queued delivery is due, so the loop can sleep until then instead of asking every second.
fn next_due_at(conn: &Connection) -> Result<Option<i64>> {
    Ok(conn.query_row("SELECT MIN(next_at) FROM notify_deliveries WHERE state = 'queued'", [], |r| r.get(0))?)
}

/// Note how one delivery went, and how the destination is doing. `Ok` means it was accepted.
fn record(conn: &Connection, event_id: i64, target_id: i64, attempts: i64, outcome: std::result::Result<(), (String, Option<i64>)>) -> Result<()> {
    let now = db::now();
    match outcome {
        Ok(()) => {
            conn.execute("UPDATE notify_deliveries SET state = 'sent', attempts = ?3, sent_at = ?4, error = NULL WHERE event_id = ?1 AND target_id = ?2", params![event_id, target_id, attempts, now])?;
            conn.execute("UPDATE notify_targets SET last_ok_at = ?2, last_error = NULL WHERE id = ?1", params![target_id, now])?;
        }
        Err((message, retry_after)) => {
            // A destination that asked for a delay gets it; otherwise the usual ladder.
            let wait = retry_after.filter(|s| *s > 0).map(|s| s.min(3_600)).or_else(|| backoff(attempts));
            match wait {
                Some(wait) => conn.execute(
                    "UPDATE notify_deliveries SET attempts = ?3, next_at = ?4, error = ?5 WHERE event_id = ?1 AND target_id = ?2",
                    params![event_id, target_id, attempts, now + wait, message],
                )?,
                None => conn.execute(
                    "UPDATE notify_deliveries SET state = 'failed', attempts = ?3, error = ?4 WHERE event_id = ?1 AND target_id = ?2",
                    params![event_id, target_id, attempts, message],
                )?,
            };
            conn.execute("UPDATE notify_targets SET last_error = ?2 WHERE id = ?1", params![target_id, message])?;
        }
    }
    Ok(())
}

/// How many of this minute a destination has already used. A busy evening must not get a webhook
/// rate-limited into silence, so the rest simply wait for the next minute.
fn over_the_minute(used: &mut HashMap<i64, (i64, usize)>, target_id: i64, now: i64) -> bool {
    let minute = now / 60;
    let slot = used.entry(target_id).or_insert((minute, 0));
    if slot.0 != minute {
        *slot = (minute, 0);
    }
    if slot.1 >= PER_MINUTE {
        return true;
    }
    slot.1 += 1;
    false
}

/// Everything queued that is due, sent once each. Returns when there is nothing left to do now.
async fn drain(app: &App, used: &mut HashMap<i64, (i64, usize)>) {
    loop {
        let now = db::now();
        let batch = match app.db.call(move |c| due(c, now, BATCH)).await {
            Ok(b) if !b.is_empty() => b,
            Ok(_) => return,
            Err(e) => {
                tracing::warn!("could not read the notification queue: {e:#}");
                return;
            }
        };
        let targets = all(app);
        for item in batch {
            let Some(target) = targets.iter().find(|t| t.id == item.target_id).cloned() else {
                // The destination was deleted while this was queued; the row goes with it on the next write.
                let _ = app.db.call(move |c| Ok(c.execute("DELETE FROM notify_deliveries WHERE target_id = ?1", [item.target_id])?)).await;
                continue;
            };
            if over_the_minute(used, target.id, db::now()) {
                let (event_id, target_id, next) = (item.event.id, target.id, (db::now() / 60 + 1) * 60);
                let _ = app.db.call(move |c| Ok(c.execute("UPDATE notify_deliveries SET next_at = ?3 WHERE event_id = ?1 AND target_id = ?2", params![event_id, target_id, next])?)).await;
                continue;
            }
            let public_url = app.settings().public_url.clone();
            let msg = message(&item.event, target.with_addresses, Some(&public_url));
            let outcome = channels::send(app, &target, &msg).await;
            let (event_id, target_id, attempts) = (item.event.id, target.id, item.attempts + 1);
            let outcome = match outcome {
                Ok(()) => Ok(()),
                Err(e) => {
                    tracing::warn!("{} did not take a notification: {}", target.name, e.message);
                    Err((e.message, e.retry_after))
                }
            };
            if let Err(e) = app.db.call(move |c| record(c, event_id, target_id, attempts, outcome)).await {
                tracing::warn!("could not note how a notification went: {e:#}");
            }
        }
    }
}

/// The loop that empties the queue: woken when something is raised, and otherwise asleep until the next
/// retry is due. An install with no destinations never wakes at all.
pub async fn run(app: App) {
    let mut used: HashMap<i64, (i64, usize)> = HashMap::new();
    let mut pruned_at = 0i64;
    loop {
        drain(&app, &mut used).await;
        if db::now() - pruned_at >= 3_600 {
            pruned_at = db::now();
            let cutoff = pruned_at - KEEP_S;
            if let Err(e) = app.db.call(move |c| Ok(c.execute("DELETE FROM notify_events WHERE created_at < ?1", [cutoff])?)).await {
                tracing::debug!("could not thin out the notification history: {e:#}");
            }
        }
        let next = app.db.call(|c| next_due_at(c)).await.ok().flatten();
        let wait = match next {
            Some(at) => (at - db::now()).clamp(1, 300) as u64,
            None => 300,
        };
        tokio::select! {
            _ = app.notify_wake.notified() => {}
            _ = tokio::time::sleep(Duration::from_secs(wait)) => {}
        }
    }
}

// ---------------------------------------------------------------- where a personal destination may point

/// A personal destination is somebody else's address that finstats would be making requests to, so it may
/// only be a public one: otherwise granting the permission would be handing out a way to knock on doors
/// inside the network. Checked when it is saved *and* before every send, because a name that answered
/// publicly yesterday can be pointed at the router today.
pub async fn must_be_public(url: &str) -> Result<()> {
    let parsed = reqwest::Url::parse(url).map_err(|_| anyhow!("That doesn't look like a valid URL"))?;
    let host = parsed.host_str().ok_or_else(|| anyhow!("The address needs a host name"))?;
    let port = parsed.port_or_known_default().unwrap_or(443);
    let addresses: Vec<std::net::IpAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|_| anyhow!("{host} could not be looked up"))?
        .map(|a| a.ip())
        .collect();
    if addresses.is_empty() {
        bail!("{host} could not be looked up");
    }
    if addresses.iter().any(|a| db::is_local_ip(&a.to_string()).unwrap_or(true)) {
        bail!("{host} is an address on this network. Your own destinations must point at a public service — ask an administrator to add it for the server instead");
    }
    Ok(())
}

// ---------------------------------------------------------------- the address somebody typed

/// What people type, made canonical, and nothing that could aim the request elsewhere or hide a second
/// address inside the first. The path stays (a base path, a Discord webhook's own token); a query stays
/// only for a plain webhook, where some receivers put their key.
pub fn clean_url(input: &str, channel: Channel) -> Result<String> {
    let typed = input.trim();
    if typed.is_empty() {
        bail!("Enter the address to send to");
    }
    if channel.is_mail() {
        return clean_mail_url(typed);
    }
    let lower = typed.to_ascii_lowercase();
    if lower.contains("://") && !lower.starts_with("http://") && !lower.starts_with("https://") {
        bail!("The address must start with http:// or https://");
    }
    let with_scheme = if lower.contains("://") { typed.to_string() } else { format!("https://{typed}") };
    let parsed = reqwest::Url::parse(&with_scheme).map_err(|_| anyhow!("That doesn't look like a valid URL"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        bail!("The address must start with http:// or https://");
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        bail!("Leave the user name and password out of the address");
    }
    if parsed.fragment().is_some() {
        bail!("The address must not contain #");
    }
    if parsed.query().is_some() && channel != Channel::Webhook {
        bail!("The address must not contain ?");
    }
    // A service finstats knows the address of takes that address and no other: a token is for the
    // service it was issued by, and a look-alike host is how it would reach somebody else.
    if let Some(fixed) = channel.fixed_url() {
        let same = parsed.scheme() == "https" && parsed.host_str() == reqwest::Url::parse(fixed).ok().and_then(|f| f.host_str().map(str::to_string)).as_deref();
        if !same || !parsed.path().trim_matches('/').is_empty() {
            bail!("{} is always reached at {fixed}, so there is no address to enter", channel.label());
        }
        return Ok(fixed.to_string());
    }
    let host = parsed.host_str().unwrap_or("");
    if channel == Channel::Discord {
        let discord = host == "discord.com" || host == "discordapp.com" || host.ends_with(".discord.com");
        if !discord || !parsed.path().starts_with("/api/webhooks/") {
            bail!("That is not a Discord webhook address. In Discord: Channel settings → Integrations → Webhooks → Copy Webhook URL");
        }
        if parsed.path().trim_end_matches('/').split('/').count() < 5 {
            bail!("That Discord webhook address is missing its token — copy the whole URL");
        }
    }
    if channel == Channel::Slack {
        if host != "hooks.slack.com" || !parsed.path().starts_with("/services/") {
            bail!("That is not a Slack webhook address. In Slack: your app → Incoming Webhooks → Add New Webhook to Workspace, then copy the URL");
        }
        if parsed.path().trim_end_matches('/').split('/').count() < 5 {
            bail!("That Slack webhook address is missing its token — copy the whole URL");
        }
    }
    let trimmed = parsed.as_str().trim_end_matches('/').to_string();
    Ok(if trimmed.is_empty() { parsed.to_string() } else { trimmed })
}

/// A mail server, which is a host and a port and nothing else. `smtps://` is encrypted from the first
/// byte and is what a bare host name becomes; `smtp://` must upgrade with STARTTLS before anything is
/// said. There is no third option: finstats does not send mail, or a password, in the clear.
fn clean_mail_url(typed: &str) -> Result<String> {
    let lower = typed.to_ascii_lowercase();
    if lower.contains("://") && !lower.starts_with("smtp://") && !lower.starts_with("smtps://") {
        bail!("A mail server address starts with smtps:// or smtp://");
    }
    let with_scheme = if lower.contains("://") { typed.to_string() } else { format!("smtps://{typed}") };
    let parsed = reqwest::Url::parse(&with_scheme).map_err(|_| anyhow!("That doesn't look like a mail server address"))?;
    if !parsed.username().is_empty() || parsed.password().is_some() {
        bail!("Leave the user name and password out of the address — there is a field for each of them");
    }
    if !parsed.path().trim_matches('/').is_empty() || parsed.query().is_some() || parsed.fragment().is_some() {
        bail!("A mail server address is a host and a port, with nothing after it");
    }
    let host = parsed.host_str().filter(|h| !h.is_empty()).ok_or_else(|| anyhow!("The address needs a host name"))?;
    let scheme = parsed.scheme();
    Ok(match parsed.port() {
        Some(port) => format!("{scheme}://{host}:{port}"),
        None => format!("{scheme}://{host}"),
    })
}

/// The one field beside the address and the token, checked the way its own service writes it: an ntfy
/// topic goes in a URL, a Telegram chat is a number, a Pushover key is 30 characters, a mailbox is an
/// address. Refusing a typo here is what saves somebody reading a 400 out of a log.
fn clean_topic(channel: Channel, topic: &str) -> Result<String> {
    let topic = topic.trim();
    let what = channel.topic_label().unwrap_or("value").to_lowercase();
    if topic.is_empty() {
        bail!("Enter the {what}");
    }
    match channel {
        Channel::Ntfy => {
            if topic.len() > 64 || !topic.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                bail!("An ntfy topic may hold letters, digits, - and _ only");
            }
        }
        Channel::Telegram => {
            let ok = match topic.strip_prefix('@') {
                // A public channel by name: Telegram's own rule is 5 characters and up, letters and _.
                Some(name) => (5..=64).contains(&name.len()) && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
                None => {
                    let digits = topic.strip_prefix('-').unwrap_or(topic);
                    !digits.is_empty() && digits.len() <= 20 && digits.chars().all(|c| c.is_ascii_digit())
                }
            };
            if !ok {
                bail!("A chat id is a number like -1001234567890, or a public channel as @name");
            }
        }
        Channel::Pushover => {
            if topic.len() != 30 || !topic.chars().all(|c| c.is_ascii_alphanumeric()) {
                bail!("A Pushover user or group key is 30 letters and digits, from the front page of pushover.net");
            }
        }
        Channel::Email => return crate::mail::address(topic),
        _ => bail!("{} has nothing to enter here", channel.label()),
    }
    Ok(topic.to_string())
}

// ---------------------------------------------------------------- API

/// Anyone who may have a destination of their own at all. Administrators always may.
fn gate(user: &AuthUser) -> Result<(), ApiError> {
    if user.is_admin || user.perms.notify { Ok(()) } else { Err(ApiError::not_permitted("be sent notifications")) }
}

fn catalogue() -> Value {
    let events: Vec<Value> = Kind::tickable()
        .map(|k| {
            json!({ "key": k.key(), "label": k.label(), "what": k.what(), "severity": k.severity(),
                    "group": k.group().map(Group::key), "group_label": k.group().map(Group::label),
                    "personal": !matches!(k.need(), Need::Anyone) })
        })
        .collect();
    let channels: Vec<Value> = Channel::ALL
        .into_iter()
        .map(|c| {
            json!({ "key": c.key(), "label": c.label(), "what": c.what(), "example": c.example(), "fixed_url": c.fixed_url(),
                    "topic_label": c.topic_label(), "topic_help": c.topic_help(), "topic_example": c.topic_example(),
                    "needs_topic": c.needs_topic(), "secret_label": c.secret_label(), "secret_required": c.secret_required(),
                    "extras": c.extras().iter().map(|e| json!({ "key": e.key, "label": e.label, "help": e.help, "example": e.example, "required": e.required })).collect::<Vec<_>>() })
        })
        .collect();
    json!({ "events": events, "channels": channels, "severities": SEVERITIES, "groups": Group::ALL.map(|g| json!({ "key": g.key(), "label": g.label() })) })
}

/// One destination as the page sees it: never the address, never the secret.
fn target_row(r: &crate::db::rusqlite::Row) -> crate::db::rusqlite::Result<Option<Value>> {
    let Some(t) = read_target(r)? else { return Ok(None) };
    let (last_ok_at, last_error): (Option<i64>, Option<String>) = (r.get(14)?, r.get(15)?);
    let owner_name: Option<String> = r.get(16)?;
    Ok(Some(json!({
        "id": t.id, "kind": t.channel.key(), "label": t.channel.label(), "name": t.name, "shown": t.shown(),
        "scope": if t.owner_id.is_some() { "me" } else { "server" }, "owner_name": owner_name,
        "topic": t.topic, "options": t.options, "has_secret": !t.secret.is_empty(), "events": t.events, "with_addresses": t.with_addresses,
        "min_severity": t.min_severity, "accept_invalid_certs": t.accept_invalid_certs, "enabled": t.enabled,
        "created_at": t.created_at, "last_ok_at": last_ok_at, "last_error": last_error,
    })))
}

fn targets_json(conn: &Connection, only_owner: Option<&str>) -> Result<Vec<Value>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {}, t.last_ok_at, t.last_error, u.name FROM notify_targets t LEFT JOIN users u ON u.id = t.owner_id
         WHERE ?1 IS NULL OR t.owner_id = ?1 ORDER BY t.owner_id IS NOT NULL, t.id",
        TARGET_COLS.split(", ").map(|c| format!("t.{c}")).collect::<Vec<_>>().join(", ")
    ))?;
    let rows = stmt.query_map([only_owner], target_row)?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows.into_iter().flatten().collect())
}

pub async fn list(State(app): State<App>, user: AuthUser) -> ApiResult {
    gate(&user)?;
    let only = (!user.is_admin).then(|| user.id.clone());
    let targets = app.db.call(move |c| targets_json(c, only.as_deref())).await?;
    let public_url = app.settings().public_url.clone();
    Ok(Json(json!({
        "targets": targets, "catalogue": catalogue(), "public_url": public_url,
        "can_add_server": user.is_admin, "max_own": MAX_OWN_TARGETS,
    })))
}

// No `Debug`: it holds a secret on its way in.
#[derive(Deserialize)]
pub struct TargetBody {
    scope: Option<String>,
    kind: Option<String>,
    name: Option<String>,
    url: Option<String>,
    secret: Option<String>,
    topic: Option<String>,
    /// What this kind of destination needs beyond those: only mail has any.
    options: Option<BTreeMap<String, String>>,
    events: Option<Vec<String>>,
    with_addresses: Option<bool>,
    min_severity: Option<String>,
    accept_invalid_certs: Option<bool>,
    enabled: Option<bool>,
}

/// The destination a request describes: its fields where given, the stored ones otherwise, so that
/// changing which events a destination wants never means typing the address and the token again.
fn describe(body: TargetBody, stored: Option<&Target>, user: &AuthUser) -> std::result::Result<Target, ApiError> {
    let bad = |m: &str| ApiError::bad_request(m.to_string());
    let channel = match (stored, body.kind.as_deref()) {
        (Some(s), _) => s.channel, // a destination never changes its kind
        (None, Some(k)) => Channel::from_key(k).ok_or_else(|| bad(&format!("Unknown kind of destination `{k}`")))?,
        (None, None) => return Err(bad("Say which kind of destination this is")),
    };
    let owner_id = match stored {
        Some(s) => s.owner_id.clone(),
        None => match body.scope.as_deref() {
            Some("server") | None if user.is_admin => None,
            Some("server") => return Err(ApiError::forbidden()),
            _ => Some(user.id.clone()),
        },
    };
    let name = match (body.name.as_deref().map(str::trim).filter(|n| !n.is_empty()), stored) {
        (Some(n), _) => n.chars().take(80).collect::<String>(),
        (None, Some(s)) => s.name.clone(),
        (None, None) => channel.label().to_string(),
    };
    let url = match (body.url.as_deref().filter(|u| !u.trim().is_empty()), stored) {
        (Some(u), _) => clean_url(u, channel).map_err(|e| bad(&format!("{e}")))?,
        (None, Some(s)) => s.url.clone(),
        // A service finstats already knows the address of is never asked for one.
        (None, None) => channel.fixed_url().map(str::to_string).ok_or_else(|| bad(&format!("Enter the address of {}", channel.label())))?,
    };
    let secret = match (body.secret.filter(|s| !s.is_empty()), stored) {
        (Some(s), _) => s,
        (None, Some(s)) => s.secret.clone(),
        (None, None) => String::new(),
    };
    if secret.len() > 512 || secret.chars().any(char::is_control) {
        return Err(bad("That does not look like a token"));
    }
    if secret.is_empty() && channel.secret_required() {
        return Err(bad(&format!("Enter the {}", channel.secret_label().unwrap_or("token"))));
    }
    let topic = match (body.topic.as_deref().filter(|t| !t.trim().is_empty()), stored) {
        _ if !channel.needs_topic() => None,
        (Some(t), _) => Some(clean_topic(channel, t).map_err(|e| bad(&format!("{e}")))?),
        (None, Some(s)) => s.topic.clone(),
        (None, None) => return Err(bad(&format!("Enter the {}", channel.topic_label().unwrap_or("value").to_lowercase()))),
    };
    // Only the fields this kind of destination asked for are kept: anything else a request carries is
    // not something finstats would know what to do with, and is not stored.
    let given = body.options.unwrap_or_default();
    let mut options = BTreeMap::new();
    for extra in channel.extras() {
        let value = match (given.get(extra.key).map(|v| v.trim()).filter(|v| !v.is_empty()), stored) {
            (Some(v), _) => v.to_string(),
            (None, Some(s)) => s.option(extra.key).to_string(),
            (None, None) => String::new(),
        };
        if value.is_empty() {
            if extra.required {
                return Err(bad(&format!("Enter the {}", extra.label.to_lowercase())));
            }
            continue;
        }
        if value.len() > 200 || value.chars().any(char::is_control) {
            return Err(bad(&format!("The {} is too long, or has something in it that cannot be sent", extra.label.to_lowercase())));
        }
        if extra.address {
            crate::mail::address(&value).map_err(|e| bad(&format!("{e}")))?;
        }
        options.insert(extra.key.to_string(), value);
    }
    let events = match (body.events, stored) {
        (Some(list), _) => {
            if let Some(bad_key) = list.iter().find(|k| Kind::from_key(k).is_none_or(|k| k.group().is_none())) {
                return Err(bad(&format!("Unknown event `{bad_key}`")));
            }
            list
        }
        (None, Some(s)) => s.events.clone(),
        (None, None) => Kind::default_events(),
    };
    let min_severity = match (body.min_severity.as_deref(), stored) {
        (Some(s), _) if SEVERITIES.contains(&s) => s.to_string(),
        (Some(s), _) => return Err(bad(&format!("Unknown severity `{s}`"))),
        (None, Some(s)) => s.min_severity.clone(),
        (None, None) => INFO.to_string(),
    };
    let pick = |given: Option<bool>, was: Option<bool>, or: bool| given.or(was).unwrap_or(or);
    Ok(Target {
        id: stored.map(|s| s.id).unwrap_or(0),
        channel,
        name,
        url,
        secret,
        topic,
        options,
        owner_id,
        events,
        with_addresses: pick(body.with_addresses, stored.map(|s| s.with_addresses), false),
        min_severity,
        // Only an administrator may switch certificate checks off; a personal destination is public anyway.
        accept_invalid_certs: user.is_admin && pick(body.accept_invalid_certs, stored.map(|s| s.accept_invalid_certs), false),
        enabled: pick(body.enabled, stored.map(|s| s.enabled), true),
        created_at: stored.map(|s| s.created_at).unwrap_or_else(db::now),
    })
}

fn stored_target(conn: &Connection, id: i64) -> Result<Option<Target>> {
    Ok(conn
        .query_row(&format!("SELECT {TARGET_COLS} FROM notify_targets WHERE id = ?1"), [id], read_target)
        .optional()?
        .flatten())
}

/// A destination belongs to the person who added it; an administrator may touch any of them.
fn may_touch(user: &AuthUser, t: &Target) -> bool {
    user.is_admin || t.owner_id.as_deref() == Some(user.id.as_str())
}

pub async fn create(State(app): State<App>, user: AuthUser, Json(body): Json<TargetBody>) -> ApiResult {
    gate(&user)?;
    let next = describe(body, None, &user)?;
    if next.owner_id.is_some() {
        must_be_public(next.url()).await.map_err(|e| ApiError::bad_request(format!("{e}")))?;
    }
    let owner = next.owner_id.clone();
    let (total, own): (i64, i64) = app
        .db
        .call(move |c| {
            Ok(c.query_row("SELECT COUNT(*), COUNT(*) FILTER (WHERE owner_id IS ?1) FROM notify_targets", [&owner], |r| Ok((r.get(0)?, r.get(1)?)))?)
        })
        .await?;
    if total as usize >= MAX_TARGETS {
        return Err(ApiError::bad_request(format!("At most {MAX_TARGETS} destinations")));
    }
    if next.owner_id.is_some() && own as usize >= MAX_OWN_TARGETS {
        return Err(ApiError::bad_request(format!("At most {MAX_OWN_TARGETS} destinations of your own")));
    }
    tracing::info!("notification destination added: {} ({})", next.name, next.channel.label());
    let id = app
        .db
        .call(move |c| {
            c.execute(
                "INSERT INTO notify_targets(kind, name, url, secret, topic, owner_id, events, with_addresses, min_severity, accept_invalid_certs, enabled, created_at, options)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![next.channel.key(), next.name, next.url, next.secret, next.topic, next.owner_id,
                    serde_json::to_string(&next.events)?, next.with_addresses, next.min_severity, next.accept_invalid_certs, next.enabled, next.created_at,
                    serde_json::to_string(&next.options)?],
            )?;
            Ok(c.last_insert_rowid())
        })
        .await?;
    reload(&app).await?;
    one(&app, &user, id).await
}

pub async fn update(State(app): State<App>, user: AuthUser, Path(id): Path<i64>, Json(body): Json<TargetBody>) -> ApiResult {
    gate(&user)?;
    let stored = app.db.call(move |c| stored_target(c, id)).await?.ok_or_else(|| ApiError::not_found("Destination"))?;
    if !may_touch(&user, &stored) {
        return Err(ApiError::forbidden());
    }
    let next = describe(body, Some(&stored), &user)?;
    if next.owner_id.is_some() && next.url != stored.url {
        must_be_public(next.url()).await.map_err(|e| ApiError::bad_request(format!("{e}")))?;
    }
    app.db
        .call(move |c| {
            c.execute(
                "UPDATE notify_targets SET name = ?2, url = ?3, secret = ?4, topic = ?5, events = ?6, with_addresses = ?7,
                    min_severity = ?8, accept_invalid_certs = ?9, enabled = ?10, options = ?11 WHERE id = ?1",
                params![id, next.name, next.url, next.secret, next.topic, serde_json::to_string(&next.events)?,
                    next.with_addresses, next.min_severity, next.accept_invalid_certs, next.enabled, serde_json::to_string(&next.options)?],
            )?;
            Ok(())
        })
        .await?;
    reload(&app).await?;
    one(&app, &user, id).await
}

pub async fn remove(State(app): State<App>, user: AuthUser, Path(id): Path<i64>) -> ApiResult {
    gate(&user)?;
    let stored = app.db.call(move |c| stored_target(c, id)).await?.ok_or_else(|| ApiError::not_found("Destination"))?;
    if !may_touch(&user, &stored) {
        return Err(ApiError::forbidden());
    }
    app.db.call(move |c| Ok(c.execute("DELETE FROM notify_targets WHERE id = ?1", [id])?)).await?;
    reload(&app).await?;
    Ok(Json(json!({ "ok": true })))
}

/// The Test button: one message to one destination, now, and the answer is what the other side said.
/// It goes down the same path as everything else, so a test that arrives proves the real thing works.
pub async fn test(State(app): State<App>, user: AuthUser, Path(id): Path<i64>) -> ApiResult {
    gate(&user)?;
    let stored = app.db.call(move |c| stored_target(c, id)).await?.ok_or_else(|| ApiError::not_found("Destination"))?;
    if !may_touch(&user, &stored) {
        return Err(ApiError::forbidden());
    }
    let now = db::now();
    let event = Stored {
        id: 0,
        kind: Kind::Test,
        severity: INFO.to_string(),
        at: now,
        user_name: Some(user.name.clone()),
        title: "finstats is connected".to_string(),
        body: format!("A test message from finstats, sent by {}. If you are reading this, this destination works.", user.name),
        link: Some("/settings".to_string()),
        data: vec![("Destination".to_string(), stored.name.clone())],
        private: vec![],
    };
    let msg = message(&event, stored.with_addresses, Some(&app.settings().public_url));
    let outcome = channels::send(&app, &stored, &msg).await;
    let now = db::now();
    let (ok, error) = match &outcome {
        Ok(()) => (true, None),
        Err(e) => (false, Some(e.message.clone())),
    };
    let error_for_db = error.clone();
    app.db
        .call(move |c| {
            Ok(match &error_for_db {
                None => c.execute("UPDATE notify_targets SET last_ok_at = ?2, last_error = NULL WHERE id = ?1", params![id, now])?,
                Some(e) => c.execute("UPDATE notify_targets SET last_error = ?2 WHERE id = ?1", params![id, e])?,
            })
        })
        .await?;
    Ok(Json(json!({ "ok": ok, "error": error })))
}

async fn one(app: &App, user: &AuthUser, id: i64) -> ApiResult {
    let only = (!user.is_admin).then(|| user.id.clone());
    let list = app.db.call(move |c| targets_json(c, only.as_deref())).await?;
    let found = list.into_iter().find(|t| t["id"].as_i64() == Some(id)).ok_or_else(|| ApiError::not_found("Destination"))?;
    Ok(Json(json!({ "target": found })))
}

#[derive(Deserialize)]
pub struct HistoryQuery {
    limit: Option<usize>,
}

/// What has been sent lately, and how it went. Somebody who only has destinations of their own sees only
/// what went to them; the events themselves are already on the pages they belong to.
pub async fn history(State(app): State<App>, user: AuthUser, Query(q): Query<HistoryQuery>) -> ApiResult {
    gate(&user)?;
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let only = (!user.is_admin).then(|| user.id.clone());
    let rows = app
        .db
        .call(move |c| {
            let mut stmt = c.prepare(
                "SELECT e.id, e.kind, e.severity, e.at, e.user_name, e.title, e.body, e.historic,
                        d.target_id, t.name, t.kind, d.state, d.attempts, d.sent_at, d.error
                 FROM notify_events e
                 LEFT JOIN notify_deliveries d ON d.event_id = e.id
                 LEFT JOIN notify_targets t ON t.id = d.target_id
                 WHERE ?1 IS NULL OR t.owner_id = ?1
                 ORDER BY e.at DESC, e.id DESC, d.target_id LIMIT ?2",
            )?;
            let rows: Vec<Value> = stmt
                .query_map(params![only, (limit * 4) as i64], |r| {
                    let kind: String = r.get(1)?;
                    let target: Option<i64> = r.get(8)?;
                    Ok(json!({
                        "id": r.get::<_, i64>(0)?, "kind": kind, "label": Kind::from_key(&kind).map(Kind::label),
                        "severity": r.get::<_, String>(2)?, "at": r.get::<_, i64>(3)?, "user_name": r.get::<_, Option<String>>(4)?,
                        "title": r.get::<_, String>(5)?, "body": r.get::<_, String>(6)?, "historic": r.get::<_, bool>(7)?,
                        "delivery": target.map(|id| json!({ "target_id": id, "target": r.get::<_, Option<String>>(9).ok(),
                            "channel": r.get::<_, Option<String>>(10).ok(), "state": r.get::<_, Option<String>>(11).ok(),
                            "attempts": r.get::<_, Option<i64>>(12).ok(), "sent_at": r.get::<_, Option<i64>>(13).ok(),
                            "error": r.get::<_, Option<String>>(14).ok() })),
                    }))
                })?
                .collect::<Result<_, _>>()?;
            Ok(rows)
        })
        .await?;
    // One entry per event, with every delivery of it underneath.
    let mut events: Vec<Value> = vec![];
    for row in rows {
        let id = row["id"].clone();
        let delivery = row["delivery"].clone();
        match events.last_mut().filter(|e| e["id"] == id) {
            Some(existing) => {
                if let (Some(list), false) = (existing["deliveries"].as_array_mut(), delivery.is_null()) {
                    list.push(delivery);
                }
            }
            None => {
                let mut event = row.clone();
                let map = event.as_object_mut().expect("built as an object");
                map.remove("delivery");
                map.insert("deliveries".into(), if delivery.is_null() { json!([]) } else { json!([delivery]) });
                events.push(event);
            }
        }
        if events.len() >= limit {
            break;
        }
    }
    Ok(Json(json!({ "events": events })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::rusqlite::Connection;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        for m in crate::db::MIGRATIONS {
            c.execute_batch(m).unwrap();
        }
        c.execute_batch(
            "INSERT INTO users(id, name, is_admin, updated_at) VALUES ('ua', 'alice', 1, 1), ('ub', 'bob', 0, 1), ('uc', 'carol', 0, 1);
             INSERT INTO user_permissions(user_id, permissions, updated_at) VALUES ('ub', '[\"sign_in\",\"notify\"]', 1), ('uc', '[\"sign_in\",\"notify\",\"see_everyone\",\"see_network\",\"see_server\"]', 1);",
        )
        .unwrap();
        c
    }

    fn target(id: i64, owner: Option<&str>, events: &[Kind]) -> Target {
        Target {
            id,
            channel: Channel::Webhook,
            name: format!("Destination {id}"),
            url: "https://example.com/hook".into(),
            secret: "s3cret-token".into(),
            topic: None,
            options: BTreeMap::new(),
            owner_id: owner.map(str::to_string),
            events: events.iter().map(|k| k.key().to_string()).collect(),
            with_addresses: false,
            min_severity: INFO.into(),
            accept_invalid_certs: false,
            enabled: true,
            created_at: 0,
        }
    }

    fn stored(kind: Kind) -> Stored {
        Stored {
            id: 1,
            kind,
            severity: kind.severity().into(),
            at: 1_000,
            user_name: Some("alice".into()),
            title: "Impossible travel: alice".into(),
            body: "Oslo, Norway and London, United Kingdom, 1160 km apart, 40 minutes apart.".into(),
            link: Some("/security".into()),
            data: vec![("Person".into(), "alice".into()), ("From".into(), "Oslo, Norway — signed in".into())],
            private: vec![("From address".into(), "203.0.113.9".into()), ("To address".into(), "198.51.100.4".into())],
        }
    }

    /// Anything that looks like an address, wherever it ended up in the message.
    fn names_an_address(text: &str) -> bool {
        text.split(|c: char| !(c.is_ascii_digit() || c == '.' || c == ':'))
            .any(|word| crate::network::canonical(word).is_some())
    }

    #[test]
    fn no_address_leaves_a_destination_that_did_not_ask_for_one() {
        for kind in Kind::ALL {
            let e = stored(kind);
            let m = message(&e, false, Some("https://finstats.example"));
            let all = format!("{} {} {}", m.title, m.text(), m.fields.iter().map(|(a, b)| format!("{a} {b}")).collect::<Vec<_>>().join(" "));
            assert!(!names_an_address(&all), "{}: an address reached a destination that did not ask for one — {all}", kind.key());
            assert_eq!(m.fields.len(), 2, "{}: only the public fields", kind.key());
        }
        let m = message(&stored(Kind::Travel), true, Some("https://finstats.example"));
        assert!(names_an_address(&m.text()), "with the switch on, the addresses are the point");
        assert_eq!(m.link.as_deref(), Some("https://finstats.example/security"));
        assert_eq!(message(&stored(Kind::Travel), true, Some("")).link, None, "no address for finstats, no link");
    }

    #[test]
    fn a_personal_destination_carries_exactly_what_its_owner_may_see() {
        let own = Some("ub");
        let plain = Perms::from_keys(["sign_in"]);
        let everyone = Perms::from_keys(["sign_in", "see_everyone"]);
        let network = Perms::from_keys(["sign_in", "see_everyone", "see_network"]);
        let server = Perms::from_keys(["sign_in", "see_server"]);
        let all = Kind::tickable().collect::<Vec<_>>();
        let t = target(1, own, &all);
        let may = |kind: Kind, about: Option<&str>, perms: Perms| wanted_by(&t, kind, kind.severity(), 10, about, Some(perms));

        assert!(may(Kind::PlayStarted, Some("ub"), plain), "their own play, always");
        assert!(!may(Kind::PlayStarted, Some("uc"), plain), "somebody else's play needs see_everyone");
        assert!(may(Kind::PlayStarted, Some("uc"), everyone));
        assert!(may(Kind::Travel, Some("ub"), plain), "an alert about yourself is yours");
        assert!(!may(Kind::Travel, Some("uc"), everyone), "somebody else's places need see_network as well");
        assert!(may(Kind::Travel, Some("uc"), network));
        assert!(!may(Kind::TaskFailed, None, everyone), "the server's business needs see_server");
        assert!(may(Kind::TaskFailed, None, server));
        assert!(may(Kind::NewItems, None, plain), "the library is the same for everyone");
        assert!(!wanted_by(&t, Kind::NewItems, INFO, 10, None, None), "somebody who may not sign in is told nothing");

        let server_own = target(2, None, &all);
        assert!(wanted_by(&server_own, Kind::Travel, ALERT, 10, Some("uc"), None), "the server's own destination is not filtered");
    }

    #[test]
    fn a_destination_hears_only_what_it_asked_for() {
        let t = target(1, None, &[Kind::NewItems]);
        assert!(wanted_by(&t, Kind::NewItems, INFO, 10, None, None));
        assert!(!wanted_by(&t, Kind::PlayStarted, INFO, 10, Some("ua"), None), "an unticked kind is never sent");

        let mut loud = target(2, None, &[Kind::NewItems, Kind::Travel]);
        loud.min_severity = WARN.into();
        assert!(!wanted_by(&loud, Kind::NewItems, INFO, 10, None, None), "below the severity it asked for");
        assert!(wanted_by(&loud, Kind::Travel, ALERT, 10, Some("ua"), None));

        let mut off = target(3, None, &[Kind::NewItems]);
        off.enabled = false;
        assert!(!wanted_by(&off, Kind::NewItems, INFO, 10, None, None));

        let mut fresh = target(4, None, &[Kind::NewItems]);
        fresh.created_at = 5_000;
        assert!(!wanted_by(&fresh, Kind::NewItems, INFO, 4_999, None, None), "nothing that happened before it existed");
        assert!(wanted_by(&fresh, Kind::NewItems, INFO, 5_000, None, None));
    }

    /// The destinations as they really are: rows in the database, which is what a delivery points at.
    fn bus(c: &Connection, targets: Vec<Target>) -> Fanout {
        for t in &targets {
            c.execute(
                "INSERT INTO notify_targets(id, kind, name, url, secret, topic, owner_id, events, min_severity, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![t.id, t.channel.key(), t.name, t.url, t.secret, t.topic, t.owner_id, serde_json::to_string(&t.events).unwrap(), t.min_severity, t.created_at],
            )
            .unwrap();
        }
        Fanout::build(c, Arc::new(targets.into_iter().map(Arc::new).collect()), &Settings::default()).unwrap()
    }

    #[test]
    fn an_event_is_written_once_and_queued_once_per_destination() {
        let c = conn();
        let f = bus(&c, vec![target(1, None, &[Kind::NewItems]), target(2, None, &[Kind::PlayStarted])]);
        let event = || Event::new(Kind::NewItems, "notify:new_items:x:1", "New in the library: Big Buck Bunny", "Big Buck Bunny");

        assert!(raise_in(&c, &f, &event()).unwrap(), "the first time is news");
        assert!(!raise_in(&c, &f, &event()).unwrap(), "deriving the same thing again is not");
        let events: i64 = c.query_row("SELECT COUNT(*) FROM notify_events", [], |r| r.get(0)).unwrap();
        let queued: i64 = c.query_row("SELECT COUNT(*) FROM notify_deliveries WHERE state = 'queued'", [], |r| r.get(0)).unwrap();
        assert_eq!((events, queued), (1, 1), "one event, and only the destination that asked for it");
    }

    #[test]
    fn nothing_is_written_down_that_nothing_is_listening_for() {
        let c = conn();
        let f = bus(&c, vec![target(1, None, &[Kind::NewItems])]);
        let play = Event::new(Kind::PlayStarted, "notify:play:start:7", "alice started watching", "Big Buck Bunny").about("ua", "alice");
        assert!(!raise_in(&c, &f, &play).unwrap(), "an install where nobody asked for plays must not write a row for every one");
        eq_rows(&c, 0);
        // The same event, once somebody is listening for it.
        let f = bus(&c, vec![target(2, None, &[Kind::PlayStarted])]);
        assert!(raise_in(&c, &f, &play).unwrap());
        eq_rows(&c, 1);
    }

    fn eq_rows(c: &Connection, want: i64) {
        let n: i64 = c.query_row("SELECT COUNT(*) FROM notify_events", [], |r| r.get(0)).unwrap();
        assert_eq!(n, want, "{want} event row(s) expected");
    }

    #[test]
    fn something_found_long_afterwards_is_recorded_and_nobody_is_told() {
        let c = conn();
        let f = bus(&c, vec![target(1, None, &[Kind::RequestAvailable])]);
        let old = Event::new(Kind::RequestAvailable, "notify:request:1:5", "Ready to watch: Winterline", "It is here")
            .at(db::now() - HISTORIC_S - 60);
        assert!(raise_in(&c, &f, &old).unwrap());
        let historic: bool = c.query_row("SELECT historic FROM notify_events", [], |r| r.get(0)).unwrap();
        let queued: i64 = c.query_row("SELECT COUNT(*) FROM notify_deliveries", [], |r| r.get(0)).unwrap();
        assert!(historic && queued == 0, "history is written down, not announced");
    }

    #[test]
    fn a_destination_that_will_not_take_it_is_given_up_on_rather_than_tried_for_ever() {
        let waits: Vec<Option<i64>> = (1..=5).map(backoff).collect();
        assert_eq!(waits, vec![Some(30), Some(120), Some(600), Some(3_600), None]);
        assert!(waits.iter().flatten().sum::<i64>() > 4_000, "long enough for anything that is coming back");
    }

    #[test]
    fn a_busy_evening_cannot_rate_limit_a_destination_into_silence() {
        let mut used = HashMap::new();
        let now = 1_000_000;
        for n in 0..PER_MINUTE {
            assert!(!over_the_minute(&mut used, 1, now), "message {n} of the minute goes out");
        }
        assert!(over_the_minute(&mut used, 1, now), "the next one waits");
        assert!(!over_the_minute(&mut used, 2, now), "another destination has its own minute");
        assert!(!over_the_minute(&mut used, 1, now + 60), "and the next minute starts again");
    }

    #[test]
    fn what_the_page_is_told_about_a_destination_never_includes_its_address_or_its_token() {
        let c = conn();
        c.execute(
            "INSERT INTO notify_targets(id, kind, name, url, secret, topic, owner_id, events, min_severity, created_at)
             VALUES (7, 'discord', 'Household', 'https://discord.com/api/webhooks/123/s3cret-token', 'tok3n', NULL, NULL, '[\"new_items\"]', 'info', 1)",
            [],
        )
        .unwrap();
        let rows = targets_json(&c, None).unwrap();
        let text = serde_json::to_string(&rows).unwrap();
        assert!(!text.contains("s3cret-token") && !text.contains("tok3n"), "a webhook URL is itself a credential: {text}");
        assert!(text.contains("discord.com"), "the host is shown, so the owner knows which one this is");
        assert_eq!(rows[0]["has_secret"], true);
        assert_eq!(rows[0]["scope"], "server");
        assert!(targets_json(&c, Some("ub")).unwrap().is_empty(), "somebody else's destination is not theirs to see");
    }

    /// A destination as an API request describes one.
    fn body(value: Value) -> TargetBody {
        serde_json::from_value(value).expect("a destination body")
    }

    fn admin() -> AuthUser {
        AuthUser { id: "ua".into(), name: "alice".into(), is_admin: true, perms: Perms::ALL }
    }

    #[test]
    fn the_field_beside_the_address_is_checked_the_way_its_own_service_writes_it() {
        assert!(clean_topic(Channel::Ntfy, "finstats-abc_1").is_ok());
        assert!(clean_topic(Channel::Ntfy, "with/a/slash").is_err(), "an ntfy topic goes in a URL");
        assert!(clean_topic(Channel::Telegram, "-1001234567890").is_ok(), "a group chat id is negative");
        assert!(clean_topic(Channel::Telegram, "@finstats_news").is_ok(), "a channel may be given by name");
        assert!(clean_topic(Channel::Telegram, "my chat").is_err());
        assert!(clean_topic(Channel::Pushover, "uQiRzpo4DXghDmr9QzzfQu27cmVRsG").is_ok());
        assert!(clean_topic(Channel::Pushover, "not a key").is_err());
        assert!(clean_topic(Channel::Email, "me@example.com").is_ok());
        assert!(clean_topic(Channel::Email, "me at example.com").is_err());
        assert!(clean_topic(Channel::Email, "me@example").is_err(), "a mailbox needs a domain that resolves");
    }

    #[test]
    fn a_letter_needs_a_sender_and_keeps_only_what_its_own_channel_asked_for() {
        let user = admin();
        let full = json!({ "kind": "email", "url": "smtps://smtp.example.com", "topic": "me@example.com",
                           "options": { "from": "finstats@example.com", "username": "finstats", "bcc": "someone@example.com" } });
        let t = describe(body(full), None, &user).ok().expect("a destination that says everything");
        assert_eq!(t.option("from"), "finstats@example.com");
        assert_eq!(t.option("username"), "finstats");
        assert_eq!(t.option("bcc"), "", "a field this kind of destination never asked for is not stored");

        let no_sender = json!({ "kind": "email", "url": "smtps://smtp.example.com", "topic": "me@example.com" });
        assert!(describe(body(no_sender), None, &user).is_err(), "there is nothing to send a letter as");
        let bad_sender = json!({ "kind": "email", "url": "smtps://smtp.example.com", "topic": "me@example.com", "options": { "from": "finstats" } });
        assert!(describe(body(bad_sender), None, &user).is_err());
        // A channel whose address is its service's own is filled in rather than asked for.
        let telegram = json!({ "kind": "telegram", "secret": "123:abc", "topic": "-1001234567890" });
        assert_eq!(describe(body(telegram), None, &user).ok().expect("a telegram destination").url(), "https://api.telegram.org");
    }

    #[test]
    fn what_a_destination_is_called_in_the_list_says_which_one_it_is_and_never_its_token() {
        let mut t = target(1, None, &[]);
        t.channel = Channel::Telegram;
        t.url = "https://api.telegram.org".into();
        t.secret = "123456:AAH-bot-token".into();
        t.topic = Some("-1001234567890".into());
        assert_eq!(t.shown(), "api.telegram.org/-1001234567890", "which chat, never the bot token that reaches it");

        t.channel = Channel::Email;
        t.url = "smtps://smtp.example.com:465".into();
        t.topic = Some("me@example.com".into());
        assert_eq!(t.shown(), "me@example.com via smtp.example.com:465");

        t.channel = Channel::Pushover;
        t.url = "https://api.pushover.net".into();
        t.topic = Some("uQiRzpo4DXghDmr9QzzfQu27cmVRsG".into());
        assert_eq!(t.shown(), "api.pushover.net/…", "a Pushover user key is half of what it takes to send");
    }

    #[test]
    fn an_address_that_could_aim_the_request_elsewhere_is_refused() {
        assert!(clean_url("ftp://example.com/x", Channel::Webhook).is_err());
        assert!(clean_url("https://user:pass@example.com/x", Channel::Webhook).is_err());
        assert!(clean_url("https://example.com/x#y", Channel::Webhook).is_err());
        assert!(clean_url("https://ntfy.sh?token=abc", Channel::Ntfy).is_err(), "only a plain webhook may carry a query");
        assert_eq!(clean_url("https://example.com/hooks/x?token=abc", Channel::Webhook).unwrap(), "https://example.com/hooks/x?token=abc");
        assert_eq!(clean_url("ntfy.sh", Channel::Ntfy).unwrap(), "https://ntfy.sh", "a missing scheme is https, never something else");
        assert!(clean_url("https://example.com/api/webhooks/1/tok", Channel::Discord).is_err(), "that is not Discord");
        assert!(clean_url("https://discord.com/api/webhooks/123", Channel::Discord).is_err(), "half a Discord webhook is no webhook");
        assert!(clean_url("https://discord.com/api/webhooks/123/abc", Channel::Discord).is_ok());
        assert!(clean_url("https://example.com/services/T0/B0/tok", Channel::Slack).is_err(), "that is not Slack");
        assert!(clean_url("https://hooks.slack.com/services/T0", Channel::Slack).is_err(), "half a Slack webhook is no webhook");
        assert!(clean_url("https://hooks.slack.com/services/T0/B0/tok", Channel::Slack).is_ok());
        // A channel whose address is its service's own takes that one and nothing else.
        assert_eq!(clean_url("api.telegram.org", Channel::Telegram).unwrap(), "https://api.telegram.org");
        assert!(clean_url("https://telegram.example.com", Channel::Telegram).is_err(), "a token is not for somebody else's server");
        assert!(clean_url("http://api.pushover.net", Channel::Pushover).is_err(), "and never in the clear");
        // Mail is not HTTP at all, and finstats will not send it in the clear.
        assert_eq!(clean_url("smtps://smtp.example.com:465", Channel::Email).unwrap(), "smtps://smtp.example.com:465");
        assert_eq!(clean_url("smtp.example.com", Channel::Email).unwrap(), "smtps://smtp.example.com", "a mail address with no scheme is the encrypted one");
        assert!(clean_url("https://smtp.example.com", Channel::Email).is_err());
        assert!(clean_url("smtps://user:pw@smtp.example.com", Channel::Email).is_err());
    }
}
