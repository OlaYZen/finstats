//! What is arriving right now, from Sonarr's and Radarr's queues.
//!
//! They already talk to the download client, whichever it is, and report the same things for a torrent as for
//! a usenet download: what it is, how far along, and what went wrong on import. That is what finstats reads,
//! rather than each client's own API — three services to keep up with instead of six, and nothing to set up
//! twice. What is lost is what only a client knows (ratio, peers), and the speed, which is worked out here
//! instead: bytes that moved between two readings, divided by the time between them.
//!
//! Two things fall out of the queue's shape, and `fold` is where they live:
//! - A season pack is several records with one `downloadId`: one download, not five.
//! - A record without one (Sonarr is between clients, or the client forgot it) still describes itself.
//!
//! The snapshot is kept in memory and never in the database: it is worthless a minute later, and nothing here
//! touches the database per tick.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use axum::Json;
use axum::extract::{Query, State};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::DownloadsViewer;
use crate::db::rusqlite::Connection;
use crate::services::{self, Kind};
use crate::state::{ApiResult, App};

/// While somebody is looking, and while nobody is.
const WATCHED_EVERY_S: u64 = 5;
const IDLE_EVERY_S: u64 = 60;
/// A page still counts as being watched this long after it last asked.
const WATCHING_FOR_S: i64 = 20;
const PER_SERVICE_TIMEOUT: Duration = Duration::from_secs(4);
/// Never list more than this, whatever a client is doing.
const MAX_ROWS: usize = 500;

// ---------------------------------------------------------------- the two halves

/// One record of a Sonarr or Radarr queue.
#[derive(Debug, Clone, PartialEq)]
pub struct Queued {
    pub service_id: i64,
    pub service_name: String,
    pub arr_media_id: Option<i64>,
    pub download_id: Option<String>,
    /// What Sonarr or Radarr is waiting for: "Low Orbit · S03E09", "Northern Static".
    pub title: String,
    pub sub: Option<String>,
    pub media_type: &'static str,
    pub tmdb_id: Option<i64>,
    pub tvdb_id: Option<i64>,
    pub season: Option<i64>,
    pub size: i64,
    pub left: i64,
    pub eta_s: Option<i64>,
    pub state: &'static str,
    pub protocol: Option<String>,
    pub client: Option<String>,
    /// What the release is called: the queue's own title.
    pub release: Option<String>,
    pub error: Option<String>,
}

fn text(v: &Value) -> Option<String> {
    v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

/// `00:12:34` or `1.02:03:04` (a .NET timespan) in seconds.
pub fn timespan(v: &Value) -> Option<i64> {
    let raw = v.as_str()?;
    let (days, rest) = match raw.split_once('.') {
        Some((d, rest)) if d.parse::<i64>().is_ok() && rest.contains(':') => (d.parse().unwrap_or(0), rest),
        _ => (0, raw),
    };
    let parts: Vec<i64> = rest.split(':').map(|p| p.split('.').next().unwrap_or("0").parse().unwrap_or(0)).collect();
    let [h, m, s] = parts[..] else { return None };
    Some(((days * 24 + h) * 60 + m) * 60 + s)
}

/// What a queue record is doing, in finstats' words. Importing and failing matter: that is where things get stuck.
pub fn queue_state(status: &str, tracked_state: &str, tracked_status: &str) -> &'static str {
    match tracked_state {
        "importPending" | "importBlocked" | "importing" => return "importing",
        "failed" | "failedPending" => return "failed",
        _ => {}
    }
    match status {
        "downloading" => "downloading",
        "queued" | "delay" => "queued",
        "paused" => "paused",
        "completed" => "importing",
        "failed" => "failed",
        "warning" | "downloadClientUnavailable" => {
            if tracked_status == "error" {
                "failed"
            } else {
                "stalled"
            }
        }
        _ => "unknown",
    }
}

