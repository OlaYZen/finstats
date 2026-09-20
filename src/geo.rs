//! Where an address is, roughly: country, city and a coordinate, read from a local database file.
//!
//! Nothing is asked of anyone per lookup. finstats reads a MaxMind-format city database (`.mmdb`) from
//! `<data dir>/geoip/` (or the file named in `FINSTATS_GEOIP_DB`): DB-IP's free "IP to City Lite",
//! MaxMind's GeoLite2-City or anything else with the same record shape. The owner can drop a file there, or
//! let finstats fetch DB-IP's monthly file (setting `geoip_download`, off by default). That download is a
//! plain GET of a public file: no address, version or identifier of this install goes with it.
//!
//! A coordinate from such a database is the centre of a city or of an ISP's region, never a household.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use chrono::Datelike;
use maxminddb::Reader;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::db::is_local_ip;
use crate::state::App;

const EARTH_RADIUS_KM: f64 = 6371.0;
/// DB-IP publishes one file per month under this name; `{}` is `YYYY-MM`.
const DBIP_URL: &str = "https://download.db-ip.com/free/dbip-city-lite-{}.mmdb.gz";
/// A downloaded file is replaced once it is this old and a newer month exists.
const REFRESH_AFTER_S: i64 = 30 * 86_400;

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Place {
    pub country_code: Option<String>,
    pub country: Option<String>,
    pub region: Option<String>,
    pub city: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub timezone: Option<String>,
}

// Only the fields finstats uses, and every one optional: vendors differ in what they fill in.
#[derive(Deserialize, Default)]
struct Named {
    #[serde(default)]
    names: BTreeMap<String, String>,
    #[serde(default)]
    iso_code: Option<String>,
}

#[derive(Deserialize, Default)]
struct Coordinates {
    #[serde(default)]
    latitude: Option<f64>,
    #[serde(default)]
    longitude: Option<f64>,
    #[serde(default)]
    time_zone: Option<String>,
}

#[derive(Deserialize, Default)]
struct Record {
    #[serde(default)]
    city: Named,
    #[serde(default)]
    country: Named,
    #[serde(default)]
    location: Coordinates,
    #[serde(default)]
    subdivisions: Vec<Named>,
}

fn english(n: &Named) -> Option<String> {
    n.names.get("en").or_else(|| n.names.values().next()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

pub struct Database {
    reader: Reader<maxminddb::Mmap>,
    pub path: PathBuf,
    /// What the file says it is, e.g. "DBIP-City-Lite" or "GeoLite2-City".
    pub kind: String,
    pub built_at: i64,
    stamp: Option<(u64, std::time::SystemTime)>,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        // Memory-mapped: a city database is over 100 MB and only the pages a lookup touches get read.
        // Safety: finstats never writes to an open file; a replacement is written next to it and renamed.
        let reader = unsafe { Reader::open_mmap(path) }.with_context(|| format!("opening {}", path.display()))?;
        let meta = reader.metadata();
        let kind = meta.database_type.clone();
        if !kind.to_ascii_lowercase().contains("city") {
            bail!("{} is a `{kind}` database; finstats needs a city database", path.display());
        }
        let built_at = meta.build_epoch as i64;
        Ok(Database { reader, path: path.to_path_buf(), kind, built_at, stamp: stamp(path) })
    }

    /// `None` for anything that has no place: private ranges, garbage, addresses the file does not know.
    pub fn lookup(&self, ip: &str) -> Option<Place> {
        let addr: IpAddr = crate::network::canonical(ip)?.parse().ok()?;
        if is_local_ip(ip) != Some(false) {
            return None;
        }
        let rec: Record = self.reader.lookup(addr).ok()?.decode().ok()??;
        let place = Place {
            country_code: rec.country.iso_code.as_deref().map(str::to_ascii_uppercase).filter(|c| c.len() == 2),
            country: english(&rec.country),
            region: rec.subdivisions.first().and_then(english),
            city: english(&rec.city),
            latitude: rec.location.latitude,
            longitude: rec.location.longitude,
            timezone: rec.location.time_zone,
        };
        (place.country_code.is_some() || place.latitude.is_some()).then_some(place)
    }

    pub fn is_dbip(&self) -> bool {
        self.kind.to_ascii_lowercase().contains("dbip")
    }
}

/// The open database, swapped as a whole when a newer file arrives.
#[derive(Default)]
pub struct Geo(RwLock<Option<Arc<Database>>>);

impl Geo {
    pub fn get(&self) -> Option<Arc<Database>> {
        self.0.read().unwrap().clone()
    }

    fn set(&self, db: Option<Database>) {
        *self.0.write().unwrap() = db.map(Arc::new);
    }
}

pub fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join("geoip")
}

