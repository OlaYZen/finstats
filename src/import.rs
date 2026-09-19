//! Import of Jellystat backups (`.jsonl`, plus the older single-document `.json`).
//!
//! What the backup means, learned from real exports:
//! - `ActivityDateInserted` is when the play *ended*; start = end − `PlaybackDuration`.
//! - For episodes `NowPlayingItemId` is the *series*; the episode itself is `EpisodeId`.
//! - `PlayState.PositionTicks` is usually the position when Jellystat first noticed the
//!   session, not where playback stopped, so it is not imported as a position.
//! - bigint columns (`PlaybackDuration`, `RunTimeTicks`, `Size`…) arrive as strings.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::Value;

use crate::db::{Db, norm_id, parse_ts, rusqlite::Connection, rusqlite::OptionalExtension, rusqlite::params};
use crate::media::{self, Streams, int, ticks_to_s};
use crate::playback::PlayRecord;
use crate::state::Tasks;
use crate::sync::backfill_playbacks;

const TASK: &str = "import";

#[derive(Debug, Default, Serialize, Clone)]
pub struct ImportResult {
    pub plays_imported: u64,
    pub plays_skipped: u64,
    pub users: u64,
    pub libraries: u64,
    pub items: u64,
    pub seasons: u64,
    pub episodes: u64,
    pub item_info: u64,
    pub unknown_rows: u64,
}

fn opt_str(v: &Value) -> Option<String> {
    v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

fn phase_label(table: &str) -> &'static str {
    match table {
        "jf_libraries" => "Reading libraries",
        "jf_library_items" => "Reading movies, shows and music",
        "jf_library_seasons" => "Reading seasons",
        "jf_library_episodes" => "Reading episodes",
        "jf_users" => "Reading users",
        "jf_playback_activity" => "Importing playback history",
        "jf_item_info" => "Reading file details",
        _ => "Reading backup",
    }
}

/// Blocking. Parses the backup at `path` and writes it in a single transaction, so a
/// failed import leaves the database exactly as it was.
pub fn run(db: &Db, path: &Path, tasks: Option<&Tasks>) -> Result<ImportResult> {
    let mut file = File::open(path).context("opening the uploaded backup")?;
    let total_bytes = file.metadata()?.len().max(1);

    // The older Jellystat format is one JSON array; the current one is a record per line.
    let mut first = [0u8; 1];
    let legacy = loop {
        if file.read(&mut first)? == 0 {
            bail!("The file is empty");
        }
        if !first[0].is_ascii_whitespace() {
            break first[0] == b'[';
        }
    };
    file.seek(SeekFrom::Start(0))?;

    let mut conn = db.conn()?;
    let tx = conn.transaction()?;
    let mut res = ImportResult::default();
    let report = |msg: &str, p: f64| {
        if let Some(t) = tasks {
            t.update(TASK, msg, Some(p));
        }
    };

    if legacy {
        report("Reading backup (older single-file format, this needs more memory)", 0.05);
        let doc: Value = serde_json::from_reader(BufReader::with_capacity(1 << 20, file))
            .context("The file is not a valid Jellystat backup (JSON could not be parsed)")?;
        let sections = doc.as_array().context("The file is not a Jellystat backup")?;
        let count = sections.len().max(1);
        for (i, section) in sections.iter().enumerate() {
            let Some(obj) = section.as_object() else { continue };
            for (table, rows) in obj {
                report(phase_label(table), 0.1 + 0.8 * i as f64 / count as f64);
                for row in rows.as_array().map(Vec::as_slice).unwrap_or_default() {
                    import_row(&tx, table, row, &mut res)?;
                }
            }
        }
    } else {
        let mut reader = BufReader::with_capacity(1 << 20, file);
        let mut line = String::new();
        let mut read_bytes = 0u64;
        let mut line_no = 0u64;
        let mut recognised = 0u64;
        let mut current_table = String::new();
        loop {
            line.clear();
            let n = reader.read_line(&mut line)?;
            if n == 0 {
                break;
            }
            read_bytes += n as u64;
            line_no += 1;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let mut v: Value = match serde_json::from_str(trimmed) {
                Ok(v) => v,
                Err(e) if line_no <= 3 && recognised == 0 => {
                    bail!("This does not look like a Jellystat backup (line {line_no}: {e})")
                }
                Err(e) => bail!("The backup is damaged at line {line_no}: {e}"),
            };
            match v["type"].as_str() {
                Some("table") => {
                    recognised += 1;
                    current_table = v["table"].as_str().unwrap_or_default().to_string();
                }
                Some("row") => {
                    recognised += 1;
                    let table = v["table"].as_str().map(str::to_string).unwrap_or_else(|| current_table.clone());
                    let data = v["data"].take();
                    import_row(&tx, &table, &data, &mut res)?;
                }
                _ => res.unknown_rows += 1,
            }
            if line_no % 2000 == 0 {
                report(phase_label(&current_table), 0.95 * read_bytes as f64 / total_bytes as f64);
            }
            if line_no == 50 && recognised == 0 {
                bail!("This does not look like a Jellystat backup: none of the first lines are backup records");
            }
        }
        if recognised == 0 {
            bail!("This does not look like a Jellystat backup: no backup records found");
        }
    }

    report("Linking plays to your library", 0.96);
    finalize(&tx)?;
    report("Saving", 0.99);
    tx.commit()?;
    let window = crate::state::Settings::load(&conn)?.group_window_s;
    crate::groups::detect(&mut conn, window, None)?;
    conn.execute_batch("PRAGMA optimize;")?;
    Ok(res)
}

