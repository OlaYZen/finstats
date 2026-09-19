//! Read-only statistics endpoints. Everything here goes through [`Scope`], which is where
//! the time window, user/library filters and the "non-admins only see themselves" rule live.

use anyhow::Result;
use axum::Json;
use axum::extract::{Path, Query, State};
use chrono::{Datelike, NaiveDate};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::auth::{AuthUser, Manager, ServerViewer};
use crate::db::rusqlite::types::ValueRef;
use crate::db::rusqlite::{Connection, OptionalExtension, Row, params_from_iter};
use crate::db::{self, SqlValue};
use crate::media;
use crate::state::{ApiError, ApiResult, App};

const BOOL_COLS: [&str; 11] = ["active", "is_admin", "is_disabled", "removed", "has_image", "item_exists", "has_backdrop", "is_local", "is_favorite", "is_actor", "is_director"];
const JSON_COLS: [&str; 4] = ["genres", "transcode", "studios", "provider_ids"];

// ---------------------------------------------------------------- plumbing

pub fn row_json(row: &Row) -> Map<String, Value> {
    let stmt = row.as_ref();
    let mut out = Map::new();
    for i in 0..stmt.column_count() {
        let name = stmt.column_name(i).unwrap_or("?").to_string();
        let v = match row.get_ref(i).unwrap_or(ValueRef::Null) {
            ValueRef::Null => Value::Null,
            ValueRef::Integer(n) if BOOL_COLS.contains(&name.as_str()) => Value::Bool(n != 0),
            ValueRef::Integer(n) => json!(n),
            ValueRef::Real(f) => json!(f),
            ValueRef::Text(t) => {
                let s = String::from_utf8_lossy(t);
                if JSON_COLS.contains(&name.as_str()) { serde_json::from_str(&s).unwrap_or(Value::Null) } else { Value::String(s.into_owned()) }
            }
            ValueRef::Blob(_) => Value::Null,
        };
        out.insert(name, v);
    }
    out
}

pub(crate) fn rows_json(conn: &Connection, sql: &str, args: &[SqlValue]) -> Result<Vec<Map<String, Value>>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params_from_iter(args.iter()), |r| Ok(row_json(r)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub(crate) fn one_json(conn: &Connection, sql: &str, args: &[SqlValue]) -> Result<Option<Map<String, Value>>> {
    Ok(conn.query_row(sql, params_from_iter(args.iter()), |r| Ok(row_json(r))).optional()?)
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct FilterQuery {
    pub days: Option<i64>,
    pub user_id: Option<String>,
    pub library_id: Option<String>,
}

/// A WHERE clause under construction over `playbacks p`.
#[derive(Clone, Default)]
pub struct Cond {
    clauses: Vec<String>,
    args: Vec<SqlValue>,
}

impl Cond {
    pub fn add(&mut self, clause: &str, v: impl Into<SqlValue>) -> &mut Self {
        self.clauses.push(clause.to_string());
        self.args.push(v.into());
        self
    }
    pub fn raw(&mut self, clause: &str) -> &mut Self {
        self.clauses.push(clause.to_string());
        self
    }
    pub fn with(&self, clause: &str, v: impl Into<SqlValue>) -> Self {
        let mut c = self.clone();
        c.add(clause, v);
        c
    }
    pub fn with_raw(&self, clause: &str) -> Self {
        let mut c = self.clone();
        c.raw(clause);
        c
    }
    pub fn sql(&self) -> String {
        if self.clauses.is_empty() { String::new() } else { format!("WHERE {}", self.clauses.join(" AND ")) }
    }
}

/// The resolved filter for one request.
#[derive(Clone)]
pub struct Scope {
    pub days: i64,
    /// Start of the window (local midnight `days-1` days ago); `None` = all time.
    pub since: Option<i64>,
    pub user_id: Option<String>,
    pub library_id: Option<String>,
    pub min_play_s: i64,
    /// What the caller may see; decides which fields survive and whose plays are counted.
    pub perms: crate::auth::Perms,
}

impl Scope {
    pub fn new(app: &App, user: &AuthUser, q: &FilterQuery) -> Self {
        let clean = |s: &Option<String>| s.as_deref().map(db::norm_id).filter(|s| !s.is_empty());
        Scope {
            days: q.days.unwrap_or(0).clamp(0, 36_500),
            since: None,
            // Without "see everyone" a request is pinned to the caller, whatever the URL asks for.
            user_id: if user.perms.see_everyone { clean(&q.user_id) } else { Some(user.id.clone()) },
            library_id: clean(&q.library_id),
            min_play_s: app.settings().min_play_s,
            perms: user.perms,
        }
    }

    /// "Last 7 days" means today plus the six full local days before it, so the chart's
    /// buckets and the totals always describe exactly the same plays.
    pub fn resolve(mut self, conn: &Connection) -> Result<Self> {
        if self.days > 0 {
            let since: i64 = conn.query_row(
                "SELECT CAST(strftime('%s', date('now', 'localtime', ?1), 'utc') AS INTEGER)",
                [format!("-{} days", self.days - 1)],
                |r| r.get(0),
            )?;
            self.since = Some(since);
        }
        Ok(self)
    }

    fn base(&self) -> Cond {
        let mut c = Cond::default();
        if let Some(u) = &self.user_id {
            c.add("p.user_id = ?", u.clone());
        }
        if let Some(l) = &self.library_id {
            c.add("p.library_id = ?", l.clone());
        }
        if self.min_play_s > 0 {
            c.add("p.duration_s >= ?", self.min_play_s);
        }
        c
    }

    pub fn cond(&self) -> Cond {
        let mut c = self.base();
        if let Some(s) = self.since {
            c.add("p.started_at >= ?", s);
        }
        c
    }

    /// The window of equal length right before the current one.
    fn previous_cond(&self) -> Option<Cond> {
        let since = self.since?;
        let mut c = self.base();
        c.add("p.started_at >= ?", since - self.days * 86_400);
        c.add("p.started_at < ?", since);
        Some(c)
    }
}

async fn scoped<T, F>(app: &App, user: &AuthUser, q: &FilterQuery, f: F) -> ApiResult<T>
where
    T: Send + 'static,
    F: FnOnce(&Connection, &Scope) -> Result<T> + Send + 'static,
{
    let scope = Scope::new(app, user, q);
    Ok(app
        .db
        .call(move |c| {
            let scope = scope.resolve(c)?;
            f(c, &scope)
        })
        .await?)
}

// ---------------------------------------------------------------- building blocks

fn totals(conn: &Connection, cond: &Cond) -> Result<Value> {
    let sql = format!(
        "SELECT COUNT(*) AS plays, COALESCE(SUM(p.duration_s), 0) AS watch_s,
                COUNT(DISTINCT p.user_id) AS active_users, COUNT(DISTINCT p.item_id) AS distinct_items
         FROM playbacks p {}",
        cond.sql()
    );
    Ok(Value::Object(one_json(conn, &sql, &cond.args)?.unwrap_or_default()))
}

const TYPE_GROUPS: [&str; 4] = ["Movie", "Episode", "Audio", "Other"];

/// Gap-free time series. Day buckets, or ISO weeks once the span passes 120 days.
fn daily(conn: &Connection, scope: &Scope, cond: &Cond) -> Result<(Vec<Value>, &'static str)> {
    let today: String = conn.query_row("SELECT date('now', 'localtime')", [], |r| r.get(0))?;
    let today = NaiveDate::parse_from_str(&today, "%Y-%m-%d")?;
    let first: Option<String> = match scope.since {
        Some(s) => conn.query_row("SELECT date(?1, 'unixepoch', 'localtime')", [s], |r| r.get(0))?,
        None => conn.query_row(
            &format!("SELECT date(MIN(p.started_at), 'unixepoch', 'localtime') FROM playbacks p {}", cond.sql()),
            params_from_iter(cond.args.iter()),
            |r| r.get(0),
        )?,
    };
    let Some(first) = first.and_then(|f| NaiveDate::parse_from_str(&f, "%Y-%m-%d").ok()) else {
        return Ok((vec![], "day"));
    };
    let weekly = (today - first).num_days() > 120;
    let (bucket_sql, step, name) = if weekly {
        ("date(p.started_at, 'unixepoch', 'localtime', 'weekday 0', '-6 days')", 7, "week")
    } else {
        ("date(p.started_at, 'unixepoch', 'localtime')", 1, "day")
    };
    let sql = format!(
        "SELECT {bucket_sql} AS d,
                CASE p.item_type WHEN 'Movie' THEN 'Movie' WHEN 'Episode' THEN 'Episode' WHEN 'Audio' THEN 'Audio' ELSE 'Other' END AS g,
                COUNT(*), COALESCE(SUM(p.duration_s), 0)
         FROM playbacks p {} GROUP BY d, g",
        cond.sql()
    );
    let mut found: std::collections::HashMap<(String, String), (i64, i64)> = Default::default();
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(cond.args.iter()))?;
    while let Some(r) = rows.next()? {
        found.insert((r.get(0)?, r.get(1)?), (r.get(2)?, r.get(3)?));
    }

    let mut cursor = if weekly { first - chrono::Duration::days(first.weekday().num_days_from_monday() as i64) } else { first };
    let mut out = vec![];
    while cursor <= today {
        let date = cursor.format("%Y-%m-%d").to_string();
        let mut by_type = Map::new();
        let (mut plays, mut watch) = (0, 0);
        for g in TYPE_GROUPS {
            let (p, w) = found.get(&(date.clone(), g.to_string())).copied().unwrap_or((0, 0));
            plays += p;
            watch += w;
            by_type.insert(g.to_string(), json!([p, w]));
        }
        out.push(json!({ "date": date, "plays": plays, "watch_s": watch, "by_type": by_type }));
        cursor += chrono::Duration::days(step);
    }
    Ok((out, name))
}

fn heatmap(conn: &Connection, cond: &Cond) -> Result<Value> {
    let sql = format!(
        "SELECT CAST(strftime('%w', p.started_at, 'unixepoch', 'localtime') AS INTEGER),
                CAST(strftime('%H', p.started_at, 'unixepoch', 'localtime') AS INTEGER),
                COUNT(*), COALESCE(SUM(p.duration_s), 0)
         FROM playbacks p {} GROUP BY 1, 2",
        cond.sql()
    );
    let mut plays = vec![vec![0i64; 24]; 7];
    let mut watch = vec![vec![0i64; 24]; 7];
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(cond.args.iter()))?;
    while let Some(r) = rows.next()? {
        let (dow, hour): (i64, i64) = (r.get(0)?, r.get(1)?);
        let day = ((dow + 6) % 7) as usize; // SQLite: 0 = Sunday. Ours: 0 = Monday.
        plays[day][hour.clamp(0, 23) as usize] = r.get(2)?;
        watch[day][hour.clamp(0, 23) as usize] = r.get(3)?;
    }
    Ok(json!({ "plays": plays, "watch_s": watch }))
}

