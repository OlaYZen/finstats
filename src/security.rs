//! Where people watch from, and what looks wrong about it.
//!
//! A *sighting* is a moment finstats knows where an account was: a play (`playbacks.remote_ip`, for as long
//! as it ran) or a sign-in in Jellyfin's activity log (`server_events.remote_ip`). `geo.rs` turns the address
//! into a place; plays from the home network count as being at home, which is wherever this network's own
//! public address is. Two things are reported, both derived from the whole history of one person by the pure
//! `detect()`, so a rescan always finds the same alerts and `dedupe` keeps them single:
//!
//! - `impossible_travel`: two sightings far apart (`travel_min_km`) that no flight connects
//!   (`travel_speed_kmh`), or that overlap. One alert per pair of places per day; a pair the owner has
//!   muted (a VPN, a mobile carrier that is "in" the capital) never reports again.
//! - `new_country`: the first sighting in a country, once there is a history to compare with.
//!
//! What is older than `HISTORIC_S` when it is found (an import, the first database) is filed as resolved.
//! A place is a city centre at best. Alerts are a reason to look, never proof.

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::Result;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::{AuthUser, Manager};
use crate::db::rusqlite::{Connection, OptionalExtension, params, params_from_iter, types::Value as SqlValue};
use crate::db::{self, norm_id};
use crate::geo::{self, Database, Place, distance_km};
use crate::state::{ApiError, ApiResult, App, Settings};
use crate::stats::{FilterQuery, Scope};

/// Found this long after it happened, an alert is history, not news.
const HISTORIC_S: i64 = 30 * 86_400;
/// The same two places report once a day at most.
const PAIR_QUIET_S: i64 = 86_400;
/// Activity-log entries that say where an account was.
const SIGN_IN_TYPES: &str = "'AuthenticationSucceeded','SessionStarted'";

#[derive(Clone, Copy, Debug)]
pub struct Rules {
    pub speed_kmh: f64,
    pub min_km: f64,
}

