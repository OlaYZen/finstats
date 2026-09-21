//! Jellyfin's `/socket`: the sessions the collector used to ask for, pushed as they change.
//!
//! This is a *transport*, not a second producer. What arrives here is the very list `GET /Sessions`
//! answers; it is handed to `collector::tick()` untouched, so one piece of code still writes every
//! play. Three things make a socket different from a poll, and each has an answer here:
//!
//!   * **Casing.** Every HTTP read asks for `profile="PascalCase"` (see `jellyfin.rs`), and a
//!     WebSocket handshake cannot: it is one GET whose `Accept` the server ignores. A server that
//!     answers camelCase would make `record_from_session` return `None` for every session — no plays
//!     at all, silently. So keys are put back into PascalCase here, and the first snapshot is held
//!     against one real `/Sessions` read before a single row is written from it.
//!   * **No `ActiveWithinSeconds`.** The push is the server's whole session list, so a client that
//!     vanished without saying stop lingers with something "playing". The collector's reconcile poll
//!     is what ends it; that is why the poll exists at all.
//!   * **Silence.** A socket can stop delivering without closing, which a poll cannot. Silence for
//!     `SILENT_MAX` is therefore death: the task says so and the collector goes back to polling.
//!
//! A play that is never seen cannot be reconstructed, so every doubt here resolves towards polling.

use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use crate::jellyfin::Jellyfin;

/// What finstats asks for: start pushing now, and at least every 1.5 s. Jellyfin also pushes on
/// every playback event, so this is a floor, not the rate.
const SESSIONS_START: &str = r#"{"MessageType":"SessionsStart","Data":"0,1500"}"#;
const KEEP_ALIVE: &str = r#"{"MessageType":"KeepAlive"}"#;
/// No session list for this long means the socket is no longer telling us anything.
const SILENT_MAX: Duration = Duration::from_secs(15);
/// A server that never answers `SessionsStart` does not speak this protocol (an older one, or
/// something in front of it eating the messages). Stop asking quickly.
const SUBSCRIBE_MAX: Duration = Duration::from_secs(10);
const HANDSHAKE_MAX: Duration = Duration::from_secs(10);
/// A snapshot is a whole picture, so an old one is worthless; a few in hand is plenty.
const QUEUE: usize = 8;
/// One snapshot of 50 sessions is a few hundred kB. Anything near this is not a session list.
const MAX_MESSAGE: usize = 8 << 20;

/// What the socket task tells the collector. One channel for both, so "it died" can never be
/// observed before the last snapshot that arrived before it.
#[derive(Debug)]
pub enum Event {
    Snapshot(Vec<Value>),
    Down(String),
}

/// A running socket task. Dropping it stops the task — switching the setting off is one assignment.
pub struct Handle {
    rx: mpsc::Receiver<Event>,
    task: JoinHandle<()>,
}

impl Handle {
    pub async fn recv(&mut self) -> Option<Event> {
        self.rx.recv().await
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// One message from Jellyfin, as far as finstats cares.
#[derive(Debug, PartialEq)]
pub enum Msg {
    /// The whole session list, already in PascalCase.
    Sessions(Vec<Value>),
    /// "Send me a KeepAlive at least this often", in seconds.
    ForceKeepAlive(u64),
    /// Anything else, including the server's own KeepAlive: not our business, never fatal.
    Other,
    /// A `Sessions` message whose payload is not a session list. A picture we cannot read is not
    /// a picture: the socket goes down rather than the collector believing an empty list.
    Unreadable,
}

/// `MessageType` and `Data` are themselves sent in either casing, so both are looked for.
fn field<'a>(v: &'a Value, pascal: &str, camel: &str) -> &'a Value {
    match v.get(pascal) {
        Some(x) => x,
        None => v.get(camel).unwrap_or(&Value::Null),
    }
}

