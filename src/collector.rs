//! Watches Jellyfin's `/Sessions` and turns what it sees into playback rows.
//!
//! A play is written the moment it is first seen (`active = 1`) and refreshed while it
//! runs, so a crash or restart loses at most a few seconds. Time is only counted while
//! the player is not paused, which makes `duration_s` "time actually watched".

use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::Result;
use serde_json::{Value, json};

use crate::db::{self, norm_id, rusqlite::OptionalExtension, rusqlite::params};
use crate::media::{self, Streams, ticks_to_s};
use crate::playback::{PlayEvent, PlayRecord, insert_events};
use crate::state::App;

const PERSIST_EVERY: Duration = Duration::from_secs(30);
const DEVICE_REFRESH: Duration = Duration::from_secs(300);
/// Plays shorter than this are accidental clicks; they are dropped when they end.
const MIN_KEEP_S: i64 = 2;
/// A position that lands further than this from where steady playback would be is a skip.
const SEEK_TOLERANCE_S: f64 = 20.0;

struct Tracked {
    row_id: i64,
    rec: PlayRecord,
    watched: f64,
    paused: f64,
    is_paused: bool,
    last_tick: Instant,
    last_persist: Instant,
    transcode_progress: Option<f64>,
}

fn opt_str(v: &Value) -> Option<String> {
    v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

/// Build a record from one Jellyfin session. `None` when nothing is playing.
fn record_from_session(s: &Value, now: i64) -> Option<PlayRecord> {
    let item = &s["NowPlayingItem"];
    let item_id = norm_id(item["Id"].as_str()?);
    let user_id = norm_id(s["UserId"].as_str()?);
    let play_state = &s["PlayState"];
    let transcode = media::compact_transcode(&s["TranscodingInfo"]);
    let play_method = media::effective_play_method(play_state["PlayMethod"].as_str(), transcode.as_ref());
    let item_type = item["Type"].as_str().unwrap_or("Unknown").to_string();
    let is_episode = item_type == "Episode";

    Some(PlayRecord {
        source: "live",
        source_id: None,
        active: true,
        user_id,
        user_name: s["UserName"].as_str().unwrap_or("Unknown").to_string(),
        item_id,
        item_name: item["Name"].as_str().unwrap_or("Unknown").to_string(),
        item_type,
        series_id: item["SeriesId"].as_str().map(norm_id),
        series_name: opt_str(&item["SeriesName"]).or_else(|| if is_episode { None } else { opt_str(&item["Album"]) }),
        season_id: item["SeasonId"].as_str().map(norm_id),
        season_number: item["ParentIndexNumber"].as_i64().filter(|_| is_episode),
        episode_number: item["IndexNumber"].as_i64().filter(|_| is_episode),
        started_at: now,
        ended_at: now,
        duration_s: 0,
        paused_s: 0,
        position_s: ticks_to_s(&play_state["PositionTicks"]),
        runtime_s: ticks_to_s(&item["RunTimeTicks"]),
        client: opt_str(&s["Client"]),
        device_name: opt_str(&s["DeviceName"]),
        device_id: opt_str(&s["DeviceId"]),
        app_version: opt_str(&s["ApplicationVersion"]),
        remote_ip: opt_str(&s["RemoteEndPoint"]),
        play_method,
        container: opt_str(&item["Container"]).map(|c| c.split(',').next().unwrap_or_default().to_string()),
        streams: Streams::extract(&item["MediaStreams"], play_state),
        transcode,
        pause_count: 0,
        seek_count: 0,
        start_position_s: ticks_to_s(&play_state["PositionTicks"]),
    })
}

fn live_json(key: &str, t: &Tracked) -> Value {
    let r = &t.rec;
    let st = &r.streams;
    let transcode = r.transcode.as_ref().map(|tc| {
        json!({
            "video_codec": tc["video_codec"], "audio_codec": tc["audio_codec"], "container": tc["container"],
            "is_video_direct": tc["is_video_direct"], "is_audio_direct": tc["is_audio_direct"],
            "hw_accel": tc["hw_accel"], "reasons": tc["reasons"],
            "progress": t.transcode_progress,
        })
    });
    json!({
        "key": key,
        "user_id": r.user_id, "user_name": r.user_name,
        "item_id": r.item_id, "item_name": r.item_name, "item_type": r.item_type,
        "series_id": r.series_id, "series_name": r.series_name,
        "season_number": r.season_number, "episode_number": r.episode_number,
        "image_item_id": if r.item_type == "Episode" { r.series_id.as_ref().unwrap_or(&r.item_id) } else { &r.item_id },
        "position_s": r.position_s, "runtime_s": r.runtime_s,
        "is_paused": t.is_paused,
        "started_at": r.started_at, "watched_s": t.watched.round() as i64,
        "client": r.client, "device_name": r.device_name, "app_version": r.app_version,
        "remote_ip": r.remote_ip,
        "play_method": r.play_method,
        "video": media::video_label(st.video_codec.as_deref(), st.width, st.height, st.video_range.as_deref()),
        "audio": media::audio_label(st.audio_codec.as_deref(), st.audio_channels, None),
        "subtitle": media::subtitle_label(st.subtitle_codec.as_deref(), st.subtitle_language.as_deref()),
        "container": r.container, "bitrate": st.bitrate,
        "transcode": transcode,
    })
}

fn clock(total: i64) -> String {
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

fn audio_of(r: &PlayRecord) -> Option<String> {
    media::audio_label(r.streams.audio_codec.as_deref(), r.streams.audio_channels, r.streams.audio_language.as_deref())
}

fn subtitle_of(r: &PlayRecord) -> Option<String> {
    media::subtitle_label(r.streams.subtitle_codec.as_deref(), r.streams.subtitle_language.as_deref())
}

fn transcode_detail(r: &PlayRecord) -> String {
    let reasons: Vec<&str> = r.transcode.as_ref().and_then(|t| t["reasons"].as_array()).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    if reasons.is_empty() { r.play_method.clone() } else { format!("{}: {}", r.play_method, reasons.join(", ")) }
}

/// What changed between two sightings of the same play. `dt` is the time between them.
fn diff_events(old: &PlayRecord, new: &mut PlayRecord, was_paused: bool, is_paused: bool, dt: f64, now: i64) -> Vec<PlayEvent> {
    let mut out = vec![];
    let ev = |kind, position_s, detail| PlayEvent { at: now, kind, position_s, detail };

    if let (Some(from), Some(to)) = (old.position_s, new.position_s) {
        let expected = from as f64 + if was_paused { 0.0 } else { dt };
        if (to as f64 - expected).abs() > SEEK_TOLERANCE_S + dt * 0.5 {
            new.seek_count += 1;
            out.push(ev("seek", Some(to), Some(format!("{} → {}", clock(expected.round() as i64), clock(to)))));
        }
    }
    if was_paused != is_paused {
        if is_paused {
            new.pause_count += 1;
        }
        out.push(ev(if is_paused { "pause" } else { "resume" }, new.position_s, None));
    }
    let (a_old, a_new) = (audio_of(old), audio_of(new));
    if a_new.is_some() && a_old != a_new {
        out.push(ev("audio", new.position_s, a_new));
    }
    let (s_old, s_new) = (subtitle_of(old), subtitle_of(new));
    if s_old != s_new {
        out.push(ev("subtitle", new.position_s, Some(s_new.unwrap_or_else(|| "Off".into()))));
    }
    if old.play_method != new.play_method || (new.transcode.is_some() && transcode_detail(old) != transcode_detail(new)) {
        out.push(ev("transcode", new.position_s, Some(transcode_detail(new))));
    }
    out
}

pub async fn run(app: App) {
    // Anything still flagged active belongs to a previous process.
    if let Err(e) = app
        .db
        .call(|c| {
            c.execute("DELETE FROM playbacks WHERE active = 1 AND duration_s < ?1", [MIN_KEEP_S])?;
            c.execute("UPDATE playbacks SET active = 0 WHERE active = 1", [])?;
            Ok(())
        })
        .await
    {
        tracing::error!("could not reset active plays: {e:#}");
    }

    let mut tracked: HashMap<String, Tracked> = HashMap::new();
    let mut devices_seen: HashMap<(String, String), Instant> = HashMap::new();
    let mut failures = 0u32;

    loop {
        let settings = app.settings();
        let Some(jf) = app.jellyfin() else {
            tokio::select! {
                _ = app.wake.notified() => {}
                _ = tokio::time::sleep(Duration::from_secs(5)) => {}
            }
            continue;
        };

        match jf.sessions().await {
            Ok(sessions) => {
                if failures > 0 {
                    tracing::info!("connection to Jellyfin restored");
                }
                failures = 0;
                if let Err(e) = tick(&app, &mut tracked, &mut devices_seen, &sessions, settings.merge_window_s).await {
                    tracing::error!("collector tick failed: {e:#}");
                }
                let mut st = app.collector.write().unwrap();
                st.connected = true;
                st.error = None;
                st.last_poll_at = db::now();
                st.active_sessions = tracked.len();
            }
            Err(e) => {
                failures += 1;
                if failures == 1 || failures % 60 == 0 {
                    tracing::warn!("cannot read sessions from Jellyfin: {e:#}");
                }
                let mut st = app.collector.write().unwrap();
                st.connected = false;
                st.error = Some(format!("{e:#}"));
            }
        }

        let base = settings.poll_interval_s.clamp(2, 60) as u64;
        // Polling stays at full rate even when idle: the delay before a new play is noticed is
        // watch time lost. Only an unreachable Jellyfin backs off (up to a minute).
        let wait = if failures > 0 { (base * failures.min(12) as u64).min(60) } else { base };
        tokio::select! {
            _ = app.wake.notified() => {}
            _ = tokio::time::sleep(Duration::from_secs(wait)) => {}
        }
    }
}

async fn tick(
    app: &App,
    tracked: &mut HashMap<String, Tracked>,
    devices_seen: &mut HashMap<(String, String), Instant>,
    sessions: &[Value],
    merge_window_s: i64,
) -> Result<()> {
    let now = db::now();
    let tick_at = Instant::now();
    let mut seen: Vec<String> = Vec::new();
    let mut device_rows: Vec<(String, String, Option<String>, Option<String>, Option<String>, Option<String>)> = vec![];

    for s in sessions {
        // Remember every device that talks to the server, playing or not.
        if let (Some(dev), Some(uid)) = (s["DeviceId"].as_str(), s["UserId"].as_str()) {
            let k = (dev.to_string(), norm_id(uid));
            if devices_seen.get(&k).is_none_or(|t| t.elapsed() > DEVICE_REFRESH) {
                devices_seen.insert(k.clone(), tick_at);
                device_rows.push((k.0, k.1, opt_str(&s["DeviceName"]), opt_str(&s["Client"]), opt_str(&s["ApplicationVersion"]), opt_str(&s["RemoteEndPoint"])));
            }
        }

        let Some(mut rec) = record_from_session(s, now) else { continue };
        let key = format!("{}:{}", s["Id"].as_str().unwrap_or(rec.device_id.as_deref().unwrap_or("?")), rec.item_id);
        let is_paused = s["PlayState"]["IsPaused"].as_bool().unwrap_or(false);
        let transcode_progress = s["TranscodingInfo"]["CompletionPercentage"].as_f64().map(|p| (p / 100.0).clamp(0.0, 1.0));
        seen.push(key.clone());

        if let Some(t) = tracked.get_mut(&key) {
            // Cap the step so a stalled poll loop or suspended host is not counted as viewing.
            let dt = tick_at.duration_since(t.last_tick).as_secs_f64().min(90.0);
            if t.is_paused { t.paused += dt } else { t.watched += dt }
            t.last_tick = tick_at;
            let pause_flipped = t.is_paused != is_paused;
            t.is_paused = is_paused;
            t.transcode_progress = transcode_progress;

            // Keep identity, start and counters; take everything that can change mid-play.
            rec.started_at = t.rec.started_at;
            rec.start_position_s = t.rec.start_position_s;
            rec.pause_count = t.rec.pause_count;
            rec.seek_count = t.rec.seek_count;
            if rec.transcode.is_none() {
                rec.transcode = t.rec.transcode.clone();
            }
            rec.duration_s = t.watched.round() as i64;
            rec.paused_s = t.paused.round() as i64;
            let was_paused = is_paused != pause_flipped;
            let events = diff_events(&t.rec, &mut rec, was_paused, is_paused, dt, now);
            // Once a play has needed transcoding it stays a transcode in the statistics.
            if t.rec.play_method == "Transcode" {
                rec.play_method = "Transcode".into();
            }
            t.rec = rec;

            if !events.is_empty() || t.last_persist.elapsed() >= PERSIST_EVERY {
                t.last_persist = tick_at;
                let (row_id, rec) = (t.row_id, t.rec.clone());
                app.db
                    .call(move |c| {
                        rec.update_progress(c, row_id)?;
                        insert_events(c, row_id, &events)
                    })
                    .await?;
            }
        } else {
            let probe = rec.clone();
            let (row_id, started_at, watched, paused, counters) = app
                .db
                .call(move |c| {
                    // Same person, same item, same device, moments later: that's one viewing.
                    let resumed = c
                        .query_row(
                            "SELECT id, started_at, duration_s, paused_s, pause_count, seek_count, start_position_s FROM playbacks
                             WHERE source = 'live' AND active = 0 AND user_id = ?1 AND item_id = ?2
                               AND device_id IS ?3 AND ended_at >= ?4
                             ORDER BY ended_at DESC LIMIT 1",
                            params![probe.user_id, probe.item_id, probe.device_id, now - merge_window_s],
                            |r| {
                                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?,
                                    (r.get::<_, i64>(4)?, r.get::<_, i64>(5)?, r.get::<_, Option<i64>>(6)?)))
                            },
                        )
                        .optional()?;
                    let start = |detail: Option<&str>| PlayEvent { at: now, kind: "start", position_s: probe.position_s, detail: detail.map(str::to_string) };
                    if let Some((id, started, dur, paused, counters)) = resumed.filter(|_| merge_window_s > 0) {
                        c.execute("UPDATE playbacks SET active = 1 WHERE id = ?1", [id])?;
                        insert_events(c, id, &[start(Some("Continued after a short break"))])?;
                        return Ok((id, started, dur as f64, paused as f64, Some(counters)));
                    }
                    let id = probe.insert(c)?.expect("live rows have no source_id and cannot collide");
                    insert_events(c, id, &[start(None)])?;
                    Ok((id, probe.started_at, 0.0, 0.0, None))
                })
                .await?;
            rec.started_at = started_at;
            if let Some((pauses, seeks, start_position)) = counters {
                rec.pause_count = pauses;
                rec.seek_count = seeks;
                rec.start_position_s = start_position;
            }
            tracing::info!("{} started {} on {}", rec.user_name, rec.item_name, rec.device_name.as_deref().unwrap_or("unknown device"));
            tracked.insert(
                key,
                Tracked { row_id, rec, watched, paused, is_paused, last_tick: tick_at, last_persist: tick_at, transcode_progress },
            );
        }
    }

    // Whatever is tracked but no longer reported has ended.
    let ended: Vec<String> = tracked.keys().filter(|k| !seen.contains(k)).cloned().collect();
    for key in ended {
        let mut t = tracked.remove(&key).expect("key came from the map");
        t.rec.active = false;
        t.rec.duration_s = t.watched.round() as i64;
        t.rec.paused_s = t.paused.round() as i64;
        let (row_id, rec) = (t.row_id, t.rec);
        tracing::info!("{} stopped {} after {}s", rec.user_name, rec.item_name, rec.duration_s);
        app.db
            .call(move |c| {
                if rec.duration_s < MIN_KEEP_S {
                    c.execute("DELETE FROM playbacks WHERE id = ?1", [row_id])?;
                } else {
                    rec.update_progress(c, row_id)?;
                    insert_events(c, row_id, &[PlayEvent { at: rec.ended_at, kind: "stop", position_s: rec.position_s, detail: None }])?;
                }
                Ok(())
            })
            .await?;
    }

    if !device_rows.is_empty() {
        app.db
            .call(move |c| {
                let mut stmt = c.prepare_cached(
                    "INSERT INTO devices(device_id, user_id, device_name, client, app_version, last_ip, first_seen, last_seen)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
                     ON CONFLICT(device_id, user_id) DO UPDATE SET
                        device_name = excluded.device_name, client = excluded.client,
                        app_version = excluded.app_version, last_ip = excluded.last_ip, last_seen = excluded.last_seen",
                )?;
                for d in device_rows {
                    stmt.execute(params![d.0, d.1, d.2, d.3, d.4, d.5, now])?;
                }
                Ok(())
            })
            .await?;
    }

    let mut live: Vec<Value> = tracked.iter().map(|(k, t)| live_json(k, t)).collect();
    live.sort_by_key(|v| v["started_at"].as_i64().unwrap_or(0));
    *app.live.write().unwrap() = live;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(position: i64) -> PlayRecord {
        PlayRecord { position_s: Some(position), play_method: "DirectPlay".into(), ..Default::default() }
    }

    #[test]
    fn steady_playback_is_not_a_seek() {
        let mut new = rec(105);
        assert!(diff_events(&rec(100), &mut new, false, false, 5.0, 0).is_empty());
        assert_eq!(new.seek_count, 0);
    }

    #[test]
    fn jumps_pauses_and_track_switches_are_events() {
        let mut new = rec(900);
        new.streams.subtitle_language = Some("eng".into());
        let kinds: Vec<_> = diff_events(&rec(100), &mut new, false, true, 5.0, 0).into_iter().map(|e| e.kind).collect();
        assert_eq!(kinds, ["seek", "pause", "subtitle"]);
        assert_eq!((new.seek_count, new.pause_count), (1, 1));

        // Sitting on pause does not drift into a "seek".
        let mut still = rec(900);
        assert!(diff_events(&rec(900), &mut still, true, true, 60.0, 0).is_empty());
    }
}
