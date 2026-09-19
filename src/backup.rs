//! finstats' own backups: everything that cannot be had again from Jellyfin, in one file.
//!
//! The format is gzip-compressed JSON Lines (`finstats-backup-YYYYMMDD-HHMMSS.jsonl.gz`). The first
//! line describes the backup; every other line is one row, `{"t": "<table>", "r": {column: value}}`.
//! A row is written and read back *by column name*, so a backup from an older finstats restores into
//! a newer one (missing columns take their defaults) and the other way round (unknown columns are
//! dropped). It streams both ways: size is bounded by disk, not memory.
//!
//! What is in it: plays and their timelines, manual seen-marks, permissions, home addresses, the
//! server log, known devices and the settings. What is not, on purpose: the Jellyfin address and API
//! key, sign-in sessions, and the library itself (items, people, played flags), which the next sync
//! reads from Jellyfin again. A backup is still a complete viewing history with IP addresses in it:
//! treat the file accordingly.
//!
//! Restoring *merges*. A play that is already there (same import id, or same user, item and start)
//! is skipped, so restoring twice, or into an instance that has been running for a while, is safe.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::db::rusqlite::types::ValueRef;
use crate::db::rusqlite::{Connection, params_from_iter};
use crate::db::{self, Db, SqlValue};
use crate::state::{Settings, Tasks};

pub const FORMAT: i64 = 1;
const PREFIX: &str = "finstats-backup-";
const SUFFIX: &str = ".jsonl.gz";

/// In the order they are written, which is also the order they must be read in: a timeline row
/// needs its play to exist.
const TABLES: [&str; 7] = ["playbacks", "playback_events", "manual_seen", "user_permissions", "home_addresses", "server_events", "devices"];

pub fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join("backups")
}

pub fn new_name() -> String {
    format!("{PREFIX}{}{SUFFIX}", chrono::Local::now().format("%Y%m%d-%H%M%S"))
}

/// Only names finstats itself would give a backup. This is what stands between a download or
/// delete request and the rest of the disk.
pub fn valid_name(name: &str) -> bool {
    name.strip_prefix(PREFIX)
        .and_then(|rest| rest.strip_suffix(SUFFIX))
        .is_some_and(|stamp| stamp.len() == 15 && stamp.bytes().enumerate().all(|(i, b)| if i == 8 { b == b'-' } else { b.is_ascii_digit() }))
}

#[derive(Serialize, Default, Debug)]
pub struct ExportResult {
    pub name: String,
    pub size_bytes: u64,
    pub plays: i64,
    pub rows: i64,
}

#[derive(Serialize, Default, Debug)]
pub struct RestoreResult {
    pub plays_imported: i64,
    pub plays_skipped: i64,
    pub events: i64,
    pub other_rows: i64,
    pub settings_restored: bool,
    pub from_version: Option<String>,
}

fn raw_row(row: &crate::db::rusqlite::Row) -> Map<String, Value> {
    let stmt = row.as_ref();
    let mut m = Map::new();
    for i in 0..stmt.column_count() {
        let v = match row.get_ref(i) {
            Ok(ValueRef::Integer(n)) => json!(n),
            Ok(ValueRef::Real(f)) => json!(f),
            Ok(ValueRef::Text(t)) => json!(String::from_utf8_lossy(t)),
            _ => Value::Null,
        };
        m.insert(stmt.column_name(i).unwrap_or_default().to_string(), v);
    }
    m
}

