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

use std::collections::{BTreeMap, HashMap};

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

/// Is this poster one the caller could have been shown? Otherwise the proxy would let anyone walk through
/// everything Sonarr and Radarr know by counting upwards.
pub fn poster_is_listed(conn: &Connection, service_id: i64, media_id: i64) -> Result<bool> {
    Ok(conn.prepare_cached("SELECT 1 FROM upcoming WHERE service_id = ?1 AND arr_media_id = ?2 LIMIT 1")?.exists(params![service_id, media_id])?)
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
        assert!(poster_is_listed(&c, 1, 12).unwrap());
        assert!(!poster_is_listed(&c, 1, 13).unwrap() && !poster_is_listed(&c, 2, 12).unwrap());
    }
}
