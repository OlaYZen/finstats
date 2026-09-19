//! The year in review: one endpoint that tells the signed-in user the story of their own year.

use std::collections::BTreeSet;

use anyhow::Result;
use axum::Json;
use axum::extract::{Query, State};
use chrono::{Datelike, NaiveDate};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::auth::AuthUser;
use crate::db::rusqlite::{Connection, params_from_iter};
use crate::db::{self, SqlValue};
use crate::state::{ApiResult, App};
use crate::stats::{one_json, rows_json};

#[derive(Deserialize)]
pub struct RecapQuery {
    year: Option<String>,
}

/// `WHERE` over `playbacks p` for the period and scope, plus its arguments.
struct Window {
    wh: String,
    args: Vec<SqlValue>,
    /// Same scope without the period: "first ever" questions need the full history.
    scope_wh: String,
    scope_args: Vec<SqlValue>,
    from: i64,
    to: i64,
}

impl Window {
    fn with(&self, extra: &str) -> String {
        format!("{} AND {extra}", self.wh)
    }
}

const TITLE_ID: &str = "COALESCE(p.series_id, p.item_id)";
const NOT_LIVE_TV: &str = "p.item_type NOT IN ('TvChannel', 'LiveTvChannel', 'Program', 'LiveTvProgram')";

pub async fn recap(State(app): State<App>, user: AuthUser, Query(q): Query<RecapQuery>) -> ApiResult {
    // A recap is personal: everyone, administrators included, only ever gets their own.
    let scope_user = Some(user.id.clone());
    let min_play_s = app.settings().min_play_s;
    let server_name = app.config.read().unwrap().as_ref().map(|c| c.server_name.clone()).unwrap_or_else(|| "Jellyfin".into());
    let requested = q.year.unwrap_or_default();
    let out = app.db.call(move |c| build(c, scope_user, min_play_s, &server_name, &requested)).await?;
    Ok(Json(out))
}

