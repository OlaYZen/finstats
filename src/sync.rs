//! Periodic copies of Jellyfin's users, libraries/items and server activity log.

use std::time::Duration;

use anyhow::{Result, anyhow};
use serde_json::{Value, json};

use crate::db::{self, norm_id, parse_ts, rusqlite::Connection, rusqlite::params};
use crate::jellyfin::Jellyfin;
use crate::media::{Streams, int, ticks_to_s};
use crate::state::App;

const PAGE: usize = 500;
/// Library kinds that only reference items living in other libraries.
const SKIPPED_COLLECTIONS: [&str; 2] = ["boxsets", "playlists"];

fn opt_str(v: &Value) -> Option<String> {
    v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

/// Runs a task unless it is already running. Returns false in that case.
pub fn spawn(app: &App, id: &'static str) -> bool {
    let Some(jf) = app.jellyfin() else { return false };
    if !app.tasks.try_start(id, "Starting…") {
        return false;
    }
    let app = app.clone();
    tokio::spawn(async move {
        let outcome = match id {
            "sync_users" => sync_users(&app, &jf).await,
            "sync_libraries" => sync_libraries(&app, &jf).await,
            "sync_events" => sync_events(&app, &jf).await,
            other => Err(anyhow!("unknown task {other}")),
        };
        app.tasks.finish(id, outcome.map(|m| (m, None)));
    });
    true
}

pub async fn scheduler(app: App) {
    let mut last_full = 0i64;
    let mut last_light = 0i64;
    loop {
        if app.is_configured() {
            let now = db::now();
            let interval = app.settings().sync_interval_h.clamp(1, 168) * 3600;
            if now - last_full >= interval {
                last_full = now;
                last_light = now;
                refresh_server_info(&app).await;
                spawn(&app, "sync_users");
                spawn(&app, "sync_events");
                spawn(&app, "sync_libraries");
            } else if now - last_light >= 900 {
                // Users and the activity log are tiny; keep them fresher than the library.
                last_light = now;
                spawn(&app, "sync_users");
                spawn(&app, "sync_events");
            }
        }
        tokio::select! {
            _ = app.wake.notified() => {}
            _ = tokio::time::sleep(Duration::from_secs(60)) => {}
        }
    }
}

async fn refresh_server_info(app: &App) {
    let Some(jf) = app.jellyfin() else { return };
    let Ok(info) = jf.system_info().await else { return };
    let (name, version) = (opt_str(&info["ServerName"]), opt_str(&info["Version"]));
    {
        let mut cfg = app.config.write().unwrap();
        if let Some(c) = cfg.as_mut() {
            if let Some(n) = &name {
                c.server_name = n.clone();
            }
            if let Some(v) = &version {
                c.server_version = v.clone();
            }
        }
    }
    let _ = app
        .db
        .call(move |c| {
            if let Some(n) = name {
                db::set_setting(c, "server_name", &n)?;
            }
            if let Some(v) = version {
                db::set_setting(c, "server_version", &v)?;
            }
            Ok(())
        })
        .await;
}

// ---------------------------------------------------------------- users

async fn sync_users(app: &App, jf: &Jellyfin) -> Result<String> {
    app.tasks.update("sync_users", "Fetching users", None);
    let users = jf.users().await?;
    let count = users.len();
    app.db
        .call(move |c| {
            let now = db::now();
            let tx = c.transaction()?;
            {
                let mut stmt = tx.prepare(
                    "INSERT INTO users(id, name, is_admin, is_disabled, image_tag, last_login_at, last_activity_at, removed, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8)
                     ON CONFLICT(id) DO UPDATE SET name = excluded.name, is_admin = excluded.is_admin,
                        is_disabled = excluded.is_disabled, image_tag = excluded.image_tag,
                        last_login_at = excluded.last_login_at, last_activity_at = excluded.last_activity_at,
                        removed = 0, updated_at = excluded.updated_at",
                )?;
                for u in &users {
                    let Some(id) = u["Id"].as_str() else { continue };
                    stmt.execute(params![
                        norm_id(id),
                        u["Name"].as_str().unwrap_or("Unknown"),
                        u["Policy"]["IsAdministrator"].as_bool().unwrap_or(false),
                        u["Policy"]["IsDisabled"].as_bool().unwrap_or(false),
                        opt_str(&u["PrimaryImageTag"]),
                        u["LastLoginDate"].as_str().and_then(parse_ts),
                        u["LastActivityDate"].as_str().and_then(parse_ts),
                        now,
                    ])?;
                }
            }
            tx.execute("UPDATE users SET removed = 1 WHERE updated_at < ?1", [now])?;
            tx.commit()?;
            Ok(())
        })
        .await?;
    Ok(format!("{count} users"))
}

// ---------------------------------------------------------------- libraries & items

pub fn upsert_item(conn: &Connection, library_id: &str, it: &Value, now: i64) -> Result<()> {
    let Some(id) = it["Id"].as_str() else { return Ok(()) };
    let source = &it["MediaSources"][0];
    let streams = Streams::extract(&source["MediaStreams"], &Value::Null);
    let genres = it["Genres"].as_array().filter(|g| !g.is_empty()).map(|g| Value::Array(g.clone()).to_string());
    conn.prepare_cached(
        "INSERT INTO items(id, library_id, type, name, series_id, season_id, series_name, index_number, parent_index_number,
            album, album_artist, runtime_s, production_year, premiere_date, date_created, community_rating, official_rating,
            genres, overview, image_tag, backdrop_tag, container, path, size_bytes, bitrate,
            video_codec, width, height, video_range, audio_codec, audio_channels, removed, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23,
            ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, 0, ?32)
         ON CONFLICT(id) DO UPDATE SET library_id = excluded.library_id, type = excluded.type, name = excluded.name,
            series_id = excluded.series_id, season_id = excluded.season_id, series_name = excluded.series_name,
            index_number = excluded.index_number, parent_index_number = excluded.parent_index_number,
            album = excluded.album, album_artist = excluded.album_artist, runtime_s = excluded.runtime_s,
            production_year = excluded.production_year, premiere_date = excluded.premiere_date,
            date_created = excluded.date_created, community_rating = excluded.community_rating,
            official_rating = excluded.official_rating, genres = excluded.genres, overview = excluded.overview,
            image_tag = excluded.image_tag, backdrop_tag = excluded.backdrop_tag, container = excluded.container,
            path = excluded.path, size_bytes = excluded.size_bytes, bitrate = excluded.bitrate,
            video_codec = excluded.video_codec, width = excluded.width, height = excluded.height,
            video_range = excluded.video_range, audio_codec = excluded.audio_codec,
            audio_channels = excluded.audio_channels, removed = 0, updated_at = excluded.updated_at",
    )?
    .execute(params![
        norm_id(id),
        library_id,
        it["Type"].as_str().unwrap_or("Unknown"),
        it["Name"].as_str().unwrap_or("Unknown"),
        it["SeriesId"].as_str().map(norm_id),
        it["SeasonId"].as_str().map(norm_id),
        opt_str(&it["SeriesName"]),
        it["IndexNumber"].as_i64(),
        it["ParentIndexNumber"].as_i64(),
        opt_str(&it["Album"]),
        opt_str(&it["AlbumArtist"]),
        ticks_to_s(&it["RunTimeTicks"]),
        it["ProductionYear"].as_i64(),
        opt_str(&it["PremiereDate"]),
        it["DateCreated"].as_str().and_then(parse_ts),
        it["CommunityRating"].as_f64(),
        opt_str(&it["OfficialRating"]),
        genres,
        opt_str(&it["Overview"]),
        opt_str(&it["ImageTags"]["Primary"]),
        opt_str(&it["BackdropImageTags"][0]),
        opt_str(&source["Container"]).or_else(|| opt_str(&it["Container"])),
        opt_str(&it["Path"]).or_else(|| opt_str(&source["Path"])),
        int(&source["Size"]),
        int(&source["Bitrate"]),
        streams.video_codec,
        streams.width,
        streams.height,
        streams.video_range,
        streams.audio_codec,
        streams.audio_channels,
        now,
    ])?;
    Ok(())
}