pub fn parse(text: &str) -> Msg {
    let Ok(v) = serde_json::from_str::<Value>(text) else { return Msg::Unreadable };
    let kind = field(&v, "MessageType", "messageType").as_str().unwrap_or_default().to_string();
    let data = field(&v, "Data", "data");
    match kind.as_str() {
        "Sessions" => match data.as_array() {
            Some(list) => Msg::Sessions(list.iter().map(pascal_keys).collect()),
            None => Msg::Unreadable,
        },
        // Sent as a number by every server seen so far, as a string by the odd one.
        "ForceKeepAlive" => Msg::ForceKeepAlive(data.as_u64().or_else(|| data.as_str()?.parse().ok()).unwrap_or(60)),
        _ => Msg::Other,
    }
}

/// Jellyfin answers HTTP in PascalCase because every read asks for it; the socket cannot ask.
/// `nowPlayingItem` → `NowPlayingItem`, all the way down. A key that is already PascalCase is
/// untouched, so a server that speaks it pays nothing but a walk of the tree.
fn pascal_keys(v: &Value) -> Value {
    match v {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, val)| {
                    let mut c = k.chars();
                    let key = match c.next() {
                        Some(first) if first.is_lowercase() => first.to_uppercase().collect::<String>() + c.as_str(),
                        _ => k.clone(),
                    };
                    (key, pascal_keys(val))
                })
                .collect(),
        ),
        Value::Array(list) => Value::Array(list.iter().map(pascal_keys).collect()),
        other => other.clone(),
    }
}

/// Is what the socket sends the same kind of thing `GET /Sessions` sends? Compared by the sessions
/// that are playing something and by the fields the collector actually reads, because a snapshot
/// missing `MediaStreams` or `TranscodingInfo` would write a timeline full of invented track changes.
pub fn looks_like_sessions(pushed: &[Value], polled: &[Value]) -> bool {
    let playing = |list: &[Value]| -> Vec<String> {
        let mut ids: Vec<String> = list
            .iter()
            .filter(|s| !s["NowPlayingItem"]["Id"].is_null())
            .filter_map(|s| s["Id"].as_str().map(str::to_string))
            .collect();
        ids.sort();
        ids
    };
    let (a, b) = (playing(pushed), playing(polled));
    // A play can start or stop between the two reads; only what both saw is evidence.
    let shared: Vec<&String> = a.iter().filter(|id| b.contains(id)).collect();
    if shared.is_empty() {
        // Nothing was playing in either, so there is nothing to compare: the shapes agree by default.
        return a.is_empty() && b.is_empty() || a.is_empty() || b.is_empty();
    }
    let by_id = |list: &[Value], id: &str| list.iter().find(|s| s["Id"].as_str() == Some(id)).cloned().unwrap_or(Value::Null);
    shared.iter().all(|id| {
        let (p, q) = (by_id(pushed, id), by_id(polled, id));
        let same = |path: &dyn Fn(&Value) -> bool| path(&p) == path(&q);
        same(&|s: &Value| !s["NowPlayingItem"]["Name"].is_null())
            && same(&|s: &Value| !s["NowPlayingItem"]["Type"].is_null())
            && same(&|s: &Value| !s["NowPlayingItem"]["MediaStreams"].is_null())
            && same(&|s: &Value| !s["PlayState"]["PositionTicks"].is_null())
            && same(&|s: &Value| !s["UserId"].is_null())
    })
}

/// Half of what the server asked for, and never so rare that it closes on us or so often that it
/// becomes chatter of its own.
pub fn keepalive_period(force: u64) -> Duration {
    Duration::from_secs((force / 2).clamp(5, 30))
}

/// 1, 2, 4 … 60 seconds. A server that is down, restarting or does not speak this is not hammered.
pub fn backoff(attempt: u32) -> Duration {
    Duration::from_secs(2u64.saturating_pow(attempt.min(6)).min(60))
}