/// `expr` is grouped on; rows beyond `max` are folded into "Other".
fn buckets(conn: &Connection, cond: &Cond, expr: &str, from_extra: &str, max: usize) -> Result<Vec<Value>> {
    let sql = format!(
        "SELECT {expr} AS name, COUNT(*) AS plays, COALESCE(SUM(p.duration_s), 0) AS watch_s
         FROM playbacks p {from_extra} {} GROUP BY 1 HAVING name IS NOT NULL ORDER BY plays DESC, watch_s DESC",
        cond.sql()
    );
    let rows = rows_json(conn, &sql, &cond.args)?;
    let mut out: Vec<Value> = vec![];
    let (mut other_p, mut other_w) = (0i64, 0i64);
    for (i, r) in rows.into_iter().enumerate() {
        if i < max {
            out.push(Value::Object(r));
        } else {
            other_p += r["plays"].as_i64().unwrap_or(0);
            other_w += r["watch_s"].as_i64().unwrap_or(0);
        }
    }
    if other_p > 0 {
        out.push(json!({ "name": "Other", "plays": other_p, "watch_s": other_w }));
    }
    Ok(out)
}

const RESOLUTION_SQL: &str = "CASE
    WHEN p.width IS NULL AND p.height IS NULL THEN NULL
    WHEN p.width >= 3800 OR p.height >= 2000 THEN '4K'
    WHEN p.width >= 2500 OR p.height >= 1400 THEN '1440p'
    WHEN p.width >= 1900 OR p.height >= 1000 THEN '1080p'
    WHEN p.width >= 1260 OR p.height >= 700 THEN '720p'
    WHEN p.width >= 1000 OR p.height >= 560 THEN '576p'
    WHEN p.width >= 700 OR p.height >= 400 THEN '480p'
    ELSE 'SD' END";

const CHANNELS_SQL: &str = "CASE p.audio_channels WHEN 0 THEN NULL WHEN 1 THEN 'Mono' WHEN 2 THEN 'Stereo'
    WHEN 6 THEN '5.1' WHEN 8 THEN '7.1' ELSE p.audio_channels || ' ch' END";

fn top(conn: &Connection, cond: &Cond, kind: &str, limit: i64, by_plays: bool) -> Result<Vec<Value>> {
    let order = if by_plays { "plays DESC, watch_s DESC" } else { "watch_s DESC, plays DESC" };
    let agg = "COUNT(*) AS plays, COALESCE(SUM(p.duration_s), 0) AS watch_s, COUNT(DISTINCT p.user_id) AS users, MAX(p.ended_at) AS last_played";
    let (sql, cond) = match kind {
        "movies" | "music" => {
            let ty = if kind == "movies" { "Movie" } else { "Audio" };
            let sub = if kind == "movies" { "CAST(i.production_year AS TEXT)" } else { "COALESCE(i.album_artist, i.album)" };
            let c = cond.with("p.item_type = ?", ty.to_string());
            (
                format!(
                    "SELECT p.item_id AS id, COALESCE(i.name, MAX(p.item_name)) AS name, {sub} AS sub, p.item_id AS image_item_id, {agg}
                     FROM playbacks p LEFT JOIN items i ON i.id = p.item_id {} GROUP BY p.item_id ORDER BY {order} LIMIT {limit}",
                    c.sql()
                ),
                c,
            )
        }
        "series" => {
            let c = cond.with_raw("p.item_type = 'Episode'");
            (
                format!(
                    "SELECT p.series_id AS id, COALESCE(i.name, MAX(p.series_name), 'Unknown series') AS name,
                            CAST(i.production_year AS TEXT) AS sub, p.series_id AS image_item_id, {agg}
                     FROM playbacks p LEFT JOIN items i ON i.id = p.series_id {}
                     GROUP BY COALESCE(p.series_id, p.series_name) ORDER BY {order} LIMIT {limit}",
                    c.sql()
                ),
                c,
            )
        }
        "users" => (
            format!(
                "SELECT p.user_id AS id, COALESCE(u.name, MAX(p.user_name)) AS name, NULL AS sub, NULL AS image_item_id,
                        COUNT(*) AS plays, COALESCE(SUM(p.duration_s), 0) AS watch_s, MAX(p.ended_at) AS last_played
                 FROM playbacks p LEFT JOIN users u ON u.id = p.user_id {} GROUP BY p.user_id ORDER BY {order} LIMIT {limit}",
                cond.sql()
            ),
            cond.clone(),
        ),
        "clients" | "devices" => {
            let col = if kind == "clients" { "p.client" } else { "p.device_name" };
            let c = cond.with_raw(&format!("{col} IS NOT NULL"));
            (
                format!(
                    "SELECT NULL AS id, {col} AS name, NULL AS sub, NULL AS image_item_id, {agg}
                     FROM playbacks p {} GROUP BY {col} ORDER BY {order} LIMIT {limit}",
                    c.sql()
                ),
                c,
            )
        }
        "libraries" => {
            let c = cond.with_raw("p.library_id IS NOT NULL");
            (
                format!(
                    "SELECT p.library_id AS id, COALESCE(l.name, 'Removed library') AS name, l.collection_type AS sub, p.library_id AS image_item_id, {agg}
                     FROM playbacks p LEFT JOIN libraries l ON l.id = p.library_id {} GROUP BY p.library_id ORDER BY {order} LIMIT {limit}",
                    c.sql()
                ),
                c,
            )
        }
        // Whatever the library holds: series for shows, the item itself for everything else.
        "library_items" => (
            format!(
                "SELECT COALESCE(p.series_id, p.item_id) AS id,
                        COALESCE(i.name, MAX(COALESCE(p.series_name, p.item_name))) AS name,
                        COALESCE(CAST(i.production_year AS TEXT), i.album_artist) AS sub,
                        COALESCE(p.series_id, p.item_id) AS image_item_id, {agg}
                 FROM playbacks p LEFT JOIN items i ON i.id = COALESCE(p.series_id, p.item_id) {}
                 GROUP BY COALESCE(p.series_id, p.item_id) ORDER BY {order} LIMIT {limit}",
                cond.sql()
            ),
            cond.clone(),
        ),
        _ => anyhow::bail!("unknown kind"),
    };
    Ok(rows_json(conn, &sql, &cond.args)?.into_iter().map(Value::Object).collect())
}

// ---------------------------------------------------------------- plays

const PLAY_SELECT: &str = "SELECT p.id, p.source, p.active, p.user_id, COALESCE(u.name, p.user_name) AS user_name,
    p.item_id, p.item_name, p.item_type, p.series_id, p.series_name, p.season_number, p.episode_number,
    CASE WHEN p.item_type = 'Episode' AND p.series_id IS NOT NULL THEN p.series_id ELSE p.item_id END AS image_item_id,
    (i.id IS NOT NULL AND i.removed = 0) AS item_exists,
    p.started_at, p.ended_at, p.duration_s, p.paused_s, p.position_s,
    COALESCE(p.runtime_s, i.runtime_s) AS runtime_s,
    p.client, p.device_name, p.device_id, p.app_version, p.remote_ip, p.play_method, p.container, p.bitrate,
    p.video_codec, p.width, p.height, p.video_range, p.bit_depth,
    p.audio_codec, p.audio_channels, p.audio_language, p.subtitle_codec, p.subtitle_language, p.transcode,
    p.pause_count, p.seek_count, p.start_position_s, p.is_local, p.group_id,
    CASE WHEN p.group_id IS NULL THEN NULL ELSE (SELECT COUNT(DISTINCT g.user_id) FROM playbacks g WHERE g.group_id = p.group_id) END AS group_size
  FROM playbacks p
  LEFT JOIN items i ON i.id = p.item_id
  LEFT JOIN users u ON u.id = p.user_id";