fn queue_row(svc_id: i64, svc_name: &str, kind: Kind, r: &Value) -> Option<Queued> {
    let size = r["size"].as_f64().unwrap_or(0.0).max(0.0) as i64;
    let left = r["sizeleft"].as_f64().unwrap_or(0.0).max(0.0) as i64;
    let error = text(&r["errorMessage"]).or_else(|| r["statusMessages"].as_array().and_then(|a| a.first().and_then(|m| m["title"].as_str().map(str::to_string))));
    let (series, movie) = (&r["series"], &r["movie"]);
    let (title, sub, media_type, tmdb, tvdb, arr_media_id) = if kind == Kind::Sonarr {
        let episode = &r["episode"];
        let code = match (r["seasonNumber"].as_i64().or_else(|| episode["seasonNumber"].as_i64()), episode["episodeNumber"].as_i64()) {
            (Some(s), Some(e)) => Some(format!("S{s:02}E{e:02}")),
            (Some(s), None) => Some(format!("Season {s}")),
            _ => None,
        };
        let name = text(&series["title"]).or_else(|| text(&r["title"])).unwrap_or_else(|| "Unknown".into());
        let sub = [code, text(&episode["title"])].into_iter().flatten().collect::<Vec<_>>().join(" · ");
        (name, (!sub.is_empty()).then_some(sub), "tv", series["tmdbId"].as_i64(), series["tvdbId"].as_i64(), r["seriesId"].as_i64())
    } else {
        let name = text(&movie["title"]).or_else(|| text(&r["title"])).unwrap_or_else(|| "Unknown".into());
        (name, movie["year"].as_i64().map(|y| y.to_string()), "movie", movie["tmdbId"].as_i64(), None, r["movieId"].as_i64())
    };
    Some(Queued {
        service_id: svc_id,
        service_name: svc_name.to_string(),
        arr_media_id: arr_media_id.filter(|n| *n > 0),
        download_id: text(&r["downloadId"]).map(|d| d.to_ascii_lowercase()),
        title,
        sub,
        media_type,
        tmdb_id: tmdb.filter(|n| *n > 0),
        tvdb_id: tvdb.filter(|n| *n > 0),
        season: r["seasonNumber"].as_i64(),
        size,
        left,
        eta_s: timespan(&r["timeleft"]),
        state: queue_state(r["status"].as_str().unwrap_or(""), r["trackedDownloadState"].as_str().unwrap_or(""), r["trackedDownloadStatus"].as_str().unwrap_or("")),
        protocol: text(&r["protocol"]),
        client: text(&r["downloadClient"]),
        release: text(&r["title"]),
        error,
    })
}

async fn read_queue(app: &App, svc: &services::Service) -> Result<Vec<Queued>> {
    let mut query = vec![("pageSize", "200".to_string()), ("page", "1".to_string())];
    if svc.kind == Kind::Sonarr {
        query.push(("includeSeries", "true".into()));
        query.push(("includeEpisode", "true".into()));
        query.push(("includeUnknownSeriesItems", "true".into()));
    } else {
        query.push(("includeMovie", "true".into()));
        query.push(("includeUnknownMovieItems", "true".into()));
    }
    let body = services::get_json(app, svc, "/api/v3/queue", &query).await?;
    let records = body["records"].as_array().cloned().unwrap_or_default();
    Ok(records.iter().filter_map(|r| queue_row(svc.id, &svc.name, svc.kind, r)).collect())
}
// ---------------------------------------------------------------- one download out of its records