/// Run the socket until it fails, then say why, wait and try again. Ends only when the handle is dropped.
pub fn spawn(jf: Jellyfin) -> Handle {
    let (tx, rx) = mpsc::channel(QUEUE);
    let task = tokio::spawn(async move {
        let mut attempt = 0u32;
        loop {
            let started = Instant::now();
            let why = match serve(&jf, &tx).await {
                Ok(reason) => reason,
                Err(e) => format!("{e:#}"),
            };
            // A socket that lived a while was healthy; the next hiccup starts counting from scratch.
            attempt = if started.elapsed() > Duration::from_secs(60) { 0 } else { attempt.saturating_add(1) };
            if tx.send(Event::Down(why)).await.is_err() {
                return; // the collector is gone
            }
            tokio::time::sleep(backoff(attempt)).await;
        }
    });
    Handle { rx, task }
}

/// One connection, from the handshake to whatever ends it. `Ok(reason)` is an ordinary end.
async fn serve(jf: &Jellyfin, tx: &mpsc::Sender<Event>) -> Result<String> {
    let (url, auth) = jf.socket_handshake()?;
    let config = WebSocketConfig::default().max_message_size(Some(MAX_MESSAGE));

    let mut req = url.clone().into_client_request().context("building the socket request")?;
    req.headers_mut().insert("Authorization", auth.parse().context("the Jellyfin key is not a header value")?);
    let connect = tokio_tungstenite::connect_async_with_config(req, Some(config), false);
    let mut ws = match tokio::time::timeout(HANDSHAKE_MAX, connect).await {
        Err(_) => bail!("{url} did not answer the handshake within {}s", HANDSHAKE_MAX.as_secs()),
        Ok(Ok((ws, _))) => ws,
        // Some setups refuse an Authorization header on an upgrade; the key may go in the query instead.
        // The URL carrying it is built here and dropped here: it is never logged and never stored.
        Ok(Err(first)) => {
            let with_key = jf.socket_url_with_key()?.into_client_request().context("building the socket request")?;
            match tokio::time::timeout(HANDSHAKE_MAX, tokio_tungstenite::connect_async_with_config(with_key, Some(config), false)).await {
                Ok(Ok((ws, _))) => ws,
                _ => bail!("{url} refused the connection ({first})"),
            }
        }
    };

    ws.send(Message::Text(SESSIONS_START.into())).await.context("asking Jellyfin to send sessions")?;

    let mut keepalive = tokio::time::interval(keepalive_period(60));
    keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut watchdog = tokio::time::interval(Duration::from_secs(1));
    let opened = Instant::now();
    let mut last_sessions: Option<Instant> = None;
    let mut checked = false;

    loop {
        tokio::select! {
            frame = ws.next() => match frame {
                None => return Ok("Jellyfin closed the connection".into()),
                Some(Err(e)) => return Ok(format!("the connection failed ({e})")),
                Some(Ok(Message::Text(text))) => match parse(text.as_str()) {
                    Msg::Sessions(list) => {
                        // Before a single row comes from the socket: does it say what /Sessions says?
                        if !checked {
                            checked = true;
                            let polled = jf.sessions().await.context("comparing the first pushed sessions with /Sessions")?;
                            if !looks_like_sessions(&list, &polled) {
                                return Ok("what Jellyfin pushes does not match what it answers for /Sessions".into());
                            }
                        }
                        last_sessions = Some(Instant::now());
                        // A newer picture is on its way in a moment; dropping one is better than waiting.
                        if let Err(mpsc::error::TrySendError::Closed(_)) = tx.try_send(Event::Snapshot(list)) {
                            return Ok("the collector stopped listening".into());
                        }
                    }
                    Msg::ForceKeepAlive(secs) => {
                        let period = keepalive_period(secs);
                        keepalive = tokio::time::interval(period);
                        keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                    }
                    Msg::Unreadable => return Ok("Jellyfin sent a session list that could not be read".into()),
                    Msg::Other => {}
                },
                Some(Ok(Message::Close(_))) => return Ok("Jellyfin closed the connection".into()),
                Some(Ok(_)) => {}   // ping/pong are answered by the library, binary frames are not ours
            },
            _ = keepalive.tick() => {
                if let Err(e) = ws.send(Message::Text(KEEP_ALIVE.into())).await {
                    return Ok(format!("the connection failed ({e})"));
                }
            }
            _ = watchdog.tick() => match last_sessions {
                None if opened.elapsed() > SUBSCRIBE_MAX => return Ok("this Jellyfin does not push what is playing".into()),
                Some(seen) if seen.elapsed() > SILENT_MAX => return Ok(format!("nothing arrived for {}s", SILENT_MAX.as_secs())),
                _ => {}
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_session_list_is_read_in_either_casing() {
        let pascal = r#"{"MessageType":"Sessions","Data":[{"Id":"a","UserId":"u1"}]}"#;
        let camel = r#"{"messageType":"Sessions","data":[{"id":"a","userId":"u1","nowPlayingItem":{"id":"i","runTimeTicks":10}}]}"#;
        assert_eq!(parse(pascal), Msg::Sessions(vec![json!({"Id": "a", "UserId": "u1"})]));
        assert_eq!(
            parse(camel),
            Msg::Sessions(vec![json!({"Id": "a", "UserId": "u1", "NowPlayingItem": {"Id": "i", "RunTimeTicks": 10}})]),
            "a camelCase server is put back into the one casing the rest of finstats reads"
        );
    }

    #[test]
    fn a_keepalive_request_is_understood_as_a_number_or_a_string() {
        assert_eq!(parse(r#"{"MessageType":"ForceKeepAlive","Data":60}"#), Msg::ForceKeepAlive(60));
        assert_eq!(parse(r#"{"MessageType":"ForceKeepAlive","Data":"60"}"#), Msg::ForceKeepAlive(60));
        assert_eq!(keepalive_period(60), Duration::from_secs(30));
        assert_eq!(keepalive_period(4), Duration::from_secs(5), "never so rare that the server hangs up");
        assert_eq!(keepalive_period(600), Duration::from_secs(30), "and never chatter");
    }

    #[test]
    fn anything_else_is_ignored_but_an_unreadable_session_list_is_not() {
        assert_eq!(parse(r#"{"MessageType":"KeepAlive"}"#), Msg::Other);
        assert_eq!(parse(r#"{"MessageType":"UserDataChanged","Data":{}}"#), Msg::Other);
        assert_eq!(parse("not json at all"), Msg::Unreadable);
        assert_eq!(parse(r#"{"MessageType":"Sessions","Data":{"oops":1}}"#), Msg::Unreadable, "an empty list would look like everybody stopped");
    }

    #[test]
    fn the_first_snapshot_is_held_against_what_sessions_answers() {
        let full = json!([{ "Id": "s1", "UserId": "u1", "NowPlayingItem": {"Id": "i1", "Name": "Big Buck Bunny", "Type": "Movie", "MediaStreams": []}, "PlayState": {"PositionTicks": 10} }]);
        let thin = json!([{ "Id": "s1", "UserId": "u1", "NowPlayingItem": {"Id": "i1", "Name": "Big Buck Bunny", "Type": "Movie"}, "PlayState": {"PositionTicks": 10} }]);
        let (full, thin) = (full.as_array().unwrap().clone(), thin.as_array().unwrap().clone());
        assert!(looks_like_sessions(&full, &full));
        assert!(!looks_like_sessions(&thin, &full), "a snapshot without the streams would invent track changes every minute");
        assert!(looks_like_sessions(&[], &[]), "nothing playing anywhere is no evidence against the socket");
        assert!(looks_like_sessions(&full, &[]), "a play that started between the two reads is not a mismatch");
    }

    #[test]
    fn backing_off_is_bounded() {
        assert_eq!(backoff(0), Duration::from_secs(1));
        assert_eq!(backoff(3), Duration::from_secs(8));
        assert_eq!(backoff(20), Duration::from_secs(60), "a server that does not speak this is not hammered");
        assert!((0..30).all(|a| backoff(a) <= backoff(a + 1)));
    }
}
