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
//!     vanished without saying stop lingers with something "playing". Nothing that is playing is ever
//!     judged by the push alone: the collector asks for the list itself for as long as anything is,
//!     and that is what ends such a play.
//!   * **Silence.** A socket can stop delivering without closing, which a poll cannot — but silence is
//!     also the *normal* state here: Jellyfin pushes a session list when something changes and sends
//!     nothing at all while nobody is watching, for hours. So what is watched is the server answering,
//!     not sessions arriving: finstats sends `KeepAlive` and Jellyfin answers, and nothing heard for
//!     `LOST_AFTER` is death. Judging a quiet socket by its sessions is what made 1.4.0 and 1.4.1 drop
//!     the connection every fifteen seconds on an idle server and never stop polling.
//!
//! A play that is never seen cannot be reconstructed, so every doubt here resolves towards polling.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use crate::jellyfin::Jellyfin;

/// Subscribe: send the list now, and look again every 1.5 s. Jellyfin only sends when that look finds
/// something different, which is why a server with nobody watching anything says nothing for hours —
/// and why one with something playing says something every 1.5 s, since a position never stops moving.
const SESSIONS_START: &str = r#"{"MessageType":"SessionsStart","Data":"0,1500"}"#;
/// Unsubscribe, and the reason this exists: the moment a play is being polled for, those pushes are a
/// second copy of what finstats is already asking for — the same list twice, and the pushed one is not
/// even compressed. The connection stays open and keeps answering `KeepAlive`, so it costs nothing and
/// is one message away from carrying again the moment the last play ends.
const SESSIONS_STOP: &str = r#"{"MessageType":"SessionsStop","Data":""}"#;
const KEEP_ALIVE: &str = r#"{"MessageType":"KeepAlive"}"#;
/// How long to wait for the list that proves the socket carries before saying so and letting the
/// collector poll meanwhile. **Not a verdict.** A Jellyfin normally answers `SessionsStart` at once
/// whether or not anything is loaded (measured: "0.0s after subscribing, 0 loaded"), but one was once
/// seen not to, and the cause was never established — a server still starting up, most likely. Nothing
/// is concluded from that silence: the connection is kept, stays subscribed, and the first real push
/// settles it. Assuming otherwise cost an earlier attempt the socket entirely.
const SUBSCRIBE_MAX: Duration = Duration::from_secs(5);
/// While still unproven, ask again this often, in case the subscription itself went missing.
const PROBE_EVERY: Duration = Duration::from_secs(300);
/// A connection that has not said one word in this long — no `ForceKeepAlive`, no answer to a
/// `KeepAlive` of ours, nothing — is not healthy, whatever it may be subscribed to.
const NOT_ANSWERING: Duration = Duration::from_secs(10);
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
    /// Subscribed, the server answering, and nothing pushed yet. A Jellyfin that has nothing to report
    /// sometimes says nothing at all rather than sending an empty list, so waiting for one is waiting
    /// for something that may never come: the connection is healthy, so the subscription is believed,
    /// and the collector reads the current state for itself once instead.
    Trusted,
    /// Jellyfin answered a `KeepAlive`. No news — that is the point: it says the picture the collector
    /// already has is still the current one, which is what "last checked" means on this transport.
    Alive,
    Down(String),
}

/// How one connection ended. The difference matters only in how soon to try again: a server that does
/// not answer a subscription is asked again in `PROBE_EVERY` rather than in a second or two, and a
/// verdict is never kept — "does not speak this" and "was restarting when we asked" look the same.
enum End {
    Unsupported(String),
    Broken(String),
}

/// A running socket task. Dropping it stops the task — one assignment, for a Jellyfin that changed.
pub struct Handle {
    rx: mpsc::Receiver<Event>,
    /// Whether session pushes are wanted right now. Survives a reconnection, so a socket that comes
    /// back in the middle of a play comes back quiet.
    want: watch::Sender<bool>,
    /// What is true *on the wire*, written by the task itself: a connection that is open, and a
    /// `SessionsStart` that has been sent on it and not yet followed by a `SessionsStop`. Asking for
    /// something is not the same as having sent it, and the status must report the second.
    wire: Arc<Wire>,
    task: JoinHandle<()>,
}