#[derive(Debug, Clone, PartialEq)]
pub struct Download {
    pub key: String,
    pub title: String,
    pub sub: Option<String>,
    pub state: &'static str,
    pub progress: f64,
    pub size: i64,
    /// Bytes still to come: what the speed is worked out from.
    pub left: i64,
    pub eta_s: Option<i64>,
    /// Worked out from two readings, because a queue reports no speed.
    pub down_bps: i64,
    /// What the release is called. Only ever shown to somebody who may see downloads.
    pub release: Option<String>,
    /// The download client Sonarr or Radarr handed it to.
    pub client: Option<String>,
    pub protocol: Option<String>,
    pub service_name: Option<String>,
    pub arr: Option<(i64, i64)>,
    pub media_type: Option<&'static str>,
    pub tmdb_id: Option<i64>,
    pub tvdb_id: Option<i64>,
    pub seasons: Vec<i64>,
    /// How many queue records share this download: a season pack is one download and many episodes.
    pub parts: usize,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Totals {
    pub down_bps: i64,
    pub downloading: usize,
    pub queued: usize,
    pub importing: usize,
    pub failed: usize,
}

/// The words finstats uses for what a download is doing, worst first: what needs a person's attention is on top.
pub const STATES: [&str; 7] = ["failed", "importing", "downloading", "stalled", "queued", "paused", "checking"];

fn rank(state: &str) -> usize {
    STATES.iter().position(|s| *s == state).unwrap_or(STATES.len())
}

/// The queue records of every connected Sonarr and Radarr as one list. Pure: the loop only feeds it.
pub fn fold(queue: Vec<Queued>) -> (Vec<Download>, Totals) {
    let mut out: Vec<Download> = vec![];
    let mut at: HashMap<String, usize> = HashMap::new();
    for q in queue {
        // Several records with one download id are one download: a season pack, and the episodes it holds.
        // Without an id (a client that forgot it) a record can only stand for itself.
        let key = match &q.download_id {
            Some(id) => format!("{id}:{}", q.service_id),
            None => format!("arr:{}:{}:{}", q.service_id, q.title, q.sub.as_deref().unwrap_or("")),
        };
        if let Some(&i) = at.get(&key) {
            let d: &mut Download = &mut out[i];
            d.parts += 1;
            if let Some(s) = q.season
                && !d.seasons.contains(&s)
            {
                d.seasons.push(s);
            }
            d.size = d.size.max(q.size);
            d.left = d.left.max(q.left);
            if rank(q.state) < rank(d.state) {
                d.state = q.state;
            }
            d.error = d.error.take().or(q.error);
            continue;
        }
        at.insert(key.clone(), out.len());
        out.push(Download {
            key,
            title: q.title,
            sub: q.sub,
            state: q.state,
            progress: if q.size > 0 { ((q.size - q.left) as f64 / q.size as f64).clamp(0.0, 1.0) } else { 0.0 },
            size: q.size,
            left: q.left,
            eta_s: q.eta_s,
            down_bps: 0,
            release: q.release,
            client: q.client,
            protocol: q.protocol,
            service_name: Some(q.service_name),
            arr: q.arr_media_id.map(|m| (q.service_id, m)),
            media_type: Some(q.media_type),
            tmdb_id: q.tmdb_id,
            tvdb_id: q.tvdb_id,
            seasons: q.season.into_iter().collect(),
            parts: 1,
            error: q.error,
        });
    }

    // A pack knows what it holds only once every one of its records has been seen.
    let mut totals = Totals::default();
    for d in &mut out {
        if d.parts > 1 {
            let episodes = format!("{} episodes", d.parts);
            d.seasons.sort_unstable();
            d.sub = Some(match d.seasons.as_slice() {
                [s] => format!("Season {s} · {episodes}"),
                [] => episodes,
                seasons => format!("{} seasons · {episodes}", seasons.len()),
            });
        }
        if d.size > 0 {
            d.progress = ((d.size - d.left) as f64 / d.size as f64).clamp(0.0, 1.0);
        }
        match d.state {
            "downloading" | "stalled" => totals.downloading += 1,
            "queued" | "paused" | "checking" => totals.queued += 1,
            "importing" => totals.importing += 1,
            "failed" => totals.failed += 1,
            _ => {}
        }
    }
    debug_assert!(out.iter().all(|d| STATES.contains(&d.state) || d.state == "unknown"), "a state nobody has a word for");
    out.sort_by(|a, b| rank(a.state).cmp(&rank(b.state)).then(b.progress.partial_cmp(&a.progress).unwrap_or(std::cmp::Ordering::Equal)).then_with(|| a.title.cmp(&b.title)));
    out.truncate(MAX_ROWS);
    (out, totals)
}

/// How fast it is going: what a queue never says. Bytes that moved since the last reading, divided by the
/// time between them. Nothing is claimed from a reading that is too close, too far apart, or of another size
/// (an upgrade replacing a download keeps the id but starts again).
pub fn speeds(rows: &mut [Download], before: &HashMap<String, (i64, i64)>, now: i64) -> HashMap<String, (i64, i64)> {
    for d in rows.iter_mut() {
        if let Some((left, at)) = before.get(&d.key) {
            let (moved, seconds) = (left - d.left, now - at);
            if (2..=180).contains(&seconds) && moved > 0 && moved <= d.size.max(*left) {
                d.down_bps = moved / seconds;
            }
        }
    }
    rows.iter().map(|d| (d.key.clone(), (d.left, now))).collect()
}

// ---------------------------------------------------------------- the snapshot

#[derive(Default)]
pub struct Snapshot {
    pub rows: Vec<Download>,
    pub totals: Totals,
    pub at: i64,
    /// A service that did not answer, and why. Shown as a note, not as an empty list.
    pub problems: Vec<(String, String)>,
    pub sources: usize,
}

/// Who asked for what, so a download can say whose wish it is. Rebuilt with the slow tick.
#[derive(Default)]
pub struct Wishes {
    by_media: HashMap<(String, i64), (Option<String>, String)>,
    item_of: HashMap<(String, i64), String>,
}

impl Wishes {
    fn read(conn: &Connection) -> Result<Wishes> {
        let mut w = Wishes::default();
        let mut stmt = conn.prepare(
            "SELECT r.media_type, r.tmdb_id, r.tvdb_id, r.user_id, COALESCE(u.name, r.seerr_user_name), r.item_id
             FROM requests r LEFT JOIN users u ON u.id = r.user_id WHERE r.removed_at IS NULL
             -- Something still being waited for wins, but a title that has arrived was still somebody's wish:
             -- what is in the queue then is an upgrade of it, and saying who asked for it is still true.
             ORDER BY (r.media_status < 5) DESC, r.requested_at",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?, r.get::<_, Option<i64>>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, Option<String>>(4)?, r.get::<_, Option<String>>(5)?))
        })?;
        for row in rows {
            let (media_type, tmdb, tvdb, user_id, name, item_id) = row?;
            for (source, id) in [("tmdb", tmdb), ("tvdb", tvdb)] {
                if let Some(id) = id {
                    let key = (format!("{media_type}:{source}"), id);
                    w.by_media.entry(key.clone()).or_insert((user_id.clone(), name.clone().unwrap_or_else(|| "Somebody".into())));
                    if let Some(item) = &item_id {
                        w.item_of.entry(key).or_insert(item.clone());
                    }
                }
            }
        }
        Ok(w)
    }

