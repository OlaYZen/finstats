//! What is coming in, read for the UI and joined with what was watched.
//!
//! The other modules (`arr.rs`, `seerr.rs`) write tables; this one reads them, under the same rule as every
//! statistic: without `see_everyone` a request is pinned to the caller (`stats::pinned_user`), and what other
//! people do is not even hinted at. On a server of five people a number is a name: "2 people are watching
//! this" tells a plain user who, so they get `you_follow` and nothing else.
//!
//! **Linking.** Sonarr, Radarr and Seerr know titles by TVDB/TMDB/IMDb id, the library knows them by the
//! same ids in `items.provider_ids`. `item_external` is that JSON turned into rows with an index. One id may
//! have several items (a film in an HD and a 4K library): joins go id → every matching item → plays, and
//! `item_id` on a row is only where a click should lead. (`relink.rs` refuses ambiguity because it rewrites
//! history. Nothing here rewrites anything.)

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::Result;
use axum::Json;
use axum::extract::{Query, State};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::AuthUser;
use crate::db::rusqlite::{Connection, params};
use crate::state::{ApiResult, App};
use crate::stats::pinned_user;

/// Someone who played an episode of a show this recently is following it.
pub const FOLLOW_DAYS: i64 = 120;

// ---------------------------------------------------------------- linking

/// `items.provider_ids` as rows. Cheap enough to redo whenever the library or a calendar changed.
pub fn rebuild_external(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM item_external", [])?;
    conn.execute(
        "INSERT OR REPLACE INTO item_external(item_id, source, value)
         SELECT i.id, j.key, CAST(j.value AS TEXT) FROM items i, json_each(i.provider_ids) j
         WHERE i.removed = 0 AND i.type IN ('Movie', 'Series') AND json_valid(i.provider_ids)
           AND j.key IN ('Tmdb', 'Tvdb', 'Imdb') AND j.type = 'text' AND j.value <> ''",
        [],
    )?;
    Ok(())
}

/// Point every row that came from a service at its title in the library, if it is there. The lowest item id
/// wins when there are several: any of them is a fine place to land, and it must not flip between runs.
pub fn link(conn: &Connection) -> Result<()> {
    rebuild_external(conn)?;
    let target = |source: &str, column: &str, item_type: &str| {
        format!(
            "(SELECT MIN(x.item_id) FROM item_external x JOIN items i ON i.id = x.item_id
              WHERE x.source = '{source}' AND x.value = CAST(upcoming.{column} AS TEXT) AND i.type = '{item_type}')"
        )
    };
    conn.execute(&format!("UPDATE upcoming SET item_id = COALESCE({}, {}) WHERE kind = 'episode'", target("Tvdb", "tvdb_id", "Series"), target("Imdb", "imdb_id", "Series")), [])?;
    conn.execute(&format!("UPDATE upcoming SET item_id = COALESCE({}, {}) WHERE kind = 'movie'", target("Tmdb", "tmdb_id", "Movie"), target("Imdb", "imdb_id", "Movie")), [])?;
    crate::seerr::link(conn)?;
    Ok(())
}

// ---------------------------------------------------------------- upcoming

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub service_id: i64,
    pub kind: String,
    pub release: String,
    pub day: String,
    pub at: Option<i64>,
    pub series_title: Option<String>,
    pub title: String,
    pub season: Option<i64>,
    pub episode: Option<i64>,
    pub finale: Option<String>,
    pub year: Option<i64>,
    pub tvdb_id: Option<i64>,
    pub tmdb_id: Option<i64>,
    pub arr_media_id: i64,
    pub has_file: bool,
    pub item_id: Option<String>,
}

/// Two Sonarrs that both follow a show report its episode twice, and an HD and a 4K Radarr the same film.
/// That is one thing to look forward to: the first wins, and it is on disk if it is on disk anywhere.
pub fn fold(rows: Vec<Entry>) -> Vec<Entry> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<Entry> = vec![];
    for r in rows {
        let key = match r.kind.as_str() {
            "episode" => format!("e:{}:{:?}:{:?}", r.tvdb_id.map(|t| t.to_string()).unwrap_or_else(|| r.series_title.clone().unwrap_or_default().to_lowercase()), r.season, r.episode),
            _ => format!("m:{}:{}", r.tmdb_id.map(|t| t.to_string()).unwrap_or_else(|| r.title.to_lowercase()), r.release),
        };
        match seen.get(&key) {
            Some(&i) => {
                out[i].has_file |= r.has_file;
                if out[i].item_id.is_none() {
                    out[i].item_id = r.item_id;
                }
            }
            None => {
                seen.insert(key, out.len());
                out.push(r);
            }
        }
    }
    out
}

