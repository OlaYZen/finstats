//! Sonarr and Radarr: what is about to air or be released.
//!
//! Both answer camelCase JSON (unlike Jellyfin's PascalCase) and are only ever sent `GET`.
//! The calendar is small and answered in one piece, so a read either replaces everything finstats knew
//! from that instance or, when it fails, nothing: a half-read calendar would look like cancelled episodes.
//!
//! An episode has a moment (`airDateUtc`). A film has days: Radarr reports `inCinemas`, `digitalRelease` and
//! `physicalRelease` as midnight UTC, and treating that as a moment would put every release on the evening
//! before for anyone west of Greenwich. `film_day` keeps the date and drops the time.

use anyhow::{Result, bail};
use serde_json::Value;

use crate::db::rusqlite::params;
use crate::db;
use crate::services::{self, Kind, Service};
use crate::state::App;

/// How far back and ahead the calendar is kept, in days.
const BEHIND_D: i64 = 7;
const AHEAD_D: i64 = 90;

#[derive(Debug, Clone, PartialEq)]
pub struct Upcoming {
    pub kind: &'static str,
    pub external_id: i64,
    pub release: &'static str,
    pub at: Option<i64>,
    pub day: Option<String>,
    pub series_title: Option<String>,
    pub title: String,
    pub season: Option<i64>,
    pub episode: Option<i64>,
    pub finale: Option<String>,
    pub year: Option<i64>,
    pub tvdb_id: Option<i64>,
    pub tmdb_id: Option<i64>,
    pub imdb_id: Option<String>,
    pub arr_media_id: i64,
    pub has_file: bool,
}