fn build(c: &Connection, scope_user: Option<String>, min_play_s: i64, server_name: &str, requested: &str) -> Result<Value> {
    // Live TV is left out of the recap altogether: a channel left on all evening says nothing about taste.
    let mut scope_wh = format!("WHERE {NOT_LIVE_TV}");
    let mut scope_args: Vec<SqlValue> = vec![];
    if let Some(u) = &scope_user {
        scope_wh.push_str(" AND p.user_id = ?");
        scope_args.push(u.clone().into());
    }
    if min_play_s > 0 {
        scope_wh.push_str(" AND p.duration_s >= ?");
        scope_args.push(min_play_s.into());
    }

    let years: Vec<i64> = rows_json(
        c,
        &format!("SELECT DISTINCT CAST(strftime('%Y', p.started_at, 'unixepoch', 'localtime') AS INTEGER) AS y FROM playbacks p {scope_wh} ORDER BY y DESC"),
        &scope_args,
    )?
    .iter()
    .filter_map(|r| r["y"].as_i64())
    .collect();

    // Period boundaries are local midnights, expressed in UTC seconds.
    let local_midnight = |expr: &str| -> Result<i64> { Ok(c.query_row(&format!("SELECT CAST(strftime('%s', {expr}, 'utc') AS INTEGER)"), [], |r| r.get(0))?) };
    let (year_json, from, to) = if requested == "last12" {
        (json!("last12"), local_midnight("date('now', 'localtime', 'start of month', '-12 months')")?, db::now() + 1)
    } else {
        let (this_year, month): (i64, i64) = c.query_row(
            "SELECT CAST(strftime('%Y', 'now', 'localtime') AS INTEGER), CAST(strftime('%m', 'now', 'localtime') AS INTEGER)",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let y = requested.parse::<i64>().ok().filter(|y| (1990..=9999).contains(y)).unwrap_or_else(|| default_year(this_year, month, &years));
        (json!(y), local_midnight(&format!("'{y:04}-01-01'"))?, local_midnight(&format!("'{:04}-01-01'", y + 1))?)
    };

    let mut args = scope_args.clone();
    args.extend([from.into(), to.into()]);
    let w = Window { wh: format!("{scope_wh} AND p.started_at >= ? AND p.started_at < ?"), args, scope_wh, scope_args, from, to };

    let user_name: Option<String> = match &scope_user {
        Some(u) => c
            .query_row("SELECT COALESCE((SELECT name FROM users WHERE id = ?1), (SELECT user_name FROM playbacks WHERE user_id = ?1 ORDER BY ended_at DESC LIMIT 1))", [u], |r| r.get(0))
            .unwrap_or(None),
        None => None,
    };

    let totals = one_json(
        c,
        &format!(
            "SELECT COUNT(*) AS plays, COALESCE(SUM(p.duration_s), 0) AS watch_s, COUNT(DISTINCT p.item_id) AS distinct_items,
                    COALESCE(SUM(p.item_type = 'Movie'), 0) AS movies, COALESCE(SUM(p.item_type = 'Episode'), 0) AS episodes,
                    COALESCE(SUM(p.item_type = 'Audio'), 0) AS tracks,
                    COUNT(DISTINCT CASE WHEN p.item_type = 'Episode' THEN COALESCE(p.series_id, p.series_name) END) AS series_count,
                    COUNT(DISTINCT date(p.started_at, 'unixepoch', 'localtime')) AS active_days,
                    COUNT(DISTINCT date(p.started_at, 'unixepoch', 'localtime') || p.user_id) AS user_days
             FROM playbacks p {}",
            w.wh
        ),
        &w.args,
    )?
    .unwrap_or_default();
    let empty = totals.get("plays").and_then(Value::as_i64).unwrap_or(0) == 0;

    let mut out = json!({
        "years": years, "year": year_json, "from": from, "to": to,
        "scope": { "user_id": scope_user, "user_name": user_name, "server_name": server_name },
        "empty": empty, "totals": totals, "rank": null,
        "top_series": [], "top_movies": [], "top_tracks": [], "top_genres": [],
        "months": [], "hours": [], "weekdays": [], "persona": null, "records": {},
        "discovery": { "new_series": 0, "one_and_done": [], "finished_movies": 0, "finished_episodes": 0 },
        "clients": [],
    });
    if empty {
        return Ok(out);
    }

    out["rank"] = rank(c, &w, scope_user.as_deref(), min_play_s)?;
    out["top_series"] = json!(top_titles(c, &w, "Episode")?);
    out["top_movies"] = json!(top_titles(c, &w, "Movie")?);
    out["top_tracks"] = json!(top_titles(c, &w, "Audio")?);
    out["top_genres"] = json!(rows_json(
        c,
        &format!(
            "SELECT g.value AS name, COUNT(*) AS plays, COALESCE(SUM(p.duration_s), 0) AS watch_s
             FROM playbacks p JOIN items gi ON gi.id = {TITLE_ID}, json_each(gi.genres) g {} GROUP BY 1 ORDER BY watch_s DESC LIMIT 6",
            w.wh
        ),
        &w.args,
    )?);
    out["months"] = json!(months(c, &w)?);

    let (hours, weekdays) = rhythm(c, &w)?;
    out["persona"] = persona(&out["totals"], &hours, &weekdays);
    if let Some(t) = out["totals"].as_object_mut() {
        t.remove("user_days");
    }
    out["hours"] = json!(hours);
    out["weekdays"] = json!(weekdays);

    out["records"] = records(c, &w)?;
    out["discovery"] = discovery(c, &w)?;
    out["clients"] = json!(rows_json(
        c,
        &format!("SELECT p.client AS name, COUNT(*) AS plays, COALESCE(SUM(p.duration_s), 0) AS watch_s FROM playbacks p {} GROUP BY 1 ORDER BY watch_s DESC LIMIT 3", w.with("p.client IS NOT NULL")),
        &w.args,
    )?);
    Ok(out)
}

/// A year's recap is "ready" in its December and stays the default until the next December.
/// Falls back to the newest year with plays when the ready year has none (a new install).
fn default_year(this_year: i64, month: i64, years_with_plays: &[i64]) -> i64 {
    let ready = if month == 12 { this_year } else { this_year - 1 };
    if years_with_plays.contains(&ready) { ready } else { years_with_plays.first().copied().unwrap_or(ready) }
}

fn rank(c: &Connection, w: &Window, user: Option<&str>, min_play_s: i64) -> Result<Value> {
    let Some(user) = user else { return Ok(Value::Null) };
    let rows = rows_json(
        c,
        &format!(
            "SELECT p.user_id, SUM(p.duration_s) AS watch_s FROM playbacks p
         WHERE p.started_at >= ?1 AND p.started_at < ?2 AND p.duration_s >= ?3 AND {NOT_LIVE_TV} GROUP BY p.user_id ORDER BY watch_s DESC"
        ),
        &[w.from.into(), w.to.into(), min_play_s.into()],
    )?;
    let total: i64 = rows.iter().filter_map(|r| r["watch_s"].as_i64()).sum();
    let Some(pos) = rows.iter().position(|r| r["user_id"].as_str() == Some(user)) else { return Ok(Value::Null) };
    let mine = rows[pos]["watch_s"].as_i64().unwrap_or(0);
    Ok(json!({ "position": pos + 1, "of": rows.len(), "share": if total > 0 { (mine as f64 / total as f64 * 1000.0).round() / 1000.0 } else { 0.0 } }))
}

fn top_titles(c: &Connection, w: &Window, item_type: &str) -> Result<Vec<Map<String, Value>>> {
    let is_series = item_type == "Episode";
    let (id, name, sub, episodes) = if is_series {
        ("p.series_id", "COALESCE(i.name, MAX(p.series_name), 'Unknown series')", "CAST(i.production_year AS TEXT)", "COUNT(DISTINCT p.item_id)")
    } else if item_type == "Audio" {
        ("p.item_id", "COALESCE(i.name, MAX(p.item_name))", "COALESCE(i.album_artist, i.album, MAX(p.series_name))", "NULL")
    } else {
        ("p.item_id", "COALESCE(i.name, MAX(p.item_name))", "CAST(i.production_year AS TEXT)", "NULL")
    };
    let group = if is_series { "COALESCE(p.series_id, p.series_name)" } else { "p.item_id" };
    let mut args = w.args.clone();
    args.push(item_type.to_string().into());
    rows_json(
        c,
        &format!(
            "SELECT {id} AS id, {name} AS name, {sub} AS sub, {id} AS image_item_id, COUNT(*) AS plays,
                    COALESCE(SUM(p.duration_s), 0) AS watch_s, {episodes} AS episodes, (i.id IS NOT NULL AND i.removed = 0) AS item_exists
             FROM playbacks p LEFT JOIN items i ON i.id = {id} {} AND p.item_type = ? GROUP BY {group} ORDER BY watch_s DESC LIMIT 5",
            w.wh
        ),
        &args,
    )
}

fn months(c: &Connection, w: &Window) -> Result<Vec<Value>> {
    let sums = rows_json(
        c,
        &format!(
            "SELECT strftime('%Y-%m', p.started_at, 'unixepoch', 'localtime') AS month, COUNT(*) AS plays, COALESCE(SUM(p.duration_s), 0) AS watch_s
             FROM playbacks p {} GROUP BY 1",
            w.wh
        ),
        &w.args,
    )?;
    // The title that owned each month: the most watched series or film.
    let tops = rows_json(
        c,
        &format!(
            "SELECT month, id, name, id AS image_item_id, watch_s FROM (
                SELECT strftime('%Y-%m', p.started_at, 'unixepoch', 'localtime') AS month, {TITLE_ID} AS id,
                       COALESCE(MAX(p.series_name), MAX(p.item_name)) AS name, SUM(p.duration_s) AS watch_s,
                       ROW_NUMBER() OVER (PARTITION BY strftime('%Y-%m', p.started_at, 'unixepoch', 'localtime') ORDER BY SUM(p.duration_s) DESC) AS rn
                FROM playbacks p {} GROUP BY 1, 2
             ) WHERE rn = 1",
            w.with("p.item_type IN ('Movie', 'Episode')")
        ),
        &w.args,
    )?;
    let first: String = c.query_row("SELECT date(?1, 'unixepoch', 'localtime', 'start of month')", [w.from], |r| r.get(0))?;
    let last: String = c.query_row("SELECT date(MIN(?1 - 1, CAST(strftime('%s', 'now') AS INTEGER)), 'unixepoch', 'localtime', 'start of month')", [w.to], |r| r.get(0))?;
    let (mut cursor, last) = (NaiveDate::parse_from_str(&first, "%Y-%m-%d")?, NaiveDate::parse_from_str(&last, "%Y-%m-%d")?);
    let mut out = vec![];
    while cursor <= last && out.len() < 36 {
        let key = cursor.format("%Y-%m").to_string();
        let sum = sums.iter().find(|r| r["month"].as_str() == Some(&key));
        let top = tops.iter().find(|r| r["month"].as_str() == Some(&key)).map(|t| json!({ "id": t["id"], "name": t["name"], "image_item_id": t["image_item_id"], "watch_s": t["watch_s"] }));
        out.push(json!({
            "month": key,
            "watch_s": sum.and_then(|s| s["watch_s"].as_i64()).unwrap_or(0),
            "plays": sum.and_then(|s| s["plays"].as_i64()).unwrap_or(0),
            "top": top,
        }));
        cursor = if cursor.month() == 12 { NaiveDate::from_ymd_opt(cursor.year() + 1, 1, 1) } else { NaiveDate::from_ymd_opt(cursor.year(), cursor.month() + 1, 1) }.unwrap_or(last + chrono::Duration::days(1));
    }
    Ok(out)
}

