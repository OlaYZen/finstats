//! Which addresses count as "at home".
//!
//! A private address is local, obviously. But so is the server's own public address: a phone on the
//! home Wi-Fi that reaches Jellyfin through its public name (a reverse proxy, NAT loopback) shows up
//! with the household's public IP, and calling that "remote" is wrong. finstats therefore learns the
//! public address of the network it runs on, remembers every one it has seen (they change), and lets
//! the owner add more by hand.

use std::net::IpAddr;
use std::time::Duration;

use anyhow::Result;
use serde_json::Value;

use crate::db::rusqlite::{Connection, params};
use crate::db::{self, is_local_ip};
use crate::state::App;
use crate::stats::rows_json;

/// Plain-text "what is my IP" services, tried in order. Each answers with the address and nothing else.
/// The request says nothing about this install: no version, no identifiers, no Jellyfin details.
///
/// Several, from different operators, because ad-blocking DNS (Pi-hole, AdGuard) commonly sinkholes this
/// kind of service. Cloudflare's trace is also asked by bare IP, which no DNS blocklist can touch.
const LOOKUPS: [&str; 5] = [
    "https://checkip.amazonaws.com",
    "https://www.cloudflare.com/cdn-cgi/trace",
    "https://1.1.1.1/cdn-cgi/trace",
    "https://api.ipify.org",
    "https://icanhazip.com",
];

/// One spelling per address, so `::ffff:203.0.113.7` and `203.0.113.7` are the same home.
pub fn canonical(ip: &str) -> Option<String> {
    Some(match ip.trim().parse::<IpAddr>().ok()? {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(|v4| v4.to_string()).unwrap_or_else(|| v6.to_string()),
        v4 => v4.to_string(),
    })
}

/// The body of a lookup answer, if it is a public address. Anything else is not worth remembering:
/// a captive portal's HTML, an error page, or a private address that is local already.
pub fn parse_public(body: &str) -> Option<String> {
    // Either the bare address, or Cloudflare's trace: `key=value` lines, one of them `ip=`.
    let text = body.lines().find_map(|l| l.trim().strip_prefix("ip=")).unwrap_or(body);
    let ip = canonical(text)?;
    (is_local_ip(&ip) == Some(false)).then_some(ip)
}

/// Local (private range or a known home address), remote, or unknown for something that is no address.
pub fn classify(conn: &Connection, ip: &str) -> Result<Option<bool>> {
    let Some(local) = is_local_ip(ip) else { return Ok(None) };
    if local {
        return Ok(Some(true));
    }
    let Some(ip) = canonical(ip) else { return Ok(None) };
    Ok(Some(conn.prepare_cached("SELECT 1 FROM home_addresses WHERE ip = ?1")?.exists([ip])?))
}

/// Decide every play's network again. Cheap: one pass over the distinct addresses.
pub fn reclassify(conn: &Connection) -> Result<usize> {
    let ips: Vec<String> = conn
        .prepare("SELECT DISTINCT remote_ip FROM playbacks WHERE remote_ip IS NOT NULL")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let mut changed = 0;
    for ip in ips {
        let local = classify(conn, &ip)?;
        changed += conn.execute("UPDATE playbacks SET is_local = ?1 WHERE remote_ip = ?2 AND is_local IS NOT ?1", params![local, ip])?;
    }
    Ok(changed)
}

/// Note that `ip` was seen as this network's public address. True when it is a new one.
pub fn remember(conn: &Connection, ip: &str) -> Result<bool> {
    let now = db::now();
    let known: bool = conn.prepare_cached("SELECT 1 FROM home_addresses WHERE ip = ?1")?.exists([ip])?;
    conn.execute(
        "INSERT INTO home_addresses(ip, source, first_seen, last_seen) VALUES (?1, 'lookup', ?2, ?2)
         ON CONFLICT(ip) DO UPDATE SET last_seen = excluded.last_seen",
        params![ip, now],
    )?;
    Ok(!known)
}

/// Make the hand-written list in the settings the set of `manual` rows. Looked-up addresses stay.
pub fn set_manual(conn: &Connection, list: &[String]) -> Result<()> {
    let now = db::now();
    let wanted: Vec<String> = list.iter().filter_map(|s| canonical(s)).collect();
    conn.execute("DELETE FROM home_addresses WHERE source = 'manual'", [])?;
    for ip in wanted {
        conn.execute("INSERT OR IGNORE INTO home_addresses(ip, source, first_seen, last_seen) VALUES (?1, 'manual', ?2, ?2)", params![ip, now])?;
    }
    Ok(())
}

pub fn list(conn: &Connection) -> Result<Vec<Value>> {
    Ok(rows_json(conn, "SELECT ip, source, first_seen, last_seen FROM home_addresses ORDER BY last_seen DESC, ip", &[])?.into_iter().map(Value::Object).collect())
}

