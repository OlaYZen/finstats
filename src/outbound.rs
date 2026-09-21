//! Everywhere finstats can reach, in one list.
//!
//! The README and `docs/security.md` promise that nothing about you leaves your network and that
//! finstats talks to your Jellyfin and — unless you say otherwise — to nothing else. A promise the
//! owner cannot check is only a sentence, so this is the same claim assembled from what the running
//! program actually knows: the collector's own connection, the two switches that can reach outside,
//! and each connection the owner entered under Settings.
//!
//! Nothing new is recorded for it. Every row is read from something that was already being kept —
//! the collector status, the addresses a lookup has produced, the geolocation file on disk, and each
//! service's last good read — so this page cannot itself be the reason finstats knows something.
//! Hosts only, never a key: `Service` does not even implement `Serialize`.

use serde_json::{Value, json};

use crate::db::rusqlite::Connection;
use crate::state::{App, CollectorStatus, Settings};

/// One destination, as the page shows it.
pub struct Dest {
    pub id: String,
    /// What it is, in the owner's words.
    pub what: String,
    /// Host and port, never a path, never a key.
    pub hosts: Vec<String>,
    /// Why finstats would talk to it at all.
    pub why: String,
    /// `always` (finstats is useless without it), `on`, or `off`.
    pub state: &'static str,
    /// When it last answered, as far as anything already recorded knows. `None` when never, or when
    /// nothing keeps that (an address looked up before this version, say).
    pub last_at: Option<i64>,
    /// What went wrong last, when something did.
    pub error: Option<String>,
}

impl Dest {
    fn json(&self) -> Value {
        json!({ "id": self.id, "what": self.what, "hosts": self.hosts, "why": self.why,
                "state": self.state, "last_at": self.last_at, "error": self.error })
    }
}

/// Only the host (and port) of a URL the owner typed: the path may carry a base, and a base is
/// nobody's business on a page about *where* traffic goes.
pub fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    rest.split(['/', '?', '#']).next().unwrap_or(rest).to_string()
}