/// The file to use: the one named in `FINSTATS_GEOIP_DB`, else the newest `.mmdb` in the folder.
pub fn find(data_dir: &Path) -> Option<PathBuf> {
    if let Some(own) = std::env::var("FINSTATS_GEOIP_DB").ok().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(own));
    }
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(dir(data_dir))
        .ok()?
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("mmdb")))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort();
    files.pop().map(|(_, p)| p)
}

/// Open whatever `find` finds. True when the database in use changed (a new file, or none any more).
pub fn load(app: &App) -> bool {
    let found = find(&app.data_dir);
    let current = app.geo.get();
    let same = match (&found, &current) {
        (Some(p), Some(db)) => *p == db.path && stamp(p) == db.stamp,
        (None, None) => true,
        _ => false,
    };
    if same {
        return false;
    }
    match found {
        None => app.geo.set(None),
        Some(path) => match Database::open(&path) {
            Ok(db) => {
                tracing::info!("geolocation database: {} ({}, built {})", path.display(), db.kind, chrono::DateTime::from_timestamp(db.built_at, 0).map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default());
                app.geo.set(Some(db));
            }
            Err(e) => {
                tracing::warn!("geolocation database unusable: {e:#}");
                app.geo.set(None);
            }
        },
    }
    true
}

/// Length and modification time: enough to notice a file replaced under the same name.
fn stamp(path: &Path) -> Option<(u64, std::time::SystemTime)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.len(), m.modified().ok()?))
}

/// Great-circle distance in kilometres.
pub fn distance_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let (dp, dl) = ((lat2 - lat1).to_radians(), (lon2 - lon1).to_radians());
    let a = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_KM * a.sqrt().min(1.0).asin()
}

// ---------------------------------------------------------------- download

fn month_name(year: i32, month: u32) -> String {
    format!("{year:04}-{month:02}")
}

/// This month and the one before: the new file appears a few days into the month.
fn candidate_months(today: chrono::NaiveDate) -> [String; 2] {
    let (y, m) = (today.year(), today.month());
    let prev = if m == 1 { (y - 1, 12) } else { (y, m - 1) };
    [month_name(y, m), month_name(prev.0, prev.1)]
}

pub fn download_url(month: &str) -> String {
    DBIP_URL.replace("{}", month)
}

/// Fetch DB-IP's current city file into the geoip folder and switch to it. The task registry shows progress.
pub async fn download(app: &App) -> Result<String> {
    const ID: &str = "geoip";
    let folder = dir(&app.data_dir);
    tokio::fs::create_dir_all(&folder).await?;
    let mut last_err = anyhow!("no month tried");
    for month in candidate_months(chrono::Utc::now().date_naive()) {
        let target = folder.join(format!("dbip-city-lite-{month}.mmdb"));
        if target.is_file() {
            load(app);
            return Ok(format!("Already have {month}"));
        }
        match fetch(app, &download_url(&month), &folder, &target, ID).await {
            Ok(()) => {
                // Older downloads of ours go; files the owner put there stay.
                if let Ok(entries) = std::fs::read_dir(&folder) {
                    for e in entries.flatten() {
                        let name = e.file_name().to_string_lossy().into_owned();
                        if name.starts_with("dbip-city-lite-") && name.ends_with(".mmdb") && e.path() != target {
                            let _ = std::fs::remove_file(e.path());
                        }
                    }
                }
                load(app);
                return Ok(format!("Downloaded DB-IP City Lite {month}"));
            }
            Err(e) => {
                tracing::debug!("geolocation database for {month}: {e:#}");
                last_err = e;
            }
        }
    }
    Err(last_err.context("downloading the geolocation database"))
}