impl Rules {
    pub fn of(s: &Settings) -> Self {
        Rules { speed_kmh: s.travel_speed_kmh as f64, min_km: s.travel_min_km as f64 }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Spot {
    /// Two sightings are in the same place when this is equal: `home`, or the coordinate to a tenth of a degree.
    pub key: String,
    pub label: String,
    pub country_code: Option<String>,
    pub country: Option<String>,
    pub lat: f64,
    pub lon: f64,
}

#[derive(Clone, Debug)]
pub struct Sighting {
    pub at: i64,
    pub until: i64,
    pub spot: Spot,
    /// "Big Buck Bunny on Living room TV", "signed in"
    pub what: String,
    pub ip: String,
    /// Stable across rescans: `play:12`, `event:340`.
    pub id: String,
}

#[derive(Debug, PartialEq)]
pub struct Finding {
    pub kind: &'static str,
    pub severity: &'static str,
    pub at: i64,
    pub dedupe: String,
    pub details: Value,
}

pub fn place_label(p: &Place) -> String {
    match (&p.city, &p.country) {
        (Some(city), Some(country)) => format!("{city}, {country}"),
        (None, Some(country)) => country.clone(),
        (Some(city), None) => city.clone(),
        (None, None) => p.country_code.clone().unwrap_or_else(|| "Unknown place".into()),
    }
}

fn spot_of(p: &Place, home: bool) -> Option<Spot> {
    let (lat, lon) = (p.latitude?, p.longitude?);
    Some(Spot {
        key: if home { "home".into() } else { format!("{lat:.1},{lon:.1}") },
        label: if home { format!("Home ({})", place_label(p)) } else { place_label(p) },
        country_code: p.country_code.clone(),
        country: p.country.clone(),
        lat,
        lon,
    })
}

fn pair_key(a: &Spot, b: &Spot) -> String {
    if a.key <= b.key { format!("{}|{}", a.key, b.key) } else { format!("{}|{}", b.key, a.key) }
}

fn side(s: &Sighting) -> Value {
    json!({ "place": s.spot.label, "country_code": s.spot.country_code, "latitude": s.spot.lat, "longitude": s.spot.lon,
            "at": s.at, "until": s.until, "what": s.what, "ip": s.ip, "home": s.spot.key == "home" })
}

/// Everything wrong with one person's sightings, which must be in the order they began.
/// `muted` holds the place pairs (`pair_key`) the owner has declared fine for this person.
pub fn detect(user_id: &str, sightings: &[Sighting], rules: &Rules, muted: &HashSet<String>) -> Vec<Finding> {
    let mut out = vec![];
    let mut countries: HashSet<&str> = HashSet::new();
    let mut last_report: HashMap<String, i64> = HashMap::new();
    // The sighting before this one, and the one that lasted longest: a film still running at home
    // says more about a sign-in abroad than a short play that ended hours ago.
    let mut prev: Option<&Sighting> = None;
    let mut longest: Option<&Sighting> = None;

    for s in sightings {
        let mut worst: Option<(&Sighting, f64, i64, Option<f64>)> = None;
        for earlier in [prev, longest].into_iter().flatten() {
            if earlier.spot.key == s.spot.key {
                continue;
            }
            let km = distance_km(earlier.spot.lat, earlier.spot.lon, s.spot.lat, s.spot.lon);
            if km < rules.min_km {
                continue;
            }
            let gap_s = s.at - earlier.until;
            let speed = (gap_s > 0).then(|| km / (gap_s as f64 / 3600.0));
            if speed.is_some_and(|v| v <= rules.speed_kmh) {
                continue;
            }
            // Overlapping beats fast, faster beats fast.
            let rank = |v: Option<f64>| v.unwrap_or(f64::INFINITY);
            if worst.as_ref().is_none_or(|w| rank(speed) > rank(w.3)) {
                worst = Some((earlier, km, gap_s, speed));
            }
        }
        if let Some((earlier, km, gap_s, speed)) = worst {
            let pair = pair_key(&earlier.spot, &s.spot);
            let quiet = last_report.get(&pair).is_some_and(|t| s.at - t < PAIR_QUIET_S);
            if !muted.contains(&pair) && !quiet {
                last_report.insert(pair.clone(), s.at);
                out.push(Finding {
                    kind: "impossible_travel",
                    severity: "high",
                    at: s.at,
                    dedupe: format!("travel:{user_id}:{}", s.id),
                    details: json!({
                        "from": side(earlier), "to": side(s), "pair": pair,
                        "distance_km": km.round(), "gap_s": gap_s.max(0), "overlap": gap_s <= 0,
                        "speed_kmh": speed.map(f64::round),
                    }),
                });
            }
        }

        if let Some(cc) = s.spot.country_code.as_deref()
            && countries.insert(cc)
            && countries.len() > 1
        {
            out.push(Finding {
                kind: "new_country",
                severity: "medium",
                at: s.at,
                dedupe: format!("country:{user_id}:{cc}"),
                details: json!({ "to": side(s), "country": s.spot.country, "country_code": cc, "known": countries.len() - 1 }),
            });
        }

        if longest.is_none_or(|l| s.until >= l.until) {
            longest = Some(s);
        }
        prev = Some(s);
    }
    out
}

// ---------------------------------------------------------------- addresses → places

/// The address in an activity-log line ("IP address: 203.0.113.9"). The wording follows the server's
/// language, so this takes the first word that is an address rather than trusting the label.
pub fn event_ip(short_overview: &str) -> Option<String> {
    short_overview.split_whitespace().find_map(|w| {
        let w = w.trim_matches(|c: char| !c.is_ascii_hexdigit() && c != ':' && c != '.');
        crate::network::canonical(w).or_else(|| crate::network::canonical(w.trim_end_matches(['.', ':'])))
    })
}

/// Sign-ins restored from an older backup, or synced before this existed, have no address column yet.
fn fill_event_ips(conn: &Connection) -> Result<usize> {
    let rows: Vec<(i64, Option<String>)> = conn
        .prepare("SELECT id, short_overview FROM server_events WHERE remote_ip IS NULL")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    let mut stmt = conn.prepare_cached("UPDATE server_events SET remote_ip = ?1 WHERE id = ?2")?;
    for (id, text) in &rows {
        stmt.execute(params![text.as_deref().and_then(event_ip).unwrap_or_default(), id])?;
    }
    Ok(rows.len())
}

/// Give every address that has none yet a row in `ip_locations` (an empty one when it has no place).
fn locate_new(conn: &Connection, geo: &Database) -> Result<usize> {
    let ips: Vec<String> = conn
        .prepare(
            "SELECT ip FROM (SELECT remote_ip AS ip FROM playbacks WHERE remote_ip IS NOT NULL
                             UNION SELECT remote_ip FROM server_events WHERE remote_ip <> ''
                             UNION SELECT ip FROM home_addresses)
             WHERE ip NOT IN (SELECT ip FROM ip_locations)",
        )?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let now = db::now();
    let mut stmt = conn.prepare_cached(
        "INSERT OR REPLACE INTO ip_locations(ip, country_code, country, region, city, latitude, longitude, timezone, looked_up_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )?;
    for ip in &ips {
        let p = geo.lookup(ip).unwrap_or_default();
        stmt.execute(params![ip, p.country_code, p.country, p.region, p.city, p.latitude, p.longitude, p.timezone, now])?;
    }
    Ok(ips.len())
}

fn stored_place(conn: &Connection, ip: &str) -> Result<Option<Place>> {
    Ok(conn
        .prepare_cached("SELECT country_code, country, region, city, latitude, longitude, timezone FROM ip_locations WHERE ip = ?1")?
        .query_row([ip], |r| {
            Ok(Place { country_code: r.get(0)?, country: r.get(1)?, region: r.get(2)?, city: r.get(3)?, latitude: r.get(4)?, longitude: r.get(5)?, timezone: r.get(6)? })
        })
        .optional()?)
}

/// Where "at home" is: the place of this network's public address, the most recently seen one that has a place.
pub fn home_place(conn: &Connection) -> Result<Option<Place>> {
    Ok(conn
        .prepare_cached(
            "SELECT l.country_code, l.country, l.region, l.city, l.latitude, l.longitude, l.timezone
             FROM home_addresses h JOIN ip_locations l ON l.ip = h.ip
             WHERE l.latitude IS NOT NULL ORDER BY (h.source = 'lookup') DESC, h.last_seen DESC LIMIT 1",
        )?
        .query_row([], |r| {
            Ok(Place { country_code: r.get(0)?, country: r.get(1)?, region: r.get(2)?, city: r.get(3)?, latitude: r.get(4)?, longitude: r.get(5)?, timezone: r.get(6)? })
        })
        .optional()?)
}

// ---------------------------------------------------------------- scanning

fn sightings_of(conn: &Connection, user_id: &str, home: Option<&Spot>) -> Result<Vec<Sighting>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT 'play:' || p.id, p.started_at, p.ended_at, p.remote_ip, p.is_local,
                COALESCE(p.series_name || ' · ', '') || p.item_name || COALESCE(' on ' || p.device_name, '')
         FROM playbacks p WHERE p.user_id = ?1 AND p.remote_ip IS NOT NULL
         UNION ALL
         SELECT 'event:' || e.id, e.date, e.date, e.remote_ip, NULL, e.name
         FROM server_events e WHERE e.user_id = ?1 AND e.remote_ip <> '' AND e.type IN ({SIGN_IN_TYPES})
         ORDER BY 2, 1"
    ))?;
    let rows: Vec<(String, i64, i64, String, Option<bool>, String)> =
        stmt.query_map([user_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?.collect::<Result<_, _>>()?;
    let mut spots: HashMap<String, Option<Spot>> = HashMap::new();
    let mut out = Vec::with_capacity(rows.len());
    for (id, at, until, ip, is_local, what) in rows {
        if !spots.contains_key(&ip) {
            let local = match is_local {
                Some(l) => l,
                None => crate::network::classify(conn, &ip)?.unwrap_or(false),
            };
            let spot = if local { home.cloned() } else { stored_place(conn, &ip)?.and_then(|p| spot_of(&p, false)) };
            spots.insert(ip.clone(), spot);
        }
        if let Some(spot) = spots[&ip].clone() {
            out.push(Sighting { at, until: until.max(at), spot, what, ip, id });
        }
    }
    Ok(out)
}

/// Look at one person's history, or everyone's, and file what is new. Returns how many alerts were added.
pub fn scan(conn: &Connection, geo: &Database, rules: &Rules, only_user: Option<&str>) -> Result<usize> {
    fill_event_ips(conn)?;
    locate_new(conn, geo)?;
    file_alerts(conn, rules, only_user)
}

/// The part of a scan that needs no database file: every address already has its place.
fn file_alerts(conn: &Connection, rules: &Rules, only_user: Option<&str>) -> Result<usize> {
    let home = home_place(conn)?.and_then(|p| spot_of(&p, true));
    let users: Vec<(String, String)> = conn
        .prepare(
            "SELECT p.user_id, COALESCE(u.name, MAX(p.user_name)) FROM playbacks p LEFT JOIN users u ON u.id = p.user_id
             WHERE p.remote_ip IS NOT NULL AND (?1 IS NULL OR p.user_id = ?1) GROUP BY p.user_id
             UNION
             SELECT e.user_id, u.name FROM server_events e JOIN users u ON u.id = e.user_id
             WHERE e.remote_ip <> '' AND (?1 IS NULL OR e.user_id = ?1) GROUP BY e.user_id",
        )?
        .query_map([only_user], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    let now = db::now();
    let mut added = 0;
    let mut seen_users = HashSet::new();
    for (user_id, user_name) in users {
        if !seen_users.insert(user_id.clone()) {
            continue;
        }
        let muted: HashSet<String> = conn
            .prepare_cached("SELECT details FROM security_alerts WHERE user_id = ?1 AND muted = 1")?
            .query_map([&user_id], |r| r.get::<_, String>(0))?
            .filter_map(|d| serde_json::from_str::<Value>(&d.ok()?).ok()?["pair"].as_str().map(str::to_string))
            .collect();
        let sightings = sightings_of(conn, &user_id, home.as_ref())?;
        for f in detect(&user_id, &sightings, rules, &muted) {
            let historic = now - f.at > HISTORIC_S;
            added += conn.prepare_cached(
                "INSERT OR IGNORE INTO security_alerts(kind, severity, user_id, user_name, at, dedupe, details, created_at, resolved_at, resolved_by, note)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            )?
            .execute(params![f.kind, f.severity, user_id, user_name, f.at, f.dedupe, f.details.to_string(), now,
                historic.then_some(now), historic.then_some("finstats"), historic.then_some("Found in older history")])?;
        }
    }
    Ok(added)
}

/// After a play begins or the log was read: is anything new wrong? Does nothing without a database.
pub async fn check(app: &App, user_id: Option<String>) {
    let Some(geo) = app.geo.get() else { return };
    let rules = Rules::of(&app.settings());
    match app.db.call(move |c| scan(c, &geo, &rules, user_id.as_deref())).await {
        Ok(n) if n > 0 => tracing::warn!("{n} new security alert{}", if n == 1 { "" } else { "s" }),
        Ok(_) => {}
        Err(e) => tracing::warn!("security scan failed: {e:#}"),
    }
}

/// The database changed: every address gets its place again, then everyone is looked at.
pub async fn refresh_all(app: &App) {
    let geo = app.geo.get();
    let rules = Rules::of(&app.settings());
    let result = app
        .db
        .call(move |c| {
            let tx = c.transaction()?;
            tx.execute("DELETE FROM ip_locations", [])?;
            let added = match &geo {
                Some(geo) => scan(&tx, geo, &rules, None)?,
                None => 0,
            };
            tx.commit()?;
            Ok(added)
        })
        .await;
    if let Err(e) = result {
        tracing::warn!("re-reading places failed: {e:#}");
    }
}

// ---------------------------------------------------------------- API

/// Places are network details about everybody: both permissions, or nothing.
fn gate(user: &AuthUser) -> Result<(), ApiError> {
    if user.perms.see_network && user.perms.see_everyone { Ok(()) } else { Err(ApiError::not_permitted("see where people watch from")) }
}

fn database_json(app: &App) -> Value {
    match app.geo.get() {
        Some(db) => json!({ "kind": db.kind, "built_at": db.built_at, "dbip": db.is_dbip(), "file": db.path.file_name().map(|n| n.to_string_lossy().into_owned()) }),
        None => Value::Null,
    }
}

pub fn status_json(app: &App) -> Value {
    json!({ "database": database_json(app), "folder": geo::dir(&app.data_dir).display().to_string(),
            "from_env": std::env::var("FINSTATS_GEOIP_DB").is_ok_and(|p| !p.trim().is_empty()), "source": geo::download_url("YYYY-MM") })
}

#[derive(Default)]
struct PlaceAgg {
    place: Place,
    home: bool,
    plays: i64,
    watch_s: i64,
    sign_ins: i64,
    ips: i64,
    last_seen: i64,
    users: BTreeMap<String, (String, i64, i64)>,
}

fn place_at(r: &crate::db::rusqlite::Row, first: usize) -> crate::db::rusqlite::Result<Place> {
    Ok(Place { latitude: r.get(first)?, longitude: r.get(first + 1)?, city: r.get(first + 2)?, region: r.get(first + 3)?, country: r.get(first + 4)?, country_code: r.get(first + 5)?, timezone: None })
}

const PLACE_COLS: &str = "l.latitude, l.longitude, l.city, l.region, l.country, l.country_code";

pub async fn overview(State(app): State<App>, user: AuthUser, Query(q): Query<FilterQuery>) -> ApiResult {
    gate(&user)?;
    let scope = Scope::new(&app, &user, &q);
    let mut live = app.live.read().unwrap().clone();
    let geo = app.geo.get();
    let with_failed = user.perms.see_server;
    let mut body = app
        .db
        .call(move |c| {
            let scope = scope.resolve(c)?;
            let (since, who) = (scope.since.unwrap_or(0), scope.user_id.clone());
            let home = home_place(c)?;
            let mut places: BTreeMap<String, PlaceAgg> = BTreeMap::new();
            let key = |p: &Place| format!("{:.4},{:.4}", p.latitude.unwrap_or(0.0), p.longitude.unwrap_or(0.0));

            // Plays away from home, by place and person.
            let mut stmt = c.prepare(&format!(
                "SELECT {PLACE_COLS}, p.user_id, COALESCE(u.name, p.user_name), COUNT(*), COALESCE(SUM(p.duration_s), 0), MAX(p.started_at), COUNT(DISTINCT p.remote_ip)
                 FROM playbacks p JOIN ip_locations l ON l.ip = p.remote_ip LEFT JOIN users u ON u.id = p.user_id
                 WHERE COALESCE(p.is_local, 0) = 0 AND l.latitude IS NOT NULL AND p.started_at >= ?1 AND (?2 IS NULL OR p.user_id = ?2)
                 GROUP BY l.latitude, l.longitude, p.user_id"
            ))?;
            let rows = stmt.query_map(params![since, who], |r| Ok((place_at(r, 0)?, r.get::<_, String>(6)?, r.get::<_, String>(7)?, r.get::<_, i64>(8)?, r.get::<_, i64>(9)?, r.get::<_, i64>(10)?, r.get::<_, i64>(11)?)))?;
            for row in rows {
                let (place, uid, name, plays, watch_s, last, ips) = row?;
                let a = places.entry(key(&place)).or_insert_with(|| PlaceAgg { place: place.clone(), ..Default::default() });
                a.plays += plays;
                a.watch_s += watch_s;
                a.ips += ips;
                a.last_seen = a.last_seen.max(last);
                let u = a.users.entry(uid).or_insert((name, 0, 0));
                u.1 += plays;
            }

            // Plays at home: one dot where this network is.
            if let Some(h) = &home {
                let mut stmt = c.prepare(
                    "SELECT p.user_id, COALESCE(u.name, p.user_name), COUNT(*), COALESCE(SUM(p.duration_s), 0), MAX(p.started_at)
                     FROM playbacks p LEFT JOIN users u ON u.id = p.user_id
                     WHERE p.is_local = 1 AND p.started_at >= ?1 AND (?2 IS NULL OR p.user_id = ?2) GROUP BY p.user_id",
                )?;
                let rows = stmt.query_map(params![since, who], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?, r.get::<_, i64>(4)?)))?;
                for row in rows {
                    let (uid, name, plays, watch_s, last) = row?;
                    let a = places.entry("home".into()).or_insert_with(|| PlaceAgg { place: h.clone(), home: true, ..Default::default() });
                    a.plays += plays;
                    a.watch_s += watch_s;
                    a.last_seen = a.last_seen.max(last);
                    a.users.entry(uid).or_insert((name, 0, 0)).1 += plays;
                }
            }

            // Sign-ins away from home.
            let mut stmt = c.prepare(&format!(
                "SELECT {PLACE_COLS}, e.user_id, u.name, COUNT(*), MAX(e.date)
                 FROM server_events e JOIN ip_locations l ON l.ip = e.remote_ip JOIN users u ON u.id = e.user_id
                 WHERE e.type = 'AuthenticationSucceeded' AND e.remote_ip <> '' AND l.latitude IS NOT NULL
                   AND e.remote_ip NOT IN (SELECT ip FROM home_addresses) AND e.date >= ?1 AND (?2 IS NULL OR e.user_id = ?2)
                 GROUP BY l.latitude, l.longitude, e.user_id"
            ))?;
            let rows = stmt.query_map(params![since, who], |r| Ok((place_at(r, 0)?, r.get::<_, String>(6)?, r.get::<_, String>(7)?, r.get::<_, i64>(8)?, r.get::<_, i64>(9)?)))?;
            for row in rows {
                let (place, uid, name, n, last) = row?;
                let a = places.entry(key(&place)).or_insert_with(|| PlaceAgg { place: place.clone(), ..Default::default() });
                a.sign_ins += n;
                a.last_seen = a.last_seen.max(last);
                a.users.entry(uid).or_insert((name, 0, 0)).2 += n;
            }

            // Failed sign-ins have no account, only a name somebody typed. Part of the server log.
            let mut failed: BTreeMap<String, (Place, i64, i64, Vec<String>)> = BTreeMap::new();
            if with_failed && who.is_none() {
                let mut stmt = c.prepare(&format!(
                    "SELECT {PLACE_COLS}, e.name, COUNT(*), MAX(e.date)
                     FROM server_events e JOIN ip_locations l ON l.ip = e.remote_ip
                     WHERE e.type = 'AuthenticationFailed' AND e.remote_ip <> '' AND l.latitude IS NOT NULL
                       AND e.remote_ip NOT IN (SELECT ip FROM home_addresses) AND e.date >= ?1
                     GROUP BY l.latitude, l.longitude, e.name ORDER BY MAX(e.date) DESC"
                ))?;
                let rows = stmt.query_map(params![since], |r| Ok((place_at(r, 0)?, r.get::<_, String>(6)?, r.get::<_, i64>(7)?, r.get::<_, i64>(8)?)))?;
                for row in rows {
                    let (place, name, n, last) = row?;
                    let f = failed.entry(key(&place)).or_insert_with(|| (place.clone(), 0, 0, vec![]));
                    f.1 += n;
                    f.2 = f.2.max(last);
                    if f.3.len() < 5 {
                        f.3.push(name);
                    }
                }
            }

            // Streams running right now.
            if let Some(u) = &who {
                live.retain(|s| s["user_id"].as_str() == Some(u.as_str()));
            }
            let mut now_playing = vec![];
            for s in &live {
                let Some(ip) = s["remote_ip"].as_str() else { continue };
                let local = crate::network::classify(c, ip)?.unwrap_or(false);
                let place = if local { home.clone() } else { geo.as_ref().and_then(|g| g.lookup(ip)) };
                let Some(p) = place.filter(|p| p.latitude.is_some()) else { continue };
                now_playing.push(json!({
                    "user_id": s["user_id"], "user_name": s["user_name"], "item_name": s["item_name"], "series_name": s["series_name"],
                    "device_name": s["device_name"], "home": local, "place": place_label(&p), "latitude": p.latitude, "longitude": p.longitude,
                }));
            }

            let mut countries: BTreeMap<String, (String, i64, HashSet<String>)> = BTreeMap::new();
            for a in places.values() {
                if let Some(cc) = &a.place.country_code {
                    let e = countries.entry(cc.clone()).or_insert_with(|| (a.place.country.clone().unwrap_or_else(|| cc.clone()), 0, HashSet::new()));
                    e.1 += a.plays;
                    e.2.extend(a.users.keys().cloned());
                }
            }
            let mut countries: Vec<Value> = countries.into_iter().map(|(cc, (name, plays, users))| json!({ "code": cc, "name": name, "plays": plays, "users": users.len() })).collect();
            countries.sort_by_key(|v| std::cmp::Reverse(v["plays"].as_i64().unwrap_or(0)));

            let mut list: Vec<Value> = places
                .into_values()
                .map(|a| {
                    let mut users: Vec<Value> = a.users.into_iter().map(|(id, (name, plays, sign_ins))| json!({ "id": id, "name": name, "plays": plays, "sign_ins": sign_ins })).collect();
                    users.sort_by_key(|u| std::cmp::Reverse(u["plays"].as_i64().unwrap_or(0) * 1000 + u["sign_ins"].as_i64().unwrap_or(0)));
                    json!({
                        "label": place_label(&a.place), "home": a.home, "city": a.place.city, "region": a.place.region, "country": a.place.country,
                        "country_code": a.place.country_code, "latitude": a.place.latitude, "longitude": a.place.longitude,
                        "plays": a.plays, "watch_s": a.watch_s, "sign_ins": a.sign_ins, "addresses": a.ips, "last_seen": a.last_seen, "users": users,
                    })
                })
                .collect();
            list.sort_by_key(|v| std::cmp::Reverse(v["plays"].as_i64().unwrap_or(0) * 1000 + v["sign_ins"].as_i64().unwrap_or(0)));
            let failed: Vec<Value> = failed
                .into_values()
                .map(|(p, n, last, names)| json!({ "label": place_label(&p), "country_code": p.country_code, "latitude": p.latitude, "longitude": p.longitude, "attempts": n, "last_at": last, "events": names }))
                .collect();

            let open_alerts: i64 = c.query_row("SELECT COUNT(*) FROM security_alerts WHERE resolved_at IS NULL AND (?1 IS NULL OR user_id = ?1)", [&who], |r| r.get(0))?;
            let unplaced: i64 = c.query_row(
                "SELECT COUNT(DISTINCT p.remote_ip) FROM playbacks p JOIN ip_locations l ON l.ip = p.remote_ip WHERE COALESCE(p.is_local, 0) = 0 AND l.latitude IS NULL",
                [],
                |r| r.get(0),
            )?;
            Ok(json!({ "places": list, "countries": countries, "failed": failed, "now_playing": now_playing, "open_alerts": open_alerts,
                       "home_known": home.is_some(), "addresses_without_place": unplaced, "days": scope.days }))
        })
        .await?;
    body["database"] = database_json(&app);
    body["can_manage"] = json!(user.perms.manage);
    Ok(Json(body))
}

#[derive(Deserialize)]
pub struct AlertsQuery {
    /// open (default) | resolved | all
    status: Option<String>,
    user_id: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

fn alert_json(r: &crate::db::rusqlite::Row) -> crate::db::rusqlite::Result<Value> {
    let details: String = r.get(6)?;
    Ok(json!({
        "id": r.get::<_, i64>(0)?, "kind": r.get::<_, String>(1)?, "severity": r.get::<_, String>(2)?,
        "user_id": r.get::<_, String>(3)?, "user_name": r.get::<_, String>(4)?, "at": r.get::<_, i64>(5)?,
        "details": serde_json::from_str::<Value>(&details).unwrap_or(Value::Null),
        "resolved_at": r.get::<_, Option<i64>>(7)?, "resolved_by": r.get::<_, Option<String>>(8)?, "note": r.get::<_, Option<String>>(9)?,
        "muted": r.get::<_, bool>(10)?, "has_image": r.get::<_, Option<bool>>(11)?.unwrap_or(false),
    }))
}

pub async fn alerts(State(app): State<App>, user: AuthUser, Query(q): Query<AlertsQuery>) -> ApiResult {
    gate(&user)?;
    let per_page = q.per_page.unwrap_or(25).clamp(1, 100);
    let page = q.page.unwrap_or(1).max(1);
    let who = q.user_id.as_deref().map(norm_id).filter(|s| !s.is_empty());
    let status = q.status.unwrap_or_else(|| "open".into());
    let body = app
        .db
        .call(move |c| {
            let mut wh = vec!["1 = 1".to_string()];
            let mut args: Vec<SqlValue> = vec![];
            match status.as_str() {
                "resolved" => wh.push("a.resolved_at IS NOT NULL".into()),
                "all" => {}
                _ => wh.push("a.resolved_at IS NULL".into()),
            }
            if let Some(u) = who {
                wh.push("a.user_id = ?".into());
                args.push(u.into());
            }
            let wh = wh.join(" AND ");
            let total: i64 = c.query_row(&format!("SELECT COUNT(*) FROM security_alerts a WHERE {wh}"), params_from_iter(args.iter()), |r| r.get(0))?;
            let open: i64 = c.query_row("SELECT COUNT(*) FROM security_alerts WHERE resolved_at IS NULL", [], |r| r.get(0))?;
            let sql = format!(
                "SELECT a.id, a.kind, a.severity, a.user_id, COALESCE(u.name, a.user_name), a.at, a.details, a.resolved_at, a.resolved_by, a.note, a.muted, (u.image_tag IS NOT NULL)
                 FROM security_alerts a LEFT JOIN users u ON u.id = a.user_id WHERE {wh}
                 ORDER BY a.at DESC, a.id DESC LIMIT {per_page} OFFSET {}",
                (page - 1) * per_page
            );
            let rows: Vec<Value> = c.prepare(&sql)?.query_map(params_from_iter(args.iter()), alert_json)?.collect::<Result<_, _>>()?;
            Ok(json!({ "rows": rows, "total": total, "open": open, "page": page, "per_page": per_page }))
        })
        .await?;
    Ok(Json(body))
}

#[derive(Deserialize, Default)]
pub struct ResolveBody {
    #[serde(default)]
    note: Option<String>,
    /// For impossible travel: never report these two places for this person again.
    #[serde(default)]
    mute: bool,
}

pub async fn resolve(State(app): State<App>, Manager(user): Manager, Path(id): Path<i64>, Json(body): Json<ResolveBody>) -> ApiResult {
    gate(&user)?;
    let note = body.note.map(|n| n.trim().chars().take(500).collect::<String>()).filter(|n| !n.is_empty());
    let mute = body.mute;
    let changed = app
        .db
        .call(move |c| {
            let tx = c.transaction()?;
            let found: Option<(String, String, String)> = tx.query_row("SELECT kind, user_id, details FROM security_alerts WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional()?;
            let Some((kind, user_id, details)) = found else { return Ok(None) };
            let now = db::now();
            tx.execute(
                "UPDATE security_alerts SET resolved_at = COALESCE(resolved_at, ?1), resolved_by = ?2, note = ?3, muted = ?4 WHERE id = ?5",
                params![now, user.name, note, mute && kind == "impossible_travel", id],
            )?;
            let mut also = 0;
            if mute && kind == "impossible_travel" {
                // The same two places, still open: they are answered by this too.
                let pair = serde_json::from_str::<Value>(&details).ok().and_then(|d| d["pair"].as_str().map(str::to_string));
                if let Some(pair) = pair {
                    also = tx.execute(
                        "UPDATE security_alerts SET resolved_at = ?1, resolved_by = ?2, note = 'Same two places' WHERE resolved_at IS NULL AND user_id = ?3 AND kind = 'impossible_travel' AND json_extract(details, '$.pair') = ?4",
                        params![now, user.name, user_id, pair],
                    )?;
                }
            }
            tx.commit()?;
            Ok(Some(also))
        })
        .await?;
    match changed {
        Some(also) => Ok(Json(json!({ "ok": true, "also_resolved": also }))),
        None => Err(ApiError::not_found("Alert")),
    }
}

pub async fn reopen(State(app): State<App>, Manager(user): Manager, Path(id): Path<i64>) -> ApiResult {
    gate(&user)?;
    let n = app.db.call(move |c| Ok(c.execute("UPDATE security_alerts SET resolved_at = NULL, resolved_by = NULL, note = NULL, muted = 0 WHERE id = ?1", [id])?)).await?;
    if n == 0 { Err(ApiError::not_found("Alert")) } else { Ok(Json(json!({ "ok": true }))) }
}

pub async fn resolve_all(State(app): State<App>, Manager(user): Manager) -> ApiResult {
    gate(&user)?;
    let n = app
        .db
        .call(move |c| Ok(c.execute("UPDATE security_alerts SET resolved_at = ?1, resolved_by = ?2 WHERE resolved_at IS NULL", params![db::now(), user.name])?))
        .await?;
    Ok(Json(json!({ "ok": true, "resolved": n })))
}

/// Fetch the database now. Managers only; the monthly refresh is the `geoip_download` setting.
pub async fn download_database(State(app): State<App>, Manager(_): Manager) -> ApiResult<Response> {
    if std::env::var("FINSTATS_GEOIP_DB").is_ok_and(|p| !p.trim().is_empty()) {
        return Err(ApiError::bad_request("The database is set with FINSTATS_GEOIP_DB; replace that file instead"));
    }
    if !geo::spawn_download(&app) {
        return Err(ApiError::new(StatusCode::CONFLICT, "The database is already being downloaded"));
    }
    Ok((StatusCode::ACCEPTED, Json(json!({ "ok": true }))).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spot(key: &str, cc: &str, lat: f64, lon: f64) -> Spot {
        Spot { key: key.into(), label: key.into(), country_code: Some(cc.into()), country: Some(cc.into()), lat, lon }
    }
    fn london() -> Spot { spot("home", "GB", 51.5, -0.1) }
    fn paris() -> Spot { spot("48.9,2.4", "FR", 48.86, 2.35) }
    fn new_york() -> Spot { spot("40.7,-74.0", "US", 40.71, -74.0) }
    fn leeds() -> Spot { spot("53.8,-1.5", "GB", 53.8, -1.55) }

    fn seen(id: i64, spot: Spot, at: i64, minutes: i64) -> Sighting {
        Sighting { at, until: at + minutes * 60, spot, what: "Big Buck Bunny".into(), ip: "203.0.113.9".into(), id: format!("play:{id}") }
    }
    const RULES: Rules = Rules { speed_kmh: 900.0, min_km: 300.0 };
    const H: i64 = 3600;

    fn kinds(f: &[Finding]) -> Vec<&str> {
        f.iter().map(|f| f.kind).collect()
    }

    #[test]
    fn a_flight_is_fine_and_a_teleport_is_not() {
        // London, then New York eight hours after the film ended: about 700 km/h.
        let ok = [seen(1, london(), 0, 90), seen(2, new_york(), 90 * 60 + 8 * H, 30)];
        assert_eq!(kinds(&detect("alice", &ok, &RULES, &HashSet::new())), ["new_country"]);

        // The same trip in one hour.
        let bad = [seen(1, london(), 0, 90), seen(2, new_york(), 90 * 60 + H, 30)];
        let found = detect("alice", &bad, &RULES, &HashSet::new());
        assert_eq!(kinds(&found), ["impossible_travel", "new_country"]);
        assert_eq!(found[0].dedupe, "travel:alice:play:2");
        assert_eq!(found[0].details["overlap"], false);
        assert!(found[0].details["speed_kmh"].as_f64().unwrap() > 5000.0);
        assert_eq!(found[0].details["from"]["home"], true);
    }

    #[test]
    fn two_places_at_once_is_reported_without_a_speed() {
        let both = [seen(1, london(), 0, 120), seen(2, new_york(), 30 * 60, 20)];
        let found = detect("alice", &both, &RULES, &HashSet::new());
        assert_eq!(found[0].kind, "impossible_travel");
        assert_eq!(found[0].details["overlap"], true);
        assert!(found[0].details["speed_kmh"].is_null());
    }

    #[test]
    fn a_long_play_still_counts_after_a_short_one() {
        // A film at home until 03:00, a glance at the phone at home, then New York at 03:20.
        // Against the glance it is a slow trip; against the film it is not.
        let s = [seen(1, london(), 0, 180), seen(2, london(), 600, 0), seen(3, new_york(), 3 * H + 20 * 60, 10)];
        assert_eq!(kinds(&detect("alice", &s, &RULES, &HashSet::new())), ["impossible_travel", "new_country"]);
    }

    #[test]
    fn nearby_places_never_count_however_fast() {
        // Home Wi-Fi, then a phone whose carrier is "in" Leeds a minute later: 270 km, under the minimum.
        let s = [seen(1, london(), 0, 10), seen(2, leeds(), 11 * 60, 10)];
        assert!(detect("alice", &s, &RULES, &HashSet::new()).is_empty());
    }

    #[test]
    fn a_pair_reports_once_a_day_and_never_when_muted() {
        // Flapping between home and a VPN exit all evening, and again two days later.
        let mut s = vec![];
        for i in 0..6 {
            s.push(seen(i, if i % 2 == 0 { london() } else { new_york() }, i * 20 * 60, 5));
        }
        s.push(seen(10, london(), 48 * H, 5));
        s.push(seen(11, new_york(), 48 * H + 600, 5));
        let found = detect("alice", &s, &RULES, &HashSet::new());
        assert_eq!(found.iter().filter(|f| f.kind == "impossible_travel").count(), 2);

        let muted: HashSet<String> = [pair_key(&london(), &new_york())].into();
        assert_eq!(kinds(&detect("alice", &s, &RULES, &muted)), ["new_country"]);
    }

    #[test]
    fn the_first_country_is_home_and_each_new_one_reports_once() {
        let s = [seen(1, london(), 0, 10), seen(2, leeds(), 24 * H, 10), seen(3, paris(), 48 * H, 10), seen(4, paris(), 72 * H, 10), seen(5, london(), 96 * H, 10)];
        let found = detect("alice", &s, &RULES, &HashSet::new());
        assert_eq!(kinds(&found), ["new_country"]);
        assert_eq!(found[0].dedupe, "country:alice:FR");
        assert_eq!(found[0].details["known"], 1);
    }

    #[test]
    fn the_address_is_found_whatever_the_servers_language() {
        assert_eq!(event_ip("IP address: 203.0.113.9").as_deref(), Some("203.0.113.9"));
        assert_eq!(event_ip("IP-adresse: ::ffff:203.0.113.9").as_deref(), Some("203.0.113.9"));
        assert_eq!(event_ip("Adresse IP : 2001:db8::1.").as_deref(), Some("2001:db8::1"));
        assert_eq!(event_ip("App: Jellyfin Web"), None);
        assert_eq!(event_ip(""), None);
    }

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        for m in crate::db::MIGRATIONS {
            c.execute_batch(m).unwrap();
        }
        c
    }

    #[test]
    fn old_findings_are_filed_as_history_and_a_rescan_adds_nothing() {
        let c = conn();
        let now = db::now();
        c.execute_batch(&format!(
            "INSERT INTO ip_locations(ip, country_code, country, city, latitude, longitude, looked_up_at) VALUES
                ('198.51.100.1', 'GB', 'United Kingdom', 'London', 51.5, -0.1, 0), ('198.51.100.2', 'US', 'United States', 'New York', 40.7, -74.0, 0);
             INSERT INTO playbacks(source, user_id, user_name, item_id, item_name, item_type, started_at, ended_at, duration_s, remote_ip, is_local) VALUES
                ('live', 'u1', 'alice', 'i1', 'Big Buck Bunny', 'Movie', {old}, {old} + 600, 600, '198.51.100.1', 0),
                ('live', 'u1', 'alice', 'i1', 'Big Buck Bunny', 'Movie', {old} + 1200, {old} + 1800, 600, '198.51.100.2', 0),
                ('live', 'u1', 'alice', 'i1', 'Big Buck Bunny', 'Movie', {recent}, {recent} + 600, 600, '198.51.100.1', 0),
                ('live', 'u1', 'alice', 'i1', 'Big Buck Bunny', 'Movie', {recent} + 1200, {recent} + 1800, 600, '198.51.100.2', 0);",
            old = now - 90 * 86_400,
            recent = now - 86_400
        ))
        .unwrap();
        assert_eq!(file_alerts(&c, &RULES, None).unwrap(), 3);
        let filed: Vec<(String, bool, Option<String>)> = c
            .prepare("SELECT kind, resolved_at IS NOT NULL, resolved_by FROM security_alerts ORDER BY at")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        // Three months old: history. Yesterday: open.
        assert_eq!(filed[0], ("impossible_travel".into(), true, Some("finstats".into())));
        assert_eq!(filed[1], ("new_country".into(), true, Some("finstats".into())));
        assert_eq!(filed[2], ("impossible_travel".into(), false, None));
        assert_eq!(file_alerts(&c, &RULES, None).unwrap(), 0);
        assert_eq!(file_alerts(&c, &RULES, Some("u1")).unwrap(), 0);
    }
}
