//! Seerr (the merged Overseerr / Jellyseerr): who asked for what, and how it is going.
//!
//! Field names and numbers are Seerr's own (`server/entity/*.ts`; its API description leaves several out).
//! A request carries ids and no title: the title comes from the library when the thing is there, from
//! Sonarr or Radarr when they have it, and only then from Seerr's `/movie/{id}` or `/tv/{id}`, once.
//!
//! **Reading.** A pass begins by asking for a single row, newest-modified first: if that one `updatedAt` is no
//! newer than the cursor, nothing has been created or changed and the pass is over — about a kilobyte, rather
//! than the page of fifty a listing costs. When something has changed, the listing is read newest-modified first
//! and stops a little past the newest change already known (`OVERLAP_S`, measured on Seerr's clock, so ours does
//! not matter). That alone would miss a request whose *media* became available, because that does not always touch
//! the request. So every request finstats believes to be open is looked at again — the ones a fresh
//! "pending/processing" listing returns anyway, the rest one by one — but only while something *is* open, and at
//! most every `OPEN_EVERY_S`: a title that has arrived is news within the quarter hour, not within the minute.
//! Once a day everything is listed, and only if that listing was complete does a request that is no longer there
//! get its `removed_at`.
//!
//! **Who.** A Seerr user is the Jellyfin user whose id Seerr reports (`jellyfinUserId`), failing that the one
//! with that Jellyfin user name. Never the display name or e-mail: those are typed by the person, and a
//! name of one's own choosing must not put requests on somebody else's page.

use std::collections::HashSet;

use anyhow::{Result, bail};
use serde_json::Value;

use crate::db::rusqlite::{Connection, params};
use crate::db::{self, norm_id};
use crate::services::{self, Kind, Service};
use crate::state::App;

const PAGE: usize = 50;
/// What the listing of *what changed* asks for. It stops at the cursor, so a page of fifty is almost all rows the
/// pass is going to throw away: one request was made, not fifty.
const SMALL_PAGE: usize = 10;
/// A first read or a full listing stops here: 10,000 requests.
const MAX_PAGES: usize = 200;
const OVERLAP_S: i64 = 600;
const FULL_EVERY_S: i64 = 86_400;
/// How often the requests that are still on their way are looked at again.
const OPEN_EVERY_S: i64 = 900;
/// Open requests that the listings did not return are asked for one by one, this many per pass.
const RECHECK_MAX: usize = 60;
/// Titles looked up per pass, and how long one rests before finstats looks again.
///
/// A request that is still moving is worth another look soon: Seerr creates it before Sonarr or Radarr have
/// been told about it, so the first look often finds nothing through no fault of anyone's. One that is
/// settled — declined, failed, or here — is not going to turn up in an Arr app now.
const LOOKUPS_PER_PASS: usize = 30;
const LOOKUP_REST_S: i64 = 30 * 60;
const LOOKUP_REST_SETTLED_S: i64 = 24 * 3600;

pub const MEDIA_AVAILABLE: i64 = 5;

#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub id: i64,
    pub media_type: String,
    pub tmdb_id: Option<i64>,
    pub tvdb_id: Option<i64>,
    pub imdb_id: Option<String>,
    pub seasons: Vec<i64>,
    pub is_4k: bool,
    pub status: i64,
    pub media_status: i64,
    pub requested_at: i64,
    pub updated_at: i64,
    pub media_added_at: Option<i64>,
    pub seerr_user_id: Option<i64>,
    pub seerr_user_name: Option<String>,
    pub jellyfin_user_id: Option<String>,
    pub jellyfin_username: Option<String>,
    pub jellyfin_media_id: Option<String>,
}

