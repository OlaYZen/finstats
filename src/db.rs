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
    // 2 — what happens *during* a play, Jellyfin's own played flags, richer item details
    r#"
    ALTER TABLE playbacks ADD COLUMN pause_count INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE playbacks ADD COLUMN seek_count INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE playbacks ADD COLUMN start_position_s INTEGER;
    ALTER TABLE playbacks ADD COLUMN is_local INTEGER;          -- NULL = unknown

    CREATE TABLE playback_events (
        id          INTEGER PRIMARY KEY,
        playback_id INTEGER NOT NULL REFERENCES playbacks(id) ON DELETE CASCADE,
        at          INTEGER NOT NULL,
        kind        TEXT NOT NULL,   -- start | pause | resume | seek | audio | subtitle | transcode | stop
        position_s  INTEGER,
        detail      TEXT
    );
    CREATE INDEX idx_pbe_playback ON playback_events(playback_id, id);

    CREATE TABLE user_items (
        user_id        TEXT NOT NULL,
        item_id        TEXT NOT NULL,
        played         INTEGER NOT NULL DEFAULT 0,
        is_favorite    INTEGER NOT NULL DEFAULT 0,
        play_count     INTEGER NOT NULL DEFAULT 0,
        last_played_at INTEGER,
        PRIMARY KEY (user_id, item_id)
    ) WITHOUT ROWID;
    CREATE INDEX idx_user_items_item ON user_items(item_id);

    ALTER TABLE items ADD COLUMN provider_ids TEXT;   -- JSON object
    ALTER TABLE items ADD COLUMN studios TEXT;        -- JSON array of names
    ALTER TABLE items ADD COLUMN bit_depth INTEGER;
    ALTER TABLE items ADD COLUMN framerate REAL;

    ALTER TABLE devices ADD COLUMN last_user_name TEXT;
    "#,
    // 3 — Jellystat has no item type, so imported Live TV channels were guessed to be films.
    //     A channel is not in the library and has neither a container nor a runtime.
    r#"
    UPDATE playbacks SET item_type = 'TvChannel'
    WHERE source = 'jellystat' AND item_type = 'Movie' AND container IS NULL AND runtime_s IS NULL
      AND NOT EXISTS (SELECT 1 FROM items i WHERE i.id = playbacks.item_id);
    "#,
    // 4 — re-linking renamed items looks titles up by name, once per orphaned item.
    r#"
    CREATE INDEX idx_items_type_name ON items(type, name COLLATE NOCASE);
    CREATE INDEX idx_items_series_episode ON items(series_id, parent_index_number, index_number);
    "#,
    // 5 — what each non-admin user has been granted, on top of the defaults in settings
    r#"
    CREATE TABLE user_permissions (
        user_id     TEXT PRIMARY KEY,
        permissions TEXT NOT NULL,          -- JSON array of permission keys
        updated_at  INTEGER NOT NULL
    ) WITHOUT ROWID;
    "#,
    // 6 — episodes a user marked as seen by hand, for what was watched while nothing was recording
    r#"
    CREATE TABLE manual_seen (
        user_id    TEXT NOT NULL,
        item_id    TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        PRIMARY KEY (user_id, item_id)
    ) WITHOUT ROWID;
    "#,
    // 7 — group watching: plays that were watched together share a group_id (the lowest play id in it)
    r#"
    ALTER TABLE playbacks ADD COLUMN group_id INTEGER;
    CREATE INDEX idx_pb_group ON playbacks(group_id) WHERE group_id IS NOT NULL;
    CREATE INDEX idx_pb_item_start ON playbacks(item_id, started_at);
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

/// LAN, loopback, link-local, CGNAT (Tailscale & friends) and IPv6 ULA count as local.
pub fn is_local_ip(ip: &str) -> Option<bool> {
    use std::net::IpAddr;
    let v4_local = |v4: std::net::Ipv4Addr| {
        v4.is_private() || v4.is_loopback() || v4.is_link_local() || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xC0) == 64)
    };
    match ip.trim().parse::<IpAddr>().ok()? {
        IpAddr::V4(v4) => Some(v4_local(v4)),
        IpAddr::V6(v6) => Some(match v6.to_ipv4_mapped() {
            Some(v4) => v4_local(v4),
            None => v6.is_loopback() || (v6.segments()[0] & 0xfe00) == 0xfc00 || (v6.segments()[0] & 0xffc0) == 0xfe80,
        }),
    }
}

/// Fill `is_local` for rows that predate the column or were imported without it.
pub fn backfill_is_local(conn: &Connection) -> Result<usize> {
    let ips: Vec<String> = conn
        .prepare("SELECT DISTINCT remote_ip FROM playbacks WHERE is_local IS NULL AND remote_ip IS NOT NULL")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let mut n = 0;
    for ip in ips {
        if let Some(local) = is_local_ip(&ip) {
            n += conn.execute("UPDATE playbacks SET is_local = ?1 WHERE remote_ip = ?2 AND is_local IS NULL", params![local, ip])?;
        }
    }
    Ok(n)
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