#[derive(Default)]
pub struct Wire {
    connected: AtomicBool,
    subscribed: AtomicBool,
}

impl Wire {
    fn set(&self, connected: bool, subscribed: bool) {
        self.connected.store(connected, Ordering::Relaxed);
        self.subscribed.store(subscribed, Ordering::Relaxed);
    }
}

impl Handle {
    pub async fn recv(&mut self) -> Option<Event> {
        self.rx.recv().await
    }

    /// Is the connection open right now?
    pub fn connected(&self) -> bool {
        self.wire.connected.load(Ordering::Relaxed)
    }

    /// Has a `SessionsStart` been sent on *this* connection, with no `SessionsStop` after it?
    pub fn subscribed(&self) -> bool {
        self.wire.subscribed.load(Ordering::Relaxed)
    }

    /// Wait for the wire to match what was last asked for, so a change of transport is never published
    /// before it has happened. Milliseconds in practice — the task is awake and a frame is one write.
    /// `false` if it did not land in time, which the caller treats as the socket being unusable.
    pub async fn settled(&self, on: bool, within: Duration) -> bool {
        let deadline = Instant::now() + within;
        while Instant::now() < deadline {
            if self.subscribed() == on {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        self.subscribed() == on
    }

    /// Ask Jellyfin to push session lists, or to stop. Saying the same thing twice sends nothing.
    pub fn listen(&self, on: bool) {
        self.want.send_if_modified(|current| {
            let changed = *current != on;
            *current = on;
            changed
        });
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
    /// The server's answer to ours. Nothing to do with sessions, and the only thing a quiet server
    /// says for hours: it is what tells a silent socket from a dead one.
    KeepAlive,
    /// Anything else: not our business, never fatal.
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
        "KeepAlive" => Msg::KeepAlive,
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
/// One thing a session is checked for: what to call it in the log, and how to see whether it is there.
type Field = (&'static str, fn(&Value) -> bool);

/// `None` when the pushed list is the same shape as the polled one, else what differed, for the log.
/// Only sessions with something loaded are compared, and only those both lists saw: an app that is
/// open with nothing playing is in one list and not the other for reasons that say nothing about the
/// socket, and a play can start or stop between the two reads.
pub fn looks_like_sessions(pushed: &[Value], polled: &[Value]) -> Option<String> {
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
        // Nothing loaded in one of them: no evidence either way, which is not evidence against.
        return None;
    }
    let by_id = |list: &[Value], id: &str| list.iter().find(|s| s["Id"].as_str() == Some(id)).cloned().unwrap_or(Value::Null);
    // What is compared is whether a field is *there*, not what is in it.
    let fields: [Field; 5] = [
        ("NowPlayingItem.Name", |s| !s["NowPlayingItem"]["Name"].is_null()),
        ("NowPlayingItem.Type", |s| !s["NowPlayingItem"]["Type"].is_null()),
        ("NowPlayingItem.MediaStreams", |s| !s["NowPlayingItem"]["MediaStreams"].is_null()),
        ("PlayState.PositionTicks", |s| !s["PlayState"]["PositionTicks"].is_null()),
        ("UserId", |s| !s["UserId"].is_null()),
    ];
    for id in shared {
        let (p, q) = (by_id(pushed, id), by_id(polled, id));
        for (name, has) in &fields {
            if has(&p) != has(&q) {
                return Some(format!("session {id}: the push {} {name}, /Sessions {}", if has(&p) { "has" } else { "has no" }, if has(&q) { "has" } else { "has none" }));
            }
        }
    }
    None
}

/// Half of what the server asked for, and never so rare that it closes on us or so often that it
/// becomes chatter of its own.
pub fn keepalive_period(force: u64) -> Duration {
    Duration::from_secs((force / 2).clamp(5, 30))
}

/// Nothing at all from the server for this long is death: two keepalives of ours unanswered, plus a
/// little room for a slow one. A session list is *not* what is being waited for — a paused play, or
/// an evening when nobody is watching, has none to send and is perfectly well.
pub fn lost_after(period: Duration) -> Duration {
    period * 2 + Duration::from_secs(10)
}

/// 1, 2, 4 … 60 seconds. A server that is down, restarting or does not speak this is not hammered.
pub fn backoff(attempt: u32) -> Duration {
    Duration::from_secs(2u64.saturating_pow(attempt.min(6)).min(60))
}

/// Run the socket until it fails, then say why, wait and try again. Ends only when the handle is dropped.
pub fn spawn(jf: Jellyfin) -> Handle {
    let (tx, rx) = mpsc::channel(QUEUE);
    let (want, mut want_rx) = watch::channel(true);
    let wire = Arc::new(Wire::default());
    let mine = wire.clone();
    let task = tokio::spawn(async move {
        let mut attempt = 0u32;
        loop {
            let started = Instant::now();
            tracing::debug!("socket: connecting to {}", jf.base());
            let end = match serve(&jf, &tx, &mut want_rx, &mine).await {
                Ok(end) => end,
                Err(e) => End::Broken(format!("{e:#}")),
            };
            // Nothing is open and nothing is subscribed between one connection and the next.
            mine.set(false, false);
            // A socket that lived a while was healthy; the next hiccup starts counting from scratch.
            attempt = if started.elapsed() > Duration::from_secs(60) { 0 } else { attempt.saturating_add(1) };
            let (why, wait) = match end {
                End::Unsupported(why) => {
                    tracing::info!("socket: {why} → fallback polling, re-probing in {}s", PROBE_EVERY.as_secs());
                    (why, PROBE_EVERY)
                }
                End::Broken(why) => {
                    tracing::debug!("socket: {why}; trying again in {}s", backoff(attempt).as_secs());
                    (why, backoff(attempt))
                }
            };
            if tx.send(Event::Down(why)).await.is_err() {
                return; // the collector is gone
            }
            tokio::time::sleep(wait).await;
        }
    });
    Handle { rx, want, wire, task }
}

/// One connection, from the handshake to whatever ends it. `Ok(reason)` is an ordinary end.
async fn serve(jf: &Jellyfin, tx: &mpsc::Sender<Event>, want: &mut watch::Receiver<bool>, wire: &Wire) -> Result<End> {
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

    // Always subscribe first, whatever the collector wants: the list that comes back is the proof that
    // this server speaks the protocol at all. One list is a small price for that.
    wire.set(true, false);
    ws.send(Message::Text(SESSIONS_START.into())).await.context("asking Jellyfin to send sessions")?;
    wire.set(true, true);
    let mut listening = true;
    let mut listening_since = Instant::now();
    // Reconnected in the middle of a play: subscribe for the proof, then go quiet again at once.
    if !*want.borrow_and_update() {
        ws.send(Message::Text(SESSIONS_STOP.into())).await.context("asking Jellyfin to stop sending sessions")?;
        wire.set(true, false);
        listening = false;
    }

    let mut period = keepalive_period(60);
    let mut keepalive = tokio::time::interval(period);
    keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut watchdog = tokio::time::interval(Duration::from_secs(1));
    // Two different questions, and 1.4.0 answered both with one clock, which is the bug this fixes.
    //   `subscribed`: did `SessionsStart` take? Jellyfin answers it with the session list as it is
    //     right now — empty when nobody is watching anything — so the first list is the proof, and
    //     no first list means this server does not speak it.
    //   `heard`: is the server still there? Anything at all counts, because after that first list a
    //     healthy Jellyfin sends nothing until something changes, which can be hours.
    let mut subscribed: Option<Instant> = None;
    // Said once per connection, so the collector is not told the same nothing every second.
    let mut unproven = false;
    let mut asked_at = Instant::now();
    let mut heard: Option<Instant> = None;
    let mut checked = false;

    loop {
        tokio::select! {
            frame = ws.next() => match frame {
                None => return Ok(End::Broken("Jellyfin closed the connection".into())),
                Some(Err(e)) => return Ok(End::Broken(format!("the connection failed ({e})"))),
                Some(Ok(Message::Text(text))) => {
                    heard = Some(Instant::now());
                    match parse(text.as_str()) {
                    Msg::Sessions(list) => {
                        // Before a single row comes from the socket: does it say what /Sessions says?
                        if !checked {
                            checked = true;
                            // Through the same gate every other `/Sessions` read goes through, and
                            // the one read that waits for its turn rather than being dropped: nothing
                            // else stands between a pushed list and the first row written from it.
                            let polled = crate::collector::sessions_waiting(jf, "socket_check", "socket_check").await.context("comparing the first pushed sessions with /Sessions")?;
                            if let Some(how) = looks_like_sessions(&list, &polled) {
                                tracing::info!("socket: consistency check mismatch: {how}");
                                return Ok(End::Unsupported(format!("what Jellyfin pushes does not match what it answers for /Sessions ({how})")));
                            }
                        }
                        if subscribed.is_none() {
                            tracing::info!("socket: Jellyfin pushes sessions ({:.1}s after subscribing, {} loaded)", listening_since.elapsed().as_secs_f64(), list.iter().filter(|s| !s["NowPlayingItem"].is_null()).count());
                            unproven = false;
                        }
                        subscribed = Some(Instant::now());
                        // A newer picture is on its way in a moment; dropping one is better than waiting.
                        if let Err(mpsc::error::TrySendError::Closed(_)) = tx.try_send(Event::Snapshot(list)) {
                            return Ok(End::Broken("the collector stopped listening".into()));
                        }
                    }
                    Msg::ForceKeepAlive(secs) => {
                        period = keepalive_period(secs);
                        keepalive = tokio::time::interval(period);
                        keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                    }
                    Msg::Unreadable => return Ok(End::Broken("Jellyfin sent a session list that could not be read".into())),
                    // Worth saying only once the socket carries: before that the collector is polling
                    // and has a better answer of its own.
                    Msg::KeepAlive => {
                        if subscribed.is_some() {
                            let _ = tx.try_send(Event::Alive);
                        }
                    }
                    Msg::Other => {}
                    }
                }
                Some(Ok(Message::Close(_))) => return Ok(End::Broken("Jellyfin closed the connection".into())),
                // A pong (the library answers pings itself) is not a message, but it is a sign of life.
                Some(Ok(_)) => heard = Some(Instant::now()),
            },
            // The collector asks for silence while it polls a play, and for the pushes back when it ends.
            _ = want.changed() => {
                let on = *want.borrow_and_update();
                if on != listening {
                    listening = on;
                    listening_since = Instant::now();
                    if let Err(e) = ws.send(Message::Text(if on { SESSIONS_START } else { SESSIONS_STOP }.into())).await {
                        return Ok(End::Broken(format!("the connection failed ({e})")));
                    }
                    wire.set(true, on);
                }
            }
            _ = keepalive.tick() => {
                if let Err(e) = ws.send(Message::Text(KEEP_ALIVE.into())).await {
                    return Ok(End::Broken(format!("the connection failed ({e})")));
                }
            }
            _ = watchdog.tick() => {
                // Nothing has been pushed yet. Say so once, so the collector asks for the list itself
                // meanwhile — and then keep waiting, subscribed, for as long as the connection lives:
                // an idle Jellyfin sends nothing because it has nothing to send, not because it cannot.
                if listening && subscribed.is_none() && !unproven && listening_since.elapsed() > SUBSCRIBE_MAX {
                    match heard {
                        // The server is answering — it simply has nothing to report. Believe the
                        // subscription; the collector reads the current state for itself once.
                        Some(_) => {
                            unproven = true;
                            tracing::info!("socket: subscribed, nothing pushed after {}s, KeepAlive answered → trusting the subscription", SUBSCRIBE_MAX.as_secs());
                            let _ = tx.try_send(Event::Trusted);
                        }
                        // Nothing at all from it, not even an answer to a KeepAlive: that is different.
                        None if listening_since.elapsed() > NOT_ANSWERING => {
                            return Ok(End::Broken(format!("no answer of any kind within {}s", NOT_ANSWERING.as_secs())));
                        }
                        None => {}
                    }
                }
                // And ask again now and then, in case the subscription itself was lost on the way.
                if listening && subscribed.is_none() && asked_at.elapsed() > PROBE_EVERY {
                    asked_at = Instant::now();
                    tracing::info!("socket: still nothing pushed; subscribing again");
                    if let Err(e) = ws.send(Message::Text(SESSIONS_START.into())).await {
                        return Ok(End::Broken(format!("the connection failed ({e})")));
                    }
                }
                let lost = lost_after(period);
                if heard.is_some_and(|at| at.elapsed() > lost) {
                    return Ok(End::Broken(format!("Jellyfin said nothing for {}s", lost.as_secs())));
                }
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
    fn a_quiet_socket_is_not_a_dead_one() {
        assert_eq!(lost_after(keepalive_period(60)), Duration::from_secs(70), "two unanswered keepalives, and a little room");
        // What 1.4.0 got wrong: it judged a connection by how lately a session list had arrived, and a
        // Jellyfin with nobody watching anything sends none for hours. These are the two clocks now.
        assert!(lost_after(keepalive_period(60)) >= keepalive_period(60) * 2, "a healthy socket must miss twice before it is given up");
        assert!(SUBSCRIBE_MAX < lost_after(keepalive_period(60)), "a server that never subscribes is found out long before one that goes quiet");
        // A keepalive answer is the only thing a quiet server ever says; it must be told apart from
        // the chatter that means nothing (a session list is what proves the subscription took).
        assert_eq!(parse(r#"{"MessageType":"KeepAlive","MessageId":"8b1a"}"#), Msg::KeepAlive);
        assert_eq!(parse(r#"{"MessageType":"Sessions","Data":[]}"#), Msg::Sessions(vec![]), "nobody watching anything is still an answer");
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
        // The server's answer to ours: on an idle server it is the only thing that ever arrives, and
        // telling it apart from anything else is what tells a quiet socket from a dead one.
        assert_eq!(parse(r#"{"MessageType":"KeepAlive"}"#), Msg::KeepAlive);
        assert_eq!(parse(r#"{"MessageType":"KeepAlive","MessageId":"8b1a"}"#), Msg::KeepAlive);
        assert_eq!(parse(r#"{"MessageType":"UserDataChanged","Data":{}}"#), Msg::Other);
        assert_eq!(parse("not json at all"), Msg::Unreadable);
        assert_eq!(parse(r#"{"MessageType":"Sessions","Data":{"oops":1}}"#), Msg::Unreadable, "an empty list would look like everybody stopped");
    }

    #[test]
    fn the_first_snapshot_is_held_against_what_sessions_answers() {
        let full = json!([{ "Id": "s1", "UserId": "u1", "NowPlayingItem": {"Id": "i1", "Name": "Big Buck Bunny", "Type": "Movie", "MediaStreams": []}, "PlayState": {"PositionTicks": 10} }]);
        let thin = json!([{ "Id": "s1", "UserId": "u1", "NowPlayingItem": {"Id": "i1", "Name": "Big Buck Bunny", "Type": "Movie"}, "PlayState": {"PositionTicks": 10} }]);
        let (full, thin) = (full.as_array().unwrap().clone(), thin.as_array().unwrap().clone());
        assert_eq!(looks_like_sessions(&full, &full), None);
        assert!(looks_like_sessions(&thin, &full).is_some_and(|w| w.contains("MediaStreams")), "a snapshot without the streams would invent track changes every minute, and the log must say which field");
        assert_eq!(looks_like_sessions(&[], &[]), None, "nothing playing anywhere is no evidence against the socket");
        assert_eq!(looks_like_sessions(&full, &[]), None, "a play that started between the two reads is not a mismatch");

        // An app open with nothing playing is in one list and not the other for reasons that have
        // nothing to do with the socket: `ActiveWithinSeconds` on the poll, no filter on the push.
        let idle_apps = json!([{ "Id": "s9", "UserId": "u9" }, { "Id": "s8", "UserId": "u8", "NowPlayingItem": Value::Null }]);
        let idle_apps = idle_apps.as_array().unwrap().clone();
        assert_eq!(looks_like_sessions(&idle_apps, &[]), None, "idle apps are not a mismatch");
        let mut pushed = full.clone();
        pushed.extend(idle_apps);
        assert_eq!(looks_like_sessions(&pushed, &full), None, "nor are they when something is also playing");
    }

    #[test]
    fn backing_off_is_bounded() {
        assert_eq!(backoff(0), Duration::from_secs(1));
        assert_eq!(backoff(3), Duration::from_secs(8));
        assert_eq!(backoff(20), Duration::from_secs(60), "a server that does not speak this is not hammered");
        assert!((0..30).all(|a| backoff(a) <= backoff(a + 1)));
    }
}