fn text(v: &Value) -> Option<String> {
    v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

fn positive(v: &Value) -> Option<i64> {
    v.as_i64().filter(|n| *n > 0)
}

/// One element of `results`. A 4K request is about the 4K copy: its state and its Jellyfin item are the `…4k` ones.
pub fn parse(v: &Value) -> Option<Request> {
    let media = &v["media"];
    let is_4k = v["is4k"].as_bool().unwrap_or(false);
    let pick = |plain: &str, uhd: &str| if is_4k && !media[uhd].is_null() { &media[uhd] } else { &media[plain] };
    let who = &v["requestedBy"];
    let media_type = match v["type"].as_str().or_else(|| media["mediaType"].as_str())? {
        "movie" => "movie",
        "tv" => "tv",
        _ => return None,
    };
    let mut seasons: Vec<i64> = v["seasons"].as_array().map(|a| a.iter().filter_map(|s| s["seasonNumber"].as_i64()).collect()).unwrap_or_default();
    seasons.sort_unstable();
    seasons.dedup();
    Some(Request {
        id: positive(&v["id"])?,
        media_type: media_type.to_string(),
        tmdb_id: positive(&media["tmdbId"]),
        tvdb_id: positive(&media["tvdbId"]),
        imdb_id: text(&media["imdbId"]),
        seasons,
        is_4k,
        status: v["status"].as_i64()?,
        media_status: pick("status", "status4k").as_i64().unwrap_or(1),
        requested_at: v["createdAt"].as_str().and_then(db::parse_ts)?,
        updated_at: v["updatedAt"].as_str().and_then(db::parse_ts).or_else(|| v["createdAt"].as_str().and_then(db::parse_ts))?,
        media_added_at: media["mediaAddedAt"].as_str().and_then(db::parse_ts),
        seerr_user_id: positive(&who["id"]),
        seerr_user_name: text(&who["displayName"]).or_else(|| text(&who["jellyfinUsername"])).or_else(|| text(&who["username"])),
        jellyfin_user_id: text(&who["jellyfinUserId"]).map(|s| norm_id(&s)).filter(|s| s.len() == 32),
        jellyfin_username: text(&who["jellyfinUsername"]),
        jellyfin_media_id: text(pick("jellyfinMediaId", "jellyfinMediaId4k")).map(|s| norm_id(&s)).filter(|s| s.len() == 32),
    })
}

/// When it arrived, from the best witness there is: Seerr's own note, else the file in the library, else the
/// moment finstats first saw it available. Never before it was asked for (an "arrived after −3 days" helps nobody):
/// something that was already there when it was requested arrived at once.
pub fn available_at(requested_at: i64, media_status: i64, media_added_at: Option<i64>, item_added_at: Option<i64>, seen_available_at: Option<i64>) -> Option<i64> {
    if media_status != MEDIA_AVAILABLE {
        return None;
    }
    media_added_at.or(item_added_at).or(seen_available_at).map(|t| t.max(requested_at))
}

/// Newest-modified first: once a request is older than what was known last time (less an overlap), the rest is too.
pub fn past_the_cursor(updated_at: i64, cursor: Option<i64>) -> bool {
    cursor.is_some_and(|c| updated_at < c - OVERLAP_S)
}

/// Has anything been created or changed since the last pass? `newest` is the one `updatedAt` the probe read.
/// No requests at all is not "something changed": the last one may have been deleted, but only a whole listing
/// may say that, and that is the daily listing's business rather than this pass's.
pub fn anything_changed(newest: Option<i64>, cursor: Option<i64>) -> bool {
    match (newest, cursor) {
        (Some(newest), Some(cursor)) => newest > cursor,
        (Some(_), None) => true,
        (None, _) => false,
    }
}

/// Is it time to look at what is still on its way? Only while finstats knows of something open: a request that
/// has just been made reaches it through the listing above, because making one does touch `updatedAt`.
pub fn recheck_open(now: i64, open: usize, open_at: Option<i64>) -> bool {
    open > 0 && now - open_at.unwrap_or(0) >= OPEN_EVERY_S
}

// ---------------------------------------------------------------- reading

struct Listing {
    rows: Vec<Request>,
    /// Every page answered, and as many results came as Seerr said there were.
    complete: bool,
}

/// One page of `/api/v1/request`, newest-modified first.
async fn page(app: &App, svc: &Service, filter: &str, take: usize, skip: usize) -> Result<(Vec<Value>, Option<u64>)> {
    let query = [("take", take.to_string()), ("skip", skip.to_string()), ("filter", filter.to_string()), ("sort", "modified".to_string()), ("sortDirection", "desc".to_string())];
    let mut body = services::get_json(app, svc, "/api/v1/request", &query).await?;
    let total = body["pageInfo"]["results"].as_u64();
    let Some(results) = body.get_mut("results").and_then(Value::as_array_mut) else { bail!("{} did not answer its requests like Seerr", svc.url) };
    Ok((std::mem::take(results), total))
}

/// The one question every pass starts with: when was anything last touched? One row, so the answer costs about a
/// kilobyte. Read straight out of the JSON rather than through `parse`, because a row this pass may not understand
/// (a kind of request finstats has no word for) still says that *something* moved.
async fn newest_change(app: &App, svc: &Service) -> Result<Option<i64>> {
    let (results, _) = page(app, svc, "all", 1, 0).await?;
    Ok(results.first().and_then(|v| v["updatedAt"].as_str().or_else(|| v["createdAt"].as_str())).and_then(db::parse_ts))
}

async fn list(app: &App, svc: &Service, filter: &str, stop_at: Option<i64>, take: usize) -> Result<Listing> {
    let mut rows = vec![];
    let mut total = None;
    for page_no in 0..MAX_PAGES {
        let (results, count) = page(app, svc, filter, take, page_no * take).await?;
        total = count.or(total);
        let got = results.len();
        let mut reached = false;
        for r in results.iter().filter_map(parse) {
            reached |= past_the_cursor(r.updated_at, stop_at);
            rows.push(r);
        }
        if got < take || reached {
            let complete = !reached && total.is_none_or(|t| t as usize == (page_no * take + got));
            return Ok(Listing { rows, complete });
        }
    }
    Ok(Listing { rows, complete: false })
}

fn store(conn: &Connection, service_id: i64, r: &Request, now: i64) -> Result<()> {
    conn.prepare_cached(
        "INSERT INTO requests(service_id, request_id, media_type, tmdb_id, tvdb_id, imdb_id, seasons, is_4k, status, media_status, requested_at, updated_at, media_added_at,
                              seen_available_at, seerr_user_id, seerr_user_name, jellyfin_user_id, jellyfin_username, jellyfin_media_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, CASE WHEN ?10 = 5 THEN ?14 END, ?15, ?16, ?17, ?18, ?19)
         ON CONFLICT(service_id, request_id) DO UPDATE SET
            media_type = excluded.media_type, tmdb_id = excluded.tmdb_id, tvdb_id = excluded.tvdb_id, imdb_id = excluded.imdb_id, seasons = excluded.seasons, is_4k = excluded.is_4k,
            status = excluded.status, media_status = excluded.media_status, requested_at = excluded.requested_at, updated_at = excluded.updated_at, media_added_at = excluded.media_added_at,
            -- the first time it is seen available, and only then
            seen_available_at = CASE WHEN excluded.media_status = 5 THEN COALESCE(requests.seen_available_at, excluded.seen_available_at) ELSE NULL END,
            seerr_user_id = excluded.seerr_user_id, seerr_user_name = excluded.seerr_user_name, jellyfin_user_id = excluded.jellyfin_user_id,
            jellyfin_username = excluded.jellyfin_username, jellyfin_media_id = excluded.jellyfin_media_id, removed_at = NULL",
    )?
    .execute(params![
        service_id, r.id, r.media_type, r.tmdb_id, r.tvdb_id, r.imdb_id, serde_json::to_string(&r.seasons)?, r.is_4k, r.status, r.media_status, r.requested_at, r.updated_at, r.media_added_at,
        now, r.seerr_user_id, r.seerr_user_name, r.jellyfin_user_id, r.jellyfin_username, r.jellyfin_media_id
    ])?;
    Ok(())
}

