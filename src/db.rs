use std::path::Path;

use anyhow::{Context, Result, bail};
use r2d2_sqlite::SqliteConnectionManager;
pub use r2d2_sqlite::rusqlite;
use rusqlite::{Connection, OptionalExtension, params};

pub type Pool = r2d2::Pool<SqliteConnectionManager>;

#[derive(Clone)]
pub struct Db {
    pool: Pool,
}

pub(crate) const MIGRATIONS: &[&str] = &[
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
    // 8 — cast and crew of films and shows, for "most watched people" in the recap. Forgetting when
    //     the library was last read makes the next start read it once more, so people arrive without
    //     waiting for Jellyfin's next scan.
    r#"
    CREATE TABLE item_people (
        item_id   TEXT NOT NULL,
        person_id TEXT NOT NULL,
        kind      TEXT NOT NULL,            -- Actor | Director
        name      TEXT NOT NULL,
        role      TEXT,
        sort      INTEGER NOT NULL,         -- billing order within the title
        has_image INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (item_id, person_id, kind)
    ) WITHOUT ROWID;
    CREATE INDEX idx_item_people_person ON item_people(person_id, kind);
    DELETE FROM settings WHERE key = 'library_synced_at';
    "#,
    // 9 — addresses that count as "at home" although they are public: this network's own public
    //     address (looked up, every one ever seen) and any the owner adds by hand
    r#"
    CREATE TABLE home_addresses (
        ip         TEXT PRIMARY KEY,
        source     TEXT NOT NULL,           -- lookup | manual
        first_seen INTEGER NOT NULL,
        last_seen  INTEGER NOT NULL
    ) WITHOUT ROWID;
    "#,
    // 10 — "recently added" walks the library newest first and stops after a few dozen rows
    r#"
    CREATE INDEX idx_items_added ON items(date_created DESC) WHERE removed = 0 AND date_created IS NOT NULL;
    "#,
    // 11 — where people watch from: the address of a sign-in (NULL = not looked at yet, '' = none in the
    //      text), a place per address from the local geolocation database, and what looked wrong
    r#"
    ALTER TABLE server_events ADD COLUMN remote_ip TEXT;
    CREATE INDEX idx_events_ip ON server_events(type, date) WHERE remote_ip <> '';
    CREATE INDEX idx_pb_ip ON playbacks(remote_ip) WHERE remote_ip IS NOT NULL;

    CREATE TABLE ip_locations (
        ip           TEXT PRIMARY KEY,        -- as written in playbacks / server_events
        country_code TEXT,                    -- all NULL: a public address the database does not know
        country      TEXT,
        region       TEXT,
        city         TEXT,
        latitude     REAL,
        longitude    REAL,
        timezone     TEXT,
        looked_up_at INTEGER NOT NULL
    ) WITHOUT ROWID;

    CREATE TABLE security_alerts (
        id          INTEGER PRIMARY KEY,
        kind        TEXT NOT NULL,            -- impossible_travel | new_country
        severity    TEXT NOT NULL,            -- high | medium
        user_id     TEXT NOT NULL,
        user_name   TEXT NOT NULL,
        at          INTEGER NOT NULL,         -- when it happened, not when it was noticed
        dedupe      TEXT NOT NULL UNIQUE,     -- a rescan never reports the same thing twice
        details     TEXT NOT NULL,            -- JSON
        created_at  INTEGER NOT NULL,
        resolved_at INTEGER,
        resolved_by TEXT,
        note        TEXT,
        muted       INTEGER NOT NULL DEFAULT 0 -- "these two places are fine for this person"
    );
    CREATE INDEX idx_alerts_open ON security_alerts(resolved_at, at);
    CREATE INDEX idx_alerts_user ON security_alerts(user_id, at);
    "#,
    // 12 — which languages a file can be played in: every audio and subtitle track, not just the first.
    //      JSON arrays of the codes Jellyfin reports (ISO 639-2, "und" for a track without one), in track
    //      order. Filled by the library read, so forget when it last ran and it runs again now.
    r#"
    ALTER TABLE items ADD COLUMN audio_languages TEXT;
    ALTER TABLE items ADD COLUMN subtitle_languages TEXT;
    DELETE FROM settings WHERE key = 'library_synced_at';
    "#,
    // 13 — connections to the services around Jellyfin (Sonarr, Radarr, Seerr, torrent clients). Their keys
    //      and passwords live here and nowhere else: never in the settings blob, never in a backup.
    //      AUTOINCREMENT: an id is never handed out twice, so rows of a deleted service cannot be adopted.
    //      `item_external` is the library's provider ids turned sideways (one id can belong to several
    //      items: the same film in an HD and a 4K library), so that a TMDB or TVDB id finds its items by index.
    r#"
    CREATE TABLE services (
        id                   INTEGER PRIMARY KEY AUTOINCREMENT,
        kind                 TEXT NOT NULL,           -- sonarr | radarr | seerr | qbittorrent | transmission | deluge
        name                 TEXT NOT NULL,
        url                  TEXT NOT NULL,
        username             TEXT,
        secret               TEXT NOT NULL,           -- API key or password
        accept_invalid_certs INTEGER NOT NULL DEFAULT 0,
        enabled              INTEGER NOT NULL DEFAULT 1,
        created_at           INTEGER NOT NULL,
        version              TEXT,
        last_ok_at           INTEGER,
        last_error           TEXT
    );

    CREATE TABLE item_external (
        item_id TEXT NOT NULL,
        source  TEXT NOT NULL,                        -- Tmdb | Tvdb | Imdb, as Jellyfin spells them
        value   TEXT NOT NULL,
        PRIMARY KEY (item_id, source)
    ) WITHOUT ROWID;
    CREATE INDEX idx_item_external ON item_external(source, value);
    "#,
    // 14 — what Sonarr and Radarr expect: episodes about to air, films about to be released. One row per
    //      instance, title and kind of release; the same episode in two Sonarrs is folded when it is read.
    //      An episode has a moment (`at`, UTC). A film has a *day*: Radarr gives midnight UTC, which as a
    //      moment would be the evening before in every zone west of Greenwich.
    r#"
    CREATE TABLE upcoming (
        service_id   INTEGER NOT NULL REFERENCES services(id) ON DELETE CASCADE,
        kind         TEXT NOT NULL,              -- episode | movie
        external_id  INTEGER NOT NULL,           -- Sonarr's episode id, Radarr's movie id
        release      TEXT NOT NULL,              -- air | cinema | digital | physical
        at           INTEGER,                    -- episodes
        day          TEXT,                       -- films: YYYY-MM-DD
        series_title TEXT,
        title        TEXT NOT NULL,
        season       INTEGER,
        episode      INTEGER,
        finale       TEXT,                       -- season | series | midseason
        year         INTEGER,
        tvdb_id      INTEGER,
        tmdb_id      INTEGER,
        imdb_id      TEXT,
        arr_media_id INTEGER NOT NULL,           -- Sonarr's series id, Radarr's movie id: where the poster is
        has_file     INTEGER NOT NULL DEFAULT 0,
        item_id      TEXT,                       -- the series or film in the library, once it is there
        PRIMARY KEY (service_id, kind, external_id, release)
    ) WITHOUT ROWID;
    CREATE INDEX idx_upcoming_at ON upcoming(at);
    CREATE INDEX idx_upcoming_day ON upcoming(day);
    "#,
    // 15 — what people asked for in Seerr. Seerr purges a request when its media is removed, so a request that
    //      disappears is kept and marked (`removed_at`): "asked for 14, watched 9" should not shrink.
    //      `status` and `media_status` are Seerr's own numbers; `available_at` is worked out by finstats.
    //      `user_id` is the Jellyfin user behind Seerr's, when Seerr says who that is; `item_id` and
    //      `arr_*` say where the poster is: in the library, or still only in Sonarr or Radarr.
    r#"
    CREATE TABLE requests (
        service_id        INTEGER NOT NULL REFERENCES services(id) ON DELETE CASCADE,
        request_id        INTEGER NOT NULL,
        media_type        TEXT NOT NULL,             -- movie | tv
        tmdb_id           INTEGER,
        tvdb_id           INTEGER,
        imdb_id           TEXT,
        title             TEXT,                      -- NULL until it has been looked up
        year              INTEGER,
        seasons           TEXT NOT NULL DEFAULT '[]', -- JSON array of season numbers; empty for a film
        is_4k             INTEGER NOT NULL DEFAULT 0,
        status            INTEGER NOT NULL,          -- 1 pending, 2 approved, 3 declined, 4 failed, 5 completed
        media_status      INTEGER NOT NULL,          -- 1 unknown, 2 pending, 3 processing, 4 partially available, 5 available, 6+ gone
        requested_at      INTEGER NOT NULL,
        updated_at        INTEGER NOT NULL,
        media_added_at    INTEGER,                   -- Seerr's own note of when it arrived
        seen_available_at INTEGER,                   -- when finstats first saw it available
        available_at      INTEGER,
        removed_at        INTEGER,
        seerr_user_id     INTEGER,
        seerr_user_name   TEXT,
        jellyfin_user_id  TEXT,
        jellyfin_username TEXT,
        jellyfin_media_id TEXT,
        user_id           TEXT,
        item_id           TEXT,
        arr_service_id    INTEGER,
        arr_media_id      INTEGER,
        looked_up_at      INTEGER,                   -- the last attempt to find its title
        PRIMARY KEY (service_id, request_id)
    ) WITHOUT ROWID;
    CREATE INDEX idx_requests_user ON requests(user_id, requested_at);
    CREATE INDEX idx_requests_when ON requests(requested_at);
    CREATE INDEX idx_requests_item ON requests(item_id);
    "#,
    // 16 — what actually came in: Sonarr's and Radarr's history, the grabbed / imported / failed events.
    //      One row per event, kept by the id the instance gave it, so reading twice changes nothing.
    r#"
    CREATE TABLE grabs (
        service_id  INTEGER NOT NULL REFERENCES services(id) ON DELETE CASCADE,
        history_id  INTEGER NOT NULL,
        event       TEXT NOT NULL,              -- grabbed | imported | failed
        at          INTEGER NOT NULL,
        media_type  TEXT NOT NULL,              -- tv | movie
        title       TEXT,                       -- the show or film
        source      TEXT,                       -- what the release was called
        season      INTEGER,
        tvdb_id     INTEGER,
        tmdb_id     INTEGER,
        size_bytes  INTEGER,
        quality     TEXT,
        indexer     TEXT,
        protocol    TEXT,
        client      TEXT,
        download_id TEXT,
        PRIMARY KEY (service_id, history_id)
    ) WITHOUT ROWID;
    CREATE INDEX idx_grabs_at ON grabs(at);
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
        let running = env!("CARGO_PKG_VERSION");
        refuse_downgrade(&*db.conn()?, running)?;
        db.migrate()?;
        record_version(&*db.conn()?, running)?;
        Ok(db)
    }

    /// A migrated database that exists only for the length of a test.
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        let manager = SqliteConnectionManager::memory().with_init(|c| c.execute_batch("PRAGMA foreign_keys = ON;"));
        // One connection: every caller must see the same in-memory database.
        let db = Db { pool: r2d2::Pool::builder().max_size(1).build(manager)? };
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