/// One connection as this page needs it: id, kind, name, URL, switched on, last good read, last error.
pub type ServiceRow = (i64, &'static str, String, String, bool, Option<i64>, Option<String>);

const ON: &str = "on";
const OFF: &str = "off";

/// The list, built from what is already known. Pure, so the shape of the answer is testable and the
/// rule "a switch that is off means nothing is contacted" is a test rather than a hope.
#[allow(clippy::too_many_arguments)]
pub fn destinations(
    jellyfin_url: Option<&str>,
    collector: &CollectorStatus,
    settings: &Settings,
    lookup_services: &[String],
    last_lookup_at: Option<i64>,
    geoip_from_env: bool,
    geoip_built_at: Option<i64>,
    services: &[ServiceRow],
) -> Vec<Dest> {
    let mut out = vec![];

    out.push(match jellyfin_url {
        Some(url) => Dest {
            id: "jellyfin".into(),
            what: "Your Jellyfin server".into(),
            hosts: vec![host_of(url)],
            why: "everything finstats shows comes from here: what is playing, your library, your users".into(),
            state: "always",
            last_at: Some(collector.last_poll_at).filter(|t| *t > 0),
            error: collector.error.clone(),
        },
        None => Dest {
            id: "jellyfin".into(),
            what: "Your Jellyfin server".into(),
            hosts: vec![],
            why: "not connected yet".into(),
            state: OFF,
            last_at: None,
            error: None,
        },
    });

    out.push(Dest {
        id: "public_ip".into(),
        what: "A \"what is my IP\" service".into(),
        hosts: lookup_services.iter().map(|u| host_of(u)).collect(),
        why: "so that people watching at home through your public address are not counted as remote. Asked once, and after that only when you press the button".into(),
        state: if settings.public_ip_lookup { ON } else { OFF },
        last_at: last_lookup_at,
        error: None,
    });

    out.push(Dest {
        id: "geoip".into(),
        what: "DB-IP's free city database".into(),
        hosts: vec![host_of(&crate::geo::download_url("YYYY-MM"))],
        why: match geoip_from_env {
            true => "not used: the database is the file FINSTATS_GEOIP_DB names".into(),
            false => "the monthly download of the file that places addresses on the Security page. Looking an address up never leaves this machine".into(),
        },
        state: if settings.geoip_download && !geoip_from_env { ON } else { OFF },
        last_at: geoip_built_at,
        error: None,
    });

    for (id, kind, name, url, enabled, last_ok_at, last_error) in services {
        out.push(Dest {
            id: format!("service:{id}"),
            what: format!("{name} ({kind})"),
            hosts: vec![host_of(url)],
            why: "a connection you added under Settings → Connections. Read-only, and on your own network unless you pointed it elsewhere".into(),
            state: if *enabled { ON } else { OFF },
            last_at: *last_ok_at,
            error: last_error.clone(),
        });
    }

    out
}

/// `GET /api/outbound` — Jellyfin administrators only, like the connections it lists.
pub async fn outbound(
    axum::extract::State(app): axum::extract::State<App>,
    _admin: crate::auth::JellyfinAdmin,
) -> crate::state::ApiResult {
    let settings = app.settings();
    let collector = app.collector.read().unwrap().clone();
    let jellyfin_url = app.config.read().unwrap().as_ref().map(|c| c.url.clone());
    let geo_status = crate::security::status_json(&app);
    let geoip_from_env = geo_status["from_env"].as_bool().unwrap_or(false);
    let geoip_built_at = geo_status["database"]["built_at"].as_i64();

    let services: Vec<_> = app
        .services
        .read()
        .unwrap()
        .iter()
        .map(|s| {
            let health = app.service_health.read().unwrap().get(&s.id).cloned();
            (s.id, s.kind.label(), s.name.clone(), s.url.clone(), s.enabled, health.as_ref().and_then(|h| h.last_ok_at), health.and_then(|h| h.last_error))
        })
        .collect();

    let last_lookup_at = app.db.call(|c| last_lookup(c)).await?;
    let list = destinations(
        jellyfin_url.as_deref(),
        &collector,
        &settings,
        &crate::network::services(),
        last_lookup_at,
        geoip_from_env,
        geoip_built_at,
        &services,
    );
    let on = list.iter().filter(|d| d.state != OFF).count();
    Ok(axum::Json(json!({ "destinations": list.iter().map(Dest::json).collect::<Vec<_>>(), "reachable": on, "total": list.len() })))
}

/// When a lookup last produced an address. A lookup that answered nothing leaves no trace, which is
/// the honest thing for a page about what finstats knows to say.
fn last_lookup(conn: &Connection) -> anyhow::Result<Option<i64>> {
    Ok(conn.query_row("SELECT MAX(last_seen) FROM home_addresses WHERE source = 'lookup'", [], |r| r.get(0))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dest(list: &[Dest], id: &str) -> (&'static str, Vec<String>) {
        let d = list.iter().find(|d| d.id == id).expect("a row for every destination, always");
        (d.state, d.hosts.clone())
    }

    #[test]
    fn a_url_is_reduced_to_its_host() {
        assert_eq!(host_of("http://nas:7878"), "nas:7878");
        assert_eq!(host_of("https://media.example:8096/jellyfin"), "media.example:8096");
        assert_eq!(host_of("https://download.db-ip.com/free/dbip-city-lite-2026-09.mmdb.gz"), "download.db-ip.com");
    }

    #[test]
    fn every_switch_that_is_off_is_listed_as_off() {
        let settings = Settings { public_ip_lookup: false, geoip_download: false, ..Default::default() };
        let list = destinations(
            Some("http://jellyfin.example:8096"),
            &CollectorStatus::default(),
            &settings,
            &["https://checkip.example".to_string()],
            None,
            false,
            None,
            &[(3, "Radarr", "Radarr 4K".into(), "http://nas:7878/radarr".into(), false, None, None)],
        );
        assert_eq!(dest(&list, "jellyfin"), ("always", vec!["jellyfin.example:8096".to_string()]));
        assert_eq!(dest(&list, "public_ip").0, "off");
        assert_eq!(dest(&list, "geoip").0, "off");
        assert_eq!(dest(&list, "service:3"), ("off", vec!["nas:7878".to_string()]), "a connection is listed by host and port, never by base path or key");
        assert_eq!(list.iter().filter(|d| d.state != "off").count(), 1, "with both switches off, Jellyfin is the only thing finstats reaches");
    }

    #[test]
    fn a_database_from_the_environment_is_never_downloaded() {
        let settings = Settings { geoip_download: true, ..Default::default() };
        let list = destinations(None, &CollectorStatus::default(), &settings, &[], None, true, Some(10), &[]);
        assert_eq!(dest(&list, "geoip").0, "off", "the setting cannot switch on what FINSTATS_GEOIP_DB has taken over");
    }
}
