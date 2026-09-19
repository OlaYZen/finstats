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
            "sync_server" => sync_server(&app, &jf).await,
            "sync_userdata" => sync_userdata(&app, &jf).await,
            other => Err(anyhow!("unknown task {other}")),
        };
        app.tasks.finish(id, outcome.map(|m| (m, None)));
    });
    true
}

const LIGHT_EVERY_S: i64 = 900;
const SCAN_CHECK_EVERY_S: i64 = 300;
/// Even when following Jellyfin's scan, re-read once a week: real-time monitoring adds
/// items without the scan task ever running.
const SAFETY_NET_S: i64 = 7 * 86_400;

/// Write a backup in the background and thin out old ones. Returns false when one is already being written.
pub fn run_backup(app: &App, automatic: bool) -> bool {
    const ID: &str = "backup";
    if !app.tasks.try_start(ID, "Writing backup") {
        return false;
    }
    let app = app.clone();
    tokio::task::spawn_blocking(move || {
        let dir = crate::backup::dir(&app.data_dir);
        let outcome = crate::backup::export(&app.db, &dir, Some((&app.tasks, ID))).map(|made| {
            let removed = crate::backup::prune(&dir, app.settings().backup_keep.clamp(1, 100) as usize);
            tracing::info!("{} backup written: {} ({} plays){}", if automatic { "automatic" } else { "manual" }, made.name, made.plays,
                if removed > 0 { format!("; removed {removed} old") } else { String::new() });
            (format!("{} plays, {:.1} MB", made.plays, made.size_bytes as f64 / 1e6), serde_json::to_value(&made).ok())
        });
        if let Err(e) = &outcome {
            tracing::error!("backup failed: {e:#}");
        }
        app.tasks.finish(ID, outcome);
    });
    true
}

/// finstats only ever *reads* from Jellyfin; it never starts a scan there. By default the
/// (expensive) library read simply follows Jellyfin's own "Scan Media Library" task.
pub async fn scheduler(app: App) {
    let mut last_light = 0i64;
    let mut last_scan_check = 0i64;
    let mut last_library: i64 = app
        .db
        .call(|c| Ok(db::get_setting(c, "library_synced_at")?.and_then(|v| v.parse().ok()).unwrap_or(0)))
        .await
        .unwrap_or(0);
    loop {
        if let Some(jf) = app.jellyfin() {
            let now = db::now();
            let settings = app.settings();
            if now - last_light >= LIGHT_EVERY_S {
                // Users, the activity log and server details are tiny; keep them fresh.
                last_light = now;
                refresh_server_info(&app).await;
                crate::network::refresh(&app).await;
                spawn(&app, "sync_users");
                spawn(&app, "sync_events");
                spawn(&app, "sync_server");
            }

            let timer_due = now - last_library >= settings.sync_interval_h.clamp(1, 168) * 3600;
            let due = if !settings.follow_jellyfin_scan {
                timer_due
            } else if now - last_scan_check >= SCAN_CHECK_EVERY_S {
                last_scan_check = now;
                match jf.library_scan_status().await {
                    Ok(Some((true, _))) => false, // mid-scan: a read now would be half old, half new
                    Ok(Some((false, Some(finished)))) => {
                        if finished > last_library {
                            tracing::info!("Jellyfin finished a library scan; reading the library");
                        }
                        finished > last_library || now - last_library >= SAFETY_NET_S
                    }
                    Ok(Some((false, None))) => timer_due, // Jellyfin has never scanned: fall back to the timer
                    Ok(None) => {
                        tracing::debug!("Jellyfin lists no library scan task; using the timer");
                        timer_due
                    }
                    Err(_) => false,                                 // Jellyfin unreachable; the collector already reports that
                }
            } else {
                false
            };
            // Automatic backups: when the newest one on disk is older than the interval. A database
            // without a single play has nothing worth keeping yet.
            if settings.backup_every_d > 0 {
                let dir = crate::backup::dir(&app.data_dir);
                let newest = crate::backup::newest_at(&dir).unwrap_or(0);
                if now - newest >= settings.backup_every_d * 86_400 {
                    let has_plays = app.db.call(|c| Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM playbacks)", [], |r| r.get::<_, bool>(0))?)).await.unwrap_or(false);
                    if has_plays {
                        run_backup(&app, true);
                    }
                }
            }

            if due && spawn(&app, "sync_libraries") {
                last_library = now;
                spawn(&app, "sync_userdata");
            }
        }
        tokio::select! {
            _ = app.wake.notified() => { last_scan_check = 0; }
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
    let provider_ids = it["ProviderIds"].as_object().filter(|p| !p.is_empty()).map(|_| it["ProviderIds"].to_string());
    let studios = it["Studios"]
        .as_array()
        .map(|a| a.iter().filter_map(|x| x["Name"].as_str()).collect::<Vec<_>>())
        .filter(|a| !a.is_empty())
        .map(|a| json!(a).to_string());
    let framerate = source["MediaStreams"]
        .as_array()
        .and_then(|a| a.iter().find(|x| x["Type"].as_str() == Some("Video")))
        .and_then(|v| v["AverageFrameRate"].as_f64().or_else(|| v["RealFrameRate"].as_f64()))
        .map(|f| (f * 1000.0).round() / 1000.0);
    conn.prepare_cached(
        "INSERT INTO items(id, library_id, type, name, series_id, season_id, series_name, index_number, parent_index_number,
            album, album_artist, runtime_s, production_year, premiere_date, date_created, community_rating, official_rating,
            genres, overview, image_tag, backdrop_tag, container, path, size_bytes, bitrate,
            video_codec, width, height, video_range, audio_codec, audio_channels, provider_ids, studios, bit_depth, framerate, removed, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23,
            ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?33, ?34, ?35, ?36, 0, ?32)
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
            audio_channels = excluded.audio_channels, provider_ids = excluded.provider_ids, studios = excluded.studios,
            bit_depth = excluded.bit_depth, framerate = excluded.framerate, removed = 0, updated_at = excluded.updated_at",
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
        provider_ids,
        studios,
        streams.bit_depth,
        framerate,
    ])?;
    Ok(())
}