/// Plays recorded before their item was known get their library (and type details) filled in.
pub fn backfill_playbacks(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "UPDATE playbacks SET library_id = COALESCE(
                (SELECT library_id FROM items WHERE items.id = playbacks.item_id),
                (SELECT library_id FROM items WHERE items.id = playbacks.series_id))
         WHERE library_id IS NULL;
         UPDATE playbacks SET runtime_s = (SELECT runtime_s FROM items WHERE items.id = playbacks.item_id)
         WHERE runtime_s IS NULL;",
    )?;
    Ok(())
}

async fn sync_libraries(app: &App, jf: &Jellyfin) -> Result<String> {
    const ID: &str = "sync_libraries";
    app.tasks.update(ID, "Fetching libraries", None);
    let started = db::now();
    let folders = jf.virtual_folders().await?;

    let mut libraries: Vec<(String, String)> = vec![];
    let lib_rows: Vec<Value> = folders
        .iter()
        .filter(|f| !SKIPPED_COLLECTIONS.contains(&f["CollectionType"].as_str().unwrap_or("")))
        .filter_map(|f| {
            let id = norm_id(f["ItemId"].as_str()?);
            let name = f["Name"].as_str().unwrap_or("Library").to_string();
            libraries.push((id.clone(), name.clone()));
            Some(json!({ "id": id, "name": name, "collection_type": f["CollectionType"], "image_tag": f["PrimaryImageItemId"] }))
        })
        .collect();

    app.db
        .call(move |c| {
            let tx = c.transaction()?;
            for l in &lib_rows {
                tx.execute(
                    "INSERT INTO libraries(id, name, collection_type, image_tag, removed, updated_at) VALUES (?1, ?2, ?3, ?4, 0, ?5)
                     ON CONFLICT(id) DO UPDATE SET name = excluded.name, collection_type = excluded.collection_type,
                        image_tag = excluded.image_tag, removed = 0, updated_at = excluded.updated_at",
                    params![l["id"].as_str(), l["name"].as_str(), l["collection_type"].as_str(), l["image_tag"].as_str(), started],
                )?;
            }
            tx.execute("UPDATE libraries SET removed = 1 WHERE updated_at < ?1", [started])?;
            tx.commit()?;
            Ok(())
        })
        .await?;

    let mut total_items = 0usize;
    let lib_count = libraries.len();
    for (n, (lib_id, lib_name)) in libraries.into_iter().enumerate() {
        let mut start = 0usize;
        let mut total = 0usize;
        loop {
            let (items, reported) = jf.items_page(&lib_id, start, PAGE).await?;
            if start == 0 {
                total = reported;
            }
            let got = items.len();
            if got == 0 {
                break;
            }
            let lib = lib_id.clone();
            app.db
                .call(move |c| {
                    let now = db::now();
                    let tx = c.transaction()?;
                    for it in &items {
                        upsert_item(&tx, &lib, it, now)?;
                    }
                    tx.commit()?;
                    Ok(())
                })
                .await?;
            start += got;
            total_items += got;
            let within = if total > 0 { start as f64 / total as f64 } else { 1.0 };
            app.tasks.update(
                ID,
                format!("{lib_name}: {start} of {} items", total.max(start)),
                Some((n as f64 + within.min(1.0)) / lib_count.max(1) as f64),
            );
            if got < PAGE {
                break;
            }
        }
        // Only after a library was read completely is "not seen" proof of removal.
        let lib = lib_id.clone();
        app.db
            .call(move |c| {
                c.execute("UPDATE items SET removed = 1 WHERE library_id = ?1 AND updated_at < ?2 AND removed = 0", params![lib, started])?;
                Ok(())
            })
            .await?;
    }

    app.tasks.update(ID, "Linking plays to libraries", Some(1.0));
    app.db.call(|c| backfill_playbacks(c)).await?;
    Ok(format!("{total_items} items in {lib_count} libraries"))
}