fn setting_i64(conn: &Connection, key: &str) -> Result<Option<i64>> {
    Ok(db::get_setting(conn, key)?.and_then(|v| v.parse().ok()))
}

/// What `available_at` is worked out from: (service, request, asked at, media status, Seerr's note, first seen available, added to the library).
type Arrival = (i64, i64, i64, i64, Option<i64>, Option<i64>, Option<i64>);

/// Whose request, which title, and when it arrived: everything that is derived, for every request. Cheap, and
/// run again after each read of Seerr and of the library, so that nothing depends on the order things happened in.
pub fn link(conn: &Connection) -> Result<()> {
    // The Jellyfin user: by the id Seerr gives, else by exactly one user of that Jellyfin name.
    conn.execute_batch(
        "UPDATE requests SET user_id = COALESCE(
            (SELECT u.id FROM users u WHERE u.id = requests.jellyfin_user_id),
            (SELECT CASE WHEN COUNT(*) = 1 THEN MIN(u.id) END FROM users u WHERE requests.jellyfin_username IS NOT NULL AND u.name = requests.jellyfin_username COLLATE NOCASE));
         -- The title in the library: the item Seerr names, else by id (the lowest of several copies, always the same one).
         UPDATE requests SET item_id = COALESCE(
            (SELECT i.id FROM items i WHERE i.id = requests.jellyfin_media_id AND i.removed = 0),
            (SELECT MIN(x.item_id) FROM item_external x JOIN items i ON i.id = x.item_id AND i.type = 'Movie' WHERE requests.media_type = 'movie' AND x.source = 'Tmdb' AND x.value = CAST(requests.tmdb_id AS TEXT)),
            (SELECT MIN(x.item_id) FROM item_external x JOIN items i ON i.id = x.item_id AND i.type = 'Movie' WHERE requests.media_type = 'movie' AND x.source = 'Imdb' AND x.value = requests.imdb_id),
            (SELECT MIN(x.item_id) FROM item_external x JOIN items i ON i.id = x.item_id AND i.type = 'Series' WHERE requests.media_type = 'tv' AND x.source = 'Tvdb' AND x.value = CAST(requests.tvdb_id AS TEXT)),
            (SELECT MIN(x.item_id) FROM item_external x JOIN items i ON i.id = x.item_id AND i.type = 'Series' WHERE requests.media_type = 'tv' AND x.source = 'Tmdb' AND x.value = CAST(requests.tmdb_id AS TEXT)));
         -- A title that is in the library is called what the library calls it.
         UPDATE requests SET title = (SELECT i.name FROM items i WHERE i.id = requests.item_id), year = COALESCE((SELECT i.production_year FROM items i WHERE i.id = requests.item_id), year)
         WHERE item_id IS NOT NULL AND EXISTS (SELECT 1 FROM items i WHERE i.id = requests.item_id);",
    )?;
    // When it arrived. For a show: when the first episode of a requested season was added.
    let rows: Vec<Arrival> = conn
        .prepare(
            "SELECT r.service_id, r.request_id, r.requested_at, r.media_status, r.media_added_at, r.seen_available_at,
                    CASE r.media_type
                      WHEN 'movie' THEN (SELECT MIN(i.date_created) FROM items i WHERE i.id = r.item_id)
                      ELSE (SELECT MIN(e.date_created) FROM items e WHERE e.series_id = r.item_id AND e.type = 'Episode' AND e.removed = 0
                              AND (r.seasons = '[]' OR e.parent_index_number IN (SELECT value FROM json_each(r.seasons))))
                    END
             FROM requests r",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)))?
        .collect::<Result<_, _>>()?;
    let mut stmt = conn.prepare_cached("UPDATE requests SET available_at = ?1 WHERE service_id = ?2 AND request_id = ?3 AND available_at IS NOT ?1")?;
    for (service_id, request_id, requested_at, media_status, media_added, seen, item_added) in rows {
        stmt.execute(params![available_at(requested_at, media_status, media_added, item_added, seen), service_id, request_id])?;
    }
    Ok(())
}