fn text(v: &Value) -> Option<String> {
    v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

/// An id of 0 is Sonarr's and Radarr's way of saying "unknown".
fn id(v: &Value) -> Option<i64> {
    v.as_i64().filter(|n| *n > 0)
}

/// `2026-10-03T00:00:00Z` → `2026-10-03`. Anything that is not a date is no day.
pub fn film_day(v: &Value) -> Option<String> {
    let day = v.as_str()?.get(..10)?;
    chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").ok().map(|_| day.to_string())
}

/// Sonarr's `/calendar?includeSeries=true`: one entry per episode.
pub fn episodes_from(calendar: &Value) -> Vec<Upcoming> {
    let Some(list) = calendar.as_array() else { return vec![] };
    list.iter()
        .filter_map(|e| {
            let series = &e["series"];
            Some(Upcoming {
                kind: "episode",
                external_id: id(&e["id"])?,
                release: "air",
                at: Some(e["airDateUtc"].as_str().and_then(db::parse_ts)?),
                day: None,
                series_title: text(&series["title"]),
                title: text(&e["title"]).unwrap_or_else(|| "TBA".into()),
                season: e["seasonNumber"].as_i64(),
                episode: e["episodeNumber"].as_i64(),
                finale: text(&e["finaleType"]),
                year: id(&series["year"]),
                tvdb_id: id(&series["tvdbId"]),
                tmdb_id: id(&series["tmdbId"]),
                imdb_id: text(&series["imdbId"]),
                arr_media_id: id(&e["seriesId"])?,
                has_file: e["hasFile"].as_bool().unwrap_or(false),
            })
        })
        .collect()
}

/// Radarr's `/calendar`: a film appears when *any* of its dates is in the window, so each date is checked again.
pub fn films_from(calendar: &Value, from_day: &str, to_day: &str) -> Vec<Upcoming> {
    let Some(list) = calendar.as_array() else { return vec![] };
    let mut out = vec![];
    for m in list {
        let (Some(movie_id), Some(title)) = (id(&m["id"]), text(&m["title"])) else { continue };
        for (field, release) in [("inCinemas", "cinema"), ("digitalRelease", "digital"), ("physicalRelease", "physical")] {
            let Some(day) = film_day(&m[field]).filter(|d| d.as_str() >= from_day && d.as_str() <= to_day) else { continue };
            out.push(Upcoming {
                kind: "movie",
                external_id: movie_id,
                release,
                at: None,
                day: Some(day),
                series_title: None,
                title: title.clone(),
                season: None,
                episode: None,
                finale: None,
                year: id(&m["year"]),
                tvdb_id: None,
                tmdb_id: id(&m["tmdbId"]),
                imdb_id: text(&m["imdbId"]),
                arr_media_id: movie_id,
                has_file: m["hasFile"].as_bool().unwrap_or(false),
            });
        }
    }
    out
}

async fn read_calendar(app: &App, svc: &Service) -> Result<Vec<Upcoming>> {
    let now = chrono::Utc::now();
    let (from, to) = (now - chrono::Duration::days(BEHIND_D), now + chrono::Duration::days(AHEAD_D));
    let iso = |d: chrono::DateTime<chrono::Utc>| d.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let mut query = vec![("start", iso(from)), ("end", iso(to)), ("unmonitored", "false".to_string())];
    if svc.kind == Kind::Sonarr {
        query.push(("includeSeries", "true".into()));
    }
    let calendar = services::get_json(app, svc, "/api/v3/calendar", &query).await?;
    if !calendar.is_array() {
        bail!("{} did not answer its calendar like {}", svc.url, svc.kind.label());
    }
    Ok(match svc.kind {
        Kind::Sonarr => episodes_from(&calendar),
        _ => films_from(&calendar, &from.format("%Y-%m-%d").to_string(), &to.format("%Y-%m-%d").to_string()),
    })
}

/// Read every connected Sonarr's and Radarr's calendar. One instance failing leaves its rows as they were.
pub async fn sync_upcoming(app: &App) -> Result<String> {
    const ID: &str = "sync_upcoming";
    let list = services::enabled(app, Kind::is_arr);
    if list.is_empty() {
        return Ok("No Sonarr or Radarr connected".into());
    }
    let (mut total, mut failed) = (0usize, vec![]);
    for (n, svc) in list.iter().enumerate() {
        app.tasks.update(ID, format!("Reading {}", svc.name), Some(n as f64 / list.len() as f64));
        match read_calendar(app, svc).await {
            Ok(rows) => {
                total += rows.len();
                let service_id = svc.id;
                app.db
                    .call(move |c| {
                        let tx = c.transaction()?;
                        tx.execute("DELETE FROM upcoming WHERE service_id = ?1", [service_id])?;
                        {
                            let mut stmt = tx.prepare(
                                "INSERT OR REPLACE INTO upcoming(service_id, kind, external_id, release, at, day, series_title, title, season, episode, finale, year, tvdb_id, tmdb_id, imdb_id, arr_media_id, has_file)
                                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
                            )?;
                            for r in &rows {
                                stmt.execute(params![service_id, r.kind, r.external_id, r.release, r.at, r.day, r.series_title, r.title, r.season, r.episode, r.finale, r.year, r.tvdb_id, r.tmdb_id, r.imdb_id, r.arr_media_id, r.has_file])?;
                            }
                        }
                        crate::pipeline::link(&tx)?;
                        tx.commit()?;
                        Ok(())
                    })
                    .await?;
                services::record(app, svc.id, Ok(None)).await;
            }
            Err(e) => {
                tracing::warn!("calendar of {} ({}): {e}", svc.name, svc.kind.label());
                services::record(app, svc.id, Err(e.to_string())).await;
                failed.push(svc.name.clone());
            }
        }
    }
    if failed.len() == list.len() {
        bail!("{} did not answer", failed.join(", "));
    }
    Ok(match failed.is_empty() {
        true => format!("{total} releases ahead"),
        false => format!("{total} releases ahead; {} did not answer", failed.join(", ")),
    })
}

pub struct Found {
    pub service_id: i64,
    pub media_id: i64,
    pub title: String,
    pub year: Option<i64>,
}

/// Does a connected Sonarr (by TVDB id) or Radarr (by TMDB id) have this title? It then also has its poster.
pub async fn find(app: &App, media_type: &str, tmdb: Option<i64>, tvdb: Option<i64>) -> Option<Found> {
    let (kind, path, key, id) = match media_type {
        "movie" => (Kind::Radarr, "/api/v3/movie", "tmdbId", tmdb?),
        _ => (Kind::Sonarr, "/api/v3/series", "tvdbId", tvdb?),
    };
    for svc in services::enabled(app, |k| k == kind) {
        let Ok(list) = services::get_json(app, &svc, path, &[(key, id.to_string())]).await else { continue };
        // Older versions ignore the filter and answer with everything: take the one that matches, not the first.
        let hit = list.as_array().and_then(|a| a.iter().find(|m| m[key].as_i64() == Some(id)));
        if let Some(m) = hit
            && let (Some(media_id), Some(title)) = (self::id(&m["id"]), text(&m["title"]))
        {
            return Some(Found { service_id: svc.id, media_id, title, year: self::id(&m["year"]) });
        }
    }
    None
}

// ---------------------------------------------------------------- what came in

/// How far back a first read goes, and how many pages it may take to get there.
const HISTORY_DAYS: i64 = 365;
const HISTORY_PAGES: usize = 40;
const HISTORY_PAGE: usize = 250;
/// Written in pieces, so a long history never holds the write lock while the collector wants it.
const HISTORY_CHUNK: usize = 500;

#[derive(Debug, Clone, PartialEq)]
pub struct Grab {
    pub history_id: i64,
    pub event: &'static str,
    pub at: i64,
    pub media_type: &'static str,
    pub title: Option<String>,
    pub source: Option<String>,
    pub season: Option<i64>,
    pub tvdb_id: Option<i64>,
    pub tmdb_id: Option<i64>,
    pub size_bytes: Option<i64>,
    pub quality: Option<String>,
    pub indexer: Option<String>,
    pub protocol: Option<String>,
    pub client: Option<String>,
    pub download_id: Option<String>,
}

/// Sonarr and Radarr name their events differently and number them differently again; the words are stable.
pub fn event_of(raw: &str) -> Option<&'static str> {
    match raw {
        "grabbed" => Some("grabbed"),
        "downloadFolderImported" | "seriesFolderImported" | "movieFolderImported" => Some("imported"),
        "downloadFailed" => Some("failed"),
        _ => None, // renames, deletions and ignores say nothing about what arrived
    }
}

