//! What is arriving right now: Sonarr's and Radarr's queues, joined with the torrent clients.
//!
//! Two sources say different halves of the truth. The queue knows *what* a download is (which episode, which
//! film, how far along, what went wrong on import); the client knows *how* it is going (speed, peers, ratio,
//! whether it is stalled). They meet at the torrent hash, which Sonarr and Radarr call `downloadId`.
//!
//! Three things follow from that, and `merge` is where they live:
//! - A season pack is several queue records with one hash: one download, not five.
//! - A usenet download has no torrent at all, so it is described from the queue alone.
//! - A torrent no Arr app knows is somebody's own download; it is listed, without pretending to know what it is.
//!
//! The snapshot is kept in memory and never in the database: it is worthless a minute later. It holds only
//! what is unfinished, because a client may seed thousands of torrents and nobody wants that list; the
//! finished ones are counted, not kept. Nothing here touches the database per tick.

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
use crate::torrents::Torrent;

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
    pub protocol: String,
    pub client: Option<String>,
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
        protocol: r["protocol"].as_str().unwrap_or("unknown").to_string(),
        client: text(&r["downloadClient"]),
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

// ---------------------------------------------------------------- one picture out of two

#[derive(Debug, Clone, PartialEq)]
pub struct Download {
    pub key: String,
    pub title: String,
    pub sub: Option<String>,
    pub state: &'static str,
    pub progress: f64,
    pub size: i64,
    pub eta_s: Option<i64>,
    pub down_bps: i64,
    pub up_bps: i64,
    pub ratio: Option<f64>,
    pub peers: Option<i64>,
    /// What the torrent is called on disk. Only ever shown to somebody who may see downloads.
    pub release: Option<String>,
    pub client: Option<String>,
    pub service_name: Option<String>,
    pub arr: Option<(i64, i64)>,
    pub media_type: Option<&'static str>,
    pub tmdb_id: Option<i64>,
    pub tvdb_id: Option<i64>,
    pub seasons: Vec<i64>,
    /// How many queue records share this download: a season pack is one download and many episodes.
    pub parts: usize,
    pub error: Option<String>,
    pub known: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Totals {
    pub down_bps: i64,
    pub up_bps: i64,
    pub downloading: usize,
    pub queued: usize,
    pub importing: usize,
    pub failed: usize,
    pub seeding: usize,
    pub torrents: usize,
}

fn rank(state: &str) -> u8 {
    match state {
        "failed" => 0,
        "importing" => 1,
        "downloading" => 2,
        "stalled" => 3,
        "queued" => 4,
        "paused" => 5,
        "checking" => 6,
        _ => 7,
    }
}

/// The queue records and the torrents as one list. Pure: the loop only feeds it.
pub fn merge(queue: Vec<Queued>, torrents: Vec<(String, Torrent)>) -> (Vec<Download>, Totals) {
    let mut totals = Totals { torrents: torrents.len(), ..Default::default() };
    let mut by_hash: HashMap<String, (String, Torrent)> = HashMap::new();
    for (client, t) in torrents {
        totals.down_bps += t.down_bps;
        totals.up_bps += t.up_bps;
        if t.state == "seeding" {
            totals.seeding += 1;
        }
        by_hash.insert(t.hash.clone(), (client, t));
    }

    // A season pack is one hash and several records: the pack is the download, the episodes are what it holds.
    let mut out: Vec<Download> = vec![];
    let mut at: HashMap<String, usize> = HashMap::new();
    let mut used: Vec<String> = vec![];
    for q in queue {
        let key = q.download_id.clone().unwrap_or_else(|| format!("arr:{}:{}", q.service_id, q.title));
        if let Some(&i) = at.get(&key) {
            // Several episodes behind one hash: one download, and the parts are what it holds.
            let d: &mut Download = &mut out[i];
            d.parts += 1;
            if let Some(s) = q.season
                && !d.seasons.contains(&s)
            {
                d.seasons.push(s);
            }
            d.size = d.size.max(q.size);
            if rank(q.state) < rank(d.state) {
                d.state = q.state;
            }
            d.error = d.error.take().or(q.error);
            continue;
        }
        let torrent = q.download_id.as_ref().and_then(|h| by_hash.get(h));
        if let Some(h) = &q.download_id {
            used.push(h.clone());
        }
        let progress = match (&torrent, q.size) {
            (Some((_, t)), _) if t.size > 0 => t.progress,
            (_, size) if size > 0 => ((size - q.left) as f64 / size as f64).clamp(0.0, 1.0),
            _ => 0.0,
        };
        // The queue knows about importing and failing; the client knows about stalling and speed.
        let state = match (q.state, torrent.map(|(_, t)| t.state)) {
            ("importing" | "failed", _) => q.state,
            // Deluge has no word for "stalled": when Sonarr says so and nothing is moving, say so.
            ("stalled", Some("downloading")) if torrent.is_some_and(|(_, t)| t.down_bps == 0) => "stalled",
            (_, Some(t)) if t != "unknown" && t != "seeding" => t,
            (s, _) => s,
        };
        at.insert(key.clone(), out.len());
        out.push(Download {
            key,
            title: q.title,
            sub: q.sub,
            state,
            progress,
            size: if q.size > 0 { q.size } else { torrent.map(|(_, t)| t.size).unwrap_or(0) },
            eta_s: torrent.and_then(|(_, t)| t.eta_s).or(q.eta_s),
            down_bps: torrent.map(|(_, t)| t.down_bps).unwrap_or(0),
            up_bps: torrent.map(|(_, t)| t.up_bps).unwrap_or(0),
            ratio: torrent.map(|(_, t)| t.ratio),
            peers: torrent.map(|(_, t)| t.peers),
            release: torrent.map(|(_, t)| t.name.clone()),
            client: torrent.map(|(c, _)| c.clone()).or(q.client),
            service_name: Some(q.service_name),
            arr: q.arr_media_id.map(|m| (q.service_id, m)),
            media_type: Some(q.media_type),
            tmdb_id: q.tmdb_id,
            tvdb_id: q.tvdb_id,
            seasons: q.season.into_iter().collect(),
            parts: 1,
            error: q.error.or_else(|| torrent.and_then(|(_, t)| t.error.clone())),
            known: true,
        });
    }

    // Torrents nobody in Sonarr or Radarr is waiting for: somebody added them by hand. Finished ones are counted only.
    for (hash, (client, t)) in by_hash {
        if used.contains(&hash) || matches!(t.state, "seeding") {
            continue;
        }
        out.push(Download {
            key: hash,
            title: t.name.clone(),
            sub: None,
            state: t.state,
            progress: t.progress,
            size: t.size,
            eta_s: t.eta_s,
            down_bps: t.down_bps,
            up_bps: t.up_bps,
            ratio: Some(t.ratio),
            peers: Some(t.peers),
            release: Some(t.name),
            client: Some(client),
            service_name: None,
            arr: None,
            media_type: None,
            tmdb_id: None,
            tvdb_id: None,
            seasons: vec![],
            parts: 1,
            error: t.error,
            known: false,
        });
    }

    for d in &out {
        match d.state {
            "downloading" | "stalled" => totals.downloading += 1,
            "queued" | "paused" | "checking" => totals.queued += 1,
            "importing" => totals.importing += 1,
            "failed" => totals.failed += 1,
            _ => {}
        }
    }
    // A pack knows what it holds only once every record has been seen.
    for d in out.iter_mut().filter(|d| d.parts > 1) {
        let episodes = format!("{} episodes", d.parts);
        d.seasons.sort_unstable();
        d.sub = Some(match d.seasons.as_slice() {
            [s] => format!("Season {s} · {episodes}"),
            [] => episodes,
            seasons => format!("{} seasons · {episodes}", seasons.len()),
        });
    }
    debug_assert!(out.iter().all(|d| crate::torrents::STATES.contains(&d.state) || d.state == "importing"), "a state nobody has a word for");
    out.sort_by(|a, b| rank(a.state).cmp(&rank(b.state)).then(b.progress.partial_cmp(&a.progress).unwrap_or(std::cmp::Ordering::Equal)).then_with(|| a.title.cmp(&b.title)));
    out.truncate(MAX_ROWS);
    (out, totals)
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
        "key": d.key, "title": d.title, "sub": d.sub, "state": d.state, "progress": d.progress, "size": d.size, "eta_s": d.eta_s,
        "down_bps": d.down_bps, "up_bps": d.up_bps, "ratio": d.ratio, "peers": d.peers, "release": d.release, "client": d.client,
        "service_name": d.service_name, "known": d.known, "error": d.error, "poster": poster,
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

fn services_for(app: &App) -> (Vec<Arc<services::Service>>, Vec<Arc<services::Service>>) {
    (services::enabled(app, Kind::is_arr), services::enabled(app, Kind::is_client))
}

async fn tick(app: &App, all: bool) {
    let (arrs, clients) = services_for(app);
    if arrs.is_empty() && clients.is_empty() {
        *app.downloads.write().unwrap() = Arc::new(Snapshot { at: crate::db::now(), ..Default::default() });
        return;
    }
    let queues = futures_util::future::join_all(arrs.iter().map(|svc| async move {
        let out = tokio::time::timeout(PER_SERVICE_TIMEOUT, read_queue(app, svc)).await.unwrap_or_else(|_| Err(anyhow::anyhow!("{} did not answer in time", svc.name)));
        (svc.clone(), out)
    }))
    .await;
    let torrents = futures_util::future::join_all(clients.iter().map(|svc| async move {
        let out = tokio::time::timeout(PER_SERVICE_TIMEOUT, crate::torrents::fetch(app, svc, all)).await.unwrap_or_else(|_| Err(anyhow::anyhow!("{} did not answer in time", svc.name)));
        (svc.clone(), out)
    }))
    .await;

    let mut queue_rows = vec![];
    let mut torrent_rows = vec![];
    let mut problems = vec![];
    let mut sources = 0;
    for (svc, out) in queues {
        match out {
            Ok(rows) => {
                sources += 1;
                queue_rows.extend(rows);
            }
            Err(e) => problems.push((svc.name.clone(), e.to_string())),
        }
    }
    for (svc, out) in torrents {
        match out {
            Ok(rows) => {
                sources += 1;
                torrent_rows.extend(rows.into_iter().map(|t| (svc.name.clone(), t)));
            }
            Err(e) => problems.push((svc.name.clone(), e.to_string())),
        }
    }
    // Only the slow tick has the whole picture; a fast one would otherwise report every finished torrent as gone.
    let (rows, mut totals) = merge(queue_rows, torrent_rows);
    if !all {
        let before = app.downloads.read().unwrap().clone();
        totals.seeding = before.totals.seeding;
        totals.torrents = totals.torrents.max(before.totals.torrents);
    }
    for (name, why) in &problems {
        tracing::debug!("{name}: {why}");
    }
    *app.downloads.write().unwrap() = Arc::new(Snapshot { rows, totals, at: crate::db::now(), problems, sources });
}

/// Its own loop, like the collector: a queue that moves every second is no business of the 60-second scheduler.
pub async fn run(app: App) {
    let mut wishes_at = 0i64;
    let mut slow_at = 0i64;
    // Something changed (a connection, a read of Seerr): look again, and ask who wished for what.
    let mut woken = true;
    loop {
        let (arrs, clients) = services_for(&app);
        let anything = !arrs.is_empty() || !clients.is_empty();
        let now = crate::db::now();
        if anything {
            // Who asked for what changes slowly, and reading it is the only database work here.
            if woken || now - wishes_at >= 60 {
                wishes_at = now;
                if let Ok(w) = app.db.call(|c| Wishes::read(c)).await {
                    *app.wishes.write().unwrap() = Arc::new(w);
                }
            }
            let full = now - slow_at >= IDLE_EVERY_S as i64;
            if full {
                slow_at = now;
            }
            tick(&app, full).await;
        }
        // Fast only while a page is actually showing this.
        let watching = now - *app.downloads_watched.lock().unwrap() <= WATCHING_FOR_S;
        let wait = if !anything { IDLE_EVERY_S } else if watching { WATCHED_EVERY_S } else { IDLE_EVERY_S };
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
        "totals": { "down_bps": t.down_bps, "up_bps": t.up_bps, "downloading": t.downloading, "queued": t.queued, "importing": t.importing, "failed": t.failed, "seeding": t.seeding, "torrents": t.torrents },
        "at": snap.at, "sources": snap.sources,
        "problems": snap.problems.iter().map(|(name, why)| json!({ "service": name, "error": why })).collect::<Vec<_>>(),
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn queued(id: i64, title: &str, hash: Option<&str>, season: Option<i64>, state: &'static str, size: i64, left: i64) -> Queued {
        Queued {
            service_id: 1, service_name: "Sonarr".into(), arr_media_id: Some(7), download_id: hash.map(|h| h.to_ascii_lowercase()), title: title.into(),
            sub: season.map(|s| format!("S{s:02}E01")), media_type: "tv", tmdb_id: None, tvdb_id: Some(id), season, size, left, eta_s: Some(600),
            state, protocol: if hash.is_some() { "torrent".into() } else { "usenet".into() }, client: Some("qBittorrent".into()), error: None,
        }
    }

    fn torrent(hash: &str, state: &'static str, progress: f64) -> Torrent {
        Torrent { hash: hash.into(), name: format!("{hash}.release.1080p"), state, progress, size: 1000, left: ((1.0 - progress) * 1000.0) as i64, down_bps: 5_000_000, up_bps: 1000, eta_s: Some(120), ratio: 0.5, peers: 12, error: None }
    }

    #[test]
    fn a_season_pack_is_one_download_however_many_episodes_it_holds() {
        let queue = vec![queued(1, "Low Orbit", Some("AABB"), Some(3), "downloading", 900, 300), queued(1, "Low Orbit", Some("aabb"), Some(3), "downloading", 900, 300), queued(1, "Low Orbit", Some("aabb"), Some(3), "importing", 900, 300)];
        let (rows, totals) = merge(queue, vec![("qBittorrent".into(), torrent("aabb", "downloading", 0.66))]);
        assert_eq!(rows.len(), 1, "one hash is one download");
        assert_eq!((rows[0].sub.as_deref(), rows[0].state, rows[0].known), (Some("Season 3 · 3 episodes"), "importing", true), "the most interesting state wins");
        assert_eq!((rows[0].down_bps, rows[0].peers), (5_000_000, Some(12)), "the client fills in what the queue cannot say");
        assert_eq!(totals.importing, 1);
    }

    #[test]
    fn usenet_needs_no_torrent_and_a_stranger_torrent_is_listed_as_what_it_is() {
        let (rows, totals) = merge(vec![queued(2, "Northern Static", None, None, "downloading", 1000, 250)], vec![("Deluge".into(), torrent("ffff", "downloading", 0.1))]);
        let usenet = rows.iter().find(|r| r.title == "Northern Static").unwrap();
        assert_eq!((usenet.progress, usenet.down_bps, usenet.known, usenet.eta_s), (0.75, 0, true, Some(600)), "progress from the queue alone");
        let stranger = rows.iter().find(|r| !r.known).unwrap();
        assert_eq!((stranger.title.as_str(), stranger.client.as_deref(), stranger.media_type), ("ffff.release.1080p", Some("Deluge"), None));
        assert_eq!(totals.down_bps, 5_000_000);
        // Anything that is only seeding is counted, not listed.
        let (rows, totals) = merge(vec![], vec![("Deluge".into(), torrent("eeee", "seeding", 1.0))]);
        assert!(rows.is_empty() && totals.seeding == 1 && totals.torrents == 1);
    }

    #[test]
    fn a_download_that_says_it_is_going_but_is_not_is_reported_as_stalled() {
        let mut idle = torrent("beef", "downloading", 0.3);
        idle.down_bps = 0;                                    // Deluge has no word for stalled
        let (rows, _) = merge(vec![queued(9, "Paper Lanterns", Some("beef"), Some(1), "stalled", 100, 70)], vec![("Deluge".into(), idle)]);
        assert_eq!(rows[0].state, "stalled");
        // But a torrent that is actually moving is downloading, whatever the queue's warning says.
        let (rows, _) = merge(vec![queued(9, "Paper Lanterns", Some("beef"), Some(1), "stalled", 100, 70)], vec![("Deluge".into(), torrent("beef", "downloading", 0.3))]);
        assert_eq!(rows[0].state, "downloading");
    }

    #[test]
    fn the_hash_is_the_same_hash_whatever_its_case_and_a_stalled_torrent_says_so() {
        let (rows, _) = merge(vec![queued(3, "Tiny Giants", Some("CaFe"), Some(1), "downloading", 100, 50)], vec![("qBittorrent".into(), torrent("cafe", "stalled", 0.5))]);
        assert_eq!((rows.len(), rows[0].state, rows[0].known), (1, "stalled", true));
        // What the queue knows beats what the client thinks: importing and failing happen after the torrent is done.
        let (rows, _) = merge(vec![queued(3, "Tiny Giants", Some("cafe"), Some(1), "importing", 100, 0)], vec![("qBittorrent".into(), torrent("cafe", "seeding", 1.0))]);
        assert_eq!(rows[0].state, "importing");
    }

    #[test]
    fn every_clients_vocabulary_lands_in_ours() {
        use crate::torrents::{dl_state, qb_state, tr_state};
        for (raw, want) in [("stalledDL", "stalled"), ("metaDL", "downloading"), ("missingFiles", "failed"), ("pausedUP", "seeding"), ("checkingResumeData", "checking"), ("nonsense", "unknown")] {
            assert_eq!(qb_state(raw), want, "qBittorrent {raw}");
        }
        assert_eq!(tr_state(4, false, 0), "downloading");
        assert_eq!(tr_state(4, true, 0), "stalled");
        assert_eq!(tr_state(6, false, 3), "failed", "an error beats the status");
        assert_eq!(tr_state(0, false, 0), "paused");
        assert_eq!(dl_state("Downloading"), "downloading");
        assert_eq!(dl_state("Error"), "failed");
        for s in [qb_state("downloading"), tr_state(6, false, 0), dl_state("Queued")] {
            assert!(crate::torrents::STATES.contains(&s));
        }
    }

    #[test]
    fn a_queue_record_says_what_it_is_waiting_for() {
        let r = json!({ "seriesId": 7, "seasonNumber": 3, "episode": { "seasonNumber": 3, "episodeNumber": 9, "title": "The Long Way Down" },
            "series": { "title": "Low Orbit", "tvdbId": 371002 }, "size": 2000.0, "sizeleft": 500.0, "timeleft": "00:12:30", "downloadId": "ABC123",
            "status": "downloading", "trackedDownloadState": "downloading", "trackedDownloadStatus": "ok", "protocol": "torrent", "downloadClient": "qBittorrent" });
        let q = queue_row(1, "Sonarr", Kind::Sonarr, &r).unwrap();
        assert_eq!((q.title.as_str(), q.sub.as_deref(), q.season, q.tvdb_id), ("Low Orbit", Some("S03E09 · The Long Way Down"), Some(3), Some(371002)));
        assert_eq!((q.download_id.as_deref(), q.eta_s, q.state), (Some("abc123"), Some(750), "downloading"));

        let film = json!({ "movieId": 21, "movie": { "title": "Northern Static", "year": 2026, "tmdbId": 981001 }, "size": 10.0, "sizeleft": 0.0,
            "status": "completed", "trackedDownloadState": "importPending", "trackedDownloadStatus": "warning", "statusMessages": [{ "title": "Not an upgrade for existing file" }], "protocol": "usenet" });
        let q = queue_row(2, "Radarr", Kind::Radarr, &film).unwrap();
        assert_eq!((q.title.as_str(), q.sub.as_deref(), q.state, q.media_type), ("Northern Static", Some("2026"), "importing", "movie"));
        assert_eq!(q.error.as_deref(), Some("Not an upgrade for existing file"));

        assert_eq!(timespan(&json!("01:02:03")), Some(3723));
        assert_eq!(timespan(&json!("1.00:00:30")), Some(86_430));
        assert_eq!(timespan(&json!("00:00:04.5000000")), Some(4));
        assert_eq!(timespan(&json!(null)), None);
        assert_eq!(timespan(&json!("soon")), None);
    }
}