/// What is still on its way, newest change first — and how many there are, which is what decides whether this
/// pass asks Seerr about open requests at all.
fn open_requests(conn: &Connection, sid: i64) -> Result<Vec<i64>> {
    Ok(conn
        .prepare("SELECT request_id FROM requests WHERE service_id = ?1 AND removed_at IS NULL AND status IN (1, 2) AND media_status < 5 ORDER BY updated_at DESC")?
        .query_map([sid], |r| r.get(0))?
        .collect::<Result<_, _>>()?)
}

async fn read(app: &App, svc: &Service) -> Result<(usize, usize)> {
    let sid = svc.id;
    let (cursor_key, full_key, open_key) = (format!("service:{sid}:requests_cursor"), format!("service:{sid}:requests_full_at"), format!("service:{sid}:requests_open_at"));
    let (ck, fk, ok) = (cursor_key.clone(), full_key.clone(), open_key.clone());
    let (cursor, full_at, open_at, mut open) = app.db.call(move |c| Ok((setting_i64(c, &ck)?, setting_i64(c, &fk)?, setting_i64(c, &ok)?, open_requests(c, sid)?))).await?;
    let now = db::now();
    let full = cursor.is_none() || now - full_at.unwrap_or(0) >= FULL_EVERY_S;
    // One row tells a quiet pass that it is already done. A full listing is going to read everything anyway.
    let changed = full || anything_changed(newest_change(app, svc).await?, cursor);
    let recheck = !full && recheck_open(now, open.len(), open_at);
    if !changed && !recheck {
        return Ok((0, 0));
    }

    let mut listing = if changed { list(app, svc, "all", if full { None } else { cursor }, if full { PAGE } else { SMALL_PAGE }).await? } else { Listing { rows: vec![], complete: false } };
    let everything = full && listing.complete;
    let mut seen: HashSet<i64> = listing.rows.iter().map(|r| r.id).collect();
    if recheck {
        // What is open changes without the request noticing: read those listings whole.
        for filter in ["pending", "processing"] {
            // A whole listing is read to its end, so a full page costs nothing extra: `take` is a limit, not a padding.
            for r in list(app, svc, filter, None, PAGE).await?.rows {
                if seen.insert(r.id) {
                    listing.rows.push(r);
                }
            }
        }
        // And what finstats believes to be open, but no listing returned, is asked for by name.
        open.retain(|id| !seen.contains(id));
        for id in open.into_iter().take(RECHECK_MAX) {
            if let Ok(v) = services::get_json(app, svc, &format!("/api/v1/request/{id}"), &[]).await
                && let Some(r) = parse(&v)
            {
                seen.insert(r.id);
                listing.rows.push(r);
            }
        }
    }

    let newest = listing.rows.iter().map(|r| r.updated_at).max();
    let count = listing.rows.len();
    let rows = listing.rows;
    let removed = app
        .db
        .call(move |c| {
            let tx = c.transaction()?;
            for r in &rows {
                store(&tx, sid, r, now)?;
            }
            let mut removed = 0;
            if everything {
                // Only a listing that was whole may say that something is gone.
                let ids: Vec<i64> = tx.prepare("SELECT request_id FROM requests WHERE service_id = ?1 AND removed_at IS NULL")?.query_map([sid], |r| r.get(0))?.collect::<Result<_, _>>()?;
                for id in ids.into_iter().filter(|id| !seen.contains(id)) {
                    removed += tx.execute("UPDATE requests SET removed_at = ?1 WHERE service_id = ?2 AND request_id = ?3", params![now, sid, id])?;
                }
                db::set_setting(&tx, &full_key, &now.to_string())?;
            }
            if let Some(n) = newest.max(cursor) {
                db::set_setting(&tx, &cursor_key, &n.to_string())?;
            }
            if recheck {
                db::set_setting(&tx, &open_key, &now.to_string())?;
            }
            crate::pipeline::link(&tx)?;
            tx.commit()?;
            Ok(removed)
        })
        .await?;
    Ok((count, removed))
}