/// Settings key: the newest finstats version that has opened this database.
const VERSION_KEY: &str = "app_version";

/// `1.2.3` as a comparable triple; a pre-release or build suffix (`-rc.1`, `+abc`) is ignored.
fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let mut parts = v.trim().split(['-', '+']).next()?.split('.').map(|p| p.parse::<u64>().ok());
    let triple = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(triple)
}

/// A database only moves forward. An older finstats knows nothing about the tables and columns a newer one added,
/// would skip the migrations without a word and write rows the newer one then misreads, so it refuses to start
/// instead. Checked before anything is written; a stored version nobody can read counts as newer.
fn refuse_downgrade(conn: &Connection, running: &str) -> Result<()> {
    let schema: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if schema == 0 {
        return Ok(()); // brand new: there is no settings table yet
    }
    // No entry: last opened by a release from before versions were recorded (1.0.4 or older).
    let stored = get_setting(conn, VERSION_KEY)?;
    let newer = stored.as_deref().is_some_and(|s| match (parse_version(s), parse_version(running)) {
        (Some(theirs), Some(ours)) => theirs > ours,
        _ => true,
    });
    if !newer && schema as usize <= MIGRATIONS.len() {
        return Ok(());
    }
    let theirs = stored.map(|s| format!("finstats {s}")).unwrap_or_else(|| "a newer finstats".into());
    bail!(
        "this database was last used by {theirs}, and this is the older finstats {running}.\n\n\
         An older version cannot safely open a newer database, so it will not start. Nothing was changed.\n\
         Fix it by running that version or a newer one again (with Docker: the image tag you used before),\n\
         or start this version on an empty data folder and bring your history back from one of the files in\n\
         the old folder's backups/ directory:   finstats restore <file>"
    )
}