fn rhythm(c: &Connection, w: &Window) -> Result<(Vec<i64>, Vec<i64>)> {
    let (mut hours, mut weekdays) = (vec![0i64; 24], vec![0i64; 7]);
    let sql = format!(
        "SELECT CAST(strftime('%w', p.started_at, 'unixepoch', 'localtime') AS INTEGER), CAST(strftime('%H', p.started_at, 'unixepoch', 'localtime') AS INTEGER),
                COALESCE(SUM(p.duration_s), 0) FROM playbacks p {} GROUP BY 1, 2",
        w.wh
    );
    let mut stmt = c.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(w.args.iter()))?;
    while let Some(r) = rows.next()? {
        let (dow, hour, secs): (i64, i64, i64) = (r.get(0)?, r.get(1)?, r.get(2)?);
        hours[hour.clamp(0, 23) as usize] += secs;
        weekdays[((dow + 6) % 7) as usize] += secs;
    }
    Ok((hours, weekdays))
}

/// A label for how this year was watched. First rule that clearly applies wins; the
/// order goes from the most distinctive habit to the most common one.
fn persona(totals: &Value, hours: &[i64], weekdays: &[i64]) -> Value {
    let total: i64 = hours.iter().sum();
    if total == 0 {
        return Value::Null;
    }
    let share = |secs: i64| secs as f64 / total as f64;
    let pct = |f: f64| (f * 100.0).round() as i64;
    let night = share(hours[22..].iter().sum::<i64>() + hours[..4].iter().sum::<i64>());
    let morning = share(hours[5..10].iter().sum());
    let weekend = share(weekdays[5] + weekdays[6]);
    let n = |k: &str| totals[k].as_i64().unwrap_or(0) as f64;
    let plays = n("plays").max(1.0);
    // Per person: on a shared server ten people watching one episode each is not a binge.
    let per_day = n("episodes") / n("user_days").max(n("active_days")).max(1.0);
    let make = |key: &str, title: &str, line: String| json!({ "key": key, "title": title, "line": line });

    if night >= 0.35 {
        make("night_owl", "Night owl", format!("{}% of the watching happened between 22:00 and 04:00", pct(night)))
    } else if morning >= 0.30 {
        make("early_bird", "Early bird", format!("{}% of the watching happened before 10 in the morning", pct(morning)))
    } else if n("tracks") / plays >= 0.5 {
        make("music_lover", "Music lover", format!("{}% of everything played was music", pct(n("tracks") / plays)))
    } else if weekend >= 0.45 {
        make("weekend_warrior", "Weekend warrior", format!("{}% of the watching happened on Saturdays and Sundays", pct(weekend)))
    } else if per_day >= 4.0 {
        make("binge_watcher", "Binge watcher", format!("{per_day:.1} episodes on an average day of watching"))
    } else if n("movies") / plays >= 0.4 {
        make("movie_buff", "Movie buff", format!("{}% of plays were films", pct(n("movies") / plays)))
    } else {
        let peak = hours.iter().enumerate().max_by_key(|(_, s)| **s).map(|(h, _)| h).unwrap_or(20);
        make("creature_of_habit", "Creature of habit", format!("Most of the watching started around {peak:02}:00, week in, week out"))
    }
}