// ---------------------------------------------------------------- server activity log

async fn sync_events(app: &App, jf: &Jellyfin) -> Result<String> {
    const ID: &str = "sync_events";
    app.tasks.update(ID, "Fetching server activity", None);
    let newest: Option<i64> = app.db.call(|c| Ok(c.query_row("SELECT MAX(date) FROM server_events", [], |r| r.get(0))?)).await?;
    let min_date = newest
        .and_then(|t| chrono::DateTime::from_timestamp(t - 60, 0))
        .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));

    let mut start = 0usize;
    let mut added = 0usize;
    loop {
        let (entries, total) = jf.activity_log(start, PAGE, min_date.as_deref()).await?;
        let got = entries.len();
        if got == 0 {
            break;
        }
        added += app
            .db
            .call(move |c| {
                let tx = c.transaction()?;
                let mut n = 0;
                {
                    let mut stmt = tx.prepare(
                        "INSERT OR IGNORE INTO server_events(id, date, name, overview, short_overview, type, severity, user_id, item_id)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    )?;
                    for e in &entries {
                        let (Some(id), Some(date)) = (e["Id"].as_i64(), e["Date"].as_str().and_then(parse_ts)) else { continue };
                        let user = e["UserId"].as_str().map(norm_id).filter(|u| u.chars().any(|ch| ch != '0'));
                        n += stmt.execute(params![
                            id,
                            date,
                            e["Name"].as_str().unwrap_or(""),
                            opt_str(&e["Overview"]),
                            opt_str(&e["ShortOverview"]),
                            opt_str(&e["Type"]),
                            opt_str(&e["Severity"]),
                            user,
                            e["ItemId"].as_str().map(norm_id),
                        ])?;
                    }
                }
                tx.commit()?;
                Ok(n)
            })
            .await?;
        start += got;
        app.tasks.update(ID, format!("{start} of {} entries", total.max(start)), Some(start as f64 / total.max(start) as f64));
        // Cap the very first backfill; the log can be enormous on old servers.
        if got < PAGE || start >= 20_000 {
            break;
        }
    }
    Ok(format!("{added} new entries"))
}