/// Remember the newest version that has opened this database; never lowers it.
fn record_version(conn: &Connection, running: &str) -> Result<()> {
    if get_setting(conn, VERSION_KEY)?.as_deref() != Some(running) {
        set_setting(conn, VERSION_KEY, running)?;
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn db_at(schema: usize, version: Option<&str>) -> Connection {
        let c = Connection::open_in_memory().unwrap();
        if schema > 0 {
            c.execute_batch(MIGRATIONS[0]).unwrap();
            c.pragma_update(None, "user_version", schema as i64).unwrap();
        }
        if let Some(v) = version {
            set_setting(&c, VERSION_KEY, v).unwrap();
        }
        c
    }

    #[test]
    fn versions_compare_as_numbers_not_text() {
        assert!(parse_version("1.10.0") > parse_version("1.9.12"));
        assert_eq!(parse_version("2.0.0-rc.1+build5"), Some((2, 0, 0)));
        assert_eq!(parse_version(" 1.0.4\n"), Some((1, 0, 4)));
        for bad in ["", "1.0", "1.0.0.1", "one.two.three", "v1.0.0"] {
            assert_eq!(parse_version(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn an_older_version_refuses_a_newer_database() {
        let n = MIGRATIONS.len();
        // New, from before versions were recorded, the same version, an upgrade: all start.
        for (schema, stored) in [(0, None), (n, None), (n, Some("1.0.4")), (n - 1, Some("1.0.3")), (n, Some("0.9.12"))] {
            assert!(refuse_downgrade(&db_at(schema, stored), "1.0.4").is_ok(), "{schema} {stored:?}");
        }
        // A newer patch, minor or major, a version nobody can read, and a schema from the future: none start.
        for (schema, stored) in [(n, Some("1.0.5")), (n, Some("1.1.0")), (n, Some("2.0.0")), (n, Some("1.10.0")), (n, Some("next")), (n + 1, None), (n + 1, Some("1.0.4"))] {
            let err = refuse_downgrade(&db_at(schema, stored), "1.0.4").expect_err(&format!("{schema} {stored:?}")).to_string();
            assert!(err.contains("older finstats 1.0.4") && err.contains("finstats restore"), "{err}");
        }
        assert!(refuse_downgrade(&db_at(n, Some("1.1.0")), "1.0.4").unwrap_err().to_string().contains("last used by finstats 1.1.0"));
    }

    #[test]
    fn opening_records_the_version_and_a_refusal_changes_nothing() {
        let dir = std::env::temp_dir().join(format!("finstats-version-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("finstats.db");
        let running = env!("CARGO_PKG_VERSION");

        let db = Db::open(&path).unwrap();
        assert_eq!(get_setting(&db.conn().unwrap(), VERSION_KEY).unwrap().as_deref(), Some(running));
        set_setting(&db.conn().unwrap(), VERSION_KEY, "999.0.0").unwrap();
        drop(db);

        assert!(Db::open(&path).is_err());
        let c = Connection::open(&path).unwrap();
        assert_eq!(get_setting(&c, VERSION_KEY).unwrap().as_deref(), Some("999.0.0"));
        drop(c);

        // An upgrade moves the mark forward.
        Connection::open(&path).unwrap().execute("UPDATE settings SET value = '0.1.0' WHERE key = ?1", [VERSION_KEY]).unwrap();
        let db = Db::open(&path).unwrap();
        assert_eq!(get_setting(&db.conn().unwrap(), VERSION_KEY).unwrap().as_deref(), Some(running));
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