const DETAIL_ONLY: [&str; 12] = [
    "device_id", "bitrate", "video_codec", "width", "height", "video_range", "bit_depth", "audio_codec", "audio_channels",
    "audio_language", "subtitle_codec", "subtitle_language",
];

fn decorate_play(mut m: Map<String, Value>, see_network: bool, detail: bool) -> Value {
    let text = |m: &Map<String, Value>, k: &str| m.get(k).and_then(Value::as_str).map(str::to_string);
    let num = |m: &Map<String, Value>, k: &str| m.get(k).and_then(Value::as_i64);

    let video = media::video_label(text(&m, "video_codec").as_deref(), num(&m, "width"), num(&m, "height"), text(&m, "video_range").as_deref());
    let audio = media::audio_label(text(&m, "audio_codec").as_deref(), num(&m, "audio_channels"), text(&m, "audio_language").as_deref());
    let subtitle = media::subtitle_label(if detail { text(&m, "subtitle_codec") } else { None }.as_deref(), text(&m, "subtitle_language").as_deref());

    // Live rows know where playback stopped. Imported rows only know how long it ran.
    let completion = match (num(&m, "runtime_s").filter(|r| *r > 0), num(&m, "position_s"), num(&m, "duration_s")) {
        (Some(rt), Some(pos), _) => Some((pos as f64 / rt as f64).clamp(0.0, 1.0)),
        (Some(rt), None, Some(d)) => Some((d as f64 / rt as f64).clamp(0.0, 1.0)),
        _ => None,
    };
    m.insert("video".into(), json!(video));
    m.insert("audio".into(), json!(audio));
    m.insert("subtitle".into(), json!(subtitle));
    m.insert("completion".into(), json!(completion.map(|c| (c * 1000.0).round() / 1000.0)));

    if !see_network {
        m.insert("remote_ip".into(), Value::Null);
        m.insert("device_id".into(), Value::Null);
        m.insert("is_local".into(), Value::Null);
    }
    if !detail {
        m.remove("start_position_s");
        for k in DETAIL_ONLY {
            m.remove(k);
        }
        m.remove("transcode");
    }
    Value::Object(m)
}

#[derive(Deserialize, Default)]
pub struct ActivityQuery {
    // Not `#[serde(flatten)]`: flattening makes serde_urlencoded hand numbers over as strings.
    days: Option<i64>,
    user_id: Option<String>,
    library_id: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
    q: Option<String>,
    method: Option<String>,
    #[serde(rename = "type")]
    item_type: Option<String>,
    item_id: Option<String>,
    series_id: Option<String>,
}

