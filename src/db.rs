use std::path::Path;

use anyhow::{Context, Result};
use r2d2_sqlite::SqliteConnectionManager;
pub use r2d2_sqlite::rusqlite;
use rusqlite::{Connection, OptionalExtension, params};

pub type Pool = r2d2::Pool<SqliteConnectionManager>;

#[derive(Clone)]
pub struct Db {
    pool: Pool,
}

const MIGRATIONS: &[&str] = &[
    // 1 — initial schema
    r#"
    CREATE TABLE settings (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    ) WITHOUT ROWID;

    CREATE TABLE sessions (
        token_hash TEXT PRIMARY KEY,
        user_id    TEXT NOT NULL,
        user_name  TEXT NOT NULL,
        is_admin   INTEGER NOT NULL,
        created_at INTEGER NOT NULL,
        expires_at INTEGER NOT NULL,
        ip         TEXT,
        user_agent TEXT
    ) WITHOUT ROWID;

    CREATE TABLE users (
        id               TEXT PRIMARY KEY,
        name             TEXT NOT NULL,
        is_admin         INTEGER NOT NULL DEFAULT 0,
        is_disabled      INTEGER NOT NULL DEFAULT 0,
        image_tag        TEXT,
        last_login_at    INTEGER,
        last_activity_at INTEGER,
        removed          INTEGER NOT NULL DEFAULT 0,
        updated_at       INTEGER NOT NULL
    ) WITHOUT ROWID;

    CREATE TABLE libraries (
        id              TEXT PRIMARY KEY,
        name            TEXT NOT NULL,
        collection_type TEXT,
        image_tag       TEXT,
        removed         INTEGER NOT NULL DEFAULT 0,
        updated_at      INTEGER NOT NULL
    ) WITHOUT ROWID;

    CREATE TABLE items (
        id                  TEXT PRIMARY KEY,
        library_id          TEXT,
        type                TEXT NOT NULL,
        name                TEXT NOT NULL,
        series_id           TEXT,
        season_id           TEXT,
        series_name         TEXT,
        index_number        INTEGER,
        parent_index_number INTEGER,
        album               TEXT,
        album_artist        TEXT,
        runtime_s           INTEGER,
        production_year     INTEGER,
        premiere_date       TEXT,
        date_created        INTEGER,
        community_rating    REAL,
        official_rating     TEXT,
        genres              TEXT,
        overview            TEXT,
        image_tag           TEXT,
        backdrop_tag        TEXT,
        container           TEXT,
        path                TEXT,
        size_bytes          INTEGER,
        bitrate             INTEGER,
        video_codec         TEXT,
        width               INTEGER,
        height              INTEGER,
        video_range         TEXT,
        audio_codec         TEXT,
        audio_channels      INTEGER,
        removed             INTEGER NOT NULL DEFAULT 0,
        updated_at          INTEGER NOT NULL
    ) WITHOUT ROWID;
    CREATE INDEX idx_items_library ON items(library_id, type);
    CREATE INDEX idx_items_series  ON items(series_id);
    CREATE INDEX idx_items_created ON items(date_created);

    CREATE TABLE playbacks (
        id                  INTEGER PRIMARY KEY,
        source              TEXT NOT NULL,           -- 'live' | 'jellystat'
        source_id           TEXT UNIQUE,             -- de-duplication key for imports
        active              INTEGER NOT NULL DEFAULT 0,
        user_id             TEXT NOT NULL,
        user_name           TEXT NOT NULL,
        item_id             TEXT NOT NULL,
        item_name           TEXT NOT NULL,
        item_type           TEXT NOT NULL,
        series_id           TEXT,
        series_name         TEXT,
        season_id           TEXT,
        season_number       INTEGER,
        episode_number      INTEGER,
        library_id          TEXT,
        started_at          INTEGER NOT NULL,
        ended_at            INTEGER NOT NULL,
        duration_s          INTEGER NOT NULL,        -- time actually spent playing
        paused_s            INTEGER NOT NULL DEFAULT 0,
        position_s          INTEGER,
        runtime_s           INTEGER,
        client              TEXT,
        device_name         TEXT,
        device_id           TEXT,
        app_version         TEXT,
        remote_ip           TEXT,
        play_method         TEXT,
        container           TEXT,
        bitrate             INTEGER,
        video_codec         TEXT,
        width               INTEGER,
        height              INTEGER,
        video_range         TEXT,
        bit_depth           INTEGER,
        audio_codec         TEXT,
        audio_channels      INTEGER,
        audio_language      TEXT,
        subtitle_codec      TEXT,
        subtitle_language   TEXT,
        transcode           TEXT                      -- JSON, null when not transcoding
    );
    CREATE INDEX idx_pb_ended  ON playbacks(ended_at);
    CREATE INDEX idx_pb_user   ON playbacks(user_id, ended_at);
    CREATE INDEX idx_pb_item   ON playbacks(item_id, ended_at);
    CREATE INDEX idx_pb_series ON playbacks(series_id, ended_at);
    CREATE INDEX idx_pb_active ON playbacks(active) WHERE active = 1;

    CREATE TABLE devices (
        device_id    TEXT NOT NULL,
        user_id      TEXT NOT NULL,
        device_name  TEXT,
        client       TEXT,
        app_version  TEXT,
        last_ip      TEXT,
        first_seen   INTEGER NOT NULL,
        last_seen    INTEGER NOT NULL,
        PRIMARY KEY (device_id, user_id)
    ) WITHOUT ROWID;

    CREATE TABLE server_events (
        id             INTEGER PRIMARY KEY,           -- Jellyfin activity log id
        date           INTEGER NOT NULL,
        name           TEXT NOT NULL,
        overview       TEXT,
        short_overview TEXT,
        type           TEXT,
        severity       TEXT,
        user_id        TEXT,
        item_id        TEXT
    );
    CREATE INDEX idx_events_date ON server_events(date);
    "#,
];

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let manager = SqliteConnectionManager::file(path).with_init(|c| {
            c.execute_batch(
                "PRAGMA journal_mode = WAL;
                 PRAGMA synchronous = NORMAL;
                 PRAGMA foreign_keys = ON;
                 PRAGMA busy_timeout = 15000;
                 PRAGMA temp_store = MEMORY;
                 PRAGMA cache_size = -8000;",
            )
        });
        let pool = r2d2::Pool::builder()
            .max_size(6)
            .min_idle(Some(1))
            .build(manager)
            .context("opening database")?;
        let db = Db { pool };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> Result<()> {
        let mut conn = self.pool.get()?;
        let current: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
            let tx = conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", (i + 1) as i64)?;
            tx.commit()?;
            tracing::info!("applied database migration {}", i + 1);
        }
        Ok(())
    }

    /// Run blocking database work off the async runtime.
    pub async fn call<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = pool.get().context("database pool exhausted")?;
            f(&mut conn)
        })
        .await
        .context("database task panicked")?
    }

    /// Synchronous access for code that is already on a blocking thread.
    pub fn conn(&self) -> Result<r2d2::PooledConnection<SqliteConnectionManager>> {
        self.pool.get().context("database pool exhausted")
    }
}

pub fn get_setting(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0))
        .optional()?)
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings(key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Jellyfin ids show up with and without dashes depending on the endpoint.
pub fn norm_id(id: &str) -> String {
    id.chars().filter(|c| *c != '-').map(|c| c.to_ascii_lowercase()).collect()
}

/// Parse an ISO-8601 timestamp (as emitted by Jellyfin / Jellystat) to unix seconds.
pub fn parse_ts(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|d| d.timestamp())
        .ok()
        .or_else(|| {
            // Jellyfin sometimes omits the offset; treat as UTC.
            chrono::NaiveDateTime::parse_from_str(s.trim_end_matches('Z'), "%Y-%m-%dT%H:%M:%S%.f")
                .ok()
                .map(|d| d.and_utc().timestamp())
        })
}

pub use rusqlite::types::Value as SqlValue;