fn records(c: &Connection, w: &Window) -> Result<Value> {
    let day = "date(p.started_at, 'unixepoch', 'localtime')";
    let biggest_day = one_json(
        c,
        &format!("SELECT {day} AS date, COALESCE(SUM(p.duration_s), 0) AS watch_s, COUNT(*) AS plays FROM playbacks p {} GROUP BY 1 ORDER BY watch_s DESC LIMIT 1", w.wh),
        &w.args,
    )?;
    let biggest_binge = one_json(
        c,
        &format!(
            "SELECT {day} AS date, p.series_id, MAX(p.series_name) AS series_name, p.series_id AS image_item_id,
                    COUNT(DISTINCT p.item_id) AS episodes, COALESCE(SUM(p.duration_s), 0) AS watch_s
             FROM playbacks p {} GROUP BY 1, COALESCE(p.series_id, p.series_name) HAVING episodes >= 3 ORDER BY episodes DESC, watch_s DESC LIMIT 1",
            w.with("p.item_type = 'Episode'")
        ),
        &w.args,
    )?;
    let longest_play = one_json(
        c,
        &format!(
            "SELECT p.item_id, CASE WHEN p.item_type = 'Episode' AND p.series_name IS NOT NULL THEN p.series_name || ' — ' || p.item_name ELSE p.item_name END AS name,
                    {TITLE_ID} AS image_item_id, p.duration_s, {day} AS date
             FROM playbacks p LEFT JOIN items i ON i.id = p.item_id {} ORDER BY p.duration_s DESC LIMIT 1",
            // A session left open overnight is not a long play: the time must fit the runtime.
            w.with("p.duration_s <= COALESCE(p.runtime_s, i.runtime_s, p.duration_s) * 1.25")
        ),
        &w.args,
    )?;
    let most_rewatched = one_json(
        c,
        &format!(
            "SELECT p.item_id AS id, CASE WHEN p.item_type = 'Episode' AND MAX(p.series_name) IS NOT NULL THEN MAX(p.series_name) || ' — ' || MAX(p.item_name) ELSE MAX(p.item_name) END AS name,
                    p.item_type AS type, {TITLE_ID} AS image_item_id, COUNT(DISTINCT {day}) AS plays
             FROM playbacks p {} GROUP BY p.item_id HAVING plays >= 2 ORDER BY plays DESC, SUM(p.duration_s) DESC LIMIT 1",
            w.with("p.item_type IN ('Movie', 'Episode') AND p.duration_s >= 300")
        ),
        &w.args,
    )?;
    let first_play = one_json(
        c,
        &format!(
            "SELECT p.item_id, CASE WHEN p.item_type = 'Episode' AND p.series_name IS NOT NULL THEN p.series_name || ' — ' || p.item_name ELSE p.item_name END AS name,
                    {TITLE_ID} AS image_item_id, p.started_at AS at FROM playbacks p {} ORDER BY p.started_at LIMIT 1",
            w.wh
        ),
        &w.args,
    )?;
    let oldest_title = one_json(
        c,
        &format!(
            "SELECT i.id, i.name, i.production_year AS year, i.id AS image_item_id
             FROM playbacks p JOIN items i ON i.id = {TITLE_ID} {} GROUP BY i.id ORDER BY i.production_year, SUM(p.duration_s) DESC LIMIT 1",
            w.with("i.production_year > 1800 AND p.duration_s >= 300")
        ),
        &w.args,
    )?;

    // Longest run of consecutive local days with at least one play.
    let days: BTreeSet<NaiveDate> = rows_json(c, &format!("SELECT DISTINCT {day} AS d FROM playbacks p {}", w.wh), &w.args)?
        .iter()
        .filter_map(|r| NaiveDate::parse_from_str(r["d"].as_str()?, "%Y-%m-%d").ok())
        .collect();
    let longest_streak = longest_run(&days).filter(|(len, _, _)| *len >= 2).map(|(len, from, to)| json!({ "days": len, "from": from.to_string(), "to": to.to_string() }));

    Ok(json!({
        "biggest_day": biggest_day, "biggest_binge": biggest_binge, "longest_streak": longest_streak,
        "longest_play": longest_play, "most_rewatched": most_rewatched, "first_play": first_play, "oldest_title": oldest_title,
    }))
}