fn entries(conn: &Connection, days: i64, only_item: Option<&str>) -> Result<Vec<Entry>> {
    // Days are local days, like every "per day" number in finstats; a film already carries its day.
    let sql = format!(
        "SELECT u.service_id, u.kind, u.release, COALESCE(u.day, date(u.at, 'unixepoch', 'localtime')) AS local_day, u.at, u.series_title, u.title,
                u.season, u.episode, u.finale, u.year, u.tvdb_id, u.tmdb_id, u.arr_media_id, u.has_file, u.item_id
         FROM upcoming u JOIN services s ON s.id = u.service_id AND s.enabled = 1
         WHERE local_day >= date('now', 'localtime') AND local_day < date('now', 'localtime', ?1) {}
         ORDER BY local_day, u.at IS NULL, u.at, u.series_title, u.season, u.episode, u.title, u.service_id",
        if only_item.is_some() { "AND u.item_id IN (SELECT x2.item_id FROM item_external x1 JOIN item_external x2 ON x2.source = x1.source AND x2.value = x1.value WHERE x1.item_id = ?2)" } else { "AND ?2 IS NULL" }
    );
    let rows = conn
        .prepare(&sql)?
        .query_map(params![format!("+{days} days"), only_item], |r| {
            Ok(Entry {
                service_id: r.get(0)?, kind: r.get(1)?, release: r.get(2)?, day: r.get(3)?, at: r.get(4)?, series_title: r.get(5)?, title: r.get(6)?, season: r.get(7)?,
                episode: r.get(8)?, finale: r.get(9)?, year: r.get(10)?, tvdb_id: r.get(11)?, tmdb_id: r.get(12)?, arr_media_id: r.get(13)?, has_file: r.get(14)?, item_id: r.get(15)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(fold(rows))
}

/// TVDB id of a show → who played an episode of it lately (any of its items: the HD and the 4K copy are one show).
pub const FOLLOWERS_SQL: &str = "SELECT x.value, p.user_id, COALESCE(u.name, MAX(p.user_name))
     FROM item_external x JOIN playbacks p ON p.series_id = x.item_id LEFT JOIN users u ON u.id = p.user_id
     WHERE x.source = 'Tvdb' AND p.ended_at >= ?1 AND x.value IN (SELECT DISTINCT CAST(tvdb_id AS TEXT) FROM upcoming WHERE kind = 'episode' AND tvdb_id IS NOT NULL)
     GROUP BY x.value, p.user_id";

fn followers(conn: &Connection) -> Result<HashMap<i64, BTreeMap<String, String>>> {
    let since = crate::db::now() - FOLLOW_DAYS * 86_400;
    let mut out: HashMap<i64, BTreeMap<String, String>> = HashMap::new();
    let mut stmt = conn.prepare_cached(FOLLOWERS_SQL)?;
    let rows = stmt.query_map([since], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?;
    for row in rows {
        let (tvdb, user_id, name) = row?;
        if let Ok(tvdb) = tvdb.parse::<i64>() {
            out.entry(tvdb).or_default().insert(user_id, name);
        }
    }
    Ok(out)
}

fn entry_json(e: &Entry) -> Value {
    // A title that is in the library has its poster there; one that is not yet only Sonarr or Radarr can show.
    let poster = match &e.item_id {
        Some(id) => json!({ "item_id": id }),
        None => json!({ "service_id": e.service_id, "media_id": e.arr_media_id }),
    };
    json!({
        "kind": e.kind, "release": e.release, "day": e.day, "at": e.at, "series_title": e.series_title, "title": e.title, "season": e.season, "episode": e.episode,
        "finale": e.finale, "year": e.year, "has_file": e.has_file, "item_id": e.item_id, "poster": poster,
    })
}

/// The agenda for one viewer. `subject` is whose shows "you follow" refers to; `everyone` adds who else does.
pub fn upcoming_json(conn: &Connection, days: i64, subject: &str, everyone: bool, mine: bool) -> Result<Vec<Value>> {
    let follows = followers(conn)?;
    let mut out = vec![];
    for e in entries(conn, days, None)? {
        let who = e.tvdb_id.filter(|_| e.kind == "episode").and_then(|t| follows.get(&t));
        let you = who.is_some_and(|w| w.contains_key(subject));
        if mine && !you {
            continue;
        }
        let mut v = entry_json(&e);
        v["you_follow"] = json!(you);
        if everyone {
            let mut names: Vec<&String> = who.map(|w| w.values().collect()).unwrap_or_default();
            names.sort_by_key(|n| n.to_lowercase());
            v["followers"] = json!(names.len());
            v["follower_names"] = json!(names);
        }
        out.push(v);
    }
    Ok(out)
}

/// The next episodes of one show, for its page. Says nothing about people.
pub fn upcoming_for_item(conn: &Connection, item_id: &str) -> Result<Vec<Value>> {
    Ok(entries(conn, 90, Some(item_id))?.iter().take(12).map(entry_json).collect())
}

#[derive(Deserialize)]
pub struct UpcomingQuery {
    days: Option<i64>,
    user_id: Option<String>,
    /// Only what the person follows.
    mine: Option<bool>,
}

pub async fn upcoming(State(app): State<App>, user: AuthUser, Query(q): Query<UpcomingQuery>) -> ApiResult {
    let days = q.days.unwrap_or(14).clamp(1, 90);
    // Whose shows: the caller's, or (with "see everyone") the person asked for.
    let subject = pinned_user(&user, q.user_id.as_deref()).unwrap_or_else(|| user.id.clone());
    let (everyone, mine) = (user.perms.see_everyone, q.mine.unwrap_or(false));
    let who = subject.clone();
    let list = app.db.call(move |c| upcoming_json(c, days, &who, everyone, mine)).await?;
    Ok(Json(json!({ "days": days, "user_id": subject, "entries": list })))
}

// ---------------------------------------------------------------- requests

/// Seerr's two numbers as one word. A request that vanished from Seerr stays "available" if it had arrived.
const STATE_SQL: &str = "CASE WHEN r.removed_at IS NOT NULL AND r.media_status <> 5 THEN 'removed'
      WHEN r.status = 3 THEN 'declined' WHEN r.status = 4 THEN 'failed'
      WHEN r.media_status = 5 THEN 'available' WHEN r.media_status = 4 THEN 'partial'
      WHEN r.status = 1 THEN 'pending' WHEN r.media_status = 3 THEN 'processing' ELSE 'approved' END";

/// Every request with every copy of its title in the library (`ri`), and what was played of it *after* it was
/// asked for (`w`): by the requester and by anyone. Each branch is driven through an index: a TMDB number means
/// one thing for a film and another for a show, so the item's type is always part of the match, and a show
/// request only counts episodes of the seasons that were asked for. `?1` is the shortest play that counts.
const WATCHED_CTE: &str = "WITH ri AS (
      SELECT r.service_id, r.request_id, x.item_id FROM requests r JOIN item_external x ON x.source = 'Tmdb' AND x.value = CAST(r.tmdb_id AS TEXT) JOIN items i ON i.id = x.item_id AND i.type = 'Movie' WHERE r.media_type = 'movie'
      UNION SELECT r.service_id, r.request_id, x.item_id FROM requests r JOIN item_external x ON x.source = 'Imdb' AND x.value = r.imdb_id JOIN items i ON i.id = x.item_id AND i.type = 'Movie' WHERE r.media_type = 'movie'
      UNION SELECT r.service_id, r.request_id, x.item_id FROM requests r JOIN item_external x ON x.source = 'Tvdb' AND x.value = CAST(r.tvdb_id AS TEXT) JOIN items i ON i.id = x.item_id AND i.type = 'Series' WHERE r.media_type = 'tv'
      UNION SELECT r.service_id, r.request_id, x.item_id FROM requests r JOIN item_external x ON x.source = 'Tmdb' AND x.value = CAST(r.tmdb_id AS TEXT) JOIN items i ON i.id = x.item_id AND i.type = 'Series' WHERE r.media_type = 'tv'
    ), plays AS (
      SELECT r.service_id, r.request_id, p.user_id = r.user_id AS mine, p.started_at
      FROM requests r JOIN ri ON ri.service_id = r.service_id AND ri.request_id = r.request_id JOIN playbacks p ON p.item_id = ri.item_id
      WHERE r.media_type = 'movie' AND p.started_at >= r.requested_at AND p.duration_s >= ?1
      UNION ALL
      SELECT r.service_id, r.request_id, p.user_id = r.user_id, p.started_at
      FROM requests r JOIN ri ON ri.service_id = r.service_id AND ri.request_id = r.request_id JOIN playbacks p ON p.series_id = ri.item_id
      WHERE r.media_type = 'tv' AND p.started_at >= r.requested_at AND p.duration_s >= ?1
        AND (r.seasons = '[]' OR p.season_number IN (SELECT value FROM json_each(r.seasons)))
    ), w AS (
      SELECT service_id, request_id, COALESCE(SUM(mine), 0) AS plays_mine, MIN(CASE WHEN mine THEN started_at END) AS first_mine, COUNT(*) AS plays_any, MIN(started_at) AS first_any
      FROM plays GROUP BY service_id, request_id
    )";

const REQUEST_SORTS: [(&str, &str); 6] = [
    ("when", "r.requested_at"),
    ("title", "COALESCE(r.title, '') COLLATE NOCASE"),
    ("user", "COALESCE(u.name, r.seerr_user_name) COLLATE NOCASE"),
    ("state", "state"),
    ("arrived", "r.available_at - r.requested_at"),
    ("watched", "w.first_mine"),
];

/// A play shorter than this is a click, not watching.
fn shortest_play(app: &App) -> i64 {
    app.settings().min_play_s.max(120)
}

struct Seen {
    everyone: bool,
    /// Whose requests: `Some` pins the list to one person (always, without "see everyone").
    who: Option<String>,
}

impl Seen {
    fn of(user: &AuthUser, asked: Option<&str>) -> Self {
        Seen { everyone: user.perms.see_everyone, who: pinned_user(user, asked) }
    }
    /// The `WHERE` that everything about requests goes through. A request nobody could be linked to has no
    /// `user_id`, so it can only ever match when nobody in particular is asked for, which needs "see everyone".
    fn clause(&self) -> &'static str {
        if self.who.is_some() { "r.user_id = ?2" } else { "?2 IS NULL" }
    }
}

fn request_json(r: &crate::db::rusqlite::Row, everyone: bool) -> crate::db::rusqlite::Result<Value> {
    let (service_id, request_id): (i64, i64) = (r.get("service_id")?, r.get("request_id")?);
    let (item_id, arr_service, arr_media): (Option<String>, Option<i64>, Option<i64>) = (r.get("item_id")?, r.get("arr_service_id")?, r.get("arr_media_id")?);
    let poster = match (&item_id, arr_service, arr_media) {
        (Some(id), _, _) => json!({ "item_id": id }),
        (None, Some(s), Some(m)) => json!({ "service_id": s, "media_id": m }),
        _ => Value::Null,
    };
    let (requested_at, available_at): (i64, Option<i64>) = (r.get("requested_at")?, r.get("available_at")?);
    let seasons: String = r.get("seasons")?;
    let mut v = json!({
        "id": format!("{service_id}:{request_id}"), "media_type": r.get::<_, String>("media_type")?, "title": r.get::<_, Option<String>>("title")?, "year": r.get::<_, Option<i64>>("year")?,
        "tmdb_id": r.get::<_, Option<i64>>("tmdb_id")?, "seasons": serde_json::from_str::<Value>(&seasons).unwrap_or_else(|_| json!([])), "is_4k": r.get::<_, bool>("is_4k")?,
        "tvdb_id": r.get::<_, Option<i64>>("tvdb_id")?,
        "state": r.get::<_, String>("state")?, "requested_at": requested_at, "available_at": available_at, "arrived_after_s": available_at.map(|a| (a - requested_at).max(0)),
        "user_id": r.get::<_, Option<String>>("user_id")?, "user_name": r.get::<_, Option<String>>("user_name")?, "has_image": r.get::<_, Option<bool>>("has_image")?.unwrap_or(false),
        "item_id": item_id, "poster": poster,
        "watched": r.get::<_, Option<i64>>("plays_mine")?.unwrap_or(0) > 0, "plays": r.get::<_, Option<i64>>("plays_mine")?.unwrap_or(0), "first_play_at": r.get::<_, Option<i64>>("first_mine")?,
    });
    if everyone {
        v["watched_by_anyone"] = json!(r.get::<_, Option<i64>>("plays_any")?.unwrap_or(0) > 0);
        v["plays_by_anyone"] = json!(r.get::<_, Option<i64>>("plays_any")?.unwrap_or(0));
    }
    Ok(v)
}

const REQUEST_COLS: &str = "r.service_id, r.request_id, r.media_type, r.title, r.year, r.tmdb_id, r.tvdb_id, r.seasons, r.is_4k, r.requested_at, r.available_at, r.user_id, r.item_id, r.arr_service_id, r.arr_media_id,
      COALESCE(u.name, r.seerr_user_name) AS user_name, (u.image_tag IS NOT NULL) AS has_image, w.plays_mine, w.first_mine, w.plays_any";

#[derive(Deserialize)]
pub struct RequestsQuery {
    /// open | arrived | declined | all (default)
    status: Option<String>,
    user_id: Option<String>,
    q: Option<String>,
    sort: Option<String>,
    dir: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

fn requests_page(conn: &Connection, seen: &Seen, q: &RequestsQuery, min_play: i64) -> Result<Value> {
    let per_page = q.per_page.unwrap_or(25).clamp(1, 100);
    let page = q.page.unwrap_or(1).clamp(1, 100_000);
    let states = match q.status.as_deref() {
        Some("open") => "state IN ('pending', 'approved', 'processing', 'partial')",
        Some("arrived") => "state = 'available'",
        Some("declined") => "state IN ('declined', 'failed', 'removed')",
        _ => "1 = 1",
    };
    let needle = q.q.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(|s| format!("%{}%", s.chars().take(80).collect::<String>().replace('%', "").replace('_', " ")));
    let from = format!(
        "FROM (SELECT r.*, {STATE_SQL} AS state FROM requests r) r LEFT JOIN users u ON u.id = r.user_id LEFT JOIN w ON w.service_id = r.service_id AND w.request_id = r.request_id
         WHERE {} AND {states} AND (?3 IS NULL OR r.title LIKE ?3)",
        seen.clause()
    );
    let args = params![min_play, seen.who, needle];
    let total: i64 = conn.query_row(&format!("{WATCHED_CTE} SELECT COUNT(*) {from}"), args, |r| r.get(0))?;
    let order = crate::stats::order_by(&REQUEST_SORTS, q.sort.as_deref(), q.dir.as_deref(), "r.requested_at DESC, r.request_id DESC");
    let sql = format!("{WATCHED_CTE} SELECT {REQUEST_COLS}, r.state {from} ORDER BY {order} LIMIT {per_page} OFFSET {}", (page - 1) * per_page);
    let rows: Vec<Value> = conn.prepare(&sql)?.query_map(args, |r| request_json(r, seen.everyone))?.collect::<Result<_, _>>()?;
    Ok(json!({ "rows": rows, "total": total, "page": page, "per_page": per_page }))
}

pub async fn requests(State(app): State<App>, user: AuthUser, Query(q): Query<RequestsQuery>) -> ApiResult {
    let seen = Seen::of(&user, q.user_id.as_deref());
    let min_play = shortest_play(&app);
    let mut out = app.db.call(move |c| requests_page(c, &seen, &q, min_play)).await?;
    // How far along it is comes from the live queue, which is memory and not the database.
    if let Some(rows) = out["rows"].as_array_mut() {
        for row in rows {
            crate::downloads::attach_progress(&app, row);
        }
    }
    Ok(Json(out))
}

/// One title one person asked for, however many requests that took: (arrived, watched by them, still open, watched by anyone).
type Wish = (bool, bool, bool, bool);
/// One person in the "who asks, who watches" list: name, Jellyfin id if known, titles asked for, arrived, watched.
type Asker = (String, Option<String>, usize, usize, usize);

fn median(mut v: Vec<i64>) -> Option<i64> {
    if v.is_empty() {
        return None;
    }
    v.sort_unstable();
    let mid = v.len() / 2;
    Some(if v.len() % 2 == 1 { v[mid] } else { (v[mid - 1] + v[mid]) / 2 })
}

/// The figures above the list, from the same rows the list may show and no others.
fn summary(conn: &Connection, seen: &Seen, days: i64, min_play: i64) -> Result<Value> {
    let since = if days > 0 { crate::db::now() - days * 86_400 } else { 0 };
    let sql = format!(
        "{WATCHED_CTE} SELECT {REQUEST_COLS}, r.state FROM (SELECT r.*, {STATE_SQL} AS state FROM requests r) r LEFT JOIN users u ON u.id = r.user_id
         LEFT JOIN w ON w.service_id = r.service_id AND w.request_id = r.request_id WHERE {} AND r.requested_at >= ?3 ORDER BY r.requested_at DESC",
        seen.clause()
    );
    let rows: Vec<Value> = conn.prepare(&sql)?.query_map(params![min_play, seen.who, since], |r| request_json(r, true))?.collect::<Result<_, _>>()?;

    // The same film asked for in HD and in 4K is one wish: count titles per person, not rows.
    let mut titles: BTreeMap<(String, String, i64), Wish> = BTreeMap::new();
    for r in &rows {
        let who = r["user_id"].as_str().or_else(|| r["user_name"].as_str()).unwrap_or("").to_string();
        let key = (who, r["media_type"].as_str().unwrap_or("").to_string(), r["tmdb_id"].as_i64().unwrap_or_else(|| -r["requested_at"].as_i64().unwrap_or(0)));
        let t = titles.entry(key).or_default();
        let state = r["state"].as_str().unwrap_or("");
        t.0 |= state == "available";
        t.1 |= r["watched"].as_bool().unwrap_or(false);
        t.2 |= matches!(state, "pending" | "approved" | "processing" | "partial");
        t.3 |= r["watched_by_anyone"].as_bool().unwrap_or(false);
    }
    let count = |f: fn(&Wish) -> bool| titles.values().filter(|t| f(t)).count();
    let arrive: Vec<i64> = rows.iter().filter_map(|r| r["arrived_after_s"].as_i64()).collect();

    // Time to arrive, month by month (the month it was asked for in).
    let mut months: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    for r in &rows {
        if let (Some(at), Some(took)) = (r["requested_at"].as_i64(), r["arrived_after_s"].as_i64())
            && let Some(d) = chrono::DateTime::from_timestamp(at, 0)
        {
            months.entry(d.with_timezone(&chrono::Local).format("%Y-%m").to_string()).or_default().push(took);
        }
    }
    let trend: Vec<Value> = months.into_iter().rev().take(12).collect::<Vec<_>>().into_iter().rev().map(|(m, v)| json!({ "month": m, "arrived": v.len(), "median_s": median(v) })).collect();

    // Arrived two weeks ago or more, and never played: by the person who asked, or (for those who may know) by anybody.
    let settled = crate::db::now() - 14 * 86_400;
    let mut listed: HashSet<(String, i64)> = HashSet::new();
    let never: Vec<Value> = rows
        .iter()
        .filter(|r| r["state"] == "available" && r["available_at"].as_i64().is_some_and(|a| a <= settled))
        .filter(|r| !r[if seen.everyone { "watched_by_anyone" } else { "watched" }].as_bool().unwrap_or(false))
        // One wish, one line: the same film asked for in HD and in 4K is not two disappointments.
        .filter(|r| listed.insert((r["media_type"].as_str().unwrap_or("").to_string(), r["tmdb_id"].as_i64().unwrap_or(0))))
        .take(15)
        .cloned()
        .collect();

    let mut out = json!({
        "days": days,
        "totals": { "requests": rows.len(), "titles": titles.len(), "open": count(|t| t.2 && !t.0), "arrived": count(|t| t.0), "watched": count(|t| t.0 && t.1) },
        "median_arrive_s": median(arrive), "trend": trend, "never_played": never,
    });
    if seen.everyone {
        out["totals"]["watched_by_anyone"] = json!(count(|t| t.0 && t.3));
    }
    // Who asks, and who watches what they asked for. Only ever for someone who may see everyone, looking at everyone.
    if seen.everyone && seen.who.is_none() {
        let mut people: BTreeMap<String, Asker> = BTreeMap::new();
        for ((who, _, _), t) in &titles {
            let name = rows.iter().find(|r| r["user_id"].as_str().or_else(|| r["user_name"].as_str()) == Some(who.as_str()));
            let e = people.entry(who.clone()).or_insert_with(|| (name.and_then(|r| r["user_name"].as_str()).unwrap_or("Unknown").to_string(), name.and_then(|r| r["user_id"].as_str()).map(str::to_string), 0, 0, 0));
            e.2 += 1;
            e.3 += t.0 as usize;
            e.4 += (t.0 && t.1) as usize;
        }
        let mut list: Vec<Value> = people.into_values().map(|(name, id, n, arrived, watched)| json!({ "user_id": id, "user_name": name, "requests": n, "arrived": arrived, "watched": watched })).collect();
        list.sort_by(|a, b| b["requests"].as_u64().cmp(&a["requests"].as_u64()).then_with(|| a["user_name"].as_str().unwrap_or("").to_lowercase().cmp(&b["user_name"].as_str().unwrap_or("").to_lowercase())));
        out["people"] = json!(list);
    }
    if !seen.everyone {
        // What a plain user gets back never mentions what anybody else did.
        if let Some(list) = out["never_played"].as_array_mut() {
            for r in list {
                if let Some(o) = r.as_object_mut() {
                    o.remove("watched_by_anyone");
                    o.remove("plays_by_anyone");
                }
            }
        }
    }
    Ok(out)
}

#[derive(Deserialize)]
pub struct SummaryQuery {
    days: Option<i64>,
    user_id: Option<String>,
}

pub async fn requests_summary(State(app): State<App>, user: AuthUser, Query(q): Query<SummaryQuery>) -> ApiResult {
    let seen = Seen::of(&user, q.user_id.as_deref());
    let (days, min_play) = (q.days.unwrap_or(0).clamp(0, 36_500), shortest_play(&app));
    Ok(Json(app.db.call(move |c| summary(c, &seen, days, min_play)).await?))
}

/// The one request behind a title's page, if it is the caller's to know about: their own, or anyone's with "see everyone".
pub fn request_for_item(conn: &Connection, item_id: &str, everyone: bool, caller: &str, min_play: i64) -> Result<Option<Value>> {
    let seen = Seen { everyone, who: if everyone { None } else { Some(caller.to_string()) } };
    let sql = format!(
        "{WATCHED_CTE} SELECT {REQUEST_COLS}, r.state FROM (SELECT r.*, {STATE_SQL} AS state FROM requests r) r LEFT JOIN users u ON u.id = r.user_id
         LEFT JOIN w ON w.service_id = r.service_id AND w.request_id = r.request_id
         WHERE {} AND EXISTS (SELECT 1 FROM ri WHERE ri.service_id = r.service_id AND ri.request_id = r.request_id AND ri.item_id = ?3) ORDER BY r.requested_at LIMIT 1",
        seen.clause()
    );
    Ok(conn.prepare(&sql)?.query_map(params![min_play, seen.who, item_id], |r| request_json(r, seen.everyone))?.next().transpose()?)
}

// ---------------------------------------------------------------- what came in, over time

#[derive(Deserialize)]
pub struct HistoryQuery {
    days: Option<i64>,
}

/// Everything about the download history: how much arrived per day, what failed, and where it came from.
/// Behind `see_downloads`, like the live queue: indexer names and release titles are the same kind of thing.
pub async fn download_history(State(app): State<App>, crate::auth::DownloadsViewer(_): crate::auth::DownloadsViewer, Query(q): Query<HistoryQuery>) -> ApiResult {
    let days = q.days.unwrap_or(30).clamp(1, 3650);
    let out = app
        .db
        .call(move |c| {
            let since: i64 = c.query_row("SELECT CAST(strftime('%s', date('now', 'localtime', ?1), 'utc') AS INTEGER)", [format!("-{} days", days - 1)], |r| r.get(0))?;
            let totals = crate::stats::one_json(
                c,
                "SELECT COALESCE(SUM(event = 'imported'), 0) AS imported, COALESCE(SUM(event = 'grabbed'), 0) AS grabbed, COALESCE(SUM(event = 'failed'), 0) AS failed,
                        COALESCE(SUM(CASE WHEN event = 'imported' THEN size_bytes END), 0) AS size_bytes
                 FROM grabs WHERE at >= ?1",
                &[since.into()],
            )?
            .unwrap_or_default();
            // Gap-free days, like every other chart here.
            let rows = crate::stats::rows_json(
                c,
                "SELECT date(at, 'unixepoch', 'localtime') AS day, COALESCE(SUM(event = 'imported'), 0) AS imported,
                        COALESCE(SUM(CASE WHEN event = 'imported' THEN size_bytes END), 0) AS size_bytes, COALESCE(SUM(event = 'failed'), 0) AS failed
                 FROM grabs WHERE at >= ?1 GROUP BY day",
                &[since.into()],
            )?;
            let found: HashMap<String, Value> = rows.into_iter().filter_map(|r| Some((r.get("day")?.as_str()?.to_string(), Value::Object(r)))).collect();
            let first: String = c.query_row("SELECT date(?1, 'unixepoch', 'localtime')", [since], |r| r.get(0))?;
            let mut daily = vec![];
            let mut day = chrono::NaiveDate::parse_from_str(&first, "%Y-%m-%d").unwrap_or_default();
            let today: String = c.query_row("SELECT date('now', 'localtime')", [], |r| r.get(0))?;
            let last = chrono::NaiveDate::parse_from_str(&today, "%Y-%m-%d").unwrap_or_default();
            while day <= last {
                let key = day.format("%Y-%m-%d").to_string();
                daily.push(found.get(&key).cloned().unwrap_or_else(|| json!({ "day": key, "imported": 0, "size_bytes": 0, "failed": 0 })));
                day += chrono::Duration::days(1);
            }
            let bucket = |expr: &str| -> Result<Vec<Value>> {
                Ok(crate::stats::rows_json(
                    c,
                    &format!(
                        "SELECT {expr} AS name, COUNT(*) AS count, COALESCE(SUM(size_bytes), 0) AS size_bytes FROM grabs
                         WHERE at >= ?1 AND event = 'imported' AND {expr} IS NOT NULL AND {expr} <> '' GROUP BY 1 ORDER BY count DESC, name LIMIT 12"
                    ),
                    &[since.into()],
                )?
                .into_iter()
                .map(Value::Object)
                .collect())
            };
            let failures = crate::stats::rows_json(
                c,
                "SELECT at, COALESCE(title, source) AS title, source, indexer, media_type FROM grabs WHERE at >= ?1 AND event = 'failed' ORDER BY at DESC LIMIT 20",
                &[since.into()],
            )?;
            Ok(json!({ "days": days, "totals": totals, "daily": daily,
                       "indexers": bucket("indexer")?, "quality": bucket("quality")?, "clients": bucket("client")?, "protocols": bucket("protocol")?,
                       "failures": failures.into_iter().map(Value::Object).collect::<Vec<_>>() }))
        })
        .await?;
    Ok(Json(out))
}

/// Is this poster one the caller could have been shown: on the calendar (which is everyone's), or on a request
/// they may see? Otherwise the proxy would let anyone walk through everything Sonarr and Radarr know by counting upwards.
pub fn poster_is_listed(conn: &Connection, service_id: i64, media_id: i64, user: &AuthUser) -> Result<bool> {
    if conn.prepare_cached("SELECT 1 FROM upcoming WHERE service_id = ?1 AND arr_media_id = ?2 LIMIT 1")?.exists(params![service_id, media_id])? {
        return Ok(true);
    }
    let mine = (!user.perms.see_everyone).then(|| user.id.clone());
    Ok(conn.prepare_cached("SELECT 1 FROM requests WHERE arr_service_id = ?1 AND arr_media_id = ?2 AND (?3 IS NULL OR user_id = ?3) LIMIT 1")?.exists(params![service_id, media_id, mine])?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        for m in crate::db::MIGRATIONS {
            c.execute_batch(m).unwrap();
        }
        c.execute_batch(
            "INSERT INTO services(id, kind, name, url, secret, created_at) VALUES (1, 'sonarr', 'Sonarr', 'http://nas:8989', 'k', 1), (2, 'sonarr', 'Sonarr anime', 'http://nas:8990', 'k', 1), (3, 'radarr', 'Radarr', 'http://nas:7878', 'k', 1);
             INSERT INTO users(id, name, is_admin, updated_at) VALUES ('ua', 'alice', 1, 1), ('ub', 'bob', 0, 1);
             INSERT INTO items(id, type, name, provider_ids, removed, updated_at) VALUES
                ('s-hd', 'Series', 'Low Orbit', '{\"Tvdb\":\"370001\",\"Imdb\":\"tt9900001\"}', 0, 1),
                ('s-4k', 'Series', 'Low Orbit', '{\"Tvdb\":\"370001\"}', 0, 1),
                ('s-gone', 'Series', 'Low Orbit', '{\"Tvdb\":\"370001\"}', 1, 1),
                ('m-1', 'Movie', 'Winterline', '{\"Tmdb\":\"990001\"}', 0, 1),
                ('e-1', 'Episode', 'Pilot', '{\"Tvdb\":\"370001\"}', 0, 1),
                ('broken', 'Series', 'Broken', 'not json', 0, 1);",
        )
        .unwrap();
        c
    }

    #[allow(clippy::too_many_arguments)]
    fn add(c: &Connection, service: i64, kind: &str, id: i64, release: &str, days_ahead: i64, tvdb: Option<i64>, tmdb: Option<i64>, has_file: bool) {
        let at = crate::db::now() + days_ahead * 86_400 + 3600;
        let day: Option<String> = (kind == "movie").then(|| c.query_row("SELECT date('now', 'localtime', ?1)", [format!("+{days_ahead} days")], |r| r.get(0)).unwrap());
        c.execute(
            "INSERT INTO upcoming(service_id, kind, external_id, release, at, day, series_title, title, season, episode, tvdb_id, tmdb_id, arr_media_id, has_file)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'A title', 2, ?3, ?8, ?9, 12, ?10)",
            params![service, kind, id, release, (kind == "episode").then_some(at), day, (kind == "episode").then_some("Low Orbit"), tvdb, tmdb, has_file],
        )
        .unwrap();
    }

    #[test]
    fn one_id_links_every_copy_in_the_library_and_nothing_that_is_gone() {
        let c = conn();
        add(&c, 1, "episode", 5, "air", 2, Some(370001), None, false);
        add(&c, 3, "movie", 7, "digital", 3, None, Some(990001), false);
        add(&c, 3, "movie", 8, "digital", 3, None, Some(123), false);
        link(&c).unwrap();
        let ext: Vec<(String, String)> = c.prepare("SELECT item_id, source FROM item_external ORDER BY 1, 2").unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect();
        assert_eq!(ext, [("m-1".into(), "Tmdb".into()), ("s-4k".into(), "Tvdb".into()), ("s-hd".into(), "Imdb".into()), ("s-hd".into(), "Tvdb".into())], "episodes, removed items and broken JSON stay out");
        let linked: Vec<Option<String>> = c.prepare("SELECT item_id FROM upcoming ORDER BY external_id").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
        assert_eq!(linked, [Some("s-4k".to_string()), Some("m-1".to_string()), None], "the lowest id, always the same one; an unknown film stays unlinked");
    }

    #[test]
    fn the_same_episode_from_two_instances_is_one_entry() {
        let c = conn();
        add(&c, 1, "episode", 5, "air", 2, Some(370001), None, false);
        add(&c, 2, "episode", 5, "air", 2, Some(370001), None, true);
        add(&c, 1, "episode", 6, "air", 9, Some(370001), None, false);
        add(&c, 3, "movie", 7, "digital", 3, None, Some(990001), false);
        add(&c, 3, "movie", 7, "physical", 40, None, Some(990001), false);
        add(&c, 1, "episode", 4, "air", -2, Some(370001), None, true);
        link(&c).unwrap();
        let week = entries(&c, 14, None).unwrap();
        assert_eq!(week.iter().map(|e| (e.kind.as_str(), e.episode, e.has_file)).collect::<Vec<_>>(), [("episode", Some(5), true), ("movie", Some(7), false), ("episode", Some(6), false)]);
        assert_eq!(entries(&c, 60, None).unwrap().len(), 4, "the physical release is further out; what aired two days ago is not upcoming");
        // A switched-off connection says nothing.
        c.execute("UPDATE services SET enabled = 0 WHERE id = 3", []).unwrap();
        assert!(entries(&c, 60, None).unwrap().iter().all(|e| e.kind == "episode"));
    }

    #[test]
    fn a_plain_user_learns_what_they_follow_and_nothing_about_anybody_else() {
        let c = conn();
        add(&c, 1, "episode", 5, "air", 2, Some(370001), None, false);
        add(&c, 3, "movie", 7, "digital", 3, None, Some(990001), false);
        link(&c).unwrap();
        let now = crate::db::now();
        // alice watches the 4K copy, bob watched the HD one last week; carol's play is from last year.
        c.execute_batch(&format!(
            "INSERT INTO playbacks(source, user_id, user_name, item_id, item_name, item_type, series_id, started_at, ended_at, duration_s) VALUES
                ('live', 'ua', 'alice', 'e-9', 'Episode 4', 'Episode', 's-4k', {recent}, {recent}, 1500),
                ('live', 'ub', 'bob', 'e-8', 'Episode 3', 'Episode', 's-hd', {recent}, {recent}, 1500),
                ('live', 'uc', 'carol', 'e-8', 'Episode 3', 'Episode', 's-hd', {old}, {old}, 1500);",
            recent = now - 7 * 86_400,
            old = now - 400 * 86_400
        ))
        .unwrap();
        let admin = upcoming_json(&c, 14, "ua", true, false).unwrap();
        assert_eq!((admin[0]["you_follow"].clone(), admin[0]["followers"].clone(), admin[0]["follower_names"].clone()), (json!(true), json!(2), json!(["alice", "bob"])));
        assert_eq!(admin[1]["followers"], json!(0), "nobody follows a film");

        let plain = upcoming_json(&c, 14, "ub", false, false).unwrap();
        assert_eq!(plain[0]["you_follow"], json!(true));
        for e in &plain {
            assert!(e.get("followers").is_none() && e.get("follower_names").is_none(), "a count is a name on a small server: {e}");
            assert!(!e.to_string().contains("alice"));
        }
        assert_eq!(upcoming_json(&c, 14, "uc", false, true).unwrap().len(), 0, "carol stopped watching long ago");
        assert_eq!(upcoming_json(&c, 14, "ub", false, true).unwrap().len(), 1, "only what bob follows: no films");

        // The show's own page: its episodes through either copy, and never a word about people.
        let page = upcoming_for_item(&c, "s-hd").unwrap();
        assert_eq!(page.len(), 1);
        assert!(page[0].get("you_follow").is_none() && page[0].get("followers").is_none());
        assert!(upcoming_for_item(&c, "m-1").unwrap().len() == 1 && upcoming_for_item(&c, "nothing").unwrap().is_empty());
    }

    #[test]
    fn followers_are_found_through_indexes() {
        let c = conn();
        let plan: Vec<String> = c.prepare(&format!("EXPLAIN QUERY PLAN {FOLLOWERS_SQL}")).unwrap().query_map([0], |r| r.get::<_, String>(3)).unwrap().map(Result::unwrap).collect();
        let plan = plan.join(" | ");
        assert!(plan.contains("idx_item_external") || plan.contains("item_external USING"), "{plan}");
        assert!(plan.contains("idx_pb_series"), "plays must be reached by series: {plan}");
        assert!(!plan.contains("SCAN p"), "{plan}");
    }

    #[test]
    fn only_listed_posters_are_served() {
        let c = conn();
        add(&c, 1, "episode", 5, "air", 2, Some(370001), None, false);
        let bob = plain("ub");
        assert!(poster_is_listed(&c, 1, 12, &bob).unwrap());
        assert!(!poster_is_listed(&c, 1, 13, &bob).unwrap() && !poster_is_listed(&c, 2, 12, &bob).unwrap());
    }

    fn plain(id: &str) -> AuthUser {
        AuthUser { id: id.into(), name: id.into(), is_admin: false, perms: crate::auth::Perms::default() }
    }
    fn everyone(id: &str) -> AuthUser {
        AuthUser { id: id.into(), name: id.into(), is_admin: false, perms: crate::auth::Perms { see_everyone: true, ..Default::default() } }
    }

    /// alice asked for a film (HD and 4K) and season 2 of a show; bob asked for a film; someone Seerr cannot place asked for another.
    fn with_requests() -> Connection {
        let c = conn();
        let now = crate::db::now();
        c.execute_batch(&format!(
            "INSERT INTO services(id, kind, name, url, secret, created_at) VALUES (9, 'seerr', 'Seerr', 'http://nas:5055', 'k', 1);
             INSERT INTO items(id, type, name, provider_ids, removed, updated_at) VALUES ('m-2', 'Movie', 'Glasshouse', '{{\"Tmdb\":\"370001\"}}', 0, 1);
             INSERT INTO requests(service_id, request_id, media_type, tmdb_id, tvdb_id, title, seasons, is_4k, status, media_status, requested_at, updated_at, media_added_at, jellyfin_user_id, seerr_user_name) VALUES
                (9, 1, 'movie', 990001, NULL, 'Winterline', '[]', 0, 2, 5, {t0}, {t0}, {t0} + 7200, 'ua', 'alice'),
                (9, 2, 'movie', 990001, NULL, 'Winterline', '[]', 1, 2, 5, {t0}, {t0}, {t0} + 9000, 'ua', 'alice'),
                (9, 3, 'tv', 370001, 370001, 'Low Orbit', '[2]', 0, 2, 5, {t0}, {t0}, {t0} + 3600, 'ua', 'alice'),
                (9, 4, 'movie', 370001, NULL, 'Glasshouse', '[]', 0, 2, 5, {t0}, {t0}, {t0} + 600, 'ub', 'bob'),
                (9, 5, 'movie', 555, NULL, 'Nobody Knows Who', '[]', 0, 1, 1, {t0}, {t0}, NULL, NULL, 'a stranger');
             INSERT INTO playbacks(source, user_id, user_name, item_id, item_name, item_type, series_id, season_number, started_at, ended_at, duration_s) VALUES
                ('live', 'ub', 'bob', 'm-1', 'Winterline', 'Movie', NULL, NULL, {t0} + 90000, {t0} + 95000, 5000),
                ('live', 'ua', 'alice', 'e-1', 'S1 episode', 'Episode', 's-hd', 1, {t0} + 90000, {t0} + 92000, 2000),
                ('live', 'ua', 'alice', 'e-2', 'S2 before asking', 'Episode', 's-4k', 2, {t0} - 5000, {t0} - 3000, 2000),
                ('live', 'ua', 'alice', 'm-2', 'Glasshouse', 'Movie', NULL, NULL, {t0} + 90000, {t0} + 90030, 30);",
            t0 = now - 40 * 86_400
        ))
        .unwrap();
        link(&c).unwrap();
        c
    }

    fn query() -> RequestsQuery {
        RequestsQuery { status: None, user_id: None, q: None, sort: None, dir: None, page: None, per_page: None }
    }

    #[test]
    fn a_plain_user_sees_their_own_requests_and_not_a_trace_of_anyone_elses() {
        let c = with_requests();
        let bob = Seen::of(&plain("ub"), Some("ua"));
        assert_eq!(bob.who.as_deref(), Some("ub"), "asking about alice gets bob");
        let page = requests_page(&c, &bob, &query(), 120).unwrap();
        assert_eq!((page["total"].clone(), page["rows"][0]["title"].clone()), (json!(1), json!("Glasshouse")));
        let text = page.to_string() + &summary(&c, &bob, 0, 120).unwrap().to_string();
        for leak in ["alice", "Winterline", "Low Orbit", "a stranger", "watched_by_anyone", "plays_by_anyone", "people"] {
            assert!(!text.contains(leak), "`{leak}` reached a plain user: {text}");
        }
        assert_eq!(summary(&c, &bob, 0, 120).unwrap()["totals"]["requests"], json!(1));

        // The title's page: bob's own request is there, alice's is not, for him. For someone who may see everyone it is.
        assert_eq!(request_for_item(&c, "m-2", false, "ub", 120).unwrap().unwrap()["user_name"], json!("bob"));
        assert!(request_for_item(&c, "m-1", false, "ub", 120).unwrap().is_none(), "somebody else's request shows on a title page");
        assert_eq!(request_for_item(&c, "m-1", true, "ub", 120).unwrap().unwrap()["user_name"], json!("alice"));

        // A request nobody could be linked to belongs to nobody's page, and only shows where everyone does.
        let all = requests_page(&c, &Seen::of(&everyone("ua"), None), &query(), 120).unwrap();
        assert_eq!(all["total"], json!(5));
        assert_eq!(requests_page(&c, &Seen::of(&everyone("ua"), Some("ub")), &query(), 120).unwrap()["total"], json!(1));
    }

    #[test]
    fn watched_means_by_the_requester_after_asking_the_seasons_asked_for_and_more_than_a_click() {
        let c = with_requests();
        let all = requests_page(&c, &Seen::of(&everyone("ua"), None), &RequestsQuery { sort: Some("when".into()), per_page: Some(100), ..query() }, 120).unwrap();
        let row = |title: &str, is_4k: bool| all["rows"].as_array().unwrap().iter().find(|r| r["title"] == title && r["is_4k"] == is_4k).unwrap().clone();
        // bob watched alice's film; she did not.
        assert_eq!((row("Winterline", false)["watched"].clone(), row("Winterline", false)["watched_by_anyone"].clone()), (json!(false), json!(true)));
        // She asked for season 2: an episode of season 1 does not count, nor one of season 2 from before she asked.
        assert_eq!((row("Low Orbit", false)["watched"].clone(), row("Low Orbit", false)["plays_by_anyone"].clone()), (json!(false), json!(0)));
        // TMDB 370001 is bob's film *and* the show's TVDB number: a show's plays are not a film's. And 30 seconds is a click.
        assert_eq!((row("Glasshouse", false)["watched"].clone(), row("Glasshouse", false)["plays_by_anyone"].clone()), (json!(false), json!(0)));
        assert_eq!(row("Glasshouse", false)["arrived_after_s"], json!(600));

        let s = summary(&c, &Seen::of(&everyone("ua"), None), 0, 120).unwrap();
        assert_eq!(s["totals"], json!({ "requests": 5, "titles": 4, "open": 1, "arrived": 3, "watched": 0, "watched_by_anyone": 1 }), "HD and 4K of one film are one wish");
        assert_eq!(s["median_arrive_s"], json!(5400), "600, 3600, 7200 and 9000 seconds");
        assert_eq!(s["people"][0], json!({ "user_id": "ua", "user_name": "alice", "requests": 2, "arrived": 2, "watched": 0 }));
        let never: Vec<&str> = s["never_played"].as_array().unwrap().iter().map(|r| r["title"].as_str().unwrap()).collect();
        assert!(never.contains(&"Low Orbit") && never.contains(&"Glasshouse") && !never.contains(&"Winterline"), "{never:?}");
        // Sorting only by known columns; anything else falls back to newest first.
        assert!(requests_page(&c, &Seen::of(&everyone("ua"), None), &RequestsQuery { sort: Some("r.secret; DROP TABLE requests".into()), ..query() }, 120).is_ok());
        assert_eq!(requests_page(&c, &Seen::of(&everyone("ua"), None), &RequestsQuery { status: Some("open".into()), ..query() }, 120).unwrap()["total"], json!(1));
    }

    #[test]
    fn a_poster_is_served_to_whoever_may_see_the_request_it_belongs_to() {
        let c = with_requests();
        c.execute("UPDATE requests SET arr_service_id = 3, arr_media_id = 77 WHERE request_id = 1", []).unwrap();
        assert!(poster_is_listed(&c, 3, 77, &plain("ua")).unwrap() && poster_is_listed(&c, 3, 77, &everyone("ub")).unwrap());
        assert!(!poster_is_listed(&c, 3, 77, &plain("ub")).unwrap(), "bob would learn what alice asked for");
    }
}
