//! Re-attaching history to items that changed identity.
//!
//! Jellyfin derives an item's id from its path, so renaming a file or folder makes a "new"
//! item and leaves every earlier play pointing at an id that no longer exists. Those plays
//! also keep whatever name the item had at the time, often the raw folder name
//! (`Title (2010) [tmdbid-37799] [imdbid-tt1285016]`) from before metadata was fetched.
//!
//! After each library sync, orphaned plays are matched to the item that is there now:
//! by provider id when the old name carries one, otherwise by cleaned name (and year).
//! A match must be unambiguous; titles that were simply deleted stay as they are.

use anyhow::Result;

use crate::db::rusqlite::{Connection, OptionalExtension, params};

/// What a raw Jellyfin folder/file name tells us once the decorations are peeled off.
#[derive(Debug, Default, PartialEq)]
pub struct ParsedName {
    /// Without `[…id-…]` tags, but still with a trailing `(year)`.
    pub without_tags: String,
    /// Without tags and without the trailing `(year)`.
    pub title: String,
    pub year: Option<i64>,
    /// (json key in `provider_ids`, value), e.g. ("Tmdb", "37799").
    pub provider_ids: Vec<(&'static str, String)>,
}

pub fn parse_name(raw: &str) -> ParsedName {
    let mut out = ParsedName::default();
    let mut rest = String::new();
    let mut tail = raw;
    // Pull out every [key-value] / [key=value] tag Jellyfin understands.
    while let Some(open) = tail.find('[') {
        let Some(close) = tail[open..].find(']').map(|i| open + i) else { break };
        let inner = &tail[open + 1..close];
        let tag = inner.split_once(['-', '=']).and_then(|(k, v)| {
            let key = match k.trim().to_ascii_lowercase().as_str() {
                "tmdbid" | "tmdb" => "Tmdb",
                "imdbid" | "imdb" => "Imdb",
                "tvdbid" | "tvdb" => "Tvdb",
                _ => return None,
            };
            let v = v.trim();
            (!v.is_empty()).then(|| (key, v.to_string()))
        });
        rest.push_str(&tail[..open]);
        match tag {
            Some(t) => out.provider_ids.push(t),
            None => rest.push_str(&tail[open..=close]), // some other bracket; part of the title
        }
        tail = &tail[close + 1..];
    }
    rest.push_str(tail);
    out.without_tags = rest.split_whitespace().collect::<Vec<_>>().join(" ");

    out.title = out.without_tags.clone();
    if let Some(open) = out.without_tags.rfind('(') {
        let inside = out.without_tags[open + 1..].trim_end().trim_end_matches(')');
        if out.without_tags.trim_end().ends_with(')') && inside.len() == 4 {
            if let Ok(y) = inside.parse::<i64>() {
                if (1870..=2200).contains(&y) {
                    out.year = Some(y);
                    out.title = out.without_tags[..open].trim_end().to_string();
                }
            }
        }
    }
    out
}

/// The one live item of `item_type` this name can only mean, if there is exactly one.
fn find_current(conn: &Connection, item_type: &str, raw_name: &str, known_year: Option<i64>) -> Result<Option<String>> {
    let parsed = parse_name(raw_name);
    for (key, value) in &parsed.provider_ids {
        let hits: Vec<String> = conn
            .prepare_cached("SELECT id FROM items WHERE removed = 0 AND type = ?1 AND json_extract(provider_ids, '$.' || ?2) = ?3 LIMIT 2")?
            .query_map(params![item_type, key, value], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        if hits.len() == 1 {
            return Ok(hits.into_iter().next());
        }
    }
    let year = parsed.year.or(known_year);
    for name in [&parsed.title, &parsed.without_tags] {
        if name.is_empty() {
            continue;
        }
        // A year, when we have one, must agree: remakes share titles.
        let hits: Vec<String> = conn
            .prepare_cached(
                "SELECT id FROM items WHERE removed = 0 AND type = ?1 AND name = ?2 COLLATE NOCASE
                   AND (?3 IS NULL OR production_year IS NULL OR production_year = ?3) LIMIT 2",
            )?
            .query_map(params![item_type, name, year], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        if hits.len() == 1 {
            return Ok(hits.into_iter().next());
        }
    }
    Ok(None)
}

#[derive(Debug, Default)]
pub struct Relinked {
    pub titles: usize,
    pub episodes: usize,
    pub names_cleaned: usize,
}

pub fn relink_orphans(conn: &Connection) -> Result<Relinked> {
    let mut done = Relinked::default();
    const ORPHAN: &str = "NOT EXISTS (SELECT 1 FROM items live WHERE live.id = p.item_id AND live.removed = 0)";

    // ---- films and other stand-alone titles
    let orphans: Vec<(String, String, String, Option<i64>)> = conn
        .prepare(&format!(
            "SELECT p.item_id, p.item_type, COALESCE(MAX(old.name), MAX(p.item_name)), MAX(old.production_year)
             FROM playbacks p LEFT JOIN items old ON old.id = p.item_id
             WHERE p.item_type IN ('Movie', 'Video', 'MusicVideo', 'Audio') AND {ORPHAN} GROUP BY p.item_id"
        ))?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<Result<_, _>>()?;
    for (old_id, item_type, name, year) in orphans {
        if let Some(new_id) = find_current(conn, &item_type, &name, year)? {
            done.titles += conn.execute(
                "UPDATE playbacks SET item_id = ?1,
                        item_name  = (SELECT name FROM items WHERE id = ?1),
                        library_id = (SELECT library_id FROM items WHERE id = ?1),
                        runtime_s  = COALESCE((SELECT runtime_s FROM items WHERE id = ?1), runtime_s)
                 WHERE item_id = ?2",
                params![new_id, old_id],
            )?;
        } else {
            // Gone for good, but it can at least be called by its name.
            let cleaned = parse_name(&name).without_tags;
            if !cleaned.is_empty() && cleaned != name {
                done.names_cleaned += conn.execute("UPDATE playbacks SET item_name = ?1 WHERE item_id = ?2 AND item_name = ?3", params![cleaned, old_id, name])?;
            }
        }
    }

    // ---- episodes: find the series as it is now, then the same season/episode number in it
    let orphans: Vec<(String, Option<String>, Option<String>, Option<i64>, Option<i64>, Option<i64>)> = conn
        .prepare(&format!(
            "SELECT p.item_id, MAX(p.series_id), COALESCE(MAX(olds.name), MAX(p.series_name)), MAX(olds.production_year),
                    MAX(COALESCE(p.season_number, olde.parent_index_number)), MAX(COALESCE(p.episode_number, olde.index_number))
             FROM playbacks p LEFT JOIN items olde ON olde.id = p.item_id LEFT JOIN items olds ON olds.id = p.series_id
             WHERE p.item_type = 'Episode' AND {ORPHAN} GROUP BY p.item_id"
        ))?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?
        .collect::<Result<_, _>>()?;
    for (old_id, series_id, series_name, year, season, episode) in orphans {
        let (Some(season), Some(episode)) = (season, episode) else { continue };
        let series_alive: Option<String> = match &series_id {
            Some(id) => conn.query_row("SELECT id FROM items WHERE id = ?1 AND removed = 0", [id], |r| r.get(0)).optional()?,
            None => None,
        };
        let series_now = match (series_alive, series_name) {
            (Some(id), _) => Some(id),
            (None, Some(name)) => find_current(conn, "Series", &name, year)?,
            _ => None,
        };
        let Some(series_now) = series_now else { continue };
        let hits: Vec<String> = conn
            .prepare_cached("SELECT id FROM items WHERE removed = 0 AND type = 'Episode' AND series_id = ?1 AND parent_index_number = ?2 AND index_number = ?3 LIMIT 2")?
            .query_map(params![series_now, season, episode], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        if let [new_id] = hits.as_slice() {
            done.episodes += conn.execute(
                "UPDATE playbacks SET item_id = ?1, series_id = ?2,
                        season_id   = (SELECT season_id FROM items WHERE id = ?1),
                        item_name   = (SELECT name FROM items WHERE id = ?1),
                        series_name = (SELECT name FROM items WHERE id = ?2),
                        library_id  = (SELECT library_id FROM items WHERE id = ?1),
                        season_number = ?3, episode_number = ?4,
                        runtime_s   = COALESCE((SELECT runtime_s FROM items WHERE id = ?1), runtime_s)
                 WHERE item_id = ?5",
                params![new_id, series_now, season, episode, old_id],
            )?;
        }
    }

    if done.titles + done.episodes + done.names_cleaned > 0 {
        tracing::info!("re-linked {} title plays and {} episode plays to renamed items; cleaned {} names", done.titles, done.episodes, done.names_cleaned);
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peels_provider_tags_and_year() {
        let p = parse_name("Big Buck Bunny (2008) [tmdbid-10378] [imdbid-tt1254207]");
        assert_eq!(p.without_tags, "Big Buck Bunny (2008)");
        assert_eq!(p.title, "Big Buck Bunny");
        assert_eq!(p.year, Some(2008));
        assert_eq!(p.provider_ids, vec![("Tmdb", "10378".to_string()), ("Imdb", "tt1254207".to_string())]);
    }

    #[test]
    fn leaves_ordinary_names_alone() {
        let p = parse_name("Blade Runner 2049");
        assert_eq!((p.title.as_str(), p.year, p.provider_ids.len()), ("Blade Runner 2049", None, 0));
        // Brackets that are not provider tags belong to the title; "(1)" is not a year.
        let p = parse_name("Some Show [Director's Cut] (1)");
        assert_eq!(p.title, "Some Show [Director's Cut] (1)");
        assert_eq!(parse_name("Show [tvdbid=12345]").provider_ids, vec![("Tvdb", "12345".to_string())]);
    }

    #[test]
    fn relinks_by_provider_id_then_name_and_never_guesses() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE items(id TEXT PRIMARY KEY, type TEXT, name TEXT, production_year INTEGER, removed INTEGER DEFAULT 0,
                                provider_ids TEXT, library_id TEXT, runtime_s INTEGER, series_id TEXT, season_id TEXT,
                                parent_index_number INTEGER, index_number INTEGER);
             CREATE TABLE playbacks(id INTEGER PRIMARY KEY, item_id TEXT, item_name TEXT, item_type TEXT, series_id TEXT, series_name TEXT,
                                    season_id TEXT, season_number INTEGER, episode_number INTEGER, library_id TEXT, runtime_s INTEGER);
             INSERT INTO items(id, type, name, production_year, provider_ids, library_id) VALUES
                ('new1', 'Movie', 'Big Buck Bunny', 2008, '{\"Tmdb\":\"10378\"}', 'lib'),
                ('remakeA', 'Movie', 'Twins', 1988, NULL, 'lib'), ('remakeB', 'Movie', 'Twins', 2024, NULL, 'lib'),
                ('s-new', 'Series', 'Test Show', 2020, NULL, 'lib');
             INSERT INTO items(id, type, name, series_id, parent_index_number, index_number, library_id) VALUES ('e-new', 'Episode', 'Pilot', 's-new', 1, 1, 'lib');
             INSERT INTO playbacks(item_id, item_name, item_type) VALUES
                ('old1', 'Big Buck Bunny (2008) [tmdbid-10378] [imdbid-tt1254207]', 'Movie'),
                ('old2', 'Twins', 'Movie'),
                ('old3', 'Deleted Film (1999) [tmdbid-1]', 'Movie');
             INSERT INTO playbacks(item_id, item_name, item_type, series_id, series_name, season_number, episode_number)
                VALUES ('e-old', 'Episode 1', 'Episode', 's-old', 'Test Show', 1, 1);",
        )
        .unwrap();
        let r = relink_orphans(&conn).unwrap();
        assert_eq!((r.titles, r.episodes, r.names_cleaned), (1, 1, 1));
        let get = |old: &str| -> (String, String) { conn.query_row("SELECT item_id, item_name FROM playbacks WHERE id = ?1", [old], |r| Ok((r.get(0)?, r.get(1)?))).unwrap() };
        assert_eq!(get("1"), ("new1".into(), "Big Buck Bunny".into()));
        assert_eq!(get("2").0, "old2", "two films called Twins and no year: ambiguous, leave it");
        assert_eq!(get("3"), ("old3".into(), "Deleted Film (1999)".into()));
        assert_eq!(get("4"), ("e-new".into(), "Pilot".into()));
    }
}