    fn of(&self, d: &Download) -> (Option<&(Option<String>, String)>, Option<&String>) {
        let media_type = d.media_type.unwrap_or("");
        for (source, id) in [("tmdb", d.tmdb_id), ("tvdb", d.tvdb_id)] {
            let Some(id) = id else { continue };
            let key = (format!("{media_type}:{source}"), id);
            if let Some(who) = self.by_media.get(&key) {
                return (Some(who), self.item_of.get(&key));
            }
        }
        (None, None)
    }
}

fn download_json(d: &Download, wishes: &Wishes) -> Value {
    let (who, item) = wishes.of(d);
    let poster = match (item, d.arr) {
        (Some(id), _) => json!({ "item_id": id }),
        (None, Some((service_id, media_id))) => json!({ "service_id": service_id, "media_id": media_id }),
        _ => Value::Null,
    };
    json!({
        "key": d.key, "title": d.title, "sub": d.sub, "state": d.state, "progress": d.progress, "size": d.size, "left": d.left, "eta_s": d.eta_s,
        "down_bps": d.down_bps, "release": d.release, "client": d.client, "protocol": d.protocol, "service_name": d.service_name, "error": d.error, "poster": poster,
        "requested_by": who.map(|(id, name)| json!({ "user_id": id, "user_name": name })),
        "item_id": item,
    })
}

/// Is this title in the queue right now? The queue is a listing finstats itself makes, so its posters are
/// ones it may show — to the people who are allowed to see the queue in the first place.
pub fn in_queue(app: &App, service_id: i64, media_id: i64) -> bool {
    app.downloads.read().unwrap().rows.iter().any(|d| d.arr == Some((service_id, media_id)))
}

/// Add "how far along is it" to a request row. This is all somebody without `see_downloads` ever learns: never
/// what the release is called, which client has it, how fast it is going or what else is in the queue.
pub fn attach_progress(app: &App, row: &mut Value) {
    let seasons: Vec<i64> = row["seasons"].as_array().map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default();
    if let Some(p) = own_progress(app, row["media_type"].as_str().unwrap_or(""), row["tmdb_id"].as_i64(), row["tvdb_id"].as_i64(), &seasons) {
        row["download"] = p;
    }
}