/// How many actors per title are kept: the billed cast, not every walk-on.
const MAX_ACTORS: usize = 12;

/// Replaces what is known about one title's actors and directors with Jellyfin's current answer.
pub fn store_people(conn: &Connection, it: &Value) -> Result<()> {
    let Some(item_id) = it["Id"].as_str().map(norm_id) else { return Ok(()) };
    conn.prepare_cached("DELETE FROM item_people WHERE item_id = ?1")?.execute([&item_id])?;
    let Some(people) = it["People"].as_array() else { return Ok(()) };
    let mut actors = 0usize;
    for (sort, p) in people.iter().enumerate() {
        let kind = match p["Type"].as_str() {
            Some("Actor") if actors < MAX_ACTORS => {
                actors += 1;
                "Actor"
            }
            Some("Director") => "Director",
            _ => continue,
        };
        let (Some(person_id), Some(name)) = (p["Id"].as_str().map(norm_id), opt_str(&p["Name"])) else { continue };
        conn.prepare_cached(
            "INSERT OR IGNORE INTO item_people(item_id, person_id, kind, name, role, sort, has_image) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?
        .execute(params![item_id, person_id, kind, name, opt_str(&p["Role"]), sort as i64, opt_str(&p["PrimaryImageTag"]).is_some()])?;
    }
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
    crate::relink::relink_orphans(conn)?;
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
        // Cast and crew, for films and shows only. A failure here never fails the library read.
        let mut start = 0usize;
        loop {
            let items = match jf.people_page(&lib_id, start, PAGE).await {
                Ok(items) => items,
                Err(e) => {
                    tracing::warn!("reading cast and crew of {lib_name} failed: {e:#}");
                    break;
                }
            };
            let got = items.len();
            if got == 0 {
                break;
            }
            app.tasks.update(ID, format!("{lib_name}: cast and crew"), None);
            app.db
                .call(move |c| {
                    let tx = c.transaction()?;
                    for it in &items {
                        store_people(&tx, it)?;
                    }
                    tx.commit()?;
                    Ok(())
                })
                .await?;
            start += got;
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
    app.db
        .call(move |c| {
            backfill_playbacks(c)?;
            db::set_setting(c, "library_synced_at", &started.to_string())
        })
        .await?;
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

// ---------------------------------------------------------------- server details & devices

fn folder(label: &str, kind: &str, f: &Value) -> Option<Value> {
    let (free, used) = (f["FreeSpace"].as_i64()?, f["UsedSpace"].as_i64()?);
    (free >= 0 && used >= 0 && free + used > 0).then(|| json!({ "label": label, "path": f["Path"], "free_bytes": free, "used_bytes": used, "kind": kind }))
}

async fn sync_server(app: &App, jf: &Jellyfin) -> Result<String> {
    const ID: &str = "sync_server";
    app.tasks.update(ID, "Reading server details", None);
    let info = jf.system_info().await?;
    let storage_raw = jf.storage().await;
    let plugins = jf.plugins().await.unwrap_or_default();
    let tasks = jf.scheduled_tasks().await.unwrap_or_default();
    app.tasks.update(ID, "Reading devices", Some(0.6));
    let devices = jf.devices().await.unwrap_or_default();

    let mut storage: Vec<Value> = vec![];
    if let Some(st) = &storage_raw {
        for lib in st["Libraries"].as_array().map(Vec::as_slice).unwrap_or_default() {
            let name = lib["Name"].as_str().unwrap_or("Library");
            storage.extend(lib["Folders"].as_array().map(Vec::as_slice).unwrap_or_default().iter().filter_map(|f| folder(name, "library", f)));
        }
        for (key, label) in [("ProgramDataFolder", "Program data"), ("CacheFolder", "Cache"), ("TranscodingTempFolder", "Transcodes"), ("InternalMetadataFolder", "Metadata"), ("LogFolder", "Logs")] {
            storage.extend(folder(label, "system", &st[key]));
        }
    }
    let snapshot = json!({
        "fetched_at": db::now(),
        "info": {
            "server_name": info["ServerName"], "version": info["Version"],
            "operating_system": opt_str(&info["OperatingSystemDisplayName"]).or_else(|| opt_str(&info["OperatingSystem"])),
            "architecture": info["SystemArchitecture"],
            "has_update_available": info["HasUpdateAvailable"].as_bool().unwrap_or(false),
            "has_pending_restart": info["HasPendingRestart"].as_bool().unwrap_or(false),
            "transcoding_temp_path": info["TranscodingTempPath"], "cache_path": info["CachePath"],
            "program_data_path": info["ProgramDataPath"], "log_path": info["LogPath"],
            "encoder_location": info["EncoderLocation"],
        },
        "storage": storage,
        "plugins": plugins.iter().map(|p| json!({ "name": p["Name"], "version": p["Version"], "status": p["Status"], "description": p["Description"] })).collect::<Vec<_>>(),
        "scheduled_tasks": tasks.iter().map(|t| {
            let last = &t["LastExecutionResult"];
            let (start, end) = (last["StartTimeUtc"].as_str().and_then(parse_ts), last["EndTimeUtc"].as_str().and_then(parse_ts));
            json!({
                "name": t["Name"], "category": t["Category"], "state": t["State"],
                "last_result": last["Status"], "last_run_at": end.or(start),
                "last_duration_s": start.zip(end).map(|(s, e)| (e - s).max(0)),
            })
        }).collect::<Vec<_>>(),
    });

    let device_count = devices.len();
    app.db
        .call(move |c| {
            let now = db::now();
            let tx = c.transaction()?;
            db::set_setting(&tx, "server_info", &snapshot.to_string())?;
            {
                let mut stmt = tx.prepare(
                    "INSERT INTO devices(device_id, user_id, device_name, client, app_version, last_user_name, first_seen, last_seen)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
                     ON CONFLICT(device_id, user_id) DO UPDATE SET device_name = excluded.device_name, client = excluded.client,
                        app_version = excluded.app_version, last_user_name = excluded.last_user_name,
                        last_seen = MAX(devices.last_seen, excluded.last_seen)",
                )?;
                for d in &devices {
                    let (Some(id), Some(user)) = (d["Id"].as_str(), d["LastUserId"].as_str()) else { continue };
                    stmt.execute(params![
                        id,
                        norm_id(user),
                        opt_str(&d["CustomName"]).or_else(|| opt_str(&d["Name"])),
                        opt_str(&d["AppName"]),
                        opt_str(&d["AppVersion"]),
                        opt_str(&d["LastUserName"]),
                        d["DateLastActivity"].as_str().and_then(parse_ts).unwrap_or(now),
                    ])?;
                }
            }
            tx.commit()?;
            Ok(())
        })
        .await?;
    Ok(format!("{device_count} devices, {} plugins", plugins.len()))
}

// ---------------------------------------------------------------- per-user played & favourite flags

/// Jellyfin remembers what each user has finished or favourited, including everything from
/// before finstats existed. That is what makes "never watched" trustworthy.
async fn sync_userdata(app: &App, jf: &Jellyfin) -> Result<String> {
    const ID: &str = "sync_userdata";
    let users: Vec<(String, String)> = app
        .db
        .call(|c| {
            let mut stmt = c.prepare("SELECT id, name FROM users WHERE removed = 0 ORDER BY name")?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
            Ok(rows)
        })
        .await?;
    let mut total = 0usize;
    let count = users.len().max(1);
    for (n, (user_id, name)) in users.into_iter().enumerate() {
        app.tasks.update(ID, format!("Reading what {name} has watched"), Some(n as f64 / count as f64));
        let mut rows: Vec<Value> = vec![];
        for (filter, types) in [("IsPlayed", "Movie,Episode"), ("IsFavorite", "Movie,Series,Episode")] {
            let mut start = 0;
            loop {
                let page = jf.user_items_page(&user_id, filter, types, start, 1000).await?;
                let got = page.len();
                rows.extend(page);
                start += got;
                if got < 1000 {
                    break;
                }
            }
        }
        total += rows.len();
        let uid = user_id.clone();
        app.db
            .call(move |c| {
                let tx = c.transaction()?;
                tx.execute("DELETE FROM user_items WHERE user_id = ?1", [&uid])?;
                {
                    let mut stmt = tx.prepare(
                        "INSERT INTO user_items(user_id, item_id, played, is_favorite, play_count, last_played_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                         ON CONFLICT(user_id, item_id) DO UPDATE SET played = MAX(played, excluded.played), is_favorite = MAX(is_favorite, excluded.is_favorite)",
                    )?;
                    for it in &rows {
                        let Some(id) = it["Id"].as_str() else { continue };
                        let ud = &it["UserData"];
                        stmt.execute(params![
                            uid,
                            norm_id(id),
                            ud["Played"].as_bool().unwrap_or(false),
                            ud["IsFavorite"].as_bool().unwrap_or(false),
                            ud["PlayCount"].as_i64().unwrap_or(0),
                            ud["LastPlayedDate"].as_str().and_then(parse_ts),
                        ])?;
                    }
                }
                tx.commit()?;
                Ok(())
            })
            .await?;
    }
    Ok(format!("{total} played or favourite items"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_billed_cast_and_directors_and_replaces_them_on_the_next_read() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE item_people(item_id TEXT NOT NULL, person_id TEXT NOT NULL, kind TEXT NOT NULL, name TEXT NOT NULL, role TEXT,
                                      sort INTEGER NOT NULL, has_image INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (item_id, person_id, kind));",
        )
        .unwrap();
        let mut people: Vec<Value> = (0..20).map(|n| json!({ "Id": format!("a{n}"), "Name": format!("Actor {n}"), "Type": "Actor", "Role": "Extra" })).collect();
        people[0]["PrimaryImageTag"] = json!("tag");
        people.push(json!({ "Id": "d1", "Name": "Jane Doe", "Type": "Director" }));
        people.push(json!({ "Id": "w1", "Name": "A Writer", "Type": "Writer" }));
        // The same person acting in and directing a title is two credits.
        people.push(json!({ "Id": "a0", "Name": "Actor 0", "Type": "Director" }));
        store_people(&conn, &json!({ "Id": "AB-CD", "People": people })).unwrap();

        let count = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(count("SELECT COUNT(*) FROM item_people WHERE kind = 'Actor'"), MAX_ACTORS as i64);
        assert_eq!(count("SELECT COUNT(*) FROM item_people WHERE kind = 'Director'"), 2);
        assert_eq!(count("SELECT COUNT(*) FROM item_people WHERE item_id = 'abcd'"), MAX_ACTORS as i64 + 2);
        assert_eq!(count("SELECT has_image FROM item_people WHERE person_id = 'a0' AND kind = 'Actor'"), 1);

        store_people(&conn, &json!({ "Id": "AB-CD", "People": [{ "Id": "d2", "Name": "John Roe", "Type": "Director" }] })).unwrap();
        assert_eq!(count("SELECT COUNT(*) FROM item_people"), 1);
    }
}