/// `data` is a dictionary of strings, whatever the value looks like.
fn data_str(data: &Value, key: &str) -> Option<String> {
    data.get(key).and_then(|v| v.as_str().map(str::to_string).or_else(|| v.as_i64().map(|n| n.to_string()))).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

pub fn grab_from(kind: Kind, h: &Value) -> Option<Grab> {
    let data = &h["data"];
    let (series, movie) = (&h["series"], &h["movie"]);
    let tv = kind == Kind::Sonarr;
    Some(Grab {
        history_id: id(&h["id"])?,
        event: event_of(h["eventType"].as_str().unwrap_or(""))?,
        at: h["date"].as_str().and_then(db::parse_ts)?,
        media_type: if tv { "tv" } else { "movie" },
        title: text(if tv { &series["title"] } else { &movie["title"] }),
        source: text(&h["sourceTitle"]),
        season: tv.then(|| h["episode"]["seasonNumber"].as_i64()).flatten(),
        tvdb_id: tv.then(|| id(&series["tvdbId"])).flatten(),
        tmdb_id: if tv { id(&series["tmdbId"]) } else { id(&movie["tmdbId"]) },
        size_bytes: data_str(data, "size").and_then(|s| s.parse().ok()).filter(|n: &i64| *n > 0),
        quality: text(&h["quality"]["quality"]["name"]),
        indexer: data_str(data, "indexer"),
        protocol: data_str(data, "protocol").map(|p| match p.as_str() {
            "1" => "usenet".to_string(),
            "2" => "torrent".to_string(),
            other => other.to_ascii_lowercase(),
        }),
        client: data_str(data, "downloadClient").or_else(|| data_str(data, "downloadClientName")),
        download_id: text(&h["downloadId"]).map(|d| d.to_ascii_lowercase()),
    })
}

async fn read_history(app: &App, svc: &Service, since: Option<i64>) -> Result<Vec<Grab>> {
    let mut out = vec![];
    match since {
        // Afterwards: everything that happened since, in one answer.
        Some(since) => {
            let from = chrono::DateTime::from_timestamp(since, 0).unwrap_or_default().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
            let body = services::get_json(app, svc, "/api/v3/history/since", &[("date", from), (if svc.kind == Kind::Sonarr { "includeSeries" } else { "includeMovie" }, "true".into())]).await?;
            out.extend(body.as_array().into_iter().flatten().filter_map(|h| grab_from(svc.kind, h)));
        }
        // The first read: newest first, and only as far back as is worth having.
        None => {
            let floor = db::now() - HISTORY_DAYS * 86_400;
            for page in 1..=HISTORY_PAGES {
                let query = [
                    ("page", page.to_string()),
                    ("pageSize", HISTORY_PAGE.to_string()),
                    ("sortKey", "date".to_string()),
                    ("sortDirection", "descending".to_string()),
                    (if svc.kind == Kind::Sonarr { "includeSeries" } else { "includeMovie" }, "true".to_string()),
                ];
                let body = services::get_json(app, svc, "/api/v3/history", &query).await?;
                let Some(records) = body["records"].as_array() else { break };
                let got = records.len();
                let mut old = false;
                for h in records {
                    let at = h["date"].as_str().and_then(db::parse_ts).unwrap_or(0);
                    old |= at < floor;
                    if at >= floor
                        && let Some(g) = grab_from(svc.kind, h)
                    {
                        out.push(g);
                    }
                }
                if old || got < HISTORY_PAGE {
                    break;
                }
            }
        }
    }
    Ok(out)
}

pub async fn sync_grabs(app: &App) -> Result<String> {
    const ID: &str = "sync_grabs";
    let list = services::enabled(app, Kind::is_arr);
    if list.is_empty() {
        return Ok("No Sonarr or Radarr connected".into());
    }
    let (mut total, mut failed) = (0usize, vec![]);
    for svc in &list {
        app.tasks.update(ID, format!("Reading the history of {}", svc.name), None);
        let key = format!("service:{}:grabs_since", svc.id);
        let (k, sid) = (key.clone(), svc.id);
        let since: Option<i64> = app.db.call(move |c| Ok(db::get_setting(c, &k)?.and_then(|v| v.parse().ok()))).await?;
        match read_history(app, svc, since).await {
            Ok(rows) => {
                total += rows.len();
                let newest = rows.iter().map(|g| g.at).max();
                // In pieces: a year of history must not hold the write lock while a play is being recorded.
                for chunk in rows.chunks(HISTORY_CHUNK) {
                    let chunk: Vec<Grab> = chunk.to_vec();
                    app.db
                        .call(move |c| {
                            let tx = c.transaction()?;
                            {
                                let mut stmt = tx.prepare(
                                    "INSERT OR REPLACE INTO grabs(service_id, history_id, event, at, media_type, title, source, season, tvdb_id, tmdb_id, size_bytes, quality, indexer, protocol, client, download_id)
                                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
                                )?;
                                for g in &chunk {
                                    stmt.execute(params![sid, g.history_id, g.event, g.at, g.media_type, g.title, g.source, g.season, g.tvdb_id, g.tmdb_id, g.size_bytes, g.quality, g.indexer, g.protocol, g.client, g.download_id])?;
                                }
                            }
                            tx.commit()?;
                            Ok(())
                        })
                        .await?;
                }
                if let Some(newest) = newest {
                    // A second of overlap: an event written while the answer was on its way is read again, not lost.
                    app.db.call(move |c| db::set_setting(c, &key, &(newest - 1).to_string())).await?;
                }
                services::record(app, svc.id, Ok(None)).await;
            }
            Err(e) => {
                tracing::warn!("history of {}: {e}", svc.name);
                services::record(app, svc.id, Err(e.to_string())).await;
                failed.push(svc.name.clone());
            }
        }
    }
    if failed.len() == list.len() {
        bail!("{} did not answer", failed.join(", "));
    }
    Ok(format!("{total} events"))
}

/// Where a title's poster lives in Sonarr or Radarr. A constant shape: nothing a caller sends ends up in it but a number.
pub fn poster_path(media_id: i64, width: u32) -> String {
    format!("/api/v3/mediacover/{media_id}/poster-{}.jpg", if width <= 250 { 250 } else { 500 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_episode_keeps_its_moment_and_its_series() {
        let cal = json!([
            { "id": 501, "seriesId": 12, "seasonNumber": 2, "episodeNumber": 5, "title": "The Long Way Down", "airDateUtc": "2026-09-25T19:00:00Z", "hasFile": false,
              "finaleType": "season", "series": { "title": "Low Orbit", "year": 2021, "tvdbId": 370001, "tmdbId": 0, "imdbId": "tt9900001" } },
            { "id": 502, "seriesId": 12, "seasonNumber": 2, "episodeNumber": 6, "airDateUtc": "2026-10-02T19:00:00Z", "hasFile": true, "series": { "title": "Low Orbit", "tvdbId": 370001 } },
            { "id": 503, "seriesId": 12, "title": "No air date yet" },
            { "seriesId": 12, "airDateUtc": "2026-10-02T19:00:00Z" }
        ]);
        let eps = episodes_from(&cal);
        assert_eq!(eps.len(), 2, "an episode without a date or an id is not upcoming");
        assert_eq!((eps[0].at, eps[0].day.clone()), (db::parse_ts("2026-09-25T19:00:00Z"), None));
        assert_eq!((eps[0].series_title.as_deref(), eps[0].season, eps[0].episode, eps[0].finale.as_deref()), (Some("Low Orbit"), Some(2), Some(5), Some("season")));
        assert_eq!((eps[0].tvdb_id, eps[0].tmdb_id, eps[0].arr_media_id), (Some(370001), None, 12), "0 means unknown");
        assert_eq!((eps[1].title.as_str(), eps[1].has_file), ("TBA", true));
        assert!(episodes_from(&json!({"error": "Unauthorized"})).is_empty());
    }

    #[test]
    fn a_film_has_days_not_moments_and_only_those_inside_the_window() {
        let cal = json!([{ "id": 7, "title": "Winterline", "year": 2026, "tmdbId": 990001, "imdbId": "tt9900002", "hasFile": false,
            "inCinemas": "2026-03-01T00:00:00Z", "digitalRelease": "2026-10-03T00:00:00Z", "physicalRelease": "2026-11-20T00:00:00Z" }]);
        let films = films_from(&cal, "2026-09-13", "2026-12-19");
        assert_eq!(films.iter().map(|f| (f.release, f.day.clone().unwrap())).collect::<Vec<_>>(), [("digital", "2026-10-03".to_string()), ("physical", "2026-11-20".to_string())]);
        assert!(films.iter().all(|f| f.at.is_none() && f.external_id == 7 && f.arr_media_id == 7 && f.tmdb_id == Some(990001)));
        // Midnight UTC is still the third of October in Los Angeles: the day is text, no zone can move it.
        assert_eq!(film_day(&json!("2026-10-03T00:00:00Z")).as_deref(), Some("2026-10-03"));
        assert_eq!(film_day(&json!("soon")), None);
        assert_eq!(film_day(&json!(null)), None);
    }

    #[test]
    fn a_history_event_says_what_arrived_and_where_it_came_from() {
        let h = json!({ "id": 9001, "eventType": "downloadFolderImported", "date": "2026-09-19T21:04:11Z", "sourceTitle": "Low.Orbit.S03E08.1080p.WEB-DL",
            "downloadId": "AABB11", "quality": { "quality": { "name": "WEBDL-1080p" } }, "episode": { "seasonNumber": 3 },
            "series": { "title": "Low Orbit", "tvdbId": 371002 }, "data": { "size": "2147483648", "indexer": "A Tracker", "protocol": "2", "downloadClient": "qBittorrent" } });
        let g = grab_from(Kind::Sonarr, &h).unwrap();
        assert_eq!((g.history_id, g.event, g.media_type, g.title.as_deref(), g.season), (9001, "imported", "tv", Some("Low Orbit"), Some(3)));
        assert_eq!((g.size_bytes, g.quality.as_deref(), g.indexer.as_deref(), g.protocol.as_deref(), g.download_id.as_deref()), (Some(2_147_483_648), Some("WEBDL-1080p"), Some("A Tracker"), Some("torrent"), Some("aabb11")));
        // Both apps' words for the same three things, and nothing else.
        for (raw, want) in [("grabbed", Some("grabbed")), ("downloadFolderImported", Some("imported")), ("movieFolderImported", Some("imported")), ("seriesFolderImported", Some("imported")), ("downloadFailed", Some("failed"))] {
            assert_eq!(event_of(raw), want, "{raw}");
        }
        for ignored in ["episodeFileRenamed", "movieFileDeleted", "downloadIgnored", "unknown", ""] {
            assert_eq!(event_of(ignored), None, "{ignored}");
        }
        assert!(grab_from(Kind::Radarr, &json!({ "id": 1, "eventType": "episodeFileRenamed", "date": "2026-09-19T21:04:11Z" })).is_none());
        assert!(grab_from(Kind::Radarr, &json!({ "id": 1, "eventType": "grabbed", "date": "not a date" })).is_none());
    }

    #[test]
    fn a_poster_path_is_made_of_a_number_and_nothing_else() {
        assert_eq!(poster_path(12, 160), "/api/v3/mediacover/12/poster-250.jpg");
        assert_eq!(poster_path(12, 480), "/api/v3/mediacover/12/poster-500.jpg");
    }
}