/// How far the thing somebody asked for has got, if anything in the queue is it.
pub fn own_progress(app: &App, media_type: &str, tmdb: Option<i64>, tvdb: Option<i64>, seasons: &[i64]) -> Option<Value> {
    let snap = app.downloads.read().unwrap().clone();
    let want = |d: &Download| {
        d.media_type == Some(if media_type == "movie" { "movie" } else { "tv" })
            && ((tmdb.is_some() && d.tmdb_id == tmdb) || (tvdb.is_some() && d.tvdb_id == tvdb))
            && (seasons.is_empty() || d.seasons.is_empty() || d.seasons.iter().any(|s| seasons.contains(s)))
    };
    let d = snap.rows.iter().filter(|d| want(d)).min_by_key(|d| rank(d.state))?;
    Some(json!({ "state": d.state, "progress": d.progress, "eta_s": d.eta_s }))
}

// ---------------------------------------------------------------- the loop

async fn tick(app: &App, before: &HashMap<String, (i64, i64)>) -> HashMap<String, (i64, i64)> {
    let arrs = services::enabled(app, Kind::is_arr);
    if arrs.is_empty() {
        *app.downloads.write().unwrap() = Arc::new(Snapshot { at: crate::db::now(), ..Default::default() });
        return HashMap::new();
    }
    let queues = futures_util::future::join_all(arrs.iter().map(|svc| async move {
        let out = tokio::time::timeout(PER_SERVICE_TIMEOUT, read_queue(app, svc)).await.unwrap_or_else(|_| Err(anyhow::anyhow!("{} did not answer in time", svc.name)));
        (svc.clone(), out)
    }))
    .await;

    let (mut rows, mut problems, mut sources) = (vec![], vec![], 0);
    for (svc, out) in queues {
        match out {
            Ok(records) => {
                sources += 1;
                rows.extend(records);
            }
            Err(e) => {
                tracing::debug!("{}: {e}", svc.name);
                problems.push((svc.name.clone(), e.to_string()));
            }
        }
    }
    let now = crate::db::now();
    let (mut rows, mut totals) = fold(rows);
    let after = speeds(&mut rows, before, now);
    totals.down_bps = rows.iter().map(|d| d.down_bps).sum();
    *app.downloads.write().unwrap() = Arc::new(Snapshot { rows, totals, at: now, problems, sources });
    after
}

/// Its own loop, like the collector: a queue that moves every second is no business of the 60-second scheduler.
pub async fn run(app: App) {
    let mut wishes_at = 0i64;
    // What was left of each download when it was last read, so a speed can be worked out.
    let mut seen: HashMap<String, (i64, i64)> = HashMap::new();
    // Something changed (a connection, a read of Seerr): look again, and ask who wished for what.
    let mut woken = true;
    loop {
        let anything = !services::enabled(&app, Kind::is_arr).is_empty();
        let now = crate::db::now();
        if anything {
            // Who asked for what changes slowly, and reading it is the only database work here.
            if woken || now - wishes_at >= 60 {
                wishes_at = now;
                if let Ok(w) = app.db.call(|c| Wishes::read(c)).await {
                    *app.wishes.write().unwrap() = Arc::new(w);
                }
            }
            seen = tick(&app, &seen).await;
        }
        // Fast only while a page is actually showing this.
        let watching = now - *app.downloads_watched.lock().unwrap() <= WATCHING_FOR_S;
        let wait = if !anything {
            IDLE_EVERY_S
        } else if watching {
            WATCHED_EVERY_S
        } else {
            IDLE_EVERY_S
        };
        woken = tokio::select! {
            _ = app.downloads_wake.notified() => true,
            _ = tokio::time::sleep(Duration::from_secs(wait)) => false,
        };
    }
}

// ---------------------------------------------------------------- API

#[derive(Deserialize)]
pub struct LiveQuery {
    /// The page is open and showing this: keep the snapshot fresh for the next few seconds.
    live: Option<String>,
}