async fn fetch(app: &App, url: &str, folder: &Path, target: &Path, task: &'static str) -> Result<()> {
    app.tasks.update(task, "Downloading", Some(0.0));
    // Says nothing about this install: a fixed agent string, no version, no identifiers.
    let mut resp = app.http.get(url).header(reqwest::header::USER_AGENT, "finstats").header(reqwest::header::ACCEPT, "*/*").timeout(Duration::from_secs(1800)).send().await?;
    if !resp.status().is_success() {
        bail!("{url} answered {}", resp.status());
    }
    let total = resp.content_length().unwrap_or(0);
    let packed = folder.join("download.mmdb.gz.tmp");
    let unpacked = folder.join("download.mmdb.tmp");
    let result = async {
        let mut file = tokio::fs::File::create(&packed).await?;
                let mut got = 0u64;
        while let Some(chunk) = resp.chunk().await? {
            got += chunk.len() as u64;
            file.write_all(&chunk).await?;
            if total > 0 {
                app.tasks.update(task, format!("Downloading ({:.0} of {:.0} MB)", got as f64 / 1e6, total as f64 / 1e6), Some(got as f64 / total as f64 * 0.9));
            }
        }
        file.flush().await?;
        drop(file);
        app.tasks.update(task, "Unpacking", Some(0.92));
        let (from, to) = (packed.clone(), unpacked.clone());
        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut gz = flate2::read::GzDecoder::new(std::io::BufReader::new(std::fs::File::open(&from)?));
            let mut out = std::io::BufWriter::new(std::fs::File::create(&to)?);
            std::io::copy(&mut gz, &mut out)?;
            // Only a file that opens as a city database may replace the one in use.
            Database::open(&to).map(|_| ())
        })
        .await??;
        tokio::fs::rename(&unpacked, target).await?;
        Ok(())
    }
    .await;
    let _ = tokio::fs::remove_file(&packed).await;
    let _ = tokio::fs::remove_file(&unpacked).await;
    result
}

/// Start the download unless one is running. Returns false in that case.
pub fn spawn_download(app: &App) -> bool {
    if !app.tasks.try_start("geoip", "Starting…") {
        return false;
    }
    let app = app.clone();
    tokio::spawn(async move {
        let outcome = download(&app).await;
        if outcome.is_ok() {
            crate::security::refresh_all(&app).await;
        }
        app.tasks.finish("geoip", outcome.map(|m| (m, None)));
    });
    true
}

/// With the other small, regular jobs: pick up a file the owner dropped in, and, when allowed, fetch a
/// newer month once the downloaded one is old.
pub async fn refresh(app: &App) {
    if load(app) {
        crate::security::refresh_all(app).await;
    }
    if !app.settings().geoip_download || std::env::var("FINSTATS_GEOIP_DB").is_ok_and(|p| !p.trim().is_empty()) {
        return;
    }
    let stale = match app.geo.get() {
        None => true,
        Some(db) => db.is_dbip() && crate::db::now() - db.built_at > REFRESH_AFTER_S && !candidate_months(chrono::Utc::now().date_naive()).iter().any(|m| db.path.to_string_lossy().contains(m.as_str())),
    };
    if stale {
        spawn_download(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distances_are_great_circle_kilometres() {
        // London to Paris is about 344 km, London to New York about 5570 km.
        assert!((distance_km(51.5074, -0.1278, 48.8566, 2.3522) - 344.0).abs() < 5.0);
        assert!((distance_km(51.5074, -0.1278, 40.7128, -74.0060) - 5570.0).abs() < 20.0);
        assert_eq!(distance_km(10.0, 20.0, 10.0, 20.0), 0.0);
        // Across the date line the short way round counts.
        assert!(distance_km(0.0, 179.5, 0.0, -179.5) < 120.0);
    }

    #[test]
    fn the_download_tries_this_month_then_the_last() {
        let d = |y, m, day| chrono::NaiveDate::from_ymd_opt(y, m, day).unwrap();
        assert_eq!(candidate_months(d(2026, 9, 20)), ["2026-09".to_string(), "2026-08".to_string()]);
        assert_eq!(candidate_months(d(2027, 1, 2)), ["2027-01".to_string(), "2026-12".to_string()]);
        assert_eq!(download_url("2026-09"), "https://download.db-ip.com/free/dbip-city-lite-2026-09.mmdb.gz");
    }
}
