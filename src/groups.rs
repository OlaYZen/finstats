//! Group watching: different people playing the same title at the same time.
//!
//! Jellyfin does not expose SyncPlay groups through its sessions, so a group is inferred:
//! plays of one item by at least two different users that start within `group_window_s` of
//! each other and overlap long enough to have been watched together. Real history shows why
//! the window cannot be a few seconds: polling jitter and "you press play, I'm a moment late"
//! spread genuine group starts over up to a minute.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use anyhow::Result;
use axum::Json;
use axum::extract::{Query, State};
use serde_json::{Value, json};

use crate::auth::AuthUser;
use crate::db::SqlValue;
use crate::db::rusqlite::{Connection, params, params_from_iter};
use crate::state::{ApiResult, App};
use crate::stats::{FilterQuery, Scope};

/// Two plays were "together" only if they shared at least this much time.
const MIN_OVERLAP_S: i64 = 120;

struct Row {
    id: i64,
    user: String,
    start: i64,
    end: i64,
}

/// Given one item's plays in start order, the plays that belong to a group: (play id, group id).
fn cluster(rows: &[Row], window_s: i64) -> Vec<(i64, i64)> {
    let mut out = vec![];
    let mut i = 0;
    while i < rows.len() {
        // A chain of starts, each within the window of the one before it.
        let mut j = i + 1;
        while j < rows.len() && rows[j].start - rows[j - 1].start <= window_s {
            j += 1;
        }
        let chain = &rows[i..j];
        // Keep the plays that really ran alongside someone else's.
        let together: Vec<&Row> = chain
            .iter()
            .filter(|a| chain.iter().any(|b| b.user != a.user && a.end.min(b.end) - a.start.max(b.start) >= MIN_OVERLAP_S))
            .collect();
        if together.iter().map(|r| r.user.as_str()).collect::<BTreeSet<_>>().len() >= 2 {
            let group_id = together.iter().map(|r| r.id).min().expect("not empty");
            out.extend(together.iter().map(|r| (r.id, group_id)));
        }
        i = j;
    }
    out
}

/// (Re)assign `group_id` for one item, or for everything. Returns the number of groups found.
pub fn detect(conn: &mut Connection, window_s: i64, only_item: Option<&str>) -> Result<usize> {
    let tx = conn.transaction()?;
    let mut groups = 0;
    {
        let filter = if only_item.is_some() { "AND item_id = ?1" } else { "" };
        let args: Vec<SqlValue> = only_item.map(|i| vec![i.to_string().into()]).unwrap_or_default();
        let mut stmt = tx.prepare(&format!("SELECT item_id, id, user_id, started_at, ended_at FROM playbacks WHERE 1 = 1 {filter} ORDER BY item_id, started_at, id"))?;
        let mut rows = stmt.query(params_from_iter(args.iter()))?;
        let mut assigned: Vec<(i64, i64)> = vec![];
        let mut current: Option<String> = None;
        let mut batch: Vec<Row> = vec![];
        let mut flush = |batch: &mut Vec<Row>| {
            if batch.len() >= 2 {
                assigned.extend(cluster(batch, window_s));
            }
            batch.clear();
        };
        while let Some(r) = rows.next()? {
            let item: String = r.get(0)?;
            if current.as_deref() != Some(item.as_str()) {
                flush(&mut batch);
                current = Some(item);
            }
            batch.push(Row { id: r.get(1)?, user: r.get(2)?, start: r.get(3)?, end: r.get(4)? });
        }
        flush(&mut batch);
        drop(rows);
        drop(stmt);

        tx.execute(&format!("UPDATE playbacks SET group_id = NULL WHERE group_id IS NOT NULL {filter}"), params_from_iter(args.iter()))?;
        let mut set = tx.prepare("UPDATE playbacks SET group_id = ?2 WHERE id = ?1")?;
        for (id, group_id) in &assigned {
            set.execute(params![id, group_id])?;
        }
        groups += assigned.iter().map(|(_, g)| *g).collect::<BTreeSet<_>>().len();
    }
    tx.commit()?;
    Ok(groups)
}