pub async fn downloads(State(app): State<App>, DownloadsViewer(_): DownloadsViewer, Query(q): Query<LiveQuery>) -> ApiResult {
    if q.live.as_deref() == Some("1") {
        let was = {
            let mut watched = app.downloads_watched.lock().unwrap();
            let was = *watched;
            *watched = crate::db::now();
            was
        };
        // Somebody has just opened the page: refresh now instead of at the end of a minute-long sleep.
        if crate::db::now() - was > WATCHING_FOR_S {
            app.downloads_wake.notify_waiters();
        }
    }
    let snap = app.downloads.read().unwrap().clone();
    let wishes = app.wishes.read().unwrap().clone();
    let t = &snap.totals;
    Ok(Json(json!({
        "rows": snap.rows.iter().map(|d| download_json(d, &wishes)).collect::<Vec<_>>(),
        "totals": { "down_bps": t.down_bps, "downloading": t.downloading, "queued": t.queued, "importing": t.importing, "failed": t.failed },
        "at": snap.at, "sources": snap.sources,
        "problems": snap.problems.iter().map(|(name, why)| json!({ "service": name, "error": why })).collect::<Vec<_>>(),
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn queued(id: i64, title: &str, download_id: Option<&str>, season: Option<i64>, state: &'static str, size: i64, left: i64) -> Queued {
        Queued {
            service_id: 1, service_name: "Sonarr".into(), arr_media_id: Some(7), download_id: download_id.map(|h| h.to_ascii_lowercase()), title: title.into(),
            sub: season.map(|s| format!("S{s:02}E01")), media_type: "tv", tmdb_id: None, tvdb_id: Some(id), season, size, left, eta_s: Some(600),
            state, protocol: Some("torrent".into()), client: Some("qBittorrent".into()), release: Some(format!("{title}.1080p.WEB-DL").replace(' ', ".")), error: None,
        }
    }

    #[test]
    fn a_season_pack_is_one_download_however_many_episodes_it_holds() {
        let queue = vec![queued(1, "Low Orbit", Some("AABB"), Some(3), "downloading", 900, 300), queued(1, "Low Orbit", Some("aabb"), Some(3), "downloading", 900, 300), queued(1, "Low Orbit", Some("aabb"), Some(3), "importing", 900, 300)];
        let (rows, totals) = fold(queue);
        assert_eq!(rows.len(), 1, "one download id is one download");
        assert_eq!((rows[0].sub.as_deref(), rows[0].state), (Some("Season 3 · 3 episodes"), "importing"), "the most interesting state wins");
        assert_eq!((rows[0].progress, rows[0].parts, totals.importing), (0.6666666666666666, 3, 1));
        // The same id in two instances is two downloads: they are each waiting for their own copy.
        let (rows, _) = fold(vec![queued(1, "Low Orbit", Some("aabb"), Some(3), "downloading", 900, 300), Queued { service_id: 2, ..queued(1, "Low Orbit", Some("aabb"), Some(3), "downloading", 900, 300) }]);
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn a_record_without_a_download_id_still_stands_for_itself() {
        let (rows, totals) = fold(vec![queued(2, "Northern Static", None, None, "downloading", 1000, 250), queued(3, "Tiny Giants", None, Some(1), "queued", 100, 100)]);
        assert_eq!(rows.len(), 2, "two nameless records are not one download");
        let film = rows.iter().find(|r| r.title == "Northern Static").unwrap();
        assert_eq!((film.progress, film.eta_s, film.client.as_deref()), (0.75, Some(600), Some("qBittorrent")), "progress and the client come from the queue");
        assert_eq!((totals.downloading, totals.queued), (1, 1));
    }

    #[test]
    fn what_needs_attention_is_on_top() {
        let rows = fold(vec![
            queued(1, "Queued thing", Some("a"), None, "queued", 10, 10),
            queued(2, "Failed thing", Some("b"), None, "failed", 10, 5),
            queued(3, "Downloading thing", Some("c"), None, "downloading", 10, 5),
            queued(4, "Importing thing", Some("d"), None, "importing", 10, 0),
        ])
        .0;
        assert_eq!(rows.iter().map(|r| r.state).collect::<Vec<_>>(), ["failed", "importing", "downloading", "queued"]);
    }

    #[test]
    fn a_speed_is_what_moved_between_two_readings_and_nothing_is_invented() {
        let mut rows = fold(vec![queued(1, "Low Orbit", Some("aa"), Some(3), "downloading", 1000, 400)]).0;
        let key = rows[0].key.clone();
        // Nothing to compare with yet.
        let after = speeds(&mut rows, &HashMap::new(), 1000);
        assert_eq!((rows[0].down_bps, after[&key]), (0, (400, 1000)));
        // 100 bytes moved in 10 seconds.
        let mut rows = fold(vec![queued(1, "Low Orbit", Some("aa"), Some(3), "downloading", 1000, 300)]).0;
        speeds(&mut rows, &after, 1010);
        assert_eq!(rows[0].down_bps, 10);
        // Too long ago, and going backwards (an upgrade that starts again), say nothing.
        let mut rows = fold(vec![queued(1, "Low Orbit", Some("aa"), Some(3), "downloading", 1000, 300)]).0;
        speeds(&mut rows, &after, 1000 + 4000);
        assert_eq!(rows[0].down_bps, 0);
        let mut rows = fold(vec![queued(1, "Low Orbit", Some("aa"), Some(3), "downloading", 1000, 900)]).0;
        speeds(&mut rows, &after, 1010);
        assert_eq!(rows[0].down_bps, 0);
    }

    #[test]
    fn a_queue_record_says_what_it_is_waiting_for() {
        let r = json!({ "seriesId": 7, "seasonNumber": 3, "episode": { "seasonNumber": 3, "episodeNumber": 9, "title": "The Long Way Down" },
            "series": { "title": "Low Orbit", "tvdbId": 371002 }, "size": 2000.0, "sizeleft": 500.0, "timeleft": "00:12:30", "downloadId": "ABC123",
            "title": "Low.Orbit.S03E09.1080p.WEB-DL", "status": "downloading", "trackedDownloadState": "downloading", "trackedDownloadStatus": "ok", "protocol": "torrent", "downloadClient": "qBittorrent" });
        let q = queue_row(1, "Sonarr", Kind::Sonarr, &r).unwrap();
        assert_eq!((q.title.as_str(), q.sub.as_deref(), q.season, q.tvdb_id), ("Low Orbit", Some("S03E09 · The Long Way Down"), Some(3), Some(371002)));
        assert_eq!((q.download_id.as_deref(), q.eta_s, q.state), (Some("abc123"), Some(750), "downloading"));
        assert_eq!((q.release.as_deref(), q.client.as_deref(), q.protocol.as_deref()), (Some("Low.Orbit.S03E09.1080p.WEB-DL"), Some("qBittorrent"), Some("torrent")));

        let film = json!({ "movieId": 21, "movie": { "title": "Northern Static", "year": 2026, "tmdbId": 981001 }, "size": 10.0, "sizeleft": 0.0,
            "status": "completed", "trackedDownloadState": "importPending", "trackedDownloadStatus": "warning", "statusMessages": [{ "title": "Not an upgrade for existing file" }], "protocol": "usenet", "downloadClient": "SABnzbd" });
        let q = queue_row(2, "Radarr", Kind::Radarr, &film).unwrap();
        assert_eq!((q.title.as_str(), q.sub.as_deref(), q.state, q.media_type), ("Northern Static", Some("2026"), "importing", "movie"));
        assert_eq!((q.error.as_deref(), q.protocol.as_deref()), (Some("Not an upgrade for existing file"), Some("usenet")));

        // What the queue is doing, in our words.
        assert_eq!(queue_state("downloading", "downloading", "ok"), "downloading");
        assert_eq!(queue_state("completed", "importPending", "ok"), "importing");
        assert_eq!(queue_state("warning", "downloading", "warning"), "stalled");
        assert_eq!(queue_state("warning", "downloading", "error"), "failed");
        assert_eq!(queue_state("downloading", "failedPending", "ok"), "failed");
        assert_eq!(queue_state("paused", "downloading", "ok"), "paused");
        assert_eq!(queue_state("nonsense", "", ""), "unknown");

        assert_eq!(timespan(&json!("01:02:03")), Some(3723));
        assert_eq!(timespan(&json!("1.00:00:30")), Some(86_430));
        assert_eq!(timespan(&json!("00:00:04.5000000")), Some(4));
        assert_eq!(timespan(&json!(null)), None);
        assert_eq!(timespan(&json!("soon")), None);
    }
}
