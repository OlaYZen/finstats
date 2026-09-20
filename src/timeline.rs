//! One person's watching as a trail, newest first. Plays that follow each other and belong together
//! (episodes of one season, tracks of one album, the same film picked up again) fold into a single
//! stop, so an evening of four episodes is one entry and not four.

use std::collections::BTreeSet;

use anyhow::Result;
use axum::Json;
use axum::extract::{Path, Query, State};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::AuthUser;
use crate::db::rusqlite::{Connection, OptionalExtension, params};
use crate::db::{self};
use crate::state::{ApiError, ApiResult, App};

const DEFAULT_LIMIT: usize = 24;
const MAX_LIMIT: usize = 60;

#[derive(Deserialize)]
pub struct TimelineQuery {
    /// Where the previous page stopped: `<started_at>.<play id>` of its oldest play.
    before: Option<String>,
    limit: Option<usize>,
    /// Comma-separated library ids; absent means every library.
    libraries: Option<String>,
}

/// One play, as much of it as the trail needs.
#[derive(Clone, Default)]
struct Play {
    id: i64,
    started_at: i64,
    ended_at: i64,
    duration_s: i64,
    active: bool,
    item_id: String,
    item_type: String,
    item_name: String,
    series_id: Option<String>,
    series_name: Option<String>,
    season_number: Option<i64>,
    episode_number: Option<i64>,
    year: Option<i64>,
    album: Option<String>,
    album_artist: Option<String>,
    /// The season's own poster, when it has one.
    season_image_id: Option<String>,
}

impl Play {
    /// Plays with the same key that follow each other are one stop.
    fn key(&self) -> String {
        match (self.item_type.as_str(), &self.series_id, &self.album) {
            ("Episode", Some(series), _) => format!("s:{series}:{}", self.season_number.map_or("?".into(), |n| n.to_string())),
            ("Audio", _, Some(album)) => format!("a:{album}\u{1f}{}", self.album_artist.as_deref().unwrap_or("")),
            _ => format!("i:{}", self.item_id),
        }
    }
}

struct Stop {
    key: String,
    newest: Play,
    from: i64,
    to: i64,
    plays: i64,
    watch_s: i64,
    /// Still playing right now.
    active: bool,
    titles: BTreeSet<String>,
    episodes: BTreeSet<i64>,
    /// Oldest play folded in so far: the cursor if the page ends here.
    oldest: (i64, i64),
}

impl Stop {
    fn start(p: Play) -> Self {
        let mut s = Stop { key: p.key(), from: p.started_at, to: p.ended_at, plays: 0, watch_s: 0, active: false, titles: BTreeSet::new(), episodes: BTreeSet::new(), oldest: (p.started_at, p.id), newest: p.clone() };
        s.add(&p);
        s
    }

    fn add(&mut self, p: &Play) {
        self.from = self.from.min(p.started_at);
        self.to = self.to.max(p.ended_at);
        self.plays += 1;
        self.watch_s += p.duration_s;
        self.active |= p.active;
        self.titles.insert(p.item_id.clone());
        self.episodes.extend(p.episode_number);
        self.oldest = (p.started_at, p.id);
    }