/// Write a backup into `dir`. The file only gets its real name once it is complete.
pub fn export(db: &Db, dir: &Path, tasks: Option<(&Tasks, &'static str)>) -> Result<ExportResult> {
    std::fs::create_dir_all(dir).context("creating the backups folder")?;
    let name = new_name();
    let (part, path) = (dir.join(format!("{name}.part")), dir.join(&name));
    let report = |msg: String, p: f64| {
        if let Some((t, id)) = tasks {
            t.update(id, msg, Some(p));
        }
    };

    let mut conn = db.conn()?;
    // One read transaction: the file is a snapshot of a single moment even while plays keep arriving.
    let tx = conn.transaction()?;
    let count = |table: &str| -> Result<i64> { Ok(tx.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))?) };
    let mut counts = Map::new();
    for t in TABLES {
        counts.insert(t.to_string(), json!(count(t)?));
    }
    let total: i64 = counts.values().filter_map(Value::as_i64).sum::<i64>().max(1);

    let result = (|| -> Result<ExportResult> {
        let mut out = GzEncoder::new(BufWriter::with_capacity(1 << 20, File::create(&part)?), Compression::new(6));
        let header = json!({
            "finstats_backup": FORMAT, "app_version": env!("CARGO_PKG_VERSION"), "created_at": db::now(),
            "server_name": db::get_setting(&tx, "server_name")?, "counts": counts.clone(),
        });
        writeln!(out, "{header}")?;
        if let Some(raw) = db::get_setting(&tx, "settings")? {
            writeln!(out, "{}", json!({ "t": "settings", "r": { "key": "settings", "value": raw } }))?;
        }
        let mut written = 0i64;
        for table in TABLES {
            // Most of these tables are WITHOUT ROWID and come out in primary-key order by themselves.
            let order = if matches!(table, "playbacks" | "playback_events" | "server_events") { " ORDER BY id" } else { "" };
            let mut stmt = tx.prepare(&format!("SELECT * FROM {table}{order}"))?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                serde_json::to_writer(&mut out, &json!({ "t": table, "r": raw_row(row) }))?;
                out.write_all(b"\n")?;
                written += 1;
                if written % 5000 == 0 {
                    report(format!("Writing {table}: {written} of {total} rows"), written as f64 / total as f64);
                }
            }
        }
        out.finish()?.into_inner().map_err(|e| anyhow::anyhow!("{e}"))?.sync_all()?;
        std::fs::rename(&part, &path)?;
        Ok(ExportResult { name: name.clone(), size_bytes: std::fs::metadata(&path)?.len(), plays: counts["playbacks"].as_i64().unwrap_or(0), rows: written })
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result
}

/// The backups on disk, newest first.
pub fn list(dir: &Path) -> Vec<Value> {
    let mut out: Vec<(i64, Value)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if !valid_name(&name) {
                return None;
            }
            let meta = e.metadata().ok()?;
            let at = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64;
            Some((at, json!({ "name": name, "size_bytes": meta.len(), "created_at": at })))
        })
        .collect();
    out.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1["name"].as_str().cmp(&a.1["name"].as_str())));
    out.into_iter().map(|(_, v)| v).collect()
}

pub fn newest_at(dir: &Path) -> Option<i64> {
    list(dir).first().and_then(|b| b["created_at"].as_i64())
}

/// Keep the newest `keep` backups. Returns how many were removed.
pub fn prune(dir: &Path, keep: usize) -> usize {
    let mut removed = 0;
    for b in list(dir).into_iter().skip(keep.max(1)) {
        if let Some(name) = b["name"].as_str() {
            if std::fs::remove_file(dir.join(name)).is_ok() {
                removed += 1;
            }
        }
    }
    removed
}

fn table_columns(conn: &Connection, table: &str) -> Result<HashSet<String>> {
    let mut stmt = conn.prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))?;
    let cols = stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<HashSet<_>, _>>()?;
    Ok(cols)
}

fn sql_value(v: &Value) -> SqlValue {
    match v {
        Value::Null => SqlValue::Null,
        Value::Bool(b) => (*b as i64).into(),
        Value::Number(n) => n.as_i64().map(SqlValue::from).or_else(|| n.as_f64().map(SqlValue::from)).unwrap_or(SqlValue::Null),
        Value::String(s) => s.clone().into(),
        other => other.to_string().into(),
    }
}

/// Insert one row by column name. Columns this database does not have are dropped; returns whether a row went in.
fn insert_row(conn: &Connection, table: &str, known: &HashSet<String>, row: &Map<String, Value>, skip: &[&str], verb: &str) -> Result<bool> {
    let cols: Vec<&String> = row.keys().filter(|k| known.contains(*k) && !skip.contains(&k.as_str())).collect();
    if cols.is_empty() {
        return Ok(false);
    }
    let sql = format!(
        "{verb} INTO {table}({}) VALUES ({})",
        cols.iter().map(|c| format!("\"{c}\"")).collect::<Vec<_>>().join(", "),
        vec!["?"; cols.len()].join(", ")
    );
    let args: Vec<SqlValue> = cols.iter().map(|c| sql_value(&row[*c])).collect();
    Ok(conn.prepare_cached(&sql)?.execute(params_from_iter(args.iter()))? > 0)
}