/// `GET /api/stats/groups` — who watches together, what, and for how long.
/// Without "see everyone" only groups the caller was part of are returned.
pub async fn groups(State(app): State<App>, user: AuthUser, Query(q): Query<FilterQuery>) -> ApiResult {
    let scope = Scope::new(&app, &user, &q);
    let out = app
        .db
        .call(move |c| {
            let scope = scope.resolve(c)?;
            let mut wh = vec!["p.group_id IS NOT NULL".to_string()];
            let mut args: Vec<SqlValue> = vec![];
            if let Some(s) = scope.since {
                wh.push("p.started_at >= ?".into());
                args.push(s.into());
            }
            if let Some(l) = &scope.library_id {
                wh.push("p.library_id = ?".into());
                args.push(l.clone().into());
            }
            if let Some(u) = &scope.user_id {
                wh.push("p.group_id IN (SELECT group_id FROM playbacks WHERE user_id = ? AND group_id IS NOT NULL)".into());
                args.push(u.clone().into());
            }
            let sql = format!(
                "SELECT p.group_id, p.user_id, COALESCE(u.name, p.user_name), (u.image_tag IS NOT NULL), p.duration_s, p.started_at,
                        p.item_id, p.item_name, p.item_type, p.series_id, p.series_name, p.season_number, p.episode_number
                 FROM playbacks p LEFT JOIN users u ON u.id = p.user_id WHERE {} ORDER BY p.group_id, p.started_at",
                wh.join(" AND ")
            );
            struct Session {
                started_at: i64,
                item: Value,
                title_id: String,
                title_name: String,
                per_user: BTreeMap<String, (String, bool, i64)>,
            }
            let mut sessions: BTreeMap<i64, Session> = BTreeMap::new();
            let mut stmt = c.prepare(&sql)?;
            let mut rows = stmt.query(params_from_iter(args.iter()))?;
            while let Some(r) = rows.next()? {
                let gid: i64 = r.get(0)?;
                let (uid, name, has_image, dur, started): (String, String, bool, i64, i64) = (r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?);
                let (item_id, item_name, item_type): (String, String, String) = (r.get(6)?, r.get(7)?, r.get(8)?);
                let (series_id, series_name): (Option<String>, Option<String>) = (r.get(9)?, r.get(10)?);
                let s = sessions.entry(gid).or_insert_with(|| Session {
                    started_at: started,
                    title_id: series_id.clone().unwrap_or_else(|| item_id.clone()),
                    title_name: series_name.clone().unwrap_or_else(|| item_name.clone()),
                    item: json!({
                        "item_id": item_id, "item_name": item_name, "item_type": item_type, "series_id": series_id, "series_name": series_name,
                        "season_number": r.get::<_, Option<i64>>(11).unwrap_or(None), "episode_number": r.get::<_, Option<i64>>(12).unwrap_or(None),
                        "image_item_id": series_id.clone().unwrap_or_else(|| item_id.clone()),
                    }),
                    per_user: BTreeMap::new(),
                });
                s.started_at = s.started_at.min(started);
                let e = s.per_user.entry(uid).or_insert((name, has_image, 0));
                e.2 += dur;
            }

            // Time together = how long at least two people were watching: the second-longest stay.
            let together = |s: &Session| {
                let mut d: Vec<i64> = s.per_user.values().map(|v| v.2).collect();
                d.sort_unstable_by(|a, b| b.cmp(a));
                d.get(1).copied().unwrap_or(0)
            };
            let (mut total_together, mut total_person) = (0i64, 0i64);
            let mut people: BTreeSet<&str> = BTreeSet::new();
            let mut companions: HashMap<Vec<String>, (Vec<Value>, i64, i64, i64)> = HashMap::new();
            let mut titles: HashMap<String, (String, i64, i64)> = HashMap::new();
            for s in sessions.values() {
                let t = together(s);
                total_together += t;
                total_person += s.per_user.values().map(|v| v.2).sum::<i64>();
                people.extend(s.per_user.keys().map(String::as_str));
                let key: Vec<String> = s.per_user.keys().cloned().collect();
                let c = companions.entry(key).or_insert_with(|| {
                    (s.per_user.iter().map(|(id, (name, img, _))| json!({ "user_id": id, "user_name": name, "has_image": img })).collect(), 0, 0, 0)
                });
                c.1 += 1;
                c.2 += t;
                c.3 = c.3.max(s.started_at);
                let ti = titles.entry(s.title_id.clone()).or_insert((s.title_name.clone(), 0, 0));
                ti.1 += 1;
                ti.2 += t;
            }
            let mut companions: Vec<Value> = companions
                .into_values()
                .map(|(members, n, t, last)| json!({ "members": members, "sessions": n, "together_s": t, "last_at": last }))
                .collect();
            companions.sort_by_key(|c| std::cmp::Reverse(c["together_s"].as_i64().unwrap_or(0)));
            companions.truncate(8);
            let mut titles: Vec<Value> = titles
                .into_iter()
                .map(|(id, (name, n, t))| json!({ "id": id, "name": name, "image_item_id": id, "sessions": n, "together_s": t }))
                .collect();
            titles.sort_by_key(|c| std::cmp::Reverse(c["together_s"].as_i64().unwrap_or(0)));
            titles.truncate(8);
            let mut recent: Vec<&Session> = sessions.values().collect();
            recent.sort_by_key(|s| std::cmp::Reverse(s.started_at));
            let recent: Vec<Value> = recent
                .into_iter()
                .take(10)
                .map(|s| {
                    let mut v = s.item.clone();
                    v["started_at"] = json!(s.started_at);
                    v["together_s"] = json!(together(s));
                    v["members"] = json!(s.per_user.iter().map(|(id, (name, img, d))| json!({ "user_id": id, "user_name": name, "has_image": img, "duration_s": d })).collect::<Vec<_>>());
                    v
                })
                .collect();
            Ok(json!({
                "totals": { "sessions": sessions.len(), "together_s": total_together, "person_s": total_person, "people": people.len() },
                "companions": companions, "titles": titles, "recent": recent,
            }))
        })
        .await?;
    Ok(Json(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(id: i64, user: &str, start: i64, len: i64) -> Row {
        Row { id, user: user.into(), start, end: start + len }
    }

    #[test]
    fn people_starting_together_form_a_group() {
        // a and b press play 4 s apart, c joins 40 s later; d watches the same thing an hour on.
        let rows = vec![r(1, "a", 0, 1500), r(2, "b", 4, 1500), r(3, "c", 44, 1400), r(4, "d", 3600, 1500)];
        assert_eq!(cluster(&rows, 60), vec![(1, 1), (2, 1), (3, 1)]);
        assert_eq!(cluster(&rows, 5), vec![(1, 1), (2, 1)], "a tight window loses the late joiner");
    }

    #[test]
    fn coincidences_and_solo_viewing_are_not_groups() {
        // Same person on two devices is not a group.
        assert!(cluster(&[r(1, "a", 0, 1500), r(2, "a", 3, 1500)], 60).is_empty());
        // Started together, but one of them left after a minute: never really together.
        assert!(cluster(&[r(1, "a", 0, 1500), r(2, "b", 2, 60)], 60).is_empty());
        // A restart by the same person stays in the group; the one who bailed at once does not.
        let rows = vec![r(1, "a", 0, 900), r(2, "b", 3, 1500), r(3, "c", 5, 30), r(4, "a", 50, 600)];
        assert_eq!(cluster(&rows, 60), vec![(1, 1), (2, 1), (4, 1)]);
    }
}