    fn json(&self) -> Value {
        let p = &self.newest;
        let (kind, id, name, sub, image) = if self.key.starts_with("s:") {
            let season = match p.season_number {
                Some(0) => Some("Specials".to_string()),
                Some(n) => Some(format!("Season {n}")),
                None => None,
            };
            ("season", p.series_id.clone().unwrap_or_default(), p.series_name.clone().unwrap_or_else(|| p.item_name.clone()), season,
             p.season_image_id.clone().or_else(|| p.series_id.clone()).unwrap_or_default())
        } else if self.key.starts_with("a:") {
            ("album", p.item_id.clone(), p.album.clone().unwrap_or_default(), p.album_artist.clone(), p.item_id.clone())
        } else {
            ("item", p.item_id.clone(), p.item_name.clone(), p.year.map(|y| y.to_string()), p.item_id.clone())
        };
        // "Episodes 3–6" only when that is the whole truth: every number known, none skipped.
        let run = (self.episodes.len() == self.titles.len()).then(|| (self.episodes.first().copied(), self.episodes.last().copied()));
        let contiguous = matches!(run, Some((Some(a), Some(b))) if (b - a + 1) as usize == self.episodes.len());
        json!({
            "kind": kind, "type": p.item_type, "id": id, "name": name, "sub": sub, "image_item_id": image,
            "from": self.from, "to": self.to, "plays": self.plays, "titles": self.titles.len(), "watch_s": self.watch_s, "active": self.active,
            "episode_from": if contiguous { self.episodes.first().copied() } else { None },
            "episode_to": if contiguous { self.episodes.last().copied() } else { None },
        })
    }
}

/// Fold plays (newest first) into at most `limit` stops. A stop is only given out once the play after
/// it has been seen, so a page never ends in the middle of one; the cursor says where the next page starts.
fn fold(plays: impl Iterator<Item = Play>, limit: usize) -> (Vec<Value>, Option<String>) {
    let mut done: Vec<Stop> = Vec::new();
    let mut open: Option<Stop> = None;
    for p in plays {
        match open.as_mut() {
            Some(s) if s.key == p.key() => s.add(&p),
            _ => {
                done.extend(open.take());
                if done.len() == limit {
                    let (at, id) = done[limit - 1].oldest;
                    return (done.iter().map(Stop::json).collect(), Some(format!("{at}.{id}")));
                }
                open = Some(Stop::start(p));
            }
        }
    }
    done.extend(open);
    (done.iter().map(Stop::json).collect(), None)
}

fn page(conn: &Connection, user_id: &str, min_play_s: i64, before: (i64, i64), libraries: &[String], limit: usize) -> Result<(Vec<Value>, Option<String>)> {
    // The ids were checked to be plain hex, so they can go into the statement as they are.
    let libs = if libraries.is_empty() { String::new() } else { format!("AND p.library_id IN ('{}')", libraries.join("','")) };
    let mut stmt = conn.prepare(&format!(
        "SELECT p.id, p.started_at, p.ended_at, p.duration_s, p.active, p.item_id, p.item_type, COALESCE(i.name, p.item_name),
                p.series_id, COALESCE(s.name, p.series_name), COALESCE(p.season_number, i.parent_index_number),
                COALESCE(p.episode_number, i.index_number), i.production_year, i.album, i.album_artist,
                CASE WHEN sn.image_tag IS NOT NULL THEN sn.id END
         FROM playbacks p
         LEFT JOIN items i ON i.id = p.item_id
         LEFT JOIN items s ON s.id = p.series_id
         LEFT JOIN items sn ON sn.id = p.season_id
         WHERE p.user_id = ?1 AND p.duration_s >= ?2 AND (p.started_at, p.id) < (?3, ?4) {libs}
         ORDER BY p.started_at DESC, p.id DESC"
    ))?;
    let rows = stmt.query_map(params![user_id, min_play_s, before.0, before.1], |r| {
        Ok(Play {
            id: r.get(0)?, started_at: r.get(1)?, ended_at: r.get(2)?, duration_s: r.get(3)?, active: r.get(4)?, item_id: r.get(5)?, item_type: r.get(6)?,
            item_name: r.get(7)?, series_id: r.get(8)?, series_name: r.get(9)?, season_number: r.get(10)?, episode_number: r.get(11)?,
            year: r.get(12)?, album: r.get(13)?, album_artist: r.get(14)?, season_image_id: r.get(15)?,
        })
    })?;
    // Rows are read only as far as the page needs them.
    let mut failed = None;
    let out = fold(rows.map_while(|r| r.map_err(|e| failed = Some(e)).ok()), limit);
    match failed {
        Some(e) => Err(e.into()),
        None => Ok(out),
    }
}