/// Merge a backup into this database. `with_settings` also brings back the settings and permissions.
pub fn restore(db: &Db, path: &Path, with_settings: bool, tasks: Option<(&Tasks, &'static str)>) -> Result<RestoreResult> {
    let mut file = File::open(path).context("opening the backup")?;
    let total_bytes = file.metadata()?.len().max(1);
    let mut magic = [0u8; 2];
    let gz = file.read(&mut magic)? == 2 && magic == [0x1f, 0x8b];
    file.seek(SeekFrom::Start(0))?;
    // Progress follows the compressed bytes consumed, which is the only total known up front.
    let counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let counted = Counted { inner: file, read: counter.clone() };
    let reader: Box<dyn BufRead> =
        if gz { Box::new(BufReader::with_capacity(1 << 20, GzDecoder::new(counted))) } else { Box::new(BufReader::with_capacity(1 << 20, counted)) };

    let mut conn = db.conn()?;
    let tx = conn.transaction()?;
    let mut res = RestoreResult::default();
    let mut known: HashMap<&'static str, HashSet<String>> = HashMap::new();
    for t in TABLES {
        known.insert(t, table_columns(&tx, t)?);
    }
    // Old play id → the id it got here. Plays that were already present are absent: their timeline is too.
    let mut play_ids: HashMap<i64, i64> = HashMap::new();
    let mut settings_raw: Option<String> = None;
    let mut saw_header = false;

    for (n, line) in reader.lines().enumerate() {
        let line = line.context("The file is damaged or not a finstats backup (could not be read)")?;
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(&line).with_context(|| format!("The file is not a finstats backup (line {} is not JSON)", n + 1))?;
        if !saw_header {
            let Some(format) = v["finstats_backup"].as_i64() else {
                bail!("This is not a finstats backup. A Jellystat backup goes under “Import from Jellystat”.");
            };
            if format > FORMAT {
                bail!("This backup was made by a newer finstats (format {format}). Update finstats, then restore it.");
            }
            res.from_version = v["app_version"].as_str().map(str::to_string);
            saw_header = true;
            continue;
        }
        let (Some(table), Some(row)) = (v["t"].as_str(), v["r"].as_object()) else { continue };
        match table {
            "settings" => settings_raw = row.get("value").and_then(Value::as_str).map(str::to_string),
            "playbacks" => {
                let exists: bool = match row.get("source_id").and_then(Value::as_str) {
                    Some(sid) => tx.prepare_cached("SELECT 1 FROM playbacks WHERE source_id = ?1")?.exists([sid])?,
                    None => tx
                        .prepare_cached("SELECT 1 FROM playbacks WHERE user_id = ?1 AND item_id = ?2 AND started_at = ?3")?
                        .exists(params_from_iter([sql_value(&row["user_id"]), sql_value(&row["item_id"]), sql_value(&row["started_at"])].iter()))?,
                };
                if exists {
                    res.plays_skipped += 1;
                    continue;
                }
                // Nothing restored is still playing, and who watched together is worked out again afterwards.
                let mut row = row.clone();
                row.insert("active".into(), json!(0));
                if insert_row(&tx, "playbacks", &known["playbacks"], &row, &["id", "group_id"], "INSERT")? {
                    if let Some(old) = row.get("id").and_then(Value::as_i64) {
                        play_ids.insert(old, tx.last_insert_rowid());
                    }
                    res.plays_imported += 1;
                }
            }
            "playback_events" => {
                let Some(new_id) = row.get("playback_id").and_then(Value::as_i64).and_then(|old| play_ids.get(&old)) else { continue };
                let mut row = row.clone();
                row.insert("playback_id".into(), json!(new_id));
                if insert_row(&tx, "playback_events", &known["playback_events"], &row, &["id"], "INSERT")? {
                    res.events += 1;
                }
            }
            "user_permissions" => {
                if with_settings && insert_row(&tx, table, &known["user_permissions"], row, &[], "INSERT OR REPLACE")? {
                    res.other_rows += 1;
                }
            }
            "manual_seen" | "home_addresses" | "server_events" | "devices" => {
                let t = TABLES.iter().find(|t| **t == table).copied().unwrap_or("devices");
                if insert_row(&tx, t, &known[t], row, &[], "INSERT OR IGNORE")? {
                    res.other_rows += 1;
                }
            }
            _ => {} // a table from a newer finstats
        }
        if n % 5000 == 0 {
            if let Some((t, id)) = tasks {
                let p = counter.load(std::sync::atomic::Ordering::Relaxed) as f64 / total_bytes as f64;
                t.update(id, format!("Restoring: {} plays so far", res.plays_imported), Some(p.min(0.97)));
            }
        }
    }
    if !saw_header {
        bail!("The file is empty");
    }

    if with_settings {
        if let Some(raw) = settings_raw {
            // Through the type, so a backup cannot plant values this version would refuse.
            let parsed: Settings = serde_json::from_str(&raw).unwrap_or_default();
            if parsed.validate().is_ok() {
                db::set_setting(&tx, "settings", &serde_json::to_string(&parsed)?)?;
                res.settings_restored = true;
            }
        }
    }
    if let Some((t, id)) = tasks {
        t.update(id, "Linking plays to the library", Some(0.98));
    }
    let settings = Settings::load(&tx)?;
    crate::network::set_manual(&tx, &settings.home_addresses)?;
    crate::network::reclassify(&tx)?;
    crate::sync::backfill_playbacks(&tx)?;
    tx.commit()?;
    // Who watched together is derived data with a transaction of its own; the restore itself is already safe.
    crate::groups::detect(&mut conn, settings.group_window_s, None)?;
    Ok(res)
}

struct Counted {
    inner: File,
    read: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl Read for Counted {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read.fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed);
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_finstats_own_backup_names_are_accepted() {
        assert!(valid_name("finstats-backup-20260920-031500.jsonl.gz"));
        assert!(valid_name(&new_name()));
        for bad in ["../finstats.db", "finstats-backup-20260920-031500.jsonl.gz/../../x", "finstats-backup-2026092-0315000.jsonl.gz", "finstats-backup-20260920-031500.jsonl", "finstats-backup-abcdefgh-ijklmn.jsonl.gz", ""] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn a_backup_round_trips_and_restoring_twice_adds_nothing() {
        let tmp = std::env::temp_dir().join(format!("finstats-backup-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let source = Db::open(&tmp.join("source.db")).unwrap();
        {
            let c = source.conn().unwrap();
            c.execute_batch(
                "INSERT INTO playbacks(id, source, user_id, user_name, item_id, item_name, item_type, started_at, ended_at, duration_s, remote_ip, active, group_id)
                   VALUES (7, 'live', 'u1', 'alice', 'i1', 'Big Buck Bunny', 'Movie', 1000, 1600, 600, '192.168.1.10', 1, 7),
                          (9, 'jellystat', 'u2', 'bob', 'i1', 'Big Buck Bunny', 'Movie', 1005, 1600, 590, '203.0.113.7', 0, 7);
                 UPDATE playbacks SET source_id = 'jellystat:abc' WHERE id = 9;
                 INSERT INTO playback_events(playback_id, at, kind, position_s) VALUES (7, 1000, 'start', 0), (7, 1300, 'pause', 300);
                 INSERT INTO manual_seen(user_id, item_id, created_at) VALUES ('u1', 'e1', 5);
                 INSERT INTO user_permissions(user_id, permissions, updated_at) VALUES ('u2', '[\"see_everyone\"]', 5);
                 INSERT INTO settings(key, value) VALUES ('settings', '{\"min_play_s\": 42, \"home_addresses\": [\"203.0.113.7\"]}'), ('api_key', 'secret-key');",
            )
            .unwrap();
        }
        let made = export(&source, &dir(&tmp), None).unwrap();
        assert_eq!((made.plays, made.rows), (2, 6));
        let file = dir(&tmp).join(&made.name);
        let mut text = String::new();
        GzDecoder::new(File::open(&file).unwrap()).read_to_string(&mut text).unwrap();
        assert!(!text.contains("secret-key"), "the Jellyfin API key must never be in a backup");

        let target = Db::open(&tmp.join("target.db")).unwrap();
        let first = restore(&target, &file, true, None).unwrap();
        assert_eq!((first.plays_imported, first.plays_skipped, first.events, first.settings_restored), (2, 0, 2, true));
        let again = restore(&target, &file, true, None).unwrap();
        assert_eq!((again.plays_imported, again.plays_skipped, again.events), (0, 2, 0));

        let c = target.conn().unwrap();
        let n = |sql: &str| -> i64 { c.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(n("SELECT COUNT(*) FROM playbacks WHERE active = 1"), 0);
        // The timeline followed its play to the new id, and the home address made bob's play local.
        assert_eq!(n("SELECT COUNT(*) FROM playback_events e JOIN playbacks p ON p.id = e.playback_id WHERE p.user_id = 'u1'"), 2);
        assert_eq!(n("SELECT is_local FROM playbacks WHERE user_id = 'u2'"), 1);
        assert_eq!(n("SELECT COUNT(DISTINCT group_id) FROM playbacks WHERE group_id IS NOT NULL"), 1);
        assert_eq!(n("SELECT COUNT(*) FROM user_permissions"), 1);
        assert_eq!(Settings::load(&c).unwrap().min_play_s, 42);

        // Without settings: history only.
        let plain = Db::open(&tmp.join("plain.db")).unwrap();
        let r = restore(&plain, &file, false, None).unwrap();
        assert!(!r.settings_restored);
        let c2 = plain.conn().unwrap();
        assert_eq!(c2.query_row("SELECT COUNT(*) FROM user_permissions", [], |r| r.get::<_, i64>(0)).unwrap(), 0);

        std::fs::write(tmp.join("not.jsonl"), "{\"hello\": 1}\n").unwrap();
        assert!(restore(&plain, &tmp.join("not.jsonl"), false, None).unwrap_err().to_string().contains("not a finstats backup"));
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