/// The longest run of consecutive days in a set: (length, first day, last day).
pub fn longest_run(days: &BTreeSet<NaiveDate>) -> Option<(i64, NaiveDate, NaiveDate)> {
    let mut best: Option<(i64, NaiveDate, NaiveDate)> = None;
    let mut run: Option<(NaiveDate, NaiveDate)> = None;
    for d in days {
        run = match run {
            Some((start, prev)) if *d == prev + chrono::Duration::days(1) => Some((start, *d)),
            _ => Some((*d, *d)),
        };
        if let Some((start, end)) = run {
            let len = (end - start).num_days() + 1;
            if best.is_none_or(|(b, _, _)| len > b) {
                best = Some((len, start, end));
            }
        }
    }
    best
}

fn discovery(c: &Connection, w: &Window) -> Result<Value> {
    // "First ever" is judged against the whole history of this scope, not just the period.
    let mut args = w.scope_args.clone();
    args.extend([w.from.into(), w.to.into()]);
    let firsts = format!(
        "SELECT p.series_id AS id, MAX(p.series_name) AS name, MIN(p.started_at) AS first_at, COUNT(DISTINCT p.item_id) AS episodes
         FROM playbacks p {} AND p.item_type = 'Episode' AND p.series_id IS NOT NULL GROUP BY p.series_id",
        w.scope_wh
    );
    let new_series: i64 = c.query_row(&format!("SELECT COUNT(*) FROM ({firsts}) WHERE first_at >= ? AND first_at < ?"), params_from_iter(args.iter()), |r| r.get(0))?;
    let one_and_done = rows_json(
        c,
        &format!("SELECT id, name, id AS image_item_id FROM ({firsts}) WHERE first_at >= ? AND first_at < ? AND episodes = 1 ORDER BY first_at DESC LIMIT 5"),
        &args,
    )?;
    let finished = one_json(
        c,
        &format!(
            "SELECT COALESCE(SUM(f AND t = 'Movie'), 0) AS finished_movies, COALESCE(SUM(f AND t = 'Episode'), 0) AS finished_episodes FROM (
                SELECT p.item_type AS t, MAX(COALESCE(p.position_s, p.duration_s) * 1.0 / COALESCE(p.runtime_s, i.runtime_s)) >= 0.9 AS f
                FROM playbacks p LEFT JOIN items i ON i.id = p.item_id {} GROUP BY p.item_id, p.user_id)",
            w.with("p.item_type IN ('Movie', 'Episode') AND COALESCE(p.runtime_s, i.runtime_s, 0) > 0")
        ),
        &w.args,
    )?
    .unwrap_or_default();
    Ok(json!({
        "new_series": new_series, "one_and_done": one_and_done,
        "finished_movies": finished.get("finished_movies"), "finished_episodes": finished.get("finished_episodes"),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_recap_flips_in_december() {
        assert_eq!(default_year(2026, 9, &[2026, 2025]), 2025);
        assert_eq!(default_year(2026, 12, &[2026, 2025]), 2026);
        assert_eq!(default_year(2027, 1, &[2027, 2026, 2025]), 2026);
        assert_eq!(default_year(2027, 11, &[2027, 2026, 2025]), 2026);
        assert_eq!(default_year(2027, 12, &[2027, 2026, 2025]), 2027);
        // Brand new install: last year has nothing, so show what there is.
        assert_eq!(default_year(2026, 9, &[2026]), 2026);
        assert_eq!(default_year(2026, 9, &[]), 2025);
    }

    #[test]
    fn persona_prefers_the_most_distinctive_habit() {
        let mut hours = vec![0i64; 24];
        hours[23] = 50;
        hours[20] = 50;
        let weekdays = vec![10, 10, 10, 10, 10, 25, 25];
        let totals = json!({ "plays": 100, "episodes": 90, "movies": 10, "tracks": 0, "active_days": 60, "user_days": 60 });
        assert_eq!(persona(&totals, &hours, &weekdays)["key"], "night_owl");

        hours[23] = 0;
        hours[20] = 100;
        assert_eq!(persona(&totals, &hours, &weekdays)["key"], "weekend_warrior");
        assert!(persona(&totals, &vec![0; 24], &weekdays).is_null());
    }
}
