//! What arrived in the libraries lately, newest first. Episodes of one show that were added on the same
//! local day are one entry ("Season 4 · 3 episodes"), so a season pack or a whole imported show does not
//! push everything else off the list.

use std::collections::HashMap;

use anyhow::Result;
use axum::Json;
use axum::extract::{Query, State};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::AuthUser;
use crate::db::rusqlite::Connection;
use crate::state::{ApiResult, App};

const DEFAULT_LIMIT: usize = 30;
const MAX_LIMIT: usize = 60;

/// Things that are an entry of their own. Seasons and series never are (their episodes speak for them), nor are
/// single tracks (the album does).
const TYPES: &str = "'Episode', 'Movie', 'MusicAlbum', 'Video', 'MusicVideo', 'Book', 'AudioBook'";

#[derive(Deserialize)]
pub struct RecentQuery {
    limit: Option<usize>,
}

#[derive(Clone, Default)]
struct Added {
    id: String,
    item_type: String,
    name: String,
    at: i64,
    /// Local calendar day of `at`.
    day: String,
    series_id: Option<String>,
    series_name: Option<String>,
    season_number: Option<i64>,
    episode_number: Option<i64>,
    /// The season's own poster, when it has one.
    season_image_id: Option<String>,
    year: Option<i64>,
    album_artist: Option<String>,
}

struct Entry {
    newest: Added,
    episodes: i64,
    seasons: Vec<Option<i64>>,
}

impl Entry {
    fn json(&self) -> Value {
        let a = &self.newest;
        let Some(series_id) = a.series_id.as_ref().filter(|_| a.item_type == "Episode") else {
            let sub = a.album_artist.clone().or_else(|| a.year.map(|y| y.to_string()));
            return json!({ "kind": "item", "type": a.item_type, "id": a.id, "name": a.name, "sub": sub, "image_item_id": a.id, "added_at": a.at });
        };
        let one_season = self.seasons.len() == 1;
        let sub = match (one_season, a.season_number) {
            (true, Some(0)) => Some("Specials".to_string()),
            (true, Some(n)) => Some(format!("Season {n}")),
            (true, None) => None,
            (false, _) => Some(format!("{} seasons", self.seasons.len())),
        };
        let image = if one_season { a.season_image_id.clone() } else { None }.unwrap_or_else(|| series_id.clone());
        json!({
            "kind": "episodes", "type": "Episode", "id": series_id, "name": a.series_name.clone().unwrap_or_else(|| a.name.clone()), "sub": sub,
            "image_item_id": image, "added_at": a.at, "episodes": self.episodes, "seasons": self.seasons.len(),
            // One episode: say which one.
            "episode_number": if self.episodes == 1 { a.episode_number } else { None },
            "episode_name": if self.episodes == 1 { Some(a.name.clone()) } else { None },
        })
    }
}

/// Fold additions (newest first) into at most `limit` entries. Reading stops as soon as nothing that follows can
/// belong to an entry on the list: everything older than the last entry's day.
fn fold(added: impl Iterator<Item = Added>, limit: usize) -> Vec<Value> {
    let mut order: Vec<Entry> = Vec::new();
    let mut index: HashMap<(String, String), usize> = HashMap::new();
    for a in added {
        let key = match (&a.series_id, a.item_type.as_str()) {
            (Some(series), "Episode") => (series.clone(), a.day.clone()),
            _ => (a.id.clone(), String::new()),
        };
        if let Some(&i) = index.get(&key) {
            let e = &mut order[i];
            e.episodes += 1;
            if !e.seasons.contains(&a.season_number) {
                e.seasons.push(a.season_number);
            }
        } else if order.len() < limit {
            index.insert(key, order.len());
            order.push(Entry { seasons: vec![a.season_number], episodes: 1, newest: a });
        } else if order.last().is_some_and(|last| a.day < last.newest.day) {
            break;
        }
    }
    order.iter().map(Entry::json).collect()
}

/// `+i.type` keeps the planner off the type index: walking `idx_items_added` newest first and stopping early
/// reads a few dozen rows, where sorting every episode by date took a quarter of a second on a real library.
const SQL: &str = "SELECT i.id, i.type, i.name, i.date_created, date(i.date_created, 'unixepoch', 'localtime'), i.series_id, COALESCE(s.name, i.series_name),
        i.parent_index_number, i.index_number, CASE WHEN sn.image_tag IS NOT NULL THEN sn.id END, i.production_year, i.album_artist
 FROM items i
 LEFT JOIN items s ON s.id = i.series_id
 LEFT JOIN items sn ON sn.id = i.season_id
 WHERE i.removed = 0 AND i.date_created IS NOT NULL AND +i.type IN ({TYPES})
 ORDER BY i.date_created DESC";