/// The libraries this person has played from, for the filter.
fn libraries_of(conn: &Connection, user_id: &str, min_play_s: i64) -> Result<Vec<Value>> {
    let mut stmt = conn.prepare(
        "SELECT l.id, l.name, l.collection_type FROM libraries l
         WHERE l.id IN (SELECT DISTINCT library_id FROM playbacks WHERE user_id = ?1 AND duration_s >= ?2) ORDER BY l.name COLLATE NOCASE",
    )?;
    let rows = stmt.query_map(params![user_id, min_play_s], |r| Ok(json!({ "id": r.get::<_, String>(0)?, "name": r.get::<_, String>(1)?, "collection_type": r.get::<_, Option<String>>(2)? })))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub async fn timeline(State(app): State<App>, user: AuthUser, Path(id): Path<String>, Query(q): Query<TimelineQuery>) -> ApiResult {
    let id = db::norm_id(&id);
    if !user.perms.see_everyone && user.id != id {
        return Err(ApiError::not_permitted("see other people's statistics"));
    }
    let before = match q.before.as_deref() {
        None | Some("") => (i64::MAX, i64::MAX),
        Some(c) => c.split_once('.').and_then(|(at, id)| Some((at.parse().ok()?, id.parse().ok()?))).ok_or_else(|| ApiError::bad_request("That is not a timeline cursor"))?,
    };
    let libraries: Vec<String> = q.libraries.as_deref().unwrap_or("").split(',').map(db::norm_id).filter(|l| !l.is_empty()).collect();
    if libraries.len() > 100 || libraries.iter().any(|l| l.len() != 32 || !l.chars().all(|c| c.is_ascii_hexdigit())) {
        return Err(ApiError::bad_request("That doesn't look like a list of library ids"));
    }
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let min_play_s = app.settings().min_play_s;
    let out = app
        .db
        .call(move |c| {
            let who = c.query_row("SELECT id, name, (image_tag IS NOT NULL) FROM users WHERE id = ?1", [&id], |r| {
                Ok(json!({ "id": r.get::<_, String>(0)?, "name": r.get::<_, String>(1)?, "has_image": r.get::<_, bool>(2)? }))
            });
            let Some(who) = who.optional()? else { return Ok(None) };
            let (stops, next) = page(c, &id, min_play_s, before, &libraries, limit)?;
            Ok(Some(json!({ "user": who, "stops": stops, "next": next, "libraries": libraries_of(c, &id, min_play_s)? })))
        })
        .await?;
    out.map(Json).ok_or_else(|| ApiError::not_found("User"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn episode(id: i64, at: i64, series: &str, season: i64, ep: i64) -> Play {
        Play { id, started_at: at, ended_at: at + 1400, duration_s: 1400, item_id: format!("{series}-s{season}e{ep}"), item_type: "Episode".into(), item_name: format!("Episode {ep}"),
               series_id: Some(series.into()), series_name: Some(series.to_uppercase()), season_number: Some(season), episode_number: Some(ep), ..Play::default() }
    }

    fn movie(id: i64, at: i64, name: &str) -> Play {
        Play { id, started_at: at, ended_at: at + 5400, duration_s: 5400, item_id: name.into(), item_type: "Movie".into(), item_name: name.into(), year: Some(2008), ..Play::default() }
    }

    #[test]
    fn plays_that_follow_each_other_fold_into_one_stop() {
        // Newest first: three episodes of one season, a film, the same show again, then its first season.
        let plays = vec![episode(9, 9000, "rookery", 2, 3), episode(8, 8000, "rookery", 2, 2), episode(7, 7000, "rookery", 2, 1), movie(6, 6000, "Big Buck Bunny"),
                         episode(5, 5000, "rookery", 2, 1), episode(4, 4000, "rookery", 1, 8)];
        let (stops, next) = fold(plays.into_iter(), 10);
        assert_eq!(next, None);
        let brief: Vec<_> = stops.iter().map(|s| (s["name"].as_str().unwrap(), s["sub"].as_str().unwrap(), s["plays"].as_i64().unwrap())).collect();
        assert_eq!(brief, vec![("ROOKERY", "Season 2", 3), ("Big Buck Bunny", "2008", 1), ("ROOKERY", "Season 2", 1), ("ROOKERY", "Season 1", 1)]);
        let first = &stops[0];
        assert_eq!((first["kind"].as_str(), first["id"].as_str(), first["from"].as_i64(), first["to"].as_i64()), (Some("season"), Some("rookery"), Some(7000), Some(10400)));
        assert_eq!((first["titles"].as_i64(), first["episode_from"].as_i64(), first["episode_to"].as_i64(), first["watch_s"].as_i64()), (Some(3), Some(1), Some(3), Some(4200)));
    }

    #[test]
    fn a_rewatch_or_a_skipped_episode_is_not_called_a_run() {
        let again = vec![episode(3, 3000, "rookery", 1, 2), episode(2, 2000, "rookery", 1, 2), episode(1, 1000, "rookery", 1, 1)];
        let (stops, _) = fold(again.into_iter(), 10);
        assert_eq!((stops[0]["plays"].as_i64(), stops[0]["titles"].as_i64(), stops[0]["episode_from"].as_i64(), stops[0]["episode_to"].as_i64()), (Some(3), Some(2), Some(1), Some(2)));
        let skipped = vec![episode(2, 2000, "rookery", 1, 5), episode(1, 1000, "rookery", 1, 2)];
        let (stops, _) = fold(skipped.into_iter(), 10);
        assert_eq!((stops[0]["titles"].as_i64(), stops[0]["episode_from"].as_i64()), (Some(2), None));
    }

    #[test]
    fn pages_never_split_a_stop_and_join_up_without_gaps() {
        let plays: Vec<Play> = (0..40).rev().map(|n| episode(n, n * 1000, ["rookery", "harbour", "kitchen"][(n / 4 % 3) as usize], 1, n % 4 + 1)).collect();
        let (all, _) = fold(plays.clone().into_iter(), 100);
        assert_eq!(all.len(), 10);

        let mut paged = Vec::new();
        let mut before = (i64::MAX, i64::MAX);
        loop {
            let (stops, next) = fold(plays.clone().into_iter().filter(|p| (p.started_at, p.id) < before), 3);
            assert!(stops.len() <= 3);
            paged.extend(stops);
            let Some(next) = next else { break };
            let (at, id) = next.split_once('.').unwrap();
            before = (at.parse().unwrap(), id.parse().unwrap());
        }
        assert_eq!(paged, all);
    }

    #[test]
    fn tracks_fold_by_album_and_a_full_page_with_nothing_after_it_has_no_cursor() {
        let track = |id: i64, at: i64, album: &str| Play { id, started_at: at, ended_at: at + 200, duration_s: 200, item_id: format!("t{id}"), item_type: "Audio".into(), item_name: format!("Track {id}"),
                                                          album: Some(album.into()), album_artist: Some("The Tidewaters".into()), ..Play::default() };
        let (stops, next) = fold(vec![track(3, 300, "Low Tide"), track(2, 200, "Low Tide"), track(1, 100, "High Tide")].into_iter(), 2);
        assert_eq!(next, None);
        assert_eq!(stops.iter().map(|s| (s["kind"].as_str().unwrap(), s["name"].as_str().unwrap(), s["sub"].as_str().unwrap(), s["titles"].as_i64().unwrap())).collect::<Vec<_>>(),
                   vec![("album", "Low Tide", "The Tidewaters", 2), ("album", "High Tide", "The Tidewaters", 1)]);
    }
}