/// The services to ask: the built-in list, or the single one named in `FINSTATS_PUBLIC_IP_URL`
/// (any URL that answers with the caller's address as plain text).
pub fn services() -> Vec<String> {
    match std::env::var("FINSTATS_PUBLIC_IP_URL").ok().map(|u| u.trim().to_string()).filter(|u| !u.is_empty()) {
        Some(own) => vec![own],
        None => LOOKUPS.iter().map(|u| u.to_string()).collect(),
    }
}

async fn lookup(http: &reqwest::Client) -> Option<String> {
    for url in services() {
        let Ok(resp) = http.get(url).header(reqwest::header::USER_AGENT, "finstats").header(reqwest::header::ACCEPT, "text/plain").timeout(Duration::from_secs(8)).send().await else { continue };
        if !resp.status().is_success() {
            continue;
        }
        if let Some(ip) = resp.text().await.ok().as_deref().and_then(parse_public) {
            return Some(ip);
        }
    }
    None
}

/// Has a lookup ever produced an address on this install? Then there is nothing to ask about.
pub fn ever_looked_up(conn: &Connection) -> Result<bool> {
    Ok(conn.prepare_cached("SELECT 1 FROM home_addresses WHERE source = 'lookup' LIMIT 1")?.exists([])?)
}

/// At start-up, and only on an install that has never learned an address. A household's public
/// address is not news that needs re-checking: one question, answered, and finstats stops asking.
/// When it does change, the owner presses "Look up now" or types the new one in.
pub async fn refresh_if_unknown(app: &App) {
    if !app.settings().public_ip_lookup {
        return;
    }
    match app.db.call(|c| ever_looked_up(c)).await {
        Ok(true) => return,
        Ok(false) => {}
        Err(e) => {
            tracing::warn!("could not tell whether this network's address is known: {e:#}");
            return;
        }
    }
    refresh(app).await;
}

/// One lookup: at start-up on a fresh install, or because somebody asked for it. Never on a timer.
pub async fn refresh(app: &App) {
    if !app.settings().public_ip_lookup {
        return;
    }
    static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    let Some(ip) = lookup(&app.http).await else {
        // Said once: usually a DNS blocklist, and then it stays that way.
        if !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            tracing::warn!("could not look up this network's public address (no service answered; a DNS blocklist?). Plays from it will count as remote until it is added under Settings → Home network");
        }
        return;
    };
    let result = app
        .db
        .call(move |c| {
            let new = remember(c, &ip)?;
            Ok(if new { Some(reclassify(c)?) } else { None })
        })
        .await;
    match result {
        Ok(Some(changed)) => tracing::info!("learned a new public address for this network; {changed} plays now count as local"),
        Ok(None) => {}
        Err(e) => tracing::warn!("remembering the public address failed: {e:#}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE home_addresses(ip TEXT PRIMARY KEY, source TEXT NOT NULL, first_seen INTEGER NOT NULL, last_seen INTEGER NOT NULL);
             CREATE TABLE playbacks(id INTEGER PRIMARY KEY, remote_ip TEXT, is_local INTEGER);
             INSERT INTO playbacks(remote_ip, is_local) VALUES ('192.168.1.10', NULL), ('203.0.113.7', 0), ('::ffff:203.0.113.7', 0), ('198.51.100.9', 1), (NULL, NULL), ('not an ip', NULL);",
        )
        .unwrap();
        c
    }

    #[test]
    fn only_a_public_address_is_worth_remembering() {
        assert_eq!(parse_public("203.0.113.7\n").as_deref(), Some("203.0.113.7"));
        assert_eq!(parse_public("::ffff:203.0.113.7").as_deref(), Some("203.0.113.7"));
        assert_eq!(parse_public("fl=1f2\nh=1.1.1.1\nip=203.0.113.7\nts=1790000000.1\nvisit_scheme=https\n").as_deref(), Some("203.0.113.7"));
        assert_eq!(parse_public("fl=1f2\nip=10.0.0.4\n"), None);
        assert_eq!(parse_public("192.168.1.10"), None);
        assert_eq!(parse_public("<html>Sign in to the hotel Wi-Fi</html>"), None);
    }

    #[test]
    fn the_homes_public_address_is_local_and_forgetting_it_undoes_that() {
        let c = conn();
        assert!(remember(&c, "203.0.113.7").unwrap());
        assert!(!remember(&c, "203.0.113.7").unwrap());
        reclassify(&c).unwrap();
        let flags = |c: &Connection| -> Vec<Option<bool>> { c.prepare("SELECT is_local FROM playbacks ORDER BY id").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect() };
        // private, home (both spellings), a stranger that was wrongly marked local, no address, garbage
        assert_eq!(flags(&c), vec![Some(true), Some(true), Some(true), Some(false), None, None]);

        set_manual(&c, &["198.51.100.9".into(), "nonsense".into()]).unwrap();
        c.execute("DELETE FROM home_addresses WHERE source = 'lookup'", []).unwrap();
        reclassify(&c).unwrap();
        assert_eq!(flags(&c), vec![Some(true), Some(false), Some(false), Some(true), None, None]);
    }
}