fn import_row(conn: &Connection, table: &str, d: &Value, res: &mut ImportResult) -> Result<()> {
    match table {
        "jf_playback_activity" => import_play(conn, d, res),
        "jf_users" => {
            let Some(id) = d["Id"].as_str() else { return Ok(()) };
            res.users += conn
                .prepare_cached(
                    "INSERT OR IGNORE INTO users(id, name, is_admin, image_tag, last_login_at, last_activity_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)",
                )?
                .execute(params![
                    norm_id(id),
                    d["Name"].as_str().unwrap_or("Unknown"),
                    d["IsAdministrator"].as_bool().unwrap_or(false),
                    opt_str(&d["PrimaryImageTag"]),
                    d["LastLoginDate"].as_str().and_then(parse_ts),
                    d["LastActivityDate"].as_str().and_then(parse_ts),
                ])? as u64;
            Ok(())
        }
        "jf_libraries" => {
            let Some(id) = d["Id"].as_str() else { return Ok(()) };
            res.libraries += conn
                .prepare_cached(
                    "INSERT OR IGNORE INTO libraries(id, name, collection_type, image_tag, removed, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, 0)",
                )?
                .execute(params![
                    norm_id(id),
                    d["Name"].as_str().unwrap_or("Library"),
                    opt_str(&d["CollectionType"]),
                    opt_str(&d["ImageTagsPrimary"]),
                    d["archived"].as_bool().unwrap_or(false),
                ])? as u64;
            Ok(())
        }
        "jf_library_items" => {
            let Some(id) = d["Id"].as_str() else { return Ok(()) };
            let genres = d["Genres"].as_array().filter(|g| !g.is_empty()).map(|g| Value::Array(g.clone()).to_string());
            res.items += conn
                .prepare_cached(
                    "INSERT OR IGNORE INTO items(id, library_id, type, name, runtime_s, production_year, premiere_date,
                        date_created, community_rating, genres, image_tag, backdrop_tag, removed, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 0)",
                )?
                .execute(params![
                    norm_id(id),
                    d["ParentId"].as_str().map(norm_id),
                    d["Type"].as_str().unwrap_or("Unknown"),
                    d["Name"].as_str().unwrap_or("Unknown"),
                    ticks_to_s(&d["RunTimeTicks"]),
                    d["ProductionYear"].as_i64(),
                    opt_str(&d["PremiereDate"]),
                    d["DateCreated"].as_str().and_then(parse_ts),
                    d["CommunityRating"].as_f64(),
                    genres,
                    opt_str(&d["ImageTagsPrimary"]),
                    opt_str(&d["BackdropImageTags"]),
                    d["archived"].as_bool().unwrap_or(false),
                ])? as u64;
            Ok(())
        }
        "jf_library_seasons" => {
            let Some(id) = d["Id"].as_str() else { return Ok(()) };
            res.seasons += conn
                .prepare_cached(
                    "INSERT OR IGNORE INTO items(id, type, name, series_id, series_name, index_number, removed, updated_at)
                     VALUES (?1, 'Season', ?2, ?3, ?4, ?5, ?6, 0)",
                )?
                .execute(params![
                    norm_id(id),
                    d["Name"].as_str().unwrap_or("Season"),
                    d["SeriesId"].as_str().map(norm_id),
                    opt_str(&d["SeriesName"]),
                    d["IndexNumber"].as_i64(),
                    d["archived"].as_bool().unwrap_or(false),
                ])? as u64;
            Ok(())
        }
        "jf_library_episodes" => {
            // `Id` is Jellystat's own composite key; `EpisodeId` is the Jellyfin item.
            let Some(id) = d["EpisodeId"].as_str() else { return Ok(()) };
            res.episodes += conn
                .prepare_cached(
                    "INSERT OR IGNORE INTO items(id, type, name, series_id, season_id, series_name, index_number,
                        parent_index_number, runtime_s, production_year, premiere_date, date_created,
                        community_rating, official_rating, removed, updated_at)
                     VALUES (?1, 'Episode', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, 0)",
                )?
                .execute(params![
                    norm_id(id),
                    d["Name"].as_str().unwrap_or("Episode"),
                    d["SeriesId"].as_str().map(norm_id),
                    d["SeasonId"].as_str().map(norm_id),
                    opt_str(&d["SeriesName"]),
                    d["IndexNumber"].as_i64(),
                    d["ParentIndexNumber"].as_i64(),
                    ticks_to_s(&d["RunTimeTicks"]),
                    d["ProductionYear"].as_i64(),
                    opt_str(&d["PremiereDate"]),
                    d["DateCreated"].as_str().and_then(parse_ts),
                    d["CommunityRating"].as_f64(),
                    opt_str(&d["OfficialRating"]),
                    d["archived"].as_bool().unwrap_or(false),
                ])? as u64;
            Ok(())
        }
        "jf_item_info" => {
            let Some(id) = d["Id"].as_str() else { return Ok(()) };
            let st = Streams::extract(&d["MediaStreams"], &Value::Null);
            let path = opt_str(&d["Path"]);
            let container = path.as_deref().and_then(|p| p.rsplit_once('.')).map(|(_, ext)| ext.to_lowercase()).filter(|e| e.len() <= 5);
            // Only fills gaps: a live sync knows better than an old backup.
            res.item_info += conn
                .prepare_cached(
                    "UPDATE items SET path = ?2, size_bytes = ?3, bitrate = ?4, container = COALESCE(container, ?5),
                        video_codec = ?6, width = ?7, height = ?8, video_range = ?9, audio_codec = ?10, audio_channels = ?11
                     WHERE id = ?1 AND size_bytes IS NULL",
                )?
                .execute(params![
                    norm_id(id),
                    path,
                    int(&d["Size"]),
                    int(&d["Bitrate"]),
                    container,
                    st.video_codec,
                    st.width,
                    st.height,
                    st.video_range,
                    st.audio_codec,
                    st.audio_channels,
                ])? as u64;
            Ok(())
        }
        // Raw Playback Reporting plugin rows: Jellystat has already folded these into
        // jf_playback_activity (flagged `imported`), so taking them again would double count.
        "jf_playback_reporting_plugin_data" => Ok(()),
        _ => {
            res.unknown_rows += 1;
            Ok(())
        }
    }
}