/// Titles for requests that are not in the library: Sonarr or Radarr first (they also have the poster), then Seerr.
async fn name_the_unnamed(app: &App, seerr: &Service) -> Result<usize> {
    let sid = seerr.id;
    let now = db::now();
    let todo: Vec<(i64, String, Option<i64>, Option<i64>)> = app
        .db
        .call(move |c| {
            Ok(c.prepare(
                "SELECT request_id, media_type, tmdb_id, tvdb_id FROM requests
                 WHERE service_id = ?1 AND removed_at IS NULL AND item_id IS NULL AND (title IS NULL OR arr_media_id IS NULL)
                   AND COALESCE(looked_up_at, 0) < (CASE WHEN status IN (3, 4) OR media_status >= 5 THEN ?2 ELSE ?3 END)
                 ORDER BY requested_at DESC LIMIT ?4",
            )?
            .query_map(params![sid, now - LOOKUP_REST_SETTLED_S, now - LOOKUP_REST_S, LOOKUPS_PER_PASS as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<Result<_, _>>()?)
        })
        .await?;
    let mut named = 0;
    for (request_id, media_type, tmdb, tvdb) in todo {
        // Could not ask: leave the row exactly as it was, so the next pass tries again in a few minutes
        // instead of writing off the title until tomorrow.
        let found = match crate::arr::find(app, &media_type, tmdb, tvdb).await {
            Ok(found) => found,
            Err(e) => {
                tracing::debug!("looking up a title: {e:#}");
                continue;
            }
        };
        let (mut title, mut year) = (found.as_ref().map(|f| f.title.clone()), found.as_ref().and_then(|f| f.year));
        if title.is_none()
            && let Some(tmdb) = tmdb
            && let Ok(v) = services::get_json(app, seerr, &format!("/api/v1/{}/{tmdb}", if media_type == "movie" { "movie" } else { "tv" }), &[]).await
        {
            title = text(&v["title"]).or_else(|| text(&v["name"]));
            year = v["releaseDate"].as_str().or_else(|| v["firstAirDate"].as_str()).and_then(|d| d.get(..4)?.parse().ok());
        }
        named += title.is_some() as usize;
        let arr = found.map(|f| (f.service_id, f.media_id));
        app.db
            .call(move |c| {
                Ok(c.execute(
                    "UPDATE requests SET title = COALESCE(?1, title), year = COALESCE(?2, year), arr_service_id = COALESCE(?3, arr_service_id), arr_media_id = COALESCE(?4, arr_media_id), looked_up_at = ?5
                     WHERE service_id = ?6 AND request_id = ?7",
                    params![title, year, arr.map(|a| a.0), arr.map(|a| a.1), now, sid, request_id],
                )?)
            })
            .await?;
    }
    Ok(named)
}

pub async fn sync_requests(app: &App) -> Result<String> {
    const ID: &str = "sync_requests";
    let list = services::enabled(app, |k| k == Kind::Seerr);
    if list.is_empty() {
        return Ok("No Seerr connected".into());
    }
    let (mut total, mut gone, mut failed) = (0, 0, vec![]);
    for svc in &list {
        app.tasks.update(ID, format!("Reading {}", svc.name), None);
        match read(app, svc).await {
            Ok((n, removed)) => {
                total += n;
                gone += removed;
                services::record(app, svc.id, Ok(None)).await;
                app.tasks.update(ID, "Looking up titles", None);
                if let Err(e) = name_the_unnamed(app, svc).await {
                    tracing::debug!("titles for {}: {e}", svc.name);
                }
            }
            Err(e) => {
                tracing::warn!("requests of {}: {e}", svc.name);
                services::record(app, svc.id, Err(e.to_string())).await;
                failed.push(svc.name.clone());
            }
        }
    }
    // Whose wish a download is may have changed.
    app.downloads_wake.notify_waiters();
    if failed.len() == list.len() {
        bail!("{} did not answer", failed.join(", "));
    }
    if total == 0 && gone == 0 {
        // A pass that found nothing has read a single row to find that out, and stopped there.
        return Ok("Nothing new".into());
    }
    Ok(format!("{total} requests read{}", if gone > 0 { format!(", {gone} no longer in Seerr") } else { String::new() }))
}

/// Is there anything to read at all? (The scheduler asks before starting the task.)
pub fn connected(app: &App) -> bool {
    !services::enabled(app, |k| k == Kind::Seerr).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> Value {
        json!({ "id": 41, "status": 2, "type": "tv", "is4k": false, "createdAt": "2026-09-01T10:00:00.000Z", "updatedAt": "2026-09-02T08:30:00.000Z",
            "seasons": [{ "seasonNumber": 2, "status": 2 }, { "seasonNumber": 1, "status": 2 }, { "seasonNumber": 2, "status": 2 }],
            "media": { "mediaType": "tv", "tmdbId": 880001, "tvdbId": 371002, "imdbId": null, "status": 3, "status4k": 1, "mediaAddedAt": null, "jellyfinMediaId": "F233952B-E918-EE70-35BC-326CB3684478" },
            "requestedBy": { "id": 3, "displayName": "Jonas the Great", "username": "jonas-local", "jellyfinUsername": "jonas", "jellyfinUserId": "E9E623EDC98A301673D2422D813F629D", "email": "j@example.invalid" } })
    }

    #[test]
    fn a_request_is_read_with_seerrs_own_names() {
        let r = parse(&sample()).unwrap();
        assert_eq!((r.id, r.media_type.as_str(), r.status, r.media_status), (41, "tv", 2, 3));
        assert_eq!(r.seasons, [1, 2], "each season once, in order");
        assert_eq!((r.tmdb_id, r.tvdb_id, r.imdb_id.clone()), (Some(880001), Some(371002), None));
        assert_eq!(r.jellyfin_user_id.as_deref(), Some("e9e623edc98a301673d2422d813f629d"));
        assert_eq!(r.jellyfin_media_id.as_deref(), Some("f233952be918ee7035bc326cb3684478"));
        assert_eq!(r.seerr_user_name.as_deref(), Some("Jonas the Great"));
        assert_eq!(r.requested_at, db::parse_ts("2026-09-01T10:00:00Z").unwrap());

        // A 4K request is about the 4K copy.
        let mut uhd = sample();
        uhd["is4k"] = json!(true);
        uhd["media"]["status4k"] = json!(5);
        uhd["media"]["jellyfinMediaId4k"] = json!("00000000000000000000000000000abc");
        let r = parse(&uhd).unwrap();
        assert_eq!((r.media_status, r.jellyfin_media_id.as_deref()), (5, Some("00000000000000000000000000000abc")));

        for broken in [json!({}), json!({ "id": 1, "status": 1, "type": "music", "createdAt": "2026-09-01T10:00:00Z" }), json!({ "id": 1, "type": "movie", "status": 1 })] {
            assert!(parse(&broken).is_none(), "{broken}");
        }
    }

    #[test]
    fn it_arrived_when_the_best_witness_says_and_never_before_it_was_asked_for() {
        let asked = 1_000_000;
        assert_eq!(available_at(asked, 3, Some(asked + 50), Some(asked + 60), Some(asked + 70)), None, "not available yet");
        assert_eq!(available_at(asked, 5, Some(asked + 50), Some(asked + 60), Some(asked + 70)), Some(asked + 50), "Seerr's own note first");
        assert_eq!(available_at(asked, 5, None, Some(asked + 60), Some(asked + 70)), Some(asked + 60), "then the file in the library");
        assert_eq!(available_at(asked, 5, None, None, Some(asked + 70)), Some(asked + 70), "then the moment finstats saw it");
        assert_eq!(available_at(asked, 5, Some(asked - 9_000), None, None), Some(asked), "it was there already: it arrived at once");
        assert_eq!(available_at(asked, 5, None, None, None), None);
    }

    #[test]
    fn a_pass_with_nothing_to_read_asks_one_question_and_stops() {
        let cursor = Some(10_000);
        assert!(!anything_changed(Some(10_000), cursor), "the newest change is the one already known: nothing to read");
        assert!(anything_changed(Some(10_001), cursor), "something was created or edited");
        assert!(anything_changed(Some(500), None), "a first read has no cursor to compare against");
        assert!(!anything_changed(None, cursor), "no requests at all is for the daily listing to notice, not this pass");

        // What is open is asked about now and then, and only while something is open.
        let now = 100_000;
        assert!(recheck_open(now, 2, None), "never looked: look now");
        assert!(recheck_open(now, 2, Some(now - OPEN_EVERY_S)));
        assert!(!recheck_open(now, 2, Some(now - OPEN_EVERY_S + 1)), "looked a moment ago");
        assert!(!recheck_open(now, 0, None), "nothing is on its way: Seerr is left alone");
    }

    #[test]
    fn reading_stops_a_little_past_what_was_known() {
        assert!(!past_the_cursor(500, None), "a first read takes everything");
        assert!(!past_the_cursor(10_000, Some(10_000)) && !past_the_cursor(10_000 - OVERLAP_S, Some(10_000)), "the overlap is read again: reading twice changes nothing");
        assert!(past_the_cursor(10_000 - OVERLAP_S - 1, Some(10_000)));
    }

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        for m in crate::db::MIGRATIONS {
            c.execute_batch(m).unwrap();
        }
        c.execute_batch(
            "INSERT INTO services(id, kind, name, url, secret, created_at) VALUES (1, 'seerr', 'Seerr', 'http://nas:5055', 'k', 1);
             INSERT INTO users(id, name, is_admin, updated_at) VALUES ('e9e623edc98a301673d2422d813f629d', 'jonas', 0, 1), ('aaaa0000aaaa0000aaaa0000aaaa0000', 'Maria', 0, 1), ('bbbb0000bbbb0000bbbb0000bbbb0000', 'sam', 0, 1), ('cccc0000cccc0000cccc0000cccc0000', 'SAM', 0, 1);
             INSERT INTO items(id, type, name, production_year, provider_ids, date_created, removed, updated_at) VALUES
                ('f233952be918ee7035bc326cb3684478', 'Series', 'Low Orbit', 2021, '{\"Tvdb\":\"371002\"}', 100, 0, 1),
                ('m-hd', 'Movie', 'Winterline', 2026, '{\"Tmdb\":\"990001\"}', 5000, 0, 1), ('m-4k', 'Movie', 'Winterline', 2026, '{\"Tmdb\":\"990001\"}', 9000, 0, 1);
             INSERT INTO items(id, type, name, series_id, parent_index_number, date_created, removed, updated_at) VALUES
                ('ep-s1', 'Episode', 'Old', 'f233952be918ee7035bc326cb3684478', 1, 200, 0, 1), ('ep-s2', 'Episode', 'New', 'f233952be918ee7035bc326cb3684478', 2, 7000, 0, 1);",
        )
        .unwrap();
        c
    }

    #[test]
    fn a_request_finds_its_person_its_title_and_the_day_it_arrived() {
        let c = conn();
        let mut tv = parse(&sample()).unwrap();
        tv.media_status = 5;
        tv.seasons = vec![2];
        tv.requested_at = 6000;
        store(&c, 1, &tv, 8000).unwrap();
        let film = Request { id: 42, media_type: "movie".into(), tmdb_id: Some(990001), tvdb_id: None, jellyfin_media_id: None, jellyfin_user_id: None, jellyfin_username: Some("maria".into()), media_status: 5, requested_at: 4000, seasons: vec![], ..tv.clone() };
        store(&c, 1, &film, 8000).unwrap();
        // Two Jellyfin users answer to "sam": nobody gets the request. And a display name never links anyone.
        let twins = Request { id: 43, jellyfin_user_id: None, jellyfin_username: Some("sam".into()), tmdb_id: Some(1), tvdb_id: None, jellyfin_media_id: None, media_status: 2, ..tv.clone() };
        store(&c, 1, &twins, 8000).unwrap();
        let named = Request { id: 44, jellyfin_user_id: None, jellyfin_username: None, seerr_user_name: Some("jonas".into()), tmdb_id: Some(2), tvdb_id: None, jellyfin_media_id: None, media_status: 2, ..tv.clone() };
        store(&c, 1, &named, 8000).unwrap();
        crate::pipeline::link(&c).unwrap();

        let row = |id: i64| -> (Option<String>, Option<String>, Option<String>, Option<i64>) {
            c.query_row("SELECT user_id, item_id, title, available_at FROM requests WHERE request_id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap()
        };
        assert_eq!(row(41), (Some("e9e623edc98a301673d2422d813f629d".into()), Some("f233952be918ee7035bc326cb3684478".into()), Some("Low Orbit".into()), Some(7000)), "season 2 arrived with its first episode, not with season 1");
        assert_eq!(row(42), (Some("aaaa0000aaaa0000aaaa0000aaaa0000".into()), Some("m-4k".into()), Some("Winterline".into()), Some(9000)), "by Jellyfin name, whatever its case; the lowest of two copies");
        assert_eq!(row(43).0, None, "an ambiguous name links nobody");
        assert_eq!(row(44).0, None, "a display name is not an identity");

        // Seen again, still available: the moment it was first seen stays.
        store(&c, 1, &tv, 99_999).unwrap();
        assert_eq!(c.query_row("SELECT seen_available_at FROM requests WHERE request_id = 41", [], |r| r.get::<_, i64>(0)).unwrap(), 8000);
    }
}