fn recent(conn: &Connection, limit: usize) -> Result<Vec<Value>> {
    let mut stmt = conn.prepare(&SQL.replace("{TYPES}", TYPES))?;
    let rows = stmt.query_map([], |r| {
        Ok(Added {
            id: r.get(0)?, item_type: r.get(1)?, name: r.get(2)?, at: r.get(3)?, day: r.get(4)?, series_id: r.get(5)?, series_name: r.get(6)?,
            season_number: r.get(7)?, episode_number: r.get(8)?, season_image_id: r.get(9)?, year: r.get(10)?, album_artist: r.get(11)?,
        })
    })?;
    let mut failed = None;
    let out = fold(rows.map_while(|r| r.map_err(|e| failed = Some(e)).ok()), limit);
    match failed {
        Some(e) => Err(e.into()),
        None => Ok(out),
    }
}

/// The library is the same for everyone who may sign in, so this is not scoped to the caller.
pub async fn recently_added(State(app): State<App>, _user: AuthUser, Query(q): Query<RecentQuery>) -> ApiResult {
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let items = app.db.call(move |c| recent(c, limit)).await?;
    Ok(Json(json!({ "items": items })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn episode(series: &str, season: i64, ep: i64, at: i64, day: &str) -> Added {
        Added { id: format!("{series}-{season}-{ep}"), item_type: "Episode".into(), name: format!("Episode {ep}"), at, day: day.into(), series_id: Some(series.into()),
                series_name: Some(series.to_uppercase()), season_number: Some(season), episode_number: Some(ep), ..Added::default() }
    }
    fn movie(name: &str, at: i64, day: &str) -> Added {
        Added { id: name.into(), item_type: "Movie".into(), name: name.into(), at, day: day.into(), year: Some(2008), ..Added::default() }
    }

    #[test]
    fn episodes_of_a_show_added_on_one_day_are_one_entry() {
        // Two shows scanned at the same time arrive interleaved; a film sits in between; the day before, more of the first show.
        let added = vec![episode("rookery", 4, 3, 900, "2026-09-18"), episode("harbour", 1, 2, 890, "2026-09-18"), episode("rookery", 4, 2, 880, "2026-09-18"),
                         movie("Big Buck Bunny", 870, "2026-09-18"), episode("harbour", 1, 1, 860, "2026-09-18"), episode("rookery", 4, 1, 100, "2026-09-17")];
        let out = fold(added.into_iter(), 30);
        let brief: Vec<_> = out.iter().map(|e| (e["name"].as_str().unwrap(), e["sub"].as_str().unwrap(), e["episodes"].as_i64(), e["added_at"].as_i64().unwrap())).collect();
        assert_eq!(brief, vec![("ROOKERY", "Season 4", Some(2), 900), ("HARBOUR", "Season 1", Some(2), 890), ("Big Buck Bunny", "2008", None, 870), ("ROOKERY", "Season 4", Some(1), 100)]);
        assert_eq!((out[0]["kind"].as_str(), out[0]["id"].as_str(), out[0]["episode_number"].as_i64()), (Some("episodes"), Some("rookery"), None));
        assert_eq!((out[3]["episode_number"].as_i64(), out[3]["episode_name"].as_str()), (Some(1), Some("Episode 1")), "a single episode says which one");
    }

    #[test]
    fn a_whole_show_imported_at_once_is_one_entry_not_one_per_season() {
        let added: Vec<Added> = (0..60).map(|n| episode("harbour", n / 20 + 1, n % 20 + 1, 1000 - n, "2026-09-18")).collect();
        let out = fold(added.into_iter(), 30);
        assert_eq!(out.len(), 1);
        assert_eq!((out[0]["sub"].as_str(), out[0]["episodes"].as_i64(), out[0]["seasons"].as_i64()), (Some("3 seasons"), Some(60), Some(3)));
    }

    #[test]
    fn a_full_list_still_counts_late_rows_of_its_last_day_then_stops_reading() {
        let mut added = vec![episode("rookery", 1, 2, 500, "2026-09-18"), movie("Winterline", 400, "2026-09-18"), episode("rookery", 1, 1, 300, "2026-09-18")];
        added.extend((0..1000).map(|n| movie(&format!("Older {n}"), 200 - n, "2026-09-10")));
        let mut read = 0;
        let out = fold(added.into_iter().inspect(|_| read += 1), 2);
        assert_eq!((out.len(), out[0]["episodes"].as_i64()), (2, Some(2)), "the third row belongs to the first entry although the list was already full");
        assert_eq!(read, 4, "stopped at the first row of an older day");
    }

    #[test]
    fn the_query_walks_the_added_index_instead_of_sorting_the_library() {
        let dir = std::env::temp_dir().join(format!("finstats-recent-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = crate::db::Db::open(&dir.join("finstats.db")).unwrap();
        let conn = db.conn().unwrap();
        let plan: Vec<String> = conn.prepare(&format!("EXPLAIN QUERY PLAN {}", SQL.replace("{TYPES}", TYPES))).unwrap()
            .query_map([], |r| r.get::<_, String>(3)).unwrap().map(|r| r.unwrap()).collect();
        assert!(plan.iter().any(|p| p.contains("idx_items_added")) && !plan.iter().any(|p| p.contains("TEMP B-TREE")), "{plan:?}");
        assert!(recent(&conn, 30).unwrap().is_empty());
        drop(conn);
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