fn import_play(conn: &Connection, d: &Value, res: &mut ImportResult) -> Result<()> {
    let (Some(source_id), Some(user_id), Some(np_id)) = (d["Id"].as_str(), d["UserId"].as_str(), d["NowPlayingItemId"].as_str()) else {
        res.plays_skipped += 1;
        return Ok(());
    };
    let Some(ended_at) = d["ActivityDateInserted"].as_str().and_then(parse_ts) else {
        res.plays_skipped += 1;
        return Ok(());
    };
    let duration_s = int(&d["PlaybackDuration"]).unwrap_or(0).max(0);
    let play_state = &d["PlayState"];
    let streams = Streams::extract(&d["MediaStreams"], play_state);
    let transcode = media::compact_transcode(&d["TranscodingInfo"]);
    let reported = d["PlayMethod"].as_str().or_else(|| play_state["PlayMethod"].as_str());
    let play_method = media::effective_play_method(reported, transcode.as_ref());

    let episode_id = d["EpisodeId"].as_str().filter(|s| !s.is_empty());
    let (item_id, series_id) = match episode_id {
        Some(ep) => (norm_id(ep), Some(norm_id(np_id))),
        None => (norm_id(np_id), None),
    };
    let item_type = if episode_id.is_some() {
        "Episode".to_string()
    } else {
        conn.prepare_cached("SELECT type FROM items WHERE id = ?1")?
            .query_row([&item_id], |r| r.get::<_, String>(0))
            .optional()?
            // Not in the library and Jellystat records no type, so go by what the stream looked like:
            // a picture without a file container is a Live TV channel, with one it was a film.
            .unwrap_or_else(|| {
                match (streams.has_video(), opt_str(&d["OriginalContainer"]).is_some()) {
                    (true, false) => "TvChannel",
                    (true, true) => "Movie",
                    (false, _) => "Audio",
                }
                .to_string()
            })
    };

    let rec = PlayRecord {
        source: "jellystat",
        source_id: Some(format!("jellystat:{source_id}")),
        active: false,
        user_id: norm_id(user_id),
        user_name: d["UserName"].as_str().unwrap_or("Unknown").to_string(),
        item_id,
        item_name: d["NowPlayingItemName"].as_str().unwrap_or("Unknown").to_string(),
        item_type,
        series_id,
        series_name: opt_str(&d["SeriesName"]),
        season_id: d["SeasonId"].as_str().map(norm_id),
        season_number: None,
        episode_number: None,
        started_at: ended_at - duration_s,
        ended_at,
        duration_s,
        paused_s: 0,
        position_s: None,
        runtime_s: None,
        client: opt_str(&d["Client"]),
        device_name: opt_str(&d["DeviceName"]),
        device_id: opt_str(&d["DeviceId"]),
        app_version: opt_str(&d["ApplicationVersion"]),
        remote_ip: opt_str(&d["RemoteEndPoint"]),
        play_method,
        container: opt_str(&d["OriginalContainer"]).map(|c| c.split(',').next().unwrap_or_default().to_string()),
        streams,
        transcode,
        pause_count: 0,
        seek_count: 0,
        start_position_s: None,
    };
    match rec.insert(conn)? {
        Some(_) => res.plays_imported += 1,
        None => res.plays_skipped += 1,
    }
    Ok(())
}

/// Cross-table fix-ups that don't depend on the order tables appear in the backup.
fn finalize(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "UPDATE items SET library_id = (SELECT s.library_id FROM items s WHERE s.id = items.series_id)
         WHERE library_id IS NULL AND series_id IS NOT NULL;

         UPDATE playbacks SET
            season_number  = (SELECT parent_index_number FROM items WHERE items.id = playbacks.item_id),
            episode_number = (SELECT index_number FROM items WHERE items.id = playbacks.item_id)
         WHERE source = 'jellystat' AND item_type = 'Episode' AND episode_number IS NULL;

         UPDATE playbacks SET item_type = (SELECT type FROM items WHERE items.id = playbacks.item_id)
         WHERE source = 'jellystat' AND item_type <> 'Episode'
           AND EXISTS (SELECT 1 FROM items WHERE items.id = playbacks.item_id AND items.type <> playbacks.item_type);",
    )?;
    backfill_playbacks(conn)?;
    crate::db::backfill_is_local(conn)?;
    Ok(())
}