pub async fn activity(State(app): State<App>, user: AuthUser, Query(q): Query<ActivityQuery>) -> ApiResult {
    let page = q.page.unwrap_or(1).max(1);
    let per_page = q.per_page.unwrap_or(50).clamp(1, 200);
    let filter = FilterQuery { days: q.days, user_id: q.user_id.clone(), library_id: q.library_id.clone() };
    let out = scoped(&app, &user, &filter, move |c, scope| {
        let mut cond = scope.cond();
        if let Some(m) = q.method.filter(|m| !m.is_empty()) {
            cond.add("p.play_method = ?", m);
        }
        if let Some(t) = q.item_type.filter(|t| !t.is_empty()) {
            match t.as_str() {
                "Other" => cond.raw("p.item_type NOT IN ('Movie', 'Episode', 'Audio')"),
                _ => cond.add("p.item_type = ?", t),
            };
        }
        if let Some(id) = q.item_id.filter(|s| !s.is_empty()) {
            cond.add("p.item_id = ?", db::norm_id(&id));
        }
        if let Some(id) = q.series_id.filter(|s| !s.is_empty()) {
            cond.add("p.series_id = ?", db::norm_id(&id));
        }
        // Word by word: "alya opera" finds plays of Alya… on Opera. Every word must be somewhere in the row.
        for word in q.q.as_deref().unwrap_or_default().split_whitespace().take(8) {
            let like = format!("%{}%", word.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
            let mut fields = vec!["p.item_name", "p.series_name", "p.user_name", "p.client", "p.device_name"];
            if scope.perms.see_network {
                fields.push("p.remote_ip");
            }
            let ors: Vec<String> = fields.iter().map(|f| format!("{f} LIKE ? ESCAPE '\\'")).collect();
            cond.clauses.push(format!("({})", ors.join(" OR ")));
            for _ in &fields {
                cond.args.push(like.clone().into());
            }
        }
        let total: i64 = c.query_row(&format!("SELECT COUNT(*) FROM playbacks p {}", cond.sql()), params_from_iter(cond.args.iter()), |r| r.get(0))?;
        let sql = format!("{PLAY_SELECT} {} ORDER BY p.ended_at DESC, p.id DESC LIMIT {per_page} OFFSET {}", cond.sql(), (page - 1) * per_page);
        let rows: Vec<Value> = rows_json(c, &sql, &cond.args)?.into_iter().map(|m| decorate_play(m, scope.perms.see_network, false)).collect();
        Ok(json!({ "total": total, "page": page, "per_page": per_page, "rows": rows }))
    })
    .await?;
    Ok(Json(out))
}

pub async fn activity_detail(State(app): State<App>, user: AuthUser, Path(id): Path<i64>) -> ApiResult {
    let found = scoped(&app, &user, &FilterQuery::default(), move |c, scope| {
        let mut cond = Cond::default();
        cond.add("p.id = ?", id);
        if let Some(u) = &scope.user_id {
            cond.add("p.user_id = ?", u.clone());
        }
        let Some(play) = one_json(c, &format!("{PLAY_SELECT} {}", cond.sql()), &cond.args)? else { return Ok(None) };
        let mut play = decorate_play(play, scope.perms.see_network, true);
        let events = rows_json(c, "SELECT at, kind, position_s, detail FROM playback_events WHERE playback_id = ?1 ORDER BY id", &[id.into()])?;
        play["events"] = json!(events);
        // The people it was watched with. Visible to anyone who can see this play: it was a shared evening.
        let with = match play["group_id"].as_i64() {
            Some(g) => rows_json(
                c,
                "SELECT DISTINCT p.user_id, COALESCE(u.name, p.user_name) AS user_name FROM playbacks p LEFT JOIN users u ON u.id = p.user_id
                 WHERE p.group_id = ?1 AND p.user_id <> ?2 ORDER BY 2",
                &[g.into(), play["user_id"].as_str().unwrap_or_default().to_string().into()],
            )?,
            None => vec![],
        };
        play["watched_with"] = json!(with);
        Ok(Some(play))
    })
    .await?;
    found.map(Json).ok_or_else(|| ApiError::not_found("Play"))
}

pub async fn activity_delete(State(app): State<App>, Manager(_): Manager, Path(id): Path<i64>) -> ApiResult {
    let n = app.db.call(move |c| Ok(c.execute("DELETE FROM playbacks WHERE id = ?1 AND active = 0", [id])?)).await?;
    if n == 0 {
        return Err(ApiError::not_found("Play"));
    }
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------- stats endpoints

pub async fn overview(State(app): State<App>, user: AuthUser, Query(q): Query<FilterQuery>) -> ApiResult {
    let out = scoped(&app, &user, &q, |c, scope| {
        let cond = scope.cond();
        let previous = scope.previous_cond().map(|p| totals(c, &p)).transpose()?;
        let (series, bucket) = daily(c, scope, &cond)?;
        let lib = match &scope.library_id {
            Some(l) => Cond::default().with("library_id = ?", l.clone()),
            None => Cond::default(),
        }
        .with_raw("removed = 0");
        let library = one_json(
            c,
            &format!(
                "SELECT COALESCE(SUM(type = 'Movie'), 0) AS movies, COALESCE(SUM(type = 'Series'), 0) AS series,
                        COALESCE(SUM(type = 'Episode'), 0) AS episodes, COALESCE(SUM(type = 'Audio'), 0) AS tracks,
                        COALESCE(SUM(size_bytes), 0) AS size_bytes,
                        (SELECT COUNT(*) FROM users WHERE removed = 0) AS users
                 FROM items {}",
                lib.sql()
            ),
            &lib.args,
        )?;
        Ok(json!({ "totals": totals(c, &cond)?, "previous": previous, "daily": series, "bucket": bucket, "library": library }))
    })
    .await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct TopQuery {
    // Not `#[serde(flatten)]`: flattening makes serde_urlencoded hand numbers over as strings.
    days: Option<i64>,
    user_id: Option<String>,
    library_id: Option<String>,
    kind: Option<String>,
    limit: Option<i64>,
    sort: Option<String>,
}

pub async fn top_handler(State(app): State<App>, user: AuthUser, Query(q): Query<TopQuery>) -> ApiResult {
    let kind = q.kind.clone().unwrap_or_else(|| "movies".into());
    if !["movies", "series", "music", "users", "clients", "devices", "libraries"].contains(&kind.as_str()) {
        return Err(ApiError::bad_request("Unknown kind"));
    }
    let limit = q.limit.unwrap_or(10).clamp(1, 100);
    let by_plays = q.sort.as_deref() == Some("plays");
    let filter = FilterQuery { days: q.days, user_id: q.user_id.clone(), library_id: q.library_id.clone() };
    let rows = scoped(&app, &user, &filter, move |c, scope| top(c, &scope.cond(), &kind, limit, by_plays)).await?;
    Ok(Json(json!({ "rows": rows })))
}

pub async fn heatmap_handler(State(app): State<App>, user: AuthUser, Query(q): Query<FilterQuery>) -> ApiResult {
    Ok(Json(scoped(&app, &user, &q, |c, scope| heatmap(c, &scope.cond())).await?))
}

pub async fn playback(State(app): State<App>, user: AuthUser, Query(q): Query<FilterQuery>) -> ApiResult {
    let out = scoped(&app, &user, &q, |c, scope| {
        let cond = scope.cond();
        let video = cond.with_raw("p.video_codec IS NOT NULL");
        let video_transcode = cond.with_raw("p.play_method = 'Transcode' AND json_extract(p.transcode, '$.is_video_direct') = 0");
        Ok(json!({
            "methods": buckets(c, &cond, "p.play_method", "", 12)?,
            "transcode_reasons": buckets(c, &cond.with_raw("p.transcode IS NOT NULL"), "j.value", ", json_each(p.transcode, '$.reasons') j", 12)?,
            "hw_accel": buckets(c, &video_transcode, "COALESCE(json_extract(p.transcode, '$.hw_accel'), 'Software')", "", 12)?,
            "video_codecs": buckets(c, &cond, "UPPER(p.video_codec)", "", 12)?,
            "audio_codecs": buckets(c, &cond, "UPPER(p.audio_codec)", "", 12)?,
            "resolutions": buckets(c, &cond, RESOLUTION_SQL, "", 12)?,
            "video_ranges": buckets(c, &video, "p.video_range", "", 12)?,
            "containers": buckets(c, &cond, "LOWER(p.container)", "", 12)?,
            "audio_channels": buckets(c, &cond, CHANNELS_SQL, "", 12)?,
            "clients": buckets(c, &cond, "p.client", "", 12)?,
            "subtitles": buckets(c, &video, "COALESCE(p.subtitle_language, 'None')", "", 12)?,
        }))
    })
    .await?;
    Ok(Json(out))
}

// ---------------------------------------------------------------- users

fn user_rows(conn: &Connection, scope: &Scope, only: Option<&str>) -> Result<Vec<Value>> {
    let cond = scope.cond();
    let mut args = cond.args.clone();
    let mut filter = String::new();
    if let Some(id) = only {
        filter = "WHERE u.id = ?".into();
        args.push(id.to_string().into());
    }
    let sql = format!(
        "SELECT u.id, u.name, u.is_admin, u.is_disabled, u.removed, (u.image_tag IS NOT NULL) AS has_image,
                u.last_login_at, u.last_activity_at,
                COALESCE(s.plays, 0) AS plays, COALESCE(s.watch_s, 0) AS watch_s, l.last_played_at,
                (SELECT CASE WHEN x.series_name IS NOT NULL AND x.item_type = 'Episode' THEN x.series_name || ' — ' || x.item_name ELSE x.item_name END
                   FROM playbacks x WHERE x.user_id = u.id ORDER BY x.ended_at DESC LIMIT 1) AS last_item_name,
                (SELECT x.client FROM playbacks x WHERE x.user_id = u.id ORDER BY x.ended_at DESC LIMIT 1) AS last_client
         FROM users u
         LEFT JOIN (SELECT p.user_id, COUNT(*) AS plays, SUM(p.duration_s) AS watch_s FROM playbacks p {} GROUP BY p.user_id) s ON s.user_id = u.id
         LEFT JOIN (SELECT user_id, MAX(ended_at) AS last_played_at FROM playbacks GROUP BY user_id) l ON l.user_id = u.id
         {filter}
         ORDER BY watch_s DESC, u.name COLLATE NOCASE",
        cond.sql()
    );
    Ok(rows_json(conn, &sql, &args)?.into_iter().map(Value::Object).collect())
}

pub async fn users(State(app): State<App>, user: AuthUser, Query(q): Query<FilterQuery>) -> ApiResult {
    // The list is about everyone: a user filter from the URL would only blank out the others.
    let q = FilterQuery { user_id: None, ..q };
    let me = (!user.perms.see_everyone).then(|| user.id.clone());
    let rows = scoped(&app, &user, &q, move |c, scope| {
        let everyone = Scope { user_id: None, ..scope.clone() };
        user_rows(c, &everyone, me.as_deref())
    })
    .await?;
    Ok(Json(json!({ "users": rows })))
}

pub async fn user_detail(State(app): State<App>, user: AuthUser, Path(id): Path<String>, Query(q): Query<FilterQuery>) -> ApiResult {
    let id = db::norm_id(&id);
    if !user.perms.see_everyone && user.id != id {
        return Err(ApiError::not_permitted("see other people's statistics"));
    }
    let q = FilterQuery { user_id: Some(id.clone()), ..q };
    // Own page or permitted: let the scope follow the id in the URL. Everything else keeps the caller's permissions.
    let as_viewer = AuthUser { perms: crate::auth::Perms { see_everyone: true, ..user.perms }, ..user.clone() };
    let see_network = user.perms.see_network;
    let out = scoped(&app, &as_viewer, &q, move |c, scope| {
        let list_scope = Scope { user_id: None, ..scope.clone() };
        let Some(u) = user_rows(c, &list_scope, Some(&id))?.into_iter().next() else { return Ok(None) };
        let cond = scope.cond();
        let mut t = totals(c, &cond)?;
        let kinds = one_json(
            c,
            &format!(
                "SELECT COALESCE(SUM(p.item_type = 'Movie'), 0) AS movies, COALESCE(SUM(p.item_type = 'Episode'), 0) AS episodes,
                        COALESCE(SUM(p.item_type = 'Audio'), 0) AS tracks FROM playbacks p {}",
                cond.sql()
            ),
            &cond.args,
        )?
        .unwrap_or_default();
        if let Some(obj) = t.as_object_mut() {
            obj.extend(kinds);
            obj.remove("active_users");
        }
        let (series, bucket) = daily(c, scope, &cond)?;
        let devices = rows_json(
            c,
            &format!(
                "SELECT p.device_id, MAX(p.device_name) AS device_name, MAX(p.client) AS client, MAX(p.app_version) AS app_version,
                        COUNT(*) AS plays, MAX(p.ended_at) AS last_seen
                 FROM playbacks p {} GROUP BY COALESCE(p.device_id, p.device_name) ORDER BY last_seen DESC LIMIT 50",
                cond.sql()
            ),
            &cond.args,
        )?;
        let ips: Vec<Value> = if see_network {
            let ipc = cond.with_raw("p.remote_ip IS NOT NULL");
            rows_json(
                c,
                &format!(
                    "SELECT p.remote_ip AS ip, COUNT(*) AS plays, MIN(p.started_at) AS first_seen, MAX(p.ended_at) AS last_seen
                     FROM playbacks p {} GROUP BY p.remote_ip ORDER BY last_seen DESC LIMIT 100",
                    ipc.sql()
                ),
                &ipc.args,
            )?
            .into_iter()
            .map(|mut m| {
                let local = m.get("ip").and_then(Value::as_str).and_then(db::is_local_ip).unwrap_or(false);
                m.insert("is_local".into(), json!(local));
                Value::Object(m)
            })
            .collect()
        } else {
            vec![]
        };
        Ok(Some(json!({
            "user": u, "totals": t, "daily": series, "bucket": bucket,
            "heatmap": heatmap(c, &cond)?,
            "top_series": top(c, &cond, "series", 8, false)?,
            "top_movies": top(c, &cond, "movies", 8, false)?,
            "clients": buckets(c, &cond, "p.client", "", 8)?,
            "methods": buckets(c, &cond, "p.play_method", "", 8)?,
            "genres": genre_buckets(c, &cond)?,
            "jellyfin": jellyfin_flags(c, &id)?,
            // Streaks are about the whole history, whatever time range the page is showing.
            "streaks": crate::profile::streaks(c, &id, scope.min_play_s)?,
            "devices": devices, "ips": ips,
        })))
    })
    .await?;
    out.map(Json).ok_or_else(|| ApiError::not_found("User"))
}

// ---------------------------------------------------------------- libraries & items

fn library_rows(conn: &Connection, scope: &Scope, only: Option<&str>) -> Result<Vec<Value>> {
    let cond = scope.cond();
    let mut args = cond.args.clone();
    // A library that was deleted in Jellyfin and never had a single play is just noise in the list.
    // (It stays reachable by id, and one with history stays listed, marked as removed.)
    let mut filter = "WHERE (l.removed = 0 OR EXISTS (SELECT 1 FROM playbacks x WHERE x.library_id = l.id))".to_string();
    if let Some(id) = only {
        filter = "WHERE l.id = ?".into();
        args.push(id.to_string().into());
    }
    let sql = format!(
        "SELECT l.id, l.name, l.collection_type, l.removed,
                COALESCE(c.item_count, 0) AS item_count, COALESCE(c.series_count, 0) AS series_count,
                COALESCE(c.episode_count, 0) AS episode_count, COALESCE(c.size_bytes, 0) AS size_bytes,
                COALESCE(s.plays, 0) AS plays, COALESCE(s.watch_s, 0) AS watch_s, s.last_played_at
         FROM libraries l
         LEFT JOIN (SELECT library_id,
                           SUM(type IN ('Movie', 'Series', 'Audio', 'MusicVideo', 'Video', 'Book', 'AudioBook')) AS item_count,
                           SUM(type = 'Series') AS series_count, SUM(type = 'Episode') AS episode_count, SUM(size_bytes) AS size_bytes
                    FROM items WHERE removed = 0 GROUP BY library_id) c ON c.library_id = l.id
         LEFT JOIN (SELECT p.library_id, COUNT(*) AS plays, SUM(p.duration_s) AS watch_s, MAX(p.ended_at) AS last_played_at
                    FROM playbacks p {} GROUP BY p.library_id) s ON s.library_id = l.id
         {filter}
         ORDER BY l.removed, watch_s DESC, l.name COLLATE NOCASE",
        cond.sql()
    );
    Ok(rows_json(conn, &sql, &args)?.into_iter().map(Value::Object).collect())
}

const ITEM_CARD: &str = "SELECT i.id, i.name, i.type, i.production_year AS year,
    COALESCE(i.album_artist, i.series_name, CAST(i.production_year AS TEXT)) AS sub, i.id AS image_item_id, i.date_created FROM items i";

pub async fn libraries(State(app): State<App>, user: AuthUser, Query(q): Query<FilterQuery>) -> ApiResult {
    let q = FilterQuery { library_id: None, ..q };
    let rows = scoped(&app, &user, &q, |c, scope| library_rows(c, scope, None)).await?;
    Ok(Json(json!({ "libraries": rows })))
}

pub async fn library_detail(State(app): State<App>, user: AuthUser, Path(id): Path<String>, Query(q): Query<FilterQuery>) -> ApiResult {
    let id = db::norm_id(&id);
    let q = FilterQuery { library_id: Some(id.clone()), ..q };
    let out = scoped(&app, &user, &q, move |c, scope| {
        let list_scope = Scope { library_id: None, ..scope.clone() };
        let Some(lib) = library_rows(c, &list_scope, Some(&id))?.into_iter().next() else { return Ok(None) };
        let cond = scope.cond();
        let (series, bucket) = daily(c, scope, &cond)?;
        let recent = rows_json(
            c,
            &format!("{ITEM_CARD} WHERE i.library_id = ?1 AND i.removed = 0 AND i.type IN ('Movie', 'Series', 'MusicAlbum', 'Video', 'MusicVideo', 'Book', 'AudioBook') ORDER BY i.date_created DESC LIMIT 18"),
            &[id.clone().into()],
        )?;
        Ok(Some(json!({
            "library": lib, "top": top(c, &cond, "library_items", 10, false)?,
            "recently_added": recent, "daily": series, "bucket": bucket,
        })))
    })
    .await?;
    out.map(Json).ok_or_else(|| ApiError::not_found("Library"))
}

pub async fn item_detail(State(app): State<App>, user: AuthUser, Path(id): Path<String>, Query(q): Query<FilterQuery>) -> ApiResult {
    let id = db::norm_id(&id);
    let out = scoped(&app, &user, &q, move |c, scope| {
        let item = one_json(
            c,
            "SELECT i.id, i.name, i.type, i.production_year AS year, i.overview, i.genres, i.community_rating, i.official_rating,
                    i.runtime_s, i.premiere_date, i.date_created, i.library_id, l.name AS library_name, i.removed,
                    i.series_id, i.series_name, i.parent_index_number AS season_number, i.index_number AS episode_number,
                    i.album, i.album_artist, i.container, i.size_bytes, i.bitrate, i.path,
                    i.video_codec, i.width, i.height, i.video_range, i.audio_codec, i.audio_channels,
                    i.bit_depth, i.framerate, i.studios, i.provider_ids,
                    (i.backdrop_tag IS NOT NULL) AS has_backdrop
             FROM items i LEFT JOIN libraries l ON l.id = i.library_id WHERE i.id = ?1",
            &[id.clone().into()],
        )?;
        // Deleted before finstats ever saw it, but it still has history: describe it from its plays.
        let item = match item {
            Some(i) => Some(i),
            None => one_json(
                c,
                "SELECT ?1 AS id, name, type, 1 AS removed, 0 AS has_backdrop FROM (
                    SELECT item_name AS name, item_type AS type, ended_at FROM playbacks WHERE item_id = ?1
                    UNION ALL
                    SELECT series_name, 'Series', ended_at FROM playbacks WHERE series_id = ?1
                 ) ORDER BY ended_at DESC LIMIT 1",
                &[id.clone().into()],
            )?,
        };
        let Some(mut item) = item else { return Ok(None) };
        let text = |m: &Map<String, Value>, k: &str| m.get(k).and_then(Value::as_str).map(str::to_string);
        let num = |m: &Map<String, Value>, k: &str| m.get(k).and_then(Value::as_i64);
        let video = media::video_label(text(&item, "video_codec").as_deref(), num(&item, "width"), num(&item, "height"), text(&item, "video_range").as_deref());
        let audio = media::audio_label(text(&item, "audio_codec").as_deref(), num(&item, "audio_channels"), None);
        item.insert("video".into(), json!(video));
        item.insert("audio".into(), json!(audio));
        for k in ["video_codec", "width", "height", "video_range", "audio_codec", "audio_channels"] {
            item.remove(k);
        }
        let provider_ids = item.remove("provider_ids");
        let external = external_links(item.get("type").and_then(Value::as_str), provider_ids);
        item.insert("external".into(), json!(external));
        if item.get("studios").is_none_or(Value::is_null) {
            item.insert("studios".into(), json!([]));
        }
        if !scope.perms.see_server {
            item.insert("path".into(), Value::Null);
        }

        let is_series = item.get("type").and_then(Value::as_str) == Some("Series");
        let cond = scope.cond().with(if is_series { "p.series_id = ?" } else { "p.item_id = ?" }, id.clone());
        let totals = one_json(
            c,
            &format!(
                "SELECT COUNT(*) AS plays, COALESCE(SUM(p.duration_s), 0) AS watch_s, COUNT(DISTINCT p.user_id) AS users,
                        MAX(p.ended_at) AS last_played_at FROM playbacks p {}",
                cond.sql()
            ),
            &cond.args,
        )?;
        let watchers = rows_json(
            c,
            &format!(
                "SELECT p.user_id, COALESCE(u.name, MAX(p.user_name)) AS user_name, COUNT(*) AS plays,
                        COALESCE(SUM(p.duration_s), 0) AS watch_s, MAX(p.ended_at) AS last_played_at
                 FROM playbacks p LEFT JOIN users u ON u.id = p.user_id {} GROUP BY p.user_id ORDER BY watch_s DESC LIMIT 50",
                cond.sql()
            ),
            &cond.args,
        )?;

        let mut seasons: Vec<Value> = vec![];
        if is_series {
            let mut args = cond.args.clone();
            args.push(id.clone().into());
            let eps = rows_json(
                c,
                &format!(
                    "SELECT e.id, e.name, e.index_number AS episode_number, e.parent_index_number AS season_number, e.season_id,
                            COALESCE(sn.name, 'Season ' || COALESCE(e.parent_index_number, '?')) AS season_name,
                            e.runtime_s, COALESCE(s.plays, 0) AS plays, COALESCE(s.watch_s, 0) AS watch_s
                     FROM items e
                     LEFT JOIN items sn ON sn.id = e.season_id
                     LEFT JOIN (SELECT p.item_id, COUNT(*) AS plays, SUM(p.duration_s) AS watch_s FROM playbacks p {} GROUP BY p.item_id) s ON s.item_id = e.id
                     WHERE e.series_id = ? AND e.type = 'Episode' AND (e.removed = 0 OR {series_removed})
                     ORDER BY COALESCE(e.parent_index_number, 9999), COALESCE(e.index_number, 9999), e.name",
                    cond.sql(),
                    series_removed = if item.get("removed").and_then(Value::as_bool).unwrap_or(false) { 1 } else { 0 },
                ),
                &args,
            )?;
            for mut e in eps {
                let key = e.get("season_id").cloned().unwrap_or(Value::Null);
                let season_number = e.get("season_number").cloned().unwrap_or(Value::Null);
                let season_name = e.remove("season_name").unwrap_or(Value::Null);
                e.remove("season_id");
                e.remove("season_number");
                let same = seasons.last().is_some_and(|s: &Value| s["id"] == key && s["season_number"] == season_number);
                if !same {
                    seasons.push(json!({ "id": key, "name": season_name, "season_number": season_number, "episodes": [] }));
                }
                if let Some(list) = seasons.last_mut().and_then(|s| s["episodes"].as_array_mut()) {
                    list.push(Value::Object(e));
                }
            }
        }
        let (series, bucket) = daily(c, scope, &cond)?;
        // Jellyfin's own flags. For a series: anyone who has finished at least one episode.
        let mut pb_args: Vec<SqlValue> = vec![id.clone().into()];
        let only_me = match &scope.user_id {
            Some(u) if !scope.perms.see_everyone => {
                pb_args.push(u.clone().into());
                "AND ui.user_id = ?2"
            }
            _ => "",
        };
        let played_by = rows_json(
            c,
            &format!(
                "SELECT ui.user_id, COALESCE(u.name, 'Unknown') AS user_name, MAX(ui.last_played_at) AS last_played_at,
                        MAX(ui.is_favorite) AS is_favorite
                 FROM user_items ui LEFT JOIN users u ON u.id = ui.user_id
                 WHERE (ui.item_id = ?1 OR ui.item_id IN (SELECT id FROM items WHERE series_id = ?1)) {only_me}
                 GROUP BY ui.user_id ORDER BY last_played_at DESC"
            ),
            &pb_args,
        )?;
        // Cast and crew are kept per film and show; an episode or season answers with its show's.
        let credited = item.get("series_id").and_then(Value::as_str).filter(|_| !is_series).map(str::to_string).unwrap_or_else(|| id.clone());
        let people = rows_json(
            c,
            "SELECT person_id AS id, name, kind, role, has_image FROM item_people WHERE item_id = ?1 ORDER BY (kind = 'Actor'), sort",
            &[credited.into()],
        )?;
        Ok(Some(json!({ "item": item, "totals": totals, "watchers": watchers, "seasons": seasons, "daily": series, "bucket": bucket, "played_by": played_by, "people": people })))
    })
    .await?;
    out.map(Json).ok_or_else(|| ApiError::not_found("Item"))
}

/// One actor or director: what they are in, and how much of it was watched (within the caller's scope).
pub async fn person_detail(State(app): State<App>, user: AuthUser, Path(id): Path<String>, Query(q): Query<FilterQuery>) -> ApiResult {
    let id = db::norm_id(&id);
    let out = scoped(&app, &user, &q, move |c, scope| {
        let person = one_json(
            c,
            "SELECT person_id AS id, MAX(name) AS name, MAX(has_image) AS has_image,
                    MAX(kind = 'Actor') AS is_actor, MAX(kind = 'Director') AS is_director, COUNT(DISTINCT item_id) AS titles
             FROM item_people WHERE person_id = ?1 GROUP BY person_id",
            &[id.clone().into()],
        )?;
        let Some(person) = person else { return Ok(None) };

        // A person can act in and direct the same title; the subquery keeps a play from counting twice.
        const TITLE: &str = "COALESCE(p.series_id, p.item_id)";
        let cond = scope.cond().with(&format!("{TITLE} IN (SELECT item_id FROM item_people WHERE person_id = ?)"), id.clone());
        let totals = one_json(
            c,
            &format!(
                "SELECT COUNT(*) AS plays, COALESCE(SUM(p.duration_s), 0) AS watch_s, COUNT(DISTINCT p.user_id) AS users,
                        COUNT(DISTINCT {TITLE}) AS titles_watched, MAX(p.ended_at) AS last_played_at FROM playbacks p {}",
                cond.sql()
            ),
            &cond.args,
        )?;
        let mut args: Vec<SqlValue> = vec![id.clone().into()];
        args.extend(cond.args.iter().cloned());
        let titles = rows_json(
            c,
            &format!(
                "SELECT i.id, COALESCE(i.name, 'Unknown title') AS name, i.type, i.production_year AS year, COALESCE(i.removed, 1) AS removed,
                        ip.kinds, ip.role, COALESCE(s.plays, 0) AS plays, COALESCE(s.watch_s, 0) AS watch_s, s.last_played_at
                 FROM (SELECT item_id, GROUP_CONCAT(kind, ',') AS kinds, MAX(role) AS role FROM item_people WHERE person_id = ? GROUP BY item_id) ip
                 LEFT JOIN items i ON i.id = ip.item_id
                 LEFT JOIN (SELECT {TITLE} AS title_id, COUNT(*) AS plays, SUM(p.duration_s) AS watch_s, MAX(p.ended_at) AS last_played_at
                            FROM playbacks p {} GROUP BY 1) s ON s.title_id = ip.item_id
                 ORDER BY watch_s DESC, i.production_year DESC, name",
                cond.sql(),
            ),
            &args,
        )?;
        let watchers = rows_json(
            c,
            &format!(
                "SELECT p.user_id, COALESCE(u.name, MAX(p.user_name)) AS user_name, COUNT(*) AS plays,
                        COALESCE(SUM(p.duration_s), 0) AS watch_s, MAX(p.ended_at) AS last_played_at
                 FROM playbacks p LEFT JOIN users u ON u.id = p.user_id {} GROUP BY p.user_id ORDER BY watch_s DESC LIMIT 50",
                cond.sql()
            ),
            &cond.args,
        )?;
        Ok(Some(json!({ "person": person, "totals": totals, "titles": titles, "watchers": watchers })))
    })
    .await?;
    out.map(Json).ok_or_else(|| ApiError::not_found("Person"))
}

#[derive(Deserialize)]
pub struct SearchQuery {
    q: Option<String>,
    limit: Option<i64>,
}

pub async fn search(State(app): State<App>, user: AuthUser, Query(q): Query<SearchQuery>) -> ApiResult {
    let Some(query) = crate::fuzzy::Query::new(q.q.as_deref().unwrap_or_default()) else {
        return Ok(Json(json!({ "items": [], "users": [] })));
    };
    let limit = q.limit.unwrap_or(12).clamp(1, 50) as usize;
    let see_everyone = user.perms.see_everyone;
    let out = app
        .db
        .call(move |c| {
            // Matching is done here rather than with LIKE: word by word, any order, accents and typos
            // forgiven. A library's worth of titles is a few thousand short strings, which is nothing.
            let mut scored: Vec<(i32, i32, Map<String, Value>)> = rows_json(
                c,
                &format!("{ITEM_CARD} WHERE i.removed = 0 AND i.type IN ('Movie', 'Series', 'MusicAlbum', 'Audio')"),
                &[],
            )?
            .into_iter()
            .filter_map(|row| {
                let name = row.get("name").and_then(Value::as_str).unwrap_or_default();
                let kind = match row.get("type").and_then(Value::as_str) {
                    Some("Series") => 0,
                    Some("Movie") => 1,
                    Some("MusicAlbum") => 2,
                    _ => 3,
                };
                // Music is also found by its artist, films and shows by title only.
                let by_title = query.score(name);
                let by_artist = if kind >= 2 { row.get("sub").and_then(Value::as_str).and_then(|a| query.score(&format!("{a} {name}"))).map(|s| s - 150) } else { None };
                by_title.max(by_artist).map(|s| (s, kind, row))
            })
            .collect();
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then_with(|| a.2["name"].as_str().cmp(&b.2["name"].as_str())));
            let items: Vec<Value> = scored.into_iter().take(limit).map(|(_, _, row)| Value::Object(row)).collect();

            let users = if see_everyone {
                let mut found: Vec<(i32, Map<String, Value>)> = rows_json(c, "SELECT id, name FROM users ORDER BY removed, name COLLATE NOCASE", &[])?
                    .into_iter()
                    .filter_map(|u| query.score(u.get("name").and_then(Value::as_str).unwrap_or_default()).map(|s| (s, u)))
                    .collect();
                found.sort_by(|a, b| b.0.cmp(&a.0));
                found.into_iter().take(8).map(|(_, u)| Value::Object(u)).collect()
            } else {
                vec![]
            };
            Ok(json!({ "items": items, "users": users }))
        })
        .await?;
    Ok(Json(out))
}

// ---------------------------------------------------------------- live & light status

pub async fn now_playing(State(app): State<App>, user: AuthUser) -> ApiResult {
    let mut sessions = app.live.read().unwrap().clone();
    // Before anything is filtered out: you may see who you are watching with, even if you can't see their streams.
    crate::groups::mark_live(&mut sessions, app.settings().group_window_s);
    if !user.perms.see_everyone {
        sessions.retain(|s| s["user_id"].as_str() == Some(user.id.as_str()));
    }
    if !user.perms.see_network {
        for s in &mut sessions {
            s["remote_ip"] = Value::Null;
        }
    }
    Ok(Json(json!({ "sessions": sessions })))
}

pub async fn summary(State(app): State<App>, user: AuthUser) -> ApiResult {
    let scope_user = (!user.perms.see_everyone).then(|| user.id.clone());
    let (plays, last_sync): (i64, Option<i64>) = app
        .db
        .call(move |c| {
            let plays = match &scope_user {
                Some(u) => c.query_row("SELECT COUNT(*) FROM playbacks WHERE user_id = ?1", [u], |r| r.get(0))?,
                None => c.query_row("SELECT COUNT(*) FROM playbacks", [], |r| r.get(0))?,
            };
            let last = c.query_row("SELECT MAX(updated_at) FROM items", [], |r| r.get::<_, Option<i64>>(0))?.filter(|t| *t > 0);
            Ok((plays, last))
        })
        .await?;
    let collector = app.collector.read().unwrap().clone();
    let active = if user.perms.see_everyone {
        collector.active_sessions
    } else {
        app.live.read().unwrap().iter().filter(|s| s["user_id"].as_str() == Some(user.id.as_str())).count()
    };
    Ok(Json(json!({
        "active_sessions": active, "plays_total": plays, "last_sync_at": last_sync,
        "collector_ok": collector.connected, "version": env!("CARGO_PKG_VERSION"),
    })))
}

// ---------------------------------------------------------------- server log

#[derive(Deserialize)]
pub struct EventsQuery {
    page: Option<i64>,
    per_page: Option<i64>,
    q: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
}

pub async fn events(State(app): State<App>, ServerViewer(_): ServerViewer, Query(q): Query<EventsQuery>) -> ApiResult {
    let page = q.page.unwrap_or(1).max(1);
    let per_page = q.per_page.unwrap_or(50).clamp(1, 200);
    let out = app
        .db
        .call(move |c| {
            let mut clauses: Vec<String> = vec![];
            let mut args: Vec<SqlValue> = vec![];
            if let Some(t) = q.kind.filter(|t| !t.is_empty()) {
                clauses.push("e.type = ?".into());
                args.push(t.into());
            }
            for word in q.q.as_deref().unwrap_or_default().split_whitespace().take(8) {
                let like = format!("%{}%", word.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
                clauses.push("(e.name LIKE ? ESCAPE '\\' OR e.short_overview LIKE ? ESCAPE '\\' OR e.overview LIKE ? ESCAPE '\\')".into());
                args.extend([like.clone().into(), like.clone().into(), like.into()]);
            }
            let wh = if clauses.is_empty() { String::new() } else { format!("WHERE {}", clauses.join(" AND ")) };
            let total: i64 = c.query_row(&format!("SELECT COUNT(*) FROM server_events e {wh}"), params_from_iter(args.iter()), |r| r.get(0))?;
            let rows = rows_json(
                c,
                &format!(
                    "SELECT e.id, e.date, e.name, COALESCE(e.overview, e.short_overview) AS overview, e.type, e.severity,
                            e.user_id, u.name AS user_name, e.item_id
                     FROM server_events e LEFT JOIN users u ON u.id = e.user_id {wh}
                     ORDER BY e.date DESC, e.id DESC LIMIT {per_page} OFFSET {}",
                    (page - 1) * per_page
                ),
                &args,
            )?;
            Ok(json!({ "total": total, "page": page, "per_page": per_page, "rows": rows }))
        })
        .await?;
    Ok(Json(out))
}

// ---------------------------------------------------------------- v0.2: insights

fn external_links(item_type: Option<&str>, provider_ids: Option<Value>) -> Vec<Value> {
    let Some(Value::Object(ids)) = provider_ids else { return vec![] };
    let clean = |v: &Value| v.as_str().filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')).map(str::to_string);
    let tmdb_kind = if matches!(item_type, Some("Series" | "Season" | "Episode")) { "tv" } else { "movie" };
    let mut out = vec![];
    for (key, value) in &ids {
        let Some(id) = clean(value) else { continue };
        let link = match key.to_ascii_lowercase().as_str() {
            "imdb" => ("IMDb", format!("https://www.imdb.com/title/{id}/")),
            "tmdb" => ("TMDB", format!("https://www.themoviedb.org/{tmdb_kind}/{id}")),
            "tvdb" if tmdb_kind == "tv" => ("TVDB", format!("https://www.thetvdb.com/dereferrer/series/{id}")),
            "musicbrainzalbum" => ("MusicBrainz", format!("https://musicbrainz.org/release/{id}")),
            _ => continue,
        };
        out.push(json!({ "label": link.0, "url": link.1 }));
    }
    out
}

/// Watch time per genre. Episodes carry no genres of their own, so they count towards their series'.
fn genre_buckets(conn: &Connection, cond: &Cond) -> Result<Vec<Value>> {
    buckets(conn, cond, "g.value", "JOIN items gi ON gi.id = COALESCE(p.series_id, p.item_id), json_each(gi.genres) g", 12)
        .map(|mut v| {
            v.sort_by_key(|b| std::cmp::Reverse(b["watch_s"].as_i64().unwrap_or(0)));
            v
        })
}

fn jellyfin_flags(conn: &Connection, user_id: &str) -> Result<Value> {
    let row = one_json(
        conn,
        "SELECT COALESCE(SUM(ui.played AND i.type = 'Movie'), 0) AS played_movies,
                COALESCE(SUM(ui.played AND i.type = 'Episode'), 0) AS played_episodes,
                COALESCE(SUM(ui.is_favorite), 0) AS favorites, COUNT(*) AS n
         FROM user_items ui LEFT JOIN items i ON i.id = ui.item_id WHERE ui.user_id = ?1",
        &[user_id.to_string().into()],
    )?;
    Ok(match row {
        Some(mut r) if r.get("n").and_then(Value::as_i64).unwrap_or(0) > 0 => {
            r.remove("n");
            Value::Object(r)
        }
        _ => Value::Null,
    })
}

/// Sweep over play intervals: the most streams (and transcodes) that ever overlapped, and the peak per bucket.
fn concurrency(conn: &Connection, scope: &Scope, cond: &Cond) -> Result<Value> {
    let sql = format!("SELECT p.started_at, p.ended_at, p.play_method = 'Transcode' FROM playbacks p {} AND p.ended_at > p.started_at", cond.with_raw("1 = 1").sql());
    let mut points: Vec<(i64, i32, i32)> = vec![];
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(cond.args.iter()))?;
    while let Some(r) = rows.next()? {
        let (start, end, transcode): (i64, i64, bool) = (r.get(0)?, r.get(1)?, r.get(2)?);
        points.push((start, 1, transcode as i32));
        points.push((end, -1, -(transcode as i32)));
    }
    // Ends sort before starts at the same instant, so back-to-back episodes are not "two streams".
    points.sort_by_key(|p| (p.0, p.1));

    let (series_template, bucket) = daily(conn, scope, cond)?;
    let weekly = bucket == "week";
    let mut per_bucket: std::collections::HashMap<String, i32> = Default::default();
    let (mut live, mut live_tc, mut peak, mut peak_at, mut peak_tc) = (0, 0, 0, None, 0);
    let mut date_stmt = conn.prepare_cached(if weekly {
        "SELECT date(?1, 'unixepoch', 'localtime', 'weekday 0', '-6 days')"
    } else {
        "SELECT date(?1, 'unixepoch', 'localtime')"
    })?;
    for (at, delta, tc_delta) in points {
        live += delta;
        live_tc += tc_delta;
        if delta > 0 {
            if live > peak {
                peak = live;
                peak_at = Some(at);
            }
            peak_tc = peak_tc.max(live_tc);
            let date: String = date_stmt.query_row([at], |r| r.get(0))?;
            let e = per_bucket.entry(date).or_insert(0);
            *e = (*e).max(live);
        }
    }
    let series: Vec<Value> = series_template
        .iter()
        .map(|d| {
            let date = d["date"].as_str().unwrap_or_default();
            json!({ "date": date, "peak": per_bucket.get(date).copied().unwrap_or(0) })
        })
        .collect();
    Ok(json!({ "peak": peak, "peak_at": peak_at, "peak_transcodes": peak_tc, "series": series, "bucket": bucket }))
}

pub async fn insights(State(app): State<App>, user: AuthUser, Query(q): Query<FilterQuery>) -> ApiResult {
    let out = scoped(&app, &user, &q, |c, scope| {
        let cond = scope.cond();
        let network = if scope.perms.see_network {
            buckets(c, &cond, "CASE p.is_local WHEN 1 THEN 'Local' WHEN 0 THEN 'Remote' ELSE 'Unknown' END", "", 3)?
        } else {
            vec![]
        };
        // What actually went over the wire: the transcoded bitrate when transcoding, else the file's.
        let data_bytes: i64 = c.query_row(
            &format!(
                "SELECT CAST(COALESCE(SUM(COALESCE(json_extract(p.transcode, '$.bitrate'), p.bitrate) / 8.0 * p.duration_s), 0) AS INTEGER)
                 FROM playbacks p {}",
                cond.sql()
            ),
            params_from_iter(cond.args.iter()),
            |r| r.get(0),
        )?;
        let cm = cond.with_raw("p.client IS NOT NULL");
        let client_methods = rows_json(
            c,
            &format!(
                "SELECT p.client, SUM(p.play_method = 'DirectPlay') AS direct_play, SUM(p.play_method = 'DirectStream') AS direct_stream,
                        SUM(p.play_method = 'Transcode') AS transcode, COALESCE(SUM(p.duration_s), 0) AS watch_s
                 FROM playbacks p {} GROUP BY p.client ORDER BY COUNT(*) DESC LIMIT 12",
                cm.sql()
            ),
            &cm.args,
        )?;
        let video = cond.with_raw("p.item_type IN ('Movie', 'Episode') AND COALESCE(p.runtime_s, 0) > 0");
        let comp = one_json(
            c,
            &format!(
                "WITH r AS (SELECT MIN(1.0, COALESCE(p.position_s, p.duration_s) * 1.0 / p.runtime_s) AS f FROM playbacks p {})
                 SELECT COALESCE(SUM(f < 0.1), 0) AS a, COALESCE(SUM(f >= 0.1 AND f < 0.5), 0) AS b,
                        COALESCE(SUM(f >= 0.5 AND f < 0.9), 0) AS c, COALESCE(SUM(f >= 0.9), 0) AS d FROM r",
                video.sql()
            ),
            &video.args,
        )?
        .unwrap_or_default();
        let completion: Vec<Value> = [("Under 10%", "a"), ("10–50%", "b"), ("50–90%", "c"), ("Finished (90%+)", "d")]
            .iter()
            .map(|(name, k)| json!({ "name": name, "plays": comp.get(*k).cloned().unwrap_or(json!(0)) }))
            .collect();
        let live = cond.with_raw("p.source = 'live' AND p.active = 0");
        let behaviour = one_json(
            c,
            &format!(
                "SELECT COUNT(*) AS plays_measured, ROUND(COALESCE(AVG(p.pause_count), 0), 2) AS avg_pauses,
                        ROUND(COALESCE(AVG(p.seek_count), 0), 2) AS avg_seeks,
                        ROUND(COALESCE(AVG(COALESCE(p.start_position_s, 0) > 30), 0), 3) AS resumed_share
                 FROM playbacks p {}",
                live.sql()
            ),
            &live.args,
        )?;
        let failed_logins = if scope.perms.see_server {
            let mut args: Vec<SqlValue> = vec![];
            let mut wh = "WHERE (e.type LIKE '%AuthenticationFail%' OR e.name LIKE '%failed%login%' OR e.name LIKE '%Failed login%')".to_string();
            if let Some(s) = scope.since {
                wh.push_str(" AND e.date >= ?");
                args.push(s.into());
            }
            rows_json(
                c,
                &format!(
                    "SELECT e.date, e.name || COALESCE(' · ' || COALESCE(e.short_overview, e.overview), '') AS overview, u.name AS user_name
                     FROM server_events e LEFT JOIN users u ON u.id = e.user_id {wh} ORDER BY e.date DESC LIMIT 10"
                ),
                &args,
            )?
        } else {
            vec![]
        };
        Ok(json!({
            "concurrency": concurrency(c, scope, &cond)?,
            "network": network, "data_bytes": data_bytes,
            "genres": genre_buckets(c, &cond)?,
            "client_methods": client_methods, "completion": completion,
            "behaviour": behaviour, "failed_logins": failed_logins,
        }))
    })
    .await?;
    Ok(Json(out))
}

// ---------------------------------------------------------------- v0.2: what the library is made of

#[derive(Deserialize)]
pub struct LibraryInsightsQuery {
    library_id: Option<String>,
}

fn lib_buckets(conn: &Connection, expr: &str, from: &str, wh: &str, args: &[SqlValue], order: &str, max: usize) -> Result<Vec<Value>> {
    let sql = format!(
        "SELECT {expr} AS name, COUNT(*) AS count, COALESCE(SUM(i.size_bytes), 0) AS size_bytes
         FROM items i {from} {wh} GROUP BY 1 HAVING name IS NOT NULL ORDER BY {order}"
    );
    let rows = rows_json(conn, &sql, args)?;
    let mut out: Vec<Value> = vec![];
    let (mut n, mut sz) = (0i64, 0i64);
    for (i, r) in rows.into_iter().enumerate() {
        if i < max {
            out.push(Value::Object(r));
        } else {
            n += r["count"].as_i64().unwrap_or(0);
            sz += r["size_bytes"].as_i64().unwrap_or(0);
        }
    }
    if n > 0 {
        out.push(json!({ "name": "Other", "count": n, "size_bytes": sz }));
    }
    Ok(out)
}

pub async fn library_insights(State(app): State<App>, _user: AuthUser, Query(q): Query<LibraryInsightsQuery>) -> ApiResult {
    let library = q.library_id.as_deref().map(db::norm_id).filter(|s| !s.is_empty());
    let out = app
        .db
        .call(move |c| {
            let (lib_sql, args): (&str, Vec<SqlValue>) = match &library {
                Some(l) => ("AND i.library_id = ?1", vec![l.clone().into()]),
                None => ("", vec![]),
            };
            let files = format!("WHERE i.removed = 0 AND i.size_bytes IS NOT NULL {lib_sql}");
            let video = format!("{files} AND i.video_codec IS NOT NULL");
            let titles = format!("WHERE i.removed = 0 AND i.type IN ('Movie', 'Series') {lib_sql}");
            let by_count = "count DESC, size_bytes DESC";
            let res_sql = RESOLUTION_SQL.replace("p.", "i.");

            let totals = one_json(
                c,
                &format!(
                    "SELECT COALESCE(SUM(i.size_bytes IS NOT NULL), 0) AS files, COALESCE(SUM(i.size_bytes), 0) AS size_bytes,
                            COALESCE(SUM(CASE WHEN i.size_bytes IS NOT NULL THEN i.runtime_s END), 0) AS runtime_s,
                            COALESCE(SUM(i.type = 'Movie'), 0) AS movies, COALESCE(SUM(i.type = 'Series'), 0) AS series,
                            COALESCE(SUM(i.type = 'Episode'), 0) AS episodes, COALESCE(SUM(i.type = 'Audio'), 0) AS tracks
                     FROM items i WHERE i.removed = 0 {lib_sql}"
                ),
                &args,
            )?;

            // Month buckets for the last two years, gap-free.
            let added_rows = rows_json(
                c,
                &format!(
                    "SELECT strftime('%Y-%m', i.date_created, 'unixepoch', 'localtime') AS month, COUNT(*) AS count
                     FROM items i WHERE i.removed = 0 AND i.type IN ('Movie', 'Episode', 'Audio', 'Video', 'MusicVideo')
                       AND i.date_created >= CAST(strftime('%s', 'now', 'start of month', '-23 months') AS INTEGER) {lib_sql} GROUP BY 1"
                ),
                &args,
            )?;
            let found: std::collections::HashMap<String, i64> =
                added_rows.iter().filter_map(|r| Some((r["month"].as_str()?.to_string(), r["count"].as_i64()?))).collect();
            let today: String = c.query_row("SELECT date('now', 'localtime', 'start of month')", [], |r| r.get(0))?;
            let mut cursor = NaiveDate::parse_from_str(&today, "%Y-%m-%d")?;
            let mut added = vec![];
            for _ in 0..24 {
                let key = cursor.format("%Y-%m").to_string();
                added.push(json!({ "month": key, "count": found.get(&key).copied().unwrap_or(0) }));
                cursor = (cursor - chrono::Duration::days(1)).with_day(1).unwrap_or(cursor);
            }
            added.reverse();

            // A series weighs what its episodes weigh.
            let sized = format!(
                "SELECT t.id, t.name, t.type, t.production_year AS year, t.date_created, t.id AS image_item_id,
                        COALESCE(t.size_bytes, (SELECT SUM(e.size_bytes) FROM items e WHERE e.series_id = t.id AND e.removed = 0), 0) AS size_bytes
                 FROM items t WHERE t.removed = 0 AND t.type IN ('Movie', 'Series') {}",
                lib_sql.replace("i.", "t.")
            );
            let largest = rows_json(c, &format!("{sized} ORDER BY size_bytes DESC LIMIT 15"), &args)?;
            // Nobody has played it here, and nobody has it marked as played in Jellyfin either.
            let never = "NOT EXISTS (SELECT 1 FROM playbacks p WHERE p.item_id = t.id OR p.series_id = t.id)
                 AND NOT EXISTS (SELECT 1 FROM user_items ui WHERE ui.played = 1 AND (ui.item_id = t.id OR ui.item_id IN (SELECT id FROM items WHERE series_id = t.id)))";
            let unwatched_items = rows_json(c, &format!("{sized} AND {never} ORDER BY size_bytes DESC LIMIT 25"), &args)?;
            let unwatched_totals =
                one_json(c, &format!("SELECT COUNT(*) AS count, COALESCE(SUM(size_bytes), 0) AS size_bytes FROM ({sized} AND {never})"), &args)?.unwrap_or_default();

            Ok(json!({
                "totals": totals,
                "resolutions": lib_buckets(c, &res_sql, "", &video, &args, by_count, 12)?,
                "video_codecs": lib_buckets(c, "UPPER(i.video_codec)", "", &video, &args, by_count, 12)?,
                "video_ranges": lib_buckets(c, "i.video_range", "", &video, &args, by_count, 12)?,
                "containers": lib_buckets(c, "LOWER(i.container)", "", &files, &args, by_count, 12)?,
                "audio_codecs": lib_buckets(c, "UPPER(i.audio_codec)", "", &files, &args, by_count, 12)?,
                // A series row has no size of its own, so a per-genre size would only count the films.
                "genres": lib_buckets(c, "g.value", ", json_each(i.genres) g", &titles, &args, by_count, 12)?
                    .into_iter()
                    .map(|mut g| {
                        g["size_bytes"] = json!(0);
                        g
                    })
                    .collect::<Vec<_>>(),
                "decades": lib_buckets(c, "(i.production_year / 10 * 10) || 's'", "", &format!("{titles} AND i.production_year > 1800"), &args, "name", 30)?,
                "added": added, "largest": largest,
                "unwatched": { "count": unwatched_totals.get("count"), "size_bytes": unwatched_totals.get("size_bytes"), "items": unwatched_items },
            }))
        })
        .await?;
    Ok(Json(out))
}

// ---------------------------------------------------------------- v0.2: the Jellyfin server

pub async fn server(State(app): State<App>, ServerViewer(_): ServerViewer) -> ApiResult {
    let out = app
        .db
        .call(|c| {
            let mut snapshot: Value = db::get_setting(c, "server_info")?.and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_else(|| {
                json!({ "fetched_at": null, "info": null, "storage": [], "plugins": [], "scheduled_tasks": [] })
            });
            let devices = rows_json(
                c,
                "SELECT d.device_id, d.device_name AS name, d.client AS app, d.app_version, d.user_id AS last_user_id,
                        COALESCE(u.name, d.last_user_name) AS last_user_name, d.last_seen
                 FROM devices d LEFT JOIN users u ON u.id = d.user_id ORDER BY d.last_seen DESC LIMIT 500",
                &[],
            )?;
            snapshot["devices"] = json!(devices);
            Ok(snapshot)
        })
        .await?;
    Ok(Json(out))
}
