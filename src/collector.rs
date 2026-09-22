//! Watches what is playing on Jellyfin and turns it into playback rows.
//!
//! A play is written the moment it is first seen (`active = 1`) and refreshed while it
//! runs, so a crash or restart loses at most a few seconds. Time is only counted while
//! the player is not paused, which makes `duration_s` "time actually watched".
//!
//! Two transports, one producer, each doing the half it is good at. The session list arrives either
//! by asking (`GET /Sessions`, on a timer) or by being told (`socket.rs`, pushed); `tick()` cannot
//! tell the difference and does not care — it is handed a list and works out what changed.
//!
//! **Nothing playing: listen.** An idle server has nothing to report, and asking it every few seconds
//! to be told so was almost all the traffic finstats ever caused. Not one request goes out.
//! **Something playing: ask,** at `active_interval_s`, until the last play ends. A pause, a seek or a
//! track change is only as sharp as the gap between two sightings, and how often a server pushes is
//! its business, not ours; asking is also what ends a play whose client vanished without saying so,
//! since the push carries no `ActiveWithinSeconds`. Reads are compressed, so the busy half is cheap.
//!
//! The moment the socket stops carrying, polling resumes on the very next pass — never a gap, because
//! a play that is never seen is lost for good.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::Result;
use serde_json::{Value, json};

use crate::db::{self, norm_id, rusqlite::OptionalExtension, rusqlite::params};
use crate::jellyfin::Jellyfin;
use crate::media::{self, Streams, ticks_to_s};
use crate::playback::{PlayEvent, PlayRecord, insert_events};
use crate::state::App;

const PERSIST_EVERY: Duration = Duration::from_secs(30);
/// How often to check the socket's word against `/Sessions`, while there is something to check.
/// While the socket carries and nothing is playing there is nothing to do and nothing to ask for, so
/// the loop only comes round this often to re-read its settings. It costs no request.
const SOCKET_IDLE_WAKE: Duration = Duration::from_secs(60);
/// While the socket carries paused sessions, one read after this much *silence* — no push and no
/// read — in case a push went missing. Silence is what is measured, and a read is as good as a push
/// at ending it: an early attempt restarted the wait on every push alone, so a client that kept reporting its
/// progress while paused reset it for ever and the net never once fell; the next measured it from
/// the last *push* and nothing else, so the first pause after a minute of playing — when the
/// subscription had been off the whole time and no push could have arrived — was already "silent",
/// and since the read it called for reset nothing either, the reads went out as fast as they could be
/// made. 2,625 of them in eight seconds, until a push happened along. Hence `Net` below: every one of
/// the three things that ends silence re-arms the clock, the attempt itself included.
const PAUSED_SAFETY_SILENCE: Duration = Duration::from_secs(60);
/// And one read this often regardless, pushes or no pushes, because a push can also be frequent and
/// wrong. This is the only read an idle server ever makes: it has nothing to be silent about.
const SAFETY_EVERY: Duration = Duration::from_secs(300);
/// However the clocks stand, never two one-shot reads closer together than this. A due time that
/// stays at zero is the shape of the storm above, and this is the floor under it.
const SAFETY_MIN_GAP: Duration = Duration::from_secs(5);
/// How often to ask when everything is paused and there is no socket to be told by. A paused session
/// changes when a person does something, and a person is not a second-by-second event.
const PAUSED_POLL_S: i64 = 15;
/// And how often when nothing is loaded at all and the socket that should be saying so is not there.
/// Never *shorter* than the owner's own idle interval: this slows a broken socket down, it does not
/// speed anything up.
const FALLBACK_IDLE_S: i64 = 30;
/// How many readings in a row must say "every active session is paused" before the socket takes over.
/// At the active interval that is about three seconds — long enough that pausing to fetch a drink and
/// starting again does not change transport twice.
const PAUSE_DEBOUNCE: u32 = 3;
const DEVICE_REFRESH: Duration = Duration::from_secs(300);
/// Plays shorter than this are accidental clicks; they are dropped when they end.
const MIN_KEEP_S: i64 = 2;
/// A position that lands further than this from where steady playback would be is a skip.
const SEEK_TOLERANCE_S: f64 = 20.0;

struct Tracked {
    row_id: i64,
    rec: PlayRecord,
    watched: f64,
    paused: f64,
    is_paused: bool,
    last_tick: Instant,
    last_persist: Instant,
    transcode_progress: Option<f64>,
}

fn opt_str(v: &Value) -> Option<String> {
    v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

/// Build a record from one Jellyfin session. `None` when nothing is playing.
fn record_from_session(s: &Value, now: i64) -> Option<PlayRecord> {
    let item = &s["NowPlayingItem"];
    let item_id = norm_id(item["Id"].as_str()?);
    let user_id = norm_id(s["UserId"].as_str()?);
    let play_state = &s["PlayState"];
    let transcode = media::compact_transcode(&s["TranscodingInfo"]);
    let play_method = media::effective_play_method(play_state["PlayMethod"].as_str(), transcode.as_ref());
    let item_type = item["Type"].as_str().unwrap_or("Unknown").to_string();
    let is_episode = item_type == "Episode";

    Some(PlayRecord {
        source: "live",
        source_id: None,
        active: true,
        user_id,
        user_name: s["UserName"].as_str().unwrap_or("Unknown").to_string(),
        item_id,
        item_name: item["Name"].as_str().unwrap_or("Unknown").to_string(),
        item_type,
        series_id: item["SeriesId"].as_str().map(norm_id),
        series_name: opt_str(&item["SeriesName"]).or_else(|| if is_episode { None } else { opt_str(&item["Album"]) }),
        season_id: item["SeasonId"].as_str().map(norm_id),
        season_number: item["ParentIndexNumber"].as_i64().filter(|_| is_episode),
        episode_number: item["IndexNumber"].as_i64().filter(|_| is_episode),
        started_at: now,
        ended_at: now,
        duration_s: 0,
        paused_s: 0,
        position_s: ticks_to_s(&play_state["PositionTicks"]),
        runtime_s: ticks_to_s(&item["RunTimeTicks"]),
        client: opt_str(&s["Client"]),
        device_name: opt_str(&s["DeviceName"]),
        device_id: opt_str(&s["DeviceId"]),
        app_version: opt_str(&s["ApplicationVersion"]),
        remote_ip: opt_str(&s["RemoteEndPoint"]),
        play_method,
        container: opt_str(&item["Container"]).map(|c| c.split(',').next().unwrap_or_default().to_string()),
        streams: Streams::extract(&item["MediaStreams"], play_state),
        transcode,
        pause_count: 0,
        seek_count: 0,
        start_position_s: ticks_to_s(&play_state["PositionTicks"]),
    })
}

fn live_json(key: &str, t: &Tracked) -> Value {
    let r = &t.rec;
    let st = &r.streams;
    let transcode = r.transcode.as_ref().map(|tc| {
        json!({
            "video_codec": tc["video_codec"], "audio_codec": tc["audio_codec"], "container": tc["container"],
            "is_video_direct": tc["is_video_direct"], "is_audio_direct": tc["is_audio_direct"],
            "hw_accel": tc["hw_accel"], "reasons": tc["reasons"],
            "progress": t.transcode_progress,
        })
    });
    json!({
        "key": key,
        "user_id": r.user_id, "user_name": r.user_name,
        "item_id": r.item_id, "item_name": r.item_name, "item_type": r.item_type,
        "series_id": r.series_id, "series_name": r.series_name,
        "season_number": r.season_number, "episode_number": r.episode_number,
        "image_item_id": if r.item_type == "Episode" { r.series_id.as_ref().unwrap_or(&r.item_id) } else { &r.item_id },
        "position_s": r.position_s, "runtime_s": r.runtime_s,
        "is_paused": t.is_paused,
        "started_at": r.started_at, "watched_s": t.watched.round() as i64,
        "client": r.client, "device_name": r.device_name, "app_version": r.app_version,
        "remote_ip": r.remote_ip,
        "play_method": r.play_method,
        "video": media::video_label(st.video_codec.as_deref(), st.width, st.height, st.video_range.as_deref()),
        "audio": media::audio_label(st.audio_codec.as_deref(), st.audio_channels, None),
        "subtitle": media::subtitle_label(st.subtitle_codec.as_deref(), st.subtitle_language.as_deref()),
        "container": r.container, "bitrate": st.bitrate,
        "transcode": transcode,
    })
}

fn clock(total: i64) -> String {
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

fn audio_of(r: &PlayRecord) -> Option<String> {
    media::audio_label(r.streams.audio_codec.as_deref(), r.streams.audio_channels, r.streams.audio_language.as_deref())
}

fn subtitle_of(r: &PlayRecord) -> Option<String> {
    media::subtitle_label(r.streams.subtitle_codec.as_deref(), r.streams.subtitle_language.as_deref())
}

fn transcode_detail(r: &PlayRecord) -> String {
    let reasons: Vec<&str> = r.transcode.as_ref().and_then(|t| t["reasons"].as_array()).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    if reasons.is_empty() { r.play_method.clone() } else { format!("{}: {}", r.play_method, reasons.join(", ")) }
}

/// What changed between two sightings of the same play. `dt` is the time between them.
fn diff_events(old: &PlayRecord, new: &mut PlayRecord, was_paused: bool, is_paused: bool, dt: f64, now: i64) -> Vec<PlayEvent> {
    let mut out = vec![];
    let ev = |kind, position_s, detail| PlayEvent { at: now, kind, position_s, detail };

    if let (Some(from), Some(to)) = (old.position_s, new.position_s) {
        let expected = from as f64 + if was_paused { 0.0 } else { dt };
        if (to as f64 - expected).abs() > SEEK_TOLERANCE_S + dt * 0.5 {
            new.seek_count += 1;
            out.push(ev("seek", Some(to), Some(format!("{} → {}", clock(expected.round() as i64), clock(to)))));
        }
    }
    if was_paused != is_paused {
        if is_paused {
            new.pause_count += 1;
        }
        out.push(ev(if is_paused { "pause" } else { "resume" }, new.position_s, None));
    }
    let (a_old, a_new) = (audio_of(old), audio_of(new));
    if a_new.is_some() && a_old != a_new {
        out.push(ev("audio", new.position_s, a_new));
    }
    let (s_old, s_new) = (subtitle_of(old), subtitle_of(new));
    if s_old != s_new {
        out.push(ev("subtitle", new.position_s, Some(s_new.unwrap_or_else(|| "Off".into()))));
    }
    if old.play_method != new.play_method || (new.transcode.is_some() && transcode_detail(old) != transcode_detail(new)) {
        out.push(ev("transcode", new.position_s, Some(transcode_detail(new))));
    }
    out
}

/// The seconds between two sightings belong to whichever state the play was *already* in. A gap while
/// paused is paused time however long the gap, which is what lets the socket carry a paused play with
/// a reading only now and then: an hour on pause cannot become an hour of watch time.
pub fn attribute(dt: f64, was_paused: bool) -> (f64, f64) {
    match was_paused {
        true => (0.0, dt),
        false => (dt, 0.0),
    }
}

/// What a session list says is happening: how many sessions have something loaded, and how many of
/// those are actually running. Counted from the list itself, so a pushed one and an asked-for one
/// decide the same thing.
pub fn activity(sessions: &[Value]) -> (usize, usize) {
    let active = sessions.iter().filter(|s| !s["NowPlayingItem"].is_null());
    let playing = active.clone().filter(|s| !s["PlayState"]["IsPaused"].as_bool().unwrap_or(false));
    (active.count(), playing.count())
}

/// Whether Jellyfin should be pushing session lists right now: not while a poll timer is running for
/// a reason, which means while something is playing *and* through the pause debounce that follows it.
/// Subscribing at the start of the debounce is what once had finstats subscribed and polling at the same
/// moment, the same list arriving twice for three seconds.
///
/// Deliberately *not* "while the collector is in listening mode": listening needs a socket that has
/// proved itself, and an earlier rule made that proof depend on a subscription it had already given up.
pub fn should_subscribe(playing: usize, settling: bool) -> bool {
    playing == 0 && !settling
}

/// What the collector is doing, in one word, for the status and the logs. `Mode` is the decision;
/// this is that decision plus why, which is what somebody checking wants to see.
pub fn session_mode(socket_live: bool, mode: Mode, active: usize) -> &'static str {
    match (socket_live, mode == Mode::Listen, active > 0) {
        // Only a socket that cannot be used is a fallback. Never a step along the way of an ordinary
        // change of transport: an early attempt called the three seconds of pause debounce
        // "fallback", which read as a fault every time anybody paused anything.
        (false, ..) => "fallback",
        (true, true, true) => "paused_socket",
        (true, true, false) => "idle_socket",
        // Asking, with a usable socket: something is running, or has only just stopped running and
        // the debounce is seeing whether it stays that way.
        (true, false, _) => "playing_poll",
    }
}

/// The beat while asking. Close while something runs; slower when the socket ought to be carrying and
/// is not, because that is a fallback and not the normal way of working; and otherwise the owner's own
/// intervals, untouched, which is what a socket that is carrying leaves them as.
pub fn poll_interval_s(active_s: i64, idle_s: i64, playing: usize, active: usize, fallback: bool) -> i64 {
    match (playing, active, fallback) {
        (1.., ..) => active_s,
        (0, 1.., true) => PAUSED_POLL_S,
        (0, 0, true) => idle_s.max(FALLBACK_IDLE_S),
        (0, 1.., false) => active_s,
        (0, 0, false) => idle_s,
    }
    .clamp(1, 60)
}

// ---------------------------------------------------------------- how often /Sessions may be asked

/// At most this many `/Sessions` reads in any one second, from every caller there is.
const READS_PER_S: usize = 2;
/// And in any one minute. A play is watched at one read a second, which is sixty; the rest is room
/// for a catch-up or a socket check on top of it, and nothing whatever above that.
const READS_PER_MIN: usize = 70;
const A_SECOND: Duration = Duration::from_secs(1);
const A_MINUTE: Duration = Duration::from_secs(60);
/// A word about reads being refused, at most this often: a limiter that shouts is its own storm.
const SUPPRESSED_EVERY: Duration = Duration::from_secs(60);
/// What the collector may be asking in a mode where it is supposed to be listening: the catch-up read
/// and the net, and nothing else. More than this is the machine disagreeing with itself.
const READS_WHILE_LISTENING: u32 = 2;

/// Every `/Sessions` read finstats makes, from every caller — the beat while something plays, the
/// one-shot net, the catch-up after trusting a subscription, the socket's own consistency check —
/// passes through this one gate. Not because any of them is expected to misbehave, but because
/// a clock that stopped being reset once turned one of them into thousands, and a limiter
/// is the only part of this that is true whatever the state machine believes. It is the floor, not
/// the plan: the plan is above, in `Net`.
struct Reads {
    /// When each read was made, and whether the collector was listening at the time. Only the last
    /// minute is kept, so this is seventy entries at the very most.
    at: Vec<(Instant, bool)>,
    suppressed: u64,
    reported: Option<Instant>,
}

/// One per process, and the only one: two limiters would be no limiter.
static READS: Mutex<Reads> = Mutex::new(Reads::new());

impl Reads {
    const fn new() -> Self {
        Self { at: Vec::new(), suppressed: 0, reported: None }
    }

    fn prune(&mut self, now: Instant) {
        self.at.retain(|(at, _)| now.saturating_duration_since(*at) < A_MINUTE);
    }

    /// How long until a read would be allowed under one window, or `None` if one is now.
    fn free_in(&self, now: Instant, window: Duration, limit: usize) -> Option<Duration> {
        let mut within: Vec<Instant> = self.at.iter().map(|(at, _)| *at).filter(|at| now.saturating_duration_since(*at) < window).collect();
        if within.len() < limit {
            return None;
        }
        within.sort_unstable();
        // The one that has to leave the window before there is room, plus a moment so it really has.
        let free_at = within[within.len() - limit] + window;
        Some(free_at.saturating_duration_since(now) + Duration::from_millis(1))
    }

    /// A slot for one read, or how long until there is one. Refusals are counted, so that a caller
    /// that keeps asking shows up as a number rather than as traffic.
    fn take(&mut self, now: Instant, listening: bool) -> Result<(), Duration> {
        self.prune(now);
        if let Some(wait) = self.free_in(now, A_SECOND, READS_PER_S).or_else(|| self.free_in(now, A_MINUTE, READS_PER_MIN)) {
            self.suppressed += 1;
            return Err(wait);
        }
        self.at.push((now, listening));
        Ok(())
    }

    /// How many reads went out in the last minute — what a packet capture would count.
    fn last_min(&mut self, now: Instant) -> usize {
        self.prune(now);
        self.at.len()
    }

    /// How many of those were made while listening and no longer ago than `since`, which is how the
    /// mode is held to its own promise of asking for next to nothing.
    fn listening_since(&mut self, now: Instant, since: Instant) -> u32 {
        self.prune(now);
        self.at.iter().filter(|(at, listening)| *listening && *at >= since).count() as u32
    }

    /// `Some(n)` when it is time to say how many reads have been refused since the last such word.
    fn report(&mut self, now: Instant) -> Option<u64> {
        if self.suppressed == 0 || self.reported.is_some_and(|at| now.saturating_duration_since(at) < SUPPRESSED_EVERY) {
            return None;
        }
        self.reported = Some(now);
        Some(std::mem::take(&mut self.suppressed))
    }
}

/// Is the collector meant to be listening in this mode? The two modes where a read is an exception
/// rather than the way of working.
fn listening_mode(mode: &str) -> bool {
    matches!(mode, "idle_socket" | "paused_socket")
}

/// Take a slot for one `/Sessions` read, or be told how long until there is one. Every caller goes
/// through here; a refusal is a bug somewhere above, so it is counted and said out loud — once a
/// minute, because the one thing a limiter must not do is make its own noise.
pub fn sessions_slot(caller: &'static str, mode: &'static str) -> Result<(), Duration> {
    let now = Instant::now();
    let mut reads = READS.lock().unwrap();
    let out = reads.take(now, listening_mode(mode));
    if let Some(n) = reads.report(now) {
        tracing::warn!("rate limit hit: {n} /Sessions requests suppressed in {mode} (caller: {caller})");
    }
    out
}

/// Read `/Sessions`, waiting for a slot rather than giving one up. For the read that cannot simply be
/// skipped: the socket's consistency check, which is the only thing standing between a pushed list
/// and the first row written from it.
pub async fn sessions_waiting(jf: &Jellyfin, caller: &'static str, mode: &'static str) -> Result<Vec<Value>> {
    for _ in 0..4 {
        match sessions_slot(caller, mode) {
            Ok(()) => return jf.sessions().await,
            Err(wait) => tokio::time::sleep(wait).await,
        }
    }
    anyhow::bail!("too many /Sessions reads are already in flight to make one more")
}

/// How many `/Sessions` reads went out in the last minute. Read straight from the gate, so it is what
/// a capture would show and not what the collector believes it asked for.
pub fn sessions_reads_last_min() -> usize {
    READS.lock().unwrap().last_min(Instant::now())
}

/// How many went out while listening since `since` — for the state machine marking its own work.
fn reads_in_mode(since: Instant) -> u32 {
    let now = Instant::now();
    // `checked_sub` because a monotonic clock is allowed to be younger than a minute.
    let window = now.checked_sub(A_MINUTE).unwrap_or(since);
    READS.lock().unwrap().listening_since(now, since.max(window))
}

// ---------------------------------------------------------------- when the net falls

/// The clocks behind the one-shot read, and the whole of the fix that ended that storm. Two of
/// them, because silence and the five-minute net are different questions:
///
///   * `activity` — when something last happened on this subscription: a push, or a read of our own.
///     A *fresh subscription* is also activity, and that is the part the first version of this
///     lacked. While a play is polled the subscription is off and no push can arrive, so the moment
///     it goes back on, a clock measured from the last push is already a minute stale and asks for
///     a read at once — and then for another, because nothing the read did touched the clock that
///     called for it.
///   * `read` — when finstats last read `/Sessions`, by any route at all, the beat during a play
///     included. A poll a second ago is better evidence than any net could fetch, so the net is not
///     due; three minutes of playing therefore end in a pause that asks for nothing.
///
/// Every way of ending silence re-arms both where it should, *the attempt itself included*: the read
/// that a due time calls for must reset the clock that called for it, or the due time never moves.
pub struct Net {
    activity: Instant,
    read: Instant,
}

impl Net {
    pub fn new(now: Instant) -> Self {
        Self { activity: now, read: now }
    }

    /// Jellyfin is being asked to push again. A fresh subscription is a fresh silence window: nothing
    /// heard before it says anything about it.
    pub fn subscribed(&mut self, now: Instant) {
        self.activity = now;
    }

    /// A pushed list: the socket is carrying, which is what silence is the absence of.
    pub fn pushed(&mut self, now: Instant) {
        self.activity = now;
    }

    /// A `/Sessions` read of any kind, counted from when it was *begun*. Both clocks: a read answers
    /// the same question a push does, and better.
    pub fn read(&mut self, now: Instant) {
        (self.activity, self.read) = (now, now);
    }

    pub fn silence(&self, now: Instant) -> Duration {
        now.saturating_duration_since(self.activity)
    }

    /// When the next one-shot read falls due, `ZERO` meaning now.
    pub fn due(&self, now: Instant, active: usize) -> Duration {
        safety_due(active, now.saturating_duration_since(self.activity), now.saturating_duration_since(self.read))
    }
}

/// When the next one-shot read falls due, `ZERO` meaning now. Silence only counts against something
/// that is paused: an idle server is silent because there is nothing to say, and asking it anyway is
/// the thing the socket exists to stop. Under both, a floor: two reads are never closer together than
/// `SAFETY_MIN_GAP`, whatever the clocks say, because a due time stuck at zero is what a storm is.
pub fn safety_due(active: usize, since_activity: Duration, since_read: Duration) -> Duration {
    let net = SAFETY_EVERY.saturating_sub(since_read);
    let due = match active > 0 {
        true => net.min(PAUSED_SAFETY_SILENCE.saturating_sub(since_activity)),
        false => net,
    };
    due.max(SAFETY_MIN_GAP.saturating_sub(since_read))
}

/// A disagreement is only worth a word when it lasts: asking the socket to stop and the stop reaching
/// the wire are two moments, and in between the picture is briefly, harmlessly inconsistent.
const SETTLE: Duration = Duration::from_secs(2);
/// How long a change of transport may take to reach the wire before it is published anyway.
const SETTLE_WIRE: Duration = Duration::from_millis(500);

/// One write of the whole picture, and the state machine marking its own work.
fn publish(app: &App, mut now: Published, mode_since: &mut i64, mode_at: &mut Instant, bad_since: &mut Option<Instant>) {
    {
        let mut st = app.collector.write().unwrap();
        if st.session_mode != now.session_mode {
            (*mode_since, *mode_at) = (db::now(), Instant::now());
            // A mode answers for what it asks for itself. The minute of one-a-second reads behind a
            // pause belongs to the play that has just ended, not to the listening that follows it.
            now.reads_in_mode = 0;
        }
        (st.session_mode, st.socket_connected, st.socket_subscribed) = (now.session_mode, now.socket_connected, now.socket_subscribed);
        (st.poll_interval_s, st.mode_since) = (now.poll_interval_s, *mode_since);
    }
    match disagrees(&now) {
        None => *bad_since = None,
        Some(what) => {
            let since = *bad_since.get_or_insert_with(Instant::now);
            if since.elapsed() > SETTLE {
                tracing::warn!("the collector disagrees with itself: {what}");
                *bad_since = Some(Instant::now());
            }
        }
    }
}

/// Everything the state machine has decided, as one picture. Written in a single lock so that no
/// request can catch it half-applied — a mode of "listening" with a poll timer still running would
/// be a lie for however many microseconds it lasted.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Published {
    pub session_mode: &'static str,
    pub socket_connected: bool,
    pub socket_subscribed: bool,
    /// The beat actually running, in seconds; `None` while nothing is being asked for. A safety read
    /// is not a beat — it is one read in case a push went missing — so it does not appear here.
    pub poll_interval_s: Option<i64>,
    /// `/Sessions` reads made while listening since this mode began, or in the last minute, whichever
    /// is the shorter time. Counted from the gate every read goes through, so it is what went out on
    /// the wire; the reads of the mode before this one are not this mode's to answer for.
    pub reads_in_mode: u32,
}

/// What must be true of that picture, checked against itself. This is the state machine marking its
/// own work: each rule is a thing that would show up on the wire if it were wrong.
pub fn disagrees(p: &Published) -> Option<String> {
    let says = |what: &str| Some(format!("{what} (mode {}, connected {}, subscribed {}, poll {:?})", p.session_mode, p.socket_connected, p.socket_subscribed, p.poll_interval_s));
    match p.session_mode {
        "idle_socket" | "paused_socket" if !p.socket_connected => says("listening without a connection"),
        "idle_socket" | "paused_socket" if !p.socket_subscribed => says("listening without a subscription"),
        "idle_socket" | "paused_socket" if p.poll_interval_s.is_some() => says("listening and polling at once"),
        // Listening means the net and the catch-up, and nothing else. The storm made 2,625 reads in the
        // eight seconds after a pause and said `paused_socket, poll null` throughout, every word of
        // it true and the whole of it wrong: nothing published counted what was actually going out.
        "idle_socket" | "paused_socket" if p.reads_in_mode > READS_WHILE_LISTENING => says(&format!("listening, and asking anyway ({} reads of /Sessions)", p.reads_in_mode)),
        "playing_poll" if p.socket_subscribed => says("polling a play while still subscribed: the same list would arrive twice"),
        // The general form of it. A safety read is one request and not a beat, so it never shows here.
        _ if p.socket_subscribed && p.poll_interval_s.is_some() && p.session_mode != "fallback" => says("subscribed and polling at the same time"),
        "playing_poll" if p.poll_interval_s.is_none() => says("playing, and asking for nothing"),
        "fallback" if p.poll_interval_s.is_none() => says("fallen back to polling, and not polling"),
        _ => None,
    }
}

/// Which transport should be carrying.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// Ask for the list. Something is running, and how sharp a pause or a seek is depends on the gap
    /// between two sightings.
    Poll,
    /// Let Jellyfin push it. Either nothing is loaded anywhere, or everything that is has been paused
    /// long enough to believe — and a paused session is exactly what a server has nothing to say about.
    Listen,
}

/// The mode, from one session list at a time. Holds only the debounce count, so it can be tested by
/// feeding it the readings of an evening.
#[derive(Default)]
pub struct Decide {
    all_paused_for: u32,
}

impl Decide {
    /// Is a pause being watched to see whether it lasts? Polling continues throughout, so the
    /// subscription stays off and the mode stays `playing_poll`.
    pub fn settling(&self) -> bool {
        (1..PAUSE_DEBOUNCE).contains(&self.all_paused_for)
    }
}

impl Decide {
    /// One reading. `socket_live` because listening is only on offer while the socket carries.
    pub fn see(&mut self, active: usize, playing: usize, socket_live: bool) -> Mode {
        if playing > 0 {
            // Somebody is watching, whoever else has paused.
            self.all_paused_for = 0;
            return Mode::Poll;
        }
        if active == 0 {
            self.all_paused_for = 0;
            return if socket_live { Mode::Listen } else { Mode::Poll };
        }
        self.all_paused_for = self.all_paused_for.saturating_add(1);
        match socket_live && self.all_paused_for >= PAUSE_DEBOUNCE {
            true => Mode::Listen,
            false => Mode::Poll,
        }
    }
}

/// Who started watching again, for the line in the log.
fn resumed_by(sessions: &[Value]) -> String {
    sessions
        .iter()
        .find(|s| !s["NowPlayingItem"].is_null() && !s["PlayState"]["IsPaused"].as_bool().unwrap_or(false))
        .map(|s| format!("{}/{}", s["UserName"].as_str().unwrap_or("somebody"), s["NowPlayingItem"]["Name"].as_str().unwrap_or("something")))
        .unwrap_or_else(|| "somebody".into())
}

/// Where one pass got its session list.
#[derive(Clone, Copy, PartialEq)]
enum Source {
    /// Pushed by Jellyfin: the socket's business is that something started or stopped.
    Push,
    /// A single read while listening: the net under a push that never came, or the catch-up that
    /// follows trusting a subscription. Not a beat, and never the transport.
    Safety,
    /// Asked for — because something is playing, or because there is no socket to be told by.
    Poll,
}

pub async fn run(app: App) {
    // Anything still flagged active belongs to a previous process.
    if let Err(e) = app
        .db
        .call(|c| {
            c.execute("DELETE FROM playbacks WHERE active = 1 AND duration_s < ?1", [MIN_KEEP_S])?;
            c.execute("UPDATE playbacks SET active = 0 WHERE active = 1", [])?;
            Ok(())
        })
        .await
    {
        tracing::error!("could not reset active plays: {e:#}");
    }

    let mut tracked: HashMap<String, Tracked> = HashMap::new();
    let mut devices_seen: HashMap<(String, String), Instant> = HashMap::new();
    let mut failures = 0u32;
    let mut sock: Option<crate::socket::Handle> = None;
    // The socket said it is carrying. Not "a session list arrived lately": Jellyfin sends one when
    // something changes and nothing at all in between, so on a quiet evening the two are opposites.
    let mut socket_live = false;
    // A pushed list that arrived while this loop was doing something else, kept for the next pass.
    let mut pending: Option<Vec<Value>> = None;
    // Which transport should carry, and what the last list said. Both persist between passes: in
    // listening mode a reading only arrives when Jellyfin has something to say, which while everything
    // is paused can be a long time.
    let mut mode = Mode::Poll;
    let mut mode_since = db::now();
    let mut mode_at = Instant::now();
    let mut bad_since: Option<Instant> = None;
    // The clocks the one-shot read hangs on: silence, and when `/Sessions` was last read at all.
    let mut net = Net::new(Instant::now());
    // Whether Jellyfin was being asked to push at the end of the last pass, so that the moment it is
    // asked again can start a fresh silence window.
    let mut was_subscribed = false;
    // Carrying on trust rather than on a pushed list; the first real push is held against it.
    let mut trusted_unproven = false;
    let mut decide = Decide::default();
    let (mut active, mut playing) = (0usize, 0usize);
    let mut socket_error: Option<String> = None;
    let mut socket_for: Option<String> = None;

    loop {
        let settings = app.settings();
        let Some(jf) = app.jellyfin() else {
            (sock, socket_live) = (None, false);
            tokio::select! {
                _ = app.wake.notified() => {}
                _ = tokio::time::sleep(Duration::from_secs(5)) => {}
            }
            continue;
        };

        // There is always a socket. Being told what is playing is how finstats collects; asking is
        // the half of it that a play deserves while it runs, not a mode of its own to switch to. A
        // socket belongs to the server it was opened against, so pointing finstats at another
        // Jellyfin drops the old one rather than leaving it talking to the wrong machine.
        if sock.is_some() && socket_for.as_deref() != Some(jf.base()) {
            (sock, socket_live, socket_error, socket_for) = (None, false, None, None);
        }
        if sock.is_none() {
            sock = Some(crate::socket::spawn(jf.clone()));
            socket_for = Some(jf.base().to_string());
            socket_error = None;
        }

        // Subscribed unless something is actually running. **Not** "unless the collector is in
        // listening mode": that was the bug this rule exists for. Listening needs the socket to
        // have proved itself, proof is the list Jellyfin sends when subscribed to, and unsubscribing
        // before it arrives meant it never did — the fallback latched for good, on exactly the idle
        // server the socket is for. While something plays the pushes are a second copy of what is
        // already being asked for, uncompressed, so those are the only moments worth silence.
        if let Some(handle) = sock.as_ref() {
            handle.listen(should_subscribe(playing, decide.settling()));
        }

        // A subscription that has just gone back on starts its silence window here. Watched on the
        // wire rather than on what was asked for, so a reconnection that re-sends `SessionsStart` of
        // its own counts too. Without this the first pause after a minute of playing looks like a
        // minute of silence the instant it begins — the socket was unsubscribed for all of it — and
        // the net falls at once and then again and again, which is the storm this is the guard against.
        let subscribed_now = sock.as_ref().is_some_and(|h| h.subscribed());
        if subscribed_now && !was_subscribed {
            net.subscribed(Instant::now());
        }
        was_subscribed = subscribed_now;

        // Each transport where it is the better one. **Nothing playing**: listen. An idle server has
        // nothing to report, and asking it every few seconds to be told so was most of the traffic
        // finstats ever caused. **Something playing**: ask, at `active_interval_s`. That is where the
        // detail lives — a pause, a seek or a track change is only as sharp as the gap between two
        // sightings, and no push cadence is ours to promise. Every read is compressed, so the
        // busy half is also the cheap half. It also means the poll that ends a play whose client
        // vanished is simply the next poll: the push carries no `ActiveWithinSeconds`, but nothing
        // that is playing is ever judged by the push alone.
        // The mode is decided from session lists; a socket that has gone means polling whatever it said.
        let on_socket = socket_live && mode == Mode::Listen;
        let mut forget_socket = false;
        // Published before anything can block, and again after the list is in: a pass can wait a
        // minute for a push, and the picture must be true for every second of that.
        let beat = poll_interval_s(settings.active_interval_s, settings.idle_interval_s, playing, active, !socket_live);
        fn seen(sock: &Option<crate::socket::Handle>, live: bool, mode: Mode, active: usize, beat: i64, reads: u32) -> Published {
            Published {
                session_mode: session_mode(live, mode, active),
                socket_connected: sock.as_ref().is_some_and(|h| h.connected()),
                socket_subscribed: sock.as_ref().is_some_and(|h| h.subscribed()),
                poll_interval_s: (!(live && mode == Mode::Listen)).then_some(beat),
                reads_in_mode: reads,
            }
        }
        // What this pass would call itself, before the list is in: the gate is told which mode a read
        // is made in, and the log says so when it has to refuse one.
        let mode_now = session_mode(socket_live, mode, active);
        publish(&app, seen(&sock, socket_live, mode, active, beat, reads_in_mode(mode_at)), &mut mode_since, &mut mode_at, &mut bad_since);

        // A list that arrived while the poller was sleeping is this pass's picture; one left over from
        // before the socket went down is not, and a poll is about to fetch a better one anyway.
        let step: Option<(Source, Result<Vec<Value>>)> = match (on_socket, pending.take()) {
            (true, Some(sessions)) => Some((Source::Push, Ok(sessions))),
            (false, _) => match sessions_slot("poll", mode_now) {
                Ok(()) => {
                    // A read of our own answers what a push would have said, and more recently: the
                    // beat during a play is why a pause that follows three minutes of it asks for
                    // nothing at all for the next minute.
                    net.read(Instant::now());
                    Some((Source::Poll, jf.sessions().await))
                }
                // Over budget, which above this line means a bug. Wait out the gate rather than
                // spinning on it; one missed second of a play is a gap, and a gap is only ever
                // attributed to the state the play was already in.
                Err(wait) => {
                    tokio::time::sleep(wait).await;
                    None
                }
            },
            (true, None) => {
                let handle = sock.as_mut().expect("a live socket means there is a socket");
                // When the next one-shot read falls due: after a spell of complete silence with
                // something paused, or on the plain five-minute net, whichever comes first. Neither
                // is a beat — `poll_interval_s` stays null — they are single reads.
                let due = net.due(Instant::now(), active);
                let wait = due.min(SOCKET_IDLE_WAKE);
                tokio::select! {
                    event = handle.recv() => match event {
                        Some(crate::socket::Event::Snapshot(sessions)) => {
                            net.pushed(Instant::now());
                            Some((Source::Push, Ok(sessions)))
                        }
                        Some(crate::socket::Event::Trusted) => None, // already carrying
                        Some(crate::socket::Event::Alive) => {
                            // Nothing has changed, and Jellyfin has just said so: the picture stands.
                            let mut st = app.collector.write().unwrap();
                            (st.connected, st.last_poll_at, st.error) = (true, db::now(), None);
                            None
                        }
                        Some(crate::socket::Event::Down(why)) => {
                            // Say it once per spell, at the level of a thing that fixed itself — and
                            // only about a socket that *was* carrying. One that has not proved itself
                            // yet has already said so in its own words.
                            if socket_error.as_deref() != Some(why.as_str()) && socket_live {
                                match active > 0 && playing == 0 {
                                    true => tracing::info!("socket lost while paused ({why}) → catch-up poll, then every {PAUSED_POLL_S}s"),
                                    false => tracing::info!("Jellyfin is no longer pushing what is playing ({why}); checking on a timer again"),
                                }
                            }
                            (socket_live, socket_error) = (false, Some(why));
                            None
                        }
                        None => {
                            forget_socket = true;
                            socket_live = false;
                            None
                        }
                    },
                    _ = tokio::time::sleep(wait) => match due.is_zero() {
                        true => {
                            // The attempt re-arms the clocks, not its answer: a net that only falls
                            // again once something *else* happens is a net that falls for ever. Both
                            // are set before the gate is asked, so even a refused read waits its turn
                            // rather than coming straight back round.
                            let silence = net.silence(Instant::now());
                            net.read(Instant::now());
                            match sessions_slot("safety", mode_now) {
                                Ok(()) => {
                                    tracing::debug!("safety poll: {active} loaded, none running, {}s of silence", silence.as_secs());
                                    Some((Source::Safety, jf.sessions().await))
                                }
                                Err(_) => None,
                            }
                        }
                        false => None, // only so the settings are read again now and then
                    },
                    _ = app.wake.notified() => None,
                }
            }
        };

        if forget_socket {
            sock = None;
        }

        let Some((source, result)) = step else { continue };

        match result {
            Ok(sessions) => {
                if source == Source::Poll {
                    if failures > 0 {
                        tracing::info!("connection to Jellyfin restored");
                    }
                    failures = 0;
                }
                if let Err(e) = tick(&app, &mut tracked, &mut devices_seen, &sessions, settings.merge_window_s, settings.group_window_s).await {
                    tracing::error!("collector tick failed: {e:#}");
                }
                // What this list means for the transport. Every reading decides, pushed or asked for.
                let was = session_mode(socket_live, mode, active);
                (active, playing) = activity(&sessions);
                mode = decide.see(active, playing, socket_live);
                let now_mode = session_mode(socket_live, mode, active);
                // The change of transport happens here, in order, before it is published: the asking
                // has stopped by the time this pass ends, so the subscription goes out now and the new
                // state is written only once the wire agrees with it. A mode announced before its
                // frame has been sent is a mode that is not true yet.
                if was != now_mode
                    && let Some(handle) = sock.as_ref()
                {
                    let want = should_subscribe(playing, decide.settling());
                    handle.listen(want);
                    if !handle.settled(want, SETTLE_WIRE).await {
                        tracing::debug!("the socket did not {} within {:?}", if want { "subscribe" } else { "go quiet" }, SETTLE_WIRE);
                    }
                }
                // One line per change of state, with the reason. "fallback" says nothing here: it is
                // announced where it is decided, by the socket going down or failing to prove itself.
                if trusted_unproven && source == Source::Push {
                    trusted_unproven = false;
                    if was != now_mode {
                        tracing::info!("the first pushed list says {now_mode}, where the catch-up read said {was} — following the push");
                    }
                }
                if source == Source::Safety && was != now_mode {
                    tracing::info!("safety poll: {active} loaded, {playing} running → {now_mode}");
                }
                match (was == now_mode, now_mode) {
                    (true, _) | (_, "fallback") => {}
                    (_, "playing_poll") => tracing::info!("session {} resumed → polling again", resumed_by(&sessions)),
                    (_, "paused_socket") => tracing::info!("all sessions paused ({active}) for {PAUSE_DEBOUNCE} readings → listening on the socket"),
                    (_, "idle_socket") => tracing::info!("no active sessions → idle on the socket"),
                    _ => {}
                }
                let beat = poll_interval_s(settings.active_interval_s, settings.idle_interval_s, playing, active, !socket_live);
                publish(&app, seen(&sock, socket_live, mode, active, beat, reads_in_mode(mode_at)), &mut mode_since, &mut mode_at, &mut bad_since);
                let mut st = app.collector.write().unwrap();
                st.connected = true;
                st.error = None;
                st.last_poll_at = db::now();
                st.active_sessions = tracked.len();
                // A safety read is made *while* listening, so it is not the transport changing.
                st.transport = if source == Source::Poll { "poll" } else { "socket" };
                st.socket_live = socket_live;
                // While something plays the list comes from a poll and the socket is still carrying:
                // only a socket that is *not* carrying has anything to explain.
                st.socket_error = socket_error.clone();
            }
            Err(e) => {
                failures += 1;
                if failures == 1 || failures % 60 == 0 {
                    tracing::warn!("cannot read sessions from Jellyfin: {e:#}");
                }
                let mut st = app.collector.write().unwrap();
                st.connected = false;
                st.error = Some(format!("{e:#}"));
                st.transport = "poll";
                st.socket_live = socket_live;
                st.socket_error = socket_error.clone();
            }
        }

        // The socket branch has already waited inside its own select; only the timer sleeps here.
        // Close watching while something plays (pauses, seeks and track switches are caught as they
        // happen), a slower beat while nothing does. The idle beat is also how late a new play can be
        // noticed, which is watch time lost, so it stays short. Only an unreachable Jellyfin backs off
        // (up to a minute).
        if !on_socket {
            // Something running is watched closely. Otherwise this is the fallback: the socket ought to
            // be carrying and is not, so the beat is slower than a second — a paused play changes when
            // a person does something, and an idle server is why the socket exists. It is never
            // hurried past the owner's own intervals, only slowed.
            let base = poll_interval_s(settings.active_interval_s, settings.idle_interval_s, playing, active, !socket_live) as u64;
            let wait = Duration::from_secs(if failures > 0 { (base * failures.min(12) as u64).min(60) } else { base });
            // A socket that comes back must be heard while finstats is polling, or it never gets a
            // second chance: this is the only place a waiting poller listens to it.
            match sock.as_mut() {
                None => {
                    tokio::select! {
                        _ = app.wake.notified() => {}
                        _ = tokio::time::sleep(wait) => {}
                    }
                }
                Some(handle) => {
                    tokio::select! {
                        _ = app.wake.notified() => {}
                        _ = tokio::time::sleep(wait) => {}
                        event = handle.recv() => match event {
                            // This is the only place a waiting poller hears that the socket is up.
                            // The picture is *kept*: with a server that pushes on change there may
                            // not be another for hours, so dropping this one could mean missing the
                            // play it announces until something else happens.
                            // Subscribed, the server answering, nothing to push: believed, and the
                            // next pass — a poll, since the mode has not moved — is the catch-up read
                            // that says what to do. That is how an idle server reaches idle_socket
                            // without ever being sent a list.
                            Some(crate::socket::Event::Trusted) => {
                                if !socket_live {
                                    tracing::info!("socket: trusting the subscription; reading the sessions once to see where we are");
                                }
                                (socket_live, socket_error, trusted_unproven) = (true, None, true);
                            }
                            Some(crate::socket::Event::Snapshot(sessions)) => {
                                if socket_error.is_some() || !socket_live {
                                    tracing::info!("Jellyfin is pushing what is playing");
                                }
                                (socket_live, socket_error) = (true, None);
                                net.pushed(Instant::now());
                                pending = Some(sessions);
                                let mut st = app.collector.write().unwrap();
                                (st.socket_live, st.socket_error) = (true, None);
                            }
                            Some(crate::socket::Event::Alive) => {}
                            Some(crate::socket::Event::Down(why)) => {
                                // Worth saying even though this pass was asking anyway: once the play
                                // ends there is nothing left to hear it from, and a socket that dies
                                // quietly is how 1.4.0 hid for a day.
                                if socket_error.as_deref() != Some(why.as_str()) && socket_live {
                                    match active > 0 && playing == 0 {
                                        true => tracing::info!("socket lost while paused ({why}) → catch-up poll, then every {PAUSED_POLL_S}s"),
                                        false => tracing::info!("Jellyfin is no longer pushing what is playing ({why}); checking on a timer again"),
                                    }
                                }
                                (socket_live, socket_error) = (false, Some(why));
                            }
                            None => forget_socket = true,
                        },
                    }
                }
            }
            if forget_socket {
                sock = None;
            }
        }
    }
}

async fn tick(
    app: &App,
    tracked: &mut HashMap<String, Tracked>,
    devices_seen: &mut HashMap<(String, String), Instant>,
    sessions: &[Value],
    merge_window_s: i64,
    group_window_s: i64,
) -> Result<()> {
    let now = db::now();
    let tick_at = Instant::now();
    let mut seen: Vec<String> = Vec::new();
    let mut device_rows: Vec<(String, String, Option<String>, Option<String>, Option<String>, Option<String>)> = vec![];

    for s in sessions {
        // Remember every device that talks to the server, playing or not.
        if let (Some(dev), Some(uid)) = (s["DeviceId"].as_str(), s["UserId"].as_str()) {
            let k = (dev.to_string(), norm_id(uid));
            if devices_seen.get(&k).is_none_or(|t| t.elapsed() > DEVICE_REFRESH) {
                devices_seen.insert(k.clone(), tick_at);
                device_rows.push((k.0, k.1, opt_str(&s["DeviceName"]), opt_str(&s["Client"]), opt_str(&s["ApplicationVersion"]), opt_str(&s["RemoteEndPoint"])));
            }
        }

        let Some(mut rec) = record_from_session(s, now) else { continue };
        let key = format!("{}:{}", s["Id"].as_str().unwrap_or(rec.device_id.as_deref().unwrap_or("?")), rec.item_id);
        let is_paused = s["PlayState"]["IsPaused"].as_bool().unwrap_or(false);
        let transcode_progress = s["TranscodingInfo"]["CompletionPercentage"].as_f64().map(|p| (p / 100.0).clamp(0.0, 1.0));
        seen.push(key.clone());

        if let Some(t) = tracked.get_mut(&key) {
            // Cap the step so a stalled poll loop or suspended host is not counted as viewing.
            let dt = tick_at.duration_since(t.last_tick).as_secs_f64().min(90.0);
            let (watched, paused) = attribute(dt, t.is_paused);
            (t.watched, t.paused) = (t.watched + watched, t.paused + paused);
            t.last_tick = tick_at;
            let pause_flipped = t.is_paused != is_paused;
            t.is_paused = is_paused;
            t.transcode_progress = transcode_progress;

            // Keep identity, start and counters; take everything that can change mid-play.
            rec.started_at = t.rec.started_at;
            rec.start_position_s = t.rec.start_position_s;
            rec.pause_count = t.rec.pause_count;
            rec.seek_count = t.rec.seek_count;
            if rec.transcode.is_none() {
                rec.transcode = t.rec.transcode.clone();
            }
            rec.duration_s = t.watched.round() as i64;
            rec.paused_s = t.paused.round() as i64;
            let was_paused = is_paused != pause_flipped;
            // Once a play has needed transcoding it stays a transcode in the statistics — and this
            // has to happen *before* the comparison below, not after it. Applied after, the kept
            // record said "Transcode" while every reading that followed said what the client had
            // settled back to, so each one looked like a change: one `transcode` event per second
            // for the rest of the play, every one of them reading "DirectPlay: <the old reasons>",
            // a line that contradicts itself and was never true: 144 of them on one play.
            if t.rec.play_method == "Transcode" {
                rec.play_method = "Transcode".into();
            }
            let events = diff_events(&t.rec, &mut rec, was_paused, is_paused, dt, now);
            t.rec = rec;

            if !events.is_empty() || t.last_persist.elapsed() >= PERSIST_EVERY {
                t.last_persist = tick_at;
                let (row_id, rec) = (t.row_id, t.rec.clone());
                app.db
                    .call(move |c| {
                        rec.update_progress(c, row_id)?;
                        insert_events(c, row_id, &events)
                    })
                    .await?;
            }
        } else {
            let probe = rec.clone();
            let (row_id, started_at, watched, paused, counters) = app
                .db
                .call(move |c| {
                    // Same person, same item, same device, moments later: that's one viewing.
                    let resumed = c
                        .query_row(
                            "SELECT id, started_at, duration_s, paused_s, pause_count, seek_count, start_position_s FROM playbacks
                             WHERE source = 'live' AND active = 0 AND user_id = ?1 AND item_id = ?2
                               AND device_id IS ?3 AND ended_at >= ?4
                             ORDER BY ended_at DESC LIMIT 1",
                            params![probe.user_id, probe.item_id, probe.device_id, now - merge_window_s],
                            |r| {
                                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?,
                                    (r.get::<_, i64>(4)?, r.get::<_, i64>(5)?, r.get::<_, Option<i64>>(6)?)))
                            },
                        )
                        .optional()?;
                    let start = |detail: Option<&str>| PlayEvent { at: now, kind: "start", position_s: probe.position_s, detail: detail.map(str::to_string) };
                    if let Some((id, started, dur, paused, counters)) = resumed.filter(|_| merge_window_s > 0) {
                        c.execute("UPDATE playbacks SET active = 1 WHERE id = ?1", [id])?;
                        insert_events(c, id, &[start(Some("Continued after a short break"))])?;
                        return Ok((id, started, dur as f64, paused as f64, Some(counters)));
                    }
                    let id = probe.insert(c)?.expect("live rows have no source_id and cannot collide");
                    insert_events(c, id, &[start(None)])?;
                    Ok((id, probe.started_at, 0.0, 0.0, None))
                })
                .await?;
            rec.started_at = started_at;
            if let Some((pauses, seeks, start_position)) = counters {
                rec.pause_count = pauses;
                rec.seek_count = seeks;
                rec.start_position_s = start_position;
            }
            tracing::info!("{} started {} on {}", rec.user_name, rec.item_name, rec.device_name.as_deref().unwrap_or("unknown device"));
            // A new sighting: is this account somewhere it cannot be?
            let (checker, who) = (app.clone(), rec.user_id.clone());
            tokio::spawn(async move { crate::security::check(&checker, Some(who)).await });
            tracked.insert(
                key,
                Tracked { row_id, rec, watched, paused, is_paused, last_tick: tick_at, last_persist: tick_at, transcode_progress },
            );
        }
    }

    // Whatever is tracked but no longer reported has ended.
    let ended: Vec<String> = tracked.keys().filter(|k| !seen.contains(k)).cloned().collect();
    for key in ended {
        let mut t = tracked.remove(&key).expect("key came from the map");
        t.rec.active = false;
        t.rec.duration_s = t.watched.round() as i64;
        t.rec.paused_s = t.paused.round() as i64;
        let (row_id, rec) = (t.row_id, t.rec);
        tracing::info!("{} stopped {} after {}s", rec.user_name, rec.item_name, rec.duration_s);
        app.db
            .call(move |c| {
                if rec.duration_s < MIN_KEEP_S {
                    c.execute("DELETE FROM playbacks WHERE id = ?1", [row_id])?;
                } else {
                    rec.update_progress(c, row_id)?;
                    insert_events(c, row_id, &[PlayEvent { at: rec.ended_at, kind: "stop", position_s: rec.position_s, detail: None }])?;
                }
                // Now that its length is known: was this one watched together with someone?
                crate::groups::detect(c, group_window_s, Some(&rec.item_id))?;
                Ok(())
            })
            .await?;
    }

    if !device_rows.is_empty() {
        app.db
            .call(move |c| {
                let mut stmt = c.prepare_cached(
                    "INSERT INTO devices(device_id, user_id, device_name, client, app_version, last_ip, first_seen, last_seen)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
                     ON CONFLICT(device_id, user_id) DO UPDATE SET
                        device_name = excluded.device_name, client = excluded.client,
                        app_version = excluded.app_version, last_ip = excluded.last_ip, last_seen = excluded.last_seen",
                )?;
                for d in device_rows {
                    stmt.execute(params![d.0, d.1, d.2, d.3, d.4, d.5, now])?;
                }
                Ok(())
            })
            .await?;
    }

    let mut live: Vec<Value> = tracked.iter().map(|(k, t)| live_json(k, t)).collect();
    live.sort_by_key(|v| v["started_at"].as_i64().unwrap_or(0));
    *app.live.write().unwrap() = live;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// One session, as Jellyfin lists it. `None` for somebody with nothing loaded.
    fn sess(user: &str, item: Option<&str>, paused: bool) -> Value {
        match item {
            None => json!({ "UserName": user, "Id": user }),
            Some(name) => json!({ "UserName": user, "Id": user, "NowPlayingItem": { "Id": name, "Name": name }, "PlayState": { "IsPaused": paused, "PositionTicks": 10 } }),
        }
    }

    /// Feed a run of readings to one decider, as the loop does, and collect the mode after each.
    fn run(readings: &[(Vec<Value>, bool)]) -> Vec<Mode> {
        let mut d = Decide::default();
        readings
            .iter()
            .map(|(list, live)| {
                let (active, playing) = activity(list);
                d.see(active, playing, *live)
            })
            .collect()
    }

    #[test]
    fn a_play_that_is_paused_hands_over_to_the_socket_and_takes_it_back_on_resume() {
        let playing = vec![sess("alice", Some("Big Buck Bunny"), false)];
        let paused = vec![sess("alice", Some("Big Buck Bunny"), true)];
        let stopped: Vec<Value> = vec![sess("alice", None, false)];
        // play · pause ×3 (the debounce) · resume · stop
        let modes = run(&[
            (playing.clone(), true),
            (paused.clone(), true),
            (paused.clone(), true),
            (paused.clone(), true),
            (playing.clone(), true),
            (stopped.clone(), true),
        ]);
        assert_eq!(modes, [Mode::Poll, Mode::Poll, Mode::Poll, Mode::Listen, Mode::Poll, Mode::Listen]);
        assert_eq!(activity(&paused), (1, 0));
        assert_eq!(activity(&stopped), (0, 0), "nothing loaded is not an active session");
    }

    #[test]
    fn one_person_watching_keeps_everyone_polling() {
        let a_paused_b_playing = vec![sess("alice", Some("Big Buck Bunny"), true), sess("bob", Some("Sintel"), false)];
        let both_paused = vec![sess("alice", Some("Big Buck Bunny"), true), sess("bob", Some("Sintel"), true)];
        let a_back = vec![sess("alice", Some("Big Buck Bunny"), false), sess("bob", Some("Sintel"), true)];
        let modes = run(&[
            (a_paused_b_playing.clone(), true),
            (a_paused_b_playing.clone(), true),
            (a_paused_b_playing.clone(), true),
            (a_paused_b_playing.clone(), true),
            (both_paused.clone(), true),
            (both_paused.clone(), true),
            (both_paused.clone(), true),
            (a_back.clone(), true),
        ]);
        assert_eq!(&modes[..4], [Mode::Poll; 4], "one person watching is watching, whoever else paused");
        assert_eq!(&modes[4..7], [Mode::Poll, Mode::Poll, Mode::Listen], "and only once both have been paused a while");
        assert_eq!(modes[7], Mode::Poll, "either of them starting again takes it back");
    }

    #[test]
    fn somebody_new_starting_while_the_rest_are_paused_takes_it_back() {
        let paused = vec![sess("alice", Some("Big Buck Bunny"), true)];
        let joined = vec![sess("alice", Some("Big Buck Bunny"), true), sess("bob", Some("Sintel"), false)];
        let modes = run(&[(paused.clone(), true), (paused.clone(), true), (paused.clone(), true), (joined, true)]);
        assert_eq!(modes, [Mode::Poll, Mode::Poll, Mode::Listen, Mode::Poll]);
    }

    #[test]
    fn a_pause_and_a_change_of_mind_never_changes_transport() {
        let playing = vec![sess("alice", Some("Big Buck Bunny"), false)];
        let paused = vec![sess("alice", Some("Big Buck Bunny"), true)];
        // Paused for two readings, then off again: below the debounce, so nothing moves.
        let modes = run(&[(playing.clone(), true), (paused.clone(), true), (paused.clone(), true), (playing.clone(), true), (paused.clone(), true), (playing.clone(), true)]);
        assert!(modes.iter().all(|m| *m == Mode::Poll), "{modes:?}");
    }

    #[test]
    fn without_a_socket_there_is_nothing_to_hand_over_to() {
        let paused = vec![sess("alice", Some("Big Buck Bunny"), true)];
        let nothing: Vec<Value> = vec![];
        // The socket drops while everything is paused: asking is all that is left, and the catch-up
        // reading decides the mode again the moment it comes back.
        let modes = run(&[(paused.clone(), true), (paused.clone(), true), (paused.clone(), true), (paused.clone(), false), (paused.clone(), true)]);
        assert_eq!(modes, [Mode::Poll, Mode::Poll, Mode::Listen, Mode::Poll, Mode::Listen]);
        assert_eq!(run(&[(nothing.clone(), false)]), [Mode::Poll], "and an idle server with no socket is polled as ever");
        assert_eq!(run(&[(nothing, true)]), [Mode::Listen]);
    }

    #[test]
    fn when_the_paused_sessions_end_it_is_simply_idle() {
        let paused = vec![sess("alice", Some("Big Buck Bunny"), true)];
        let gone: Vec<Value> = vec![];
        let modes = run(&[(paused.clone(), true), (paused.clone(), true), (paused.clone(), true), (gone, true)]);
        assert_eq!(modes, [Mode::Poll, Mode::Poll, Mode::Listen, Mode::Listen], "still listening, now with nothing loaded at all");
    }

    fn shown(session_mode: &'static str, socket_connected: bool, socket_subscribed: bool, poll_interval_s: Option<i64>) -> Published {
        Published { session_mode, socket_connected, socket_subscribed, poll_interval_s, reads_in_mode: 0 }
    }

    #[test]
    fn the_state_machine_marks_its_own_work() {
        // The four states as they are meant to look, each matching what a packet capture would show.
        assert_eq!(disagrees(&shown("idle_socket", true, true, None)), None);
        assert_eq!(disagrees(&shown("paused_socket", true, true, None)), None);
        assert_eq!(disagrees(&shown("playing_poll", true, false, Some(1))), None);
        assert_eq!(disagrees(&shown("fallback", false, false, Some(30))), None);

        // And every way of being wrong that would show up on the wire.
        assert!(disagrees(&shown("idle_socket", false, true, None)).is_some_and(|w| w.contains("without a connection")));
        assert!(disagrees(&shown("paused_socket", true, false, None)).is_some_and(|w| w.contains("without a subscription")));
        assert!(disagrees(&shown("idle_socket", true, true, Some(30))).is_some_and(|w| w.contains("listening and polling at once")));
        assert!(disagrees(&shown("playing_poll", true, true, Some(1))).is_some_and(|w| w.contains("the same list would arrive twice")));
        assert!(disagrees(&shown("playing_poll", true, false, None)).is_some_and(|w| w.contains("asking for nothing")));
        assert!(disagrees(&shown("fallback", false, false, None)).is_some_and(|w| w.contains("not polling")));
        // The words of the complaint carry the values, so a line in the log is enough to see it.
        assert!(disagrees(&shown("idle_socket", true, true, Some(30))).unwrap().contains("poll Some(30)"));

        // And the one that went unnoticed through a whole storm: every word of the status was true
        // while it made 2,625 reads in eight seconds, because nothing published counted the reads.
        let asking = |mode, n| Published { reads_in_mode: n, ..shown(mode, true, true, None) };
        assert_eq!(disagrees(&asking("paused_socket", READS_WHILE_LISTENING)), None, "the catch-up and the net are allowed");
        assert!(disagrees(&asking("paused_socket", 3)).is_some_and(|w| w.contains("asking anyway")));
        assert!(disagrees(&asking("idle_socket", 400)).unwrap().contains("400 reads"));
        assert_eq!(disagrees(&Published { reads_in_mode: 60, ..shown("playing_poll", true, false, Some(1)) }), None, "a play is one read a second, and that is the point of it");
    }

    #[test]
    fn the_beat_follows_what_is_happening_and_never_hurries_the_owner() {
        let (active_s, idle_s) = (1, 5);
        assert_eq!(poll_interval_s(active_s, idle_s, 1, 1, false), 1, "something running is watched closely");
        assert_eq!(poll_interval_s(active_s, idle_s, 0, 0, false), 5, "the socket is carrying: the owner's own idle interval stands");
        assert_eq!(poll_interval_s(active_s, idle_s, 0, 0, true), 30, "the socket ought to be carrying and is not");
        assert_eq!(poll_interval_s(active_s, idle_s, 0, 2, true), 15, "everything paused, with no socket to be told by");
        assert_eq!(poll_interval_s(active_s, 45, 0, 0, true), 45, "and a slower interval of the owner's own is never hurried up");
    }

    #[test]
    fn the_mode_says_which_of_the_four_things_is_happening() {
        // Somebody watching is polling.
        assert_eq!(session_mode(true, Mode::Poll, 1), "playing_poll");
        // Nothing running, and the socket carrying: idle or paused, by whether anything is loaded.
        assert_eq!(session_mode(true, Mode::Listen, 0), "idle_socket");
        assert_eq!(session_mode(true, Mode::Listen, 2), "paused_socket");
        // The pause debounce is still `playing_poll`: it is the same polling, seeing whether the
        // pause lasts. An early attempt called it "fallback", so every pause read as a fault for
        // three seconds.
        assert_eq!(session_mode(true, Mode::Poll, 2), "playing_poll", "paused, but not for long enough yet");
        // Only a socket that cannot be used is a fallback, whatever else is going on.
        assert_eq!(session_mode(false, Mode::Listen, 0), "fallback");
        assert_eq!(session_mode(false, Mode::Poll, 1), "fallback");
    }

    #[test]
    fn the_subscription_is_off_for_the_whole_of_a_pause_debounce() {
        // An early attempt subscribed the moment a play was paused, while still polling every
        // second: the same list arrived twice for as long as the debounce ran.
        assert!(!should_subscribe(1, false), "something running");
        assert!(!should_subscribe(0, true), "paused, and still being watched to see whether it lasts");
        assert!(should_subscribe(0, false), "paused for long enough, or nothing loaded at all");

        let mut d = Decide::default();
        assert!(!d.settling(), "nothing has been seen yet");
        let paused = || (2usize, 0usize);
        for reading in 1..PAUSE_DEBOUNCE {
            d.see(paused().0, paused().1, true);
            assert!(d.settling(), "reading {reading} of the debounce");
            assert!(!should_subscribe(0, d.settling()));
        }
        d.see(paused().0, paused().1, true);
        assert!(!d.settling(), "the debounce is over");
        assert!(should_subscribe(0, d.settling()), "and only now is the subscription wanted");
    }

    #[test]
    fn the_net_falls_on_silence_and_on_the_clock_but_never_on_an_idle_server() {
        let m = |s| Duration::from_secs(s);
        // Something paused and the pushes have stopped: read once, after a minute of nothing.
        assert_eq!(safety_due(2, m(0), m(0)), PAUSED_SAFETY_SILENCE);
        assert!(safety_due(2, m(60), m(60)).is_zero(), "a minute of silence with something paused");
        // A client that keeps talking while paused resets the silence but not the clock: an early
        // attempt let it reset both, so with a chatty client the net never fell at all.
        assert_eq!(safety_due(2, m(0), m(299)), m(1), "the five-minute net is still coming");
        assert!(safety_due(2, m(0), m(300)).is_zero(), "and it falls, pushes or no pushes");
        // Nothing loaded: silence is the normal state and is never a reason to ask.
        assert_eq!(safety_due(0, m(3_600), m(0)), SAFETY_EVERY, "an hour of quiet on an idle server asks nothing");
        assert!(safety_due(0, m(0), m(300)).is_zero(), "only the plain net, once every five minutes");
        // And the floor under all of it: two reads are never closer together than this, however the
        // other clock stands. A due time that can stay at zero is the whole of the read storm.
        assert_eq!(safety_due(2, m(60), m(0)), SAFETY_MIN_GAP, "a read a moment ago is not answered with another");
        assert_eq!(safety_due(2, m(600), m(1)), SAFETY_MIN_GAP - m(1));
        assert!(!safety_due(2, m(600), m(0)).is_zero(), "there is no arrangement of the clocks that asks twice at once");
    }

    /// The whole thing on a fake clock: a second at a time, with the collector's own rules about what
    /// re-arms which clock. `push` is a client that keeps reporting while paused; `poll` is the beat
    /// while something plays. Answers the reads the net would have made.
    fn net_over(seconds: u64, active: usize, mut happens: impl FnMut(u64) -> (bool, bool)) -> Vec<u64> {
        let t0 = Instant::now();
        let mut net = Net::new(t0);
        let mut reads = vec![];
        for s in 0..seconds {
            let now = t0 + Duration::from_secs(s);
            let (push, poll) = happens(s);
            if push {
                net.pushed(now);
            }
            if poll {
                net.read(now);
            }
            if !poll && net.due(now, active).is_zero() {
                net.read(now);
                reads.push(s);
            }
        }
        reads
    }

    #[test]
    fn a_pause_after_a_long_play_asks_for_nothing_at_all() {
        // The read storm, on the clock. Three minutes of playing — polled every second, the socket
        // unsubscribed throughout, so not one push — and then a pause, and a quiet client.
        let mut net = Net::new(Instant::now());
        let t0 = Instant::now();
        for s in 0..180 {
            net.read(t0 + Duration::from_secs(s));
        }
        let paused_at = t0 + Duration::from_secs(180);
        net.subscribed(paused_at);
        assert_eq!(net.due(paused_at, 1), PAUSED_SAFETY_SILENCE, "a fresh subscription is a fresh minute, not a minute already gone");
        assert_eq!(net.due(paused_at + Duration::from_secs(59), 1), Duration::from_secs(1), "and nothing is asked for in the meantime");
        assert!(net.due(paused_at + Duration::from_secs(60), 1).is_zero(), "the net falls once, a minute in");
        // And the read it calls for re-arms the clock that called for it. It did not before, which is
        // why there were 2,625 of them in the eight seconds until a push happened along.
        net.read(paused_at + Duration::from_secs(60));
        assert_eq!(net.due(paused_at + Duration::from_secs(60), 1), PAUSED_SAFETY_SILENCE);
    }

    #[test]
    fn a_quiet_client_on_pause_is_read_once_a_minute_and_a_chatty_one_once_in_five() {
        // Nothing pushed at all for five minutes: one read a minute, never more.
        let quiet = net_over(300, 1, |_| (false, false));
        assert_eq!(quiet, [60, 120, 180, 240], "one a minute, and the minute starts again at each read");
        // A client that keeps reporting its progress while paused: silence never falls, so only the
        // plain five-minute net does — once in six minutes, not once every five seconds.
        let chatty = net_over(360, 1, |s| (s % 5 == 0, false));
        assert_eq!(chatty, [300], "exactly one, and it is the five-minute one");
    }

    #[test]
    fn a_pause_a_resume_and_a_pause_again_never_burst() {
        // Paused at 0, resumed at 30 (polled every second while it runs), paused again at 50.
        let reads = net_over(200, 1, |s| (false, (30..50).contains(&s)));
        assert_eq!(reads, [109, 169], "a minute from the last poll of the play, not a minute from the first pause");
        assert!(reads.windows(2).all(|w| w[1] - w[0] >= 60), "and never two close together");
    }

    #[test]
    fn an_idle_server_is_asked_once_in_five_minutes_and_no_more() {
        assert_eq!(net_over(1_200, 0, |_| (false, false)), [300, 600, 900], "silence is what an idle server is for");
    }

    #[test]
    fn the_gate_holds_whatever_asks_it() {
        let t0 = Instant::now();
        let mut reads = Reads::new();
        // The beat while something plays: one a second, sixty a minute, never refused.
        for s in 0..60 {
            assert!(reads.take(t0 + Duration::from_secs(s), false).is_ok(), "second {s} of a play");
        }
        assert_eq!(reads.last_min(t0 + Duration::from_secs(59)), 60, "and that is what a capture would count");

        // A bug of that kind: a caller asking as fast as it can be asked. Two a second, and
        // seventy a minute, and then nothing at all.
        let mut reads = Reads::new();
        let at = |ms| t0 + Duration::from_millis(ms);
        assert!(reads.take(at(0), true).is_ok());
        assert!(reads.take(at(1), true).is_ok());
        assert!(reads.take(at(2), true).is_err(), "two in a second is the most there is");
        assert!(reads.take(at(1_001), true).is_ok(), "and a second later there is room again");
        let mut taken = 2 + 1;
        for ms in 1_002..120_000 {
            if reads.take(at(ms), true).is_ok() {
                taken += 1;
            }
        }
        assert!(taken <= 2 * READS_PER_MIN + READS_PER_S, "two minutes of asking as fast as possible is {taken} reads");
        assert!(reads.suppressed > 1_000, "and every refusal is counted");
        assert!(reads.last_min(at(120_000)) <= READS_PER_MIN, "the minute stays full and never fuller");
    }

    #[test]
    fn the_gate_says_so_at_most_once_a_minute() {
        let t0 = Instant::now();
        let mut reads = Reads::new();
        assert_eq!(reads.report(t0), None, "nothing has been refused");
        for ms in 0..5_000 {
            let _ = reads.take(t0 + Duration::from_millis(ms), true);
        }
        let first = reads.report(t0 + Duration::from_secs(5)).expect("a refusal is worth a word");
        assert!(first > 100, "{first} refused");
        assert_eq!(reads.report(t0 + Duration::from_secs(10)), None, "and not another word for a minute");
        // Two in one instant a while later: one goes out, one is refused, and that one is reported.
        assert!(reads.take(t0 + Duration::from_secs(20), true).is_ok());
        assert!(reads.take(t0 + Duration::from_secs(20), true).is_ok());
        assert!(reads.take(t0 + Duration::from_secs(20), true).is_err());
        assert_eq!(reads.report(t0 + Duration::from_secs(65)), Some(1), "then the ones since, counted from scratch");
    }

    #[test]
    fn what_a_mode_is_answerable_for_is_what_it_asked_for_itself() {
        let t0 = Instant::now();
        let mut reads = Reads::new();
        // A minute of playing, then the pause: the reads of the play are not the pause's to answer for.
        for s in 0..60 {
            assert!(reads.take(t0 + Duration::from_secs(s), false).is_ok());
        }
        let paused_at = t0 + Duration::from_secs(60);
        assert_eq!(reads.last_min(t0 + Duration::from_millis(59_500)), 60, "though the capture still counts them, which is what /api/status shows");
        assert_eq!(reads.listening_since(paused_at, paused_at), 0, "the pause has asked for nothing yet");
        assert!(reads.take(paused_at + Duration::from_millis(1), true).is_ok());
        assert_eq!(reads.listening_since(paused_at + Duration::from_secs(1), paused_at), 1);
    }

    #[test]
    fn nothing_running_means_subscribed_whatever_the_collector_is_doing() {
        assert!(should_subscribe(0, false), "an idle server: the pushes are how a play is ever noticed");
        assert!(should_subscribe(0, false), "everything paused: nothing to push about, and nothing asked for");
        assert!(!should_subscribe(1, false), "one person watching: the pushes would be the poll a second time");
        assert!(!should_subscribe(3, false));
        // The bug this whole rule is for: it cannot be decided from the mode. Reaching the
        // listening mode needs the socket to have proved itself, the proof is the list a
        // subscription brings, and unsubscribing first meant it never came. So with nothing running
        // it is subscribed even from the fallback.
        assert_eq!(session_mode(false, Mode::Poll, 0), "fallback");
        assert!(should_subscribe(0, false), "and it is still subscribed, which is the only way out of the fallback");
    }

    #[test]
    fn a_gap_while_paused_is_paused_time_however_long_it_is() {
        assert_eq!(attribute(1.0, false), (1.0, 0.0), "a second of watching");
        assert_eq!(attribute(60.0, true), (0.0, 60.0), "a minute between two readings of a paused play");
        assert_eq!(attribute(90.0, true), (0.0, 90.0), "and the longest gap there can be");
        // The whole point: however rarely a paused play is looked at, none of it is watch time.
        let (mut watched, mut paused) = (0.0, 0.0);
        for dt in [1.0, 60.0, 60.0, 60.0] {
            let (w, p) = attribute(dt, true);
            (watched, paused) = (watched + w, paused + p);
        }
        assert_eq!((watched, paused), (0.0, 181.0));
    }

    #[test]
    fn the_name_in_the_log_is_whoever_started_again() {
        let list = vec![sess("alice", Some("Big Buck Bunny"), true), sess("bob", Some("Sintel"), false)];
        assert_eq!(resumed_by(&list), "bob/Sintel");
        assert_eq!(resumed_by(&[sess("alice", Some("Big Buck Bunny"), true)]), "somebody", "nobody has");
    }

    fn rec(position: i64) -> PlayRecord {
        PlayRecord { position_s: Some(position), play_method: "DirectPlay".into(), ..Default::default() }
    }

    #[test]
    fn steady_playback_is_not_a_seek() {
        let mut new = rec(105);
        assert!(diff_events(&rec(100), &mut new, false, false, 5.0, 0).is_empty());
        assert_eq!(new.seek_count, 0);
    }

    #[test]
    fn a_play_that_settles_back_to_direct_is_not_a_transcode_every_second() {
        // The bug as it was recorded: the kept record was forced to "Transcode" after the
        // comparison, so it disagreed with every reading that followed and each one wrote another
        // event — 144 of them on one play, all saying "DirectPlay: ContainerBitrateExceedsLimit".
        let reasons = json!({ "reasons": ["ContainerBitrateExceedsLimit"] });
        let transcoding = PlayRecord { play_method: "Transcode".into(), transcode: Some(reasons.clone()), position_s: Some(100), ..Default::default() };
        // What the session says once the client has settled: direct play, the reasons still attached.
        let settled = || PlayRecord { play_method: "DirectPlay".into(), transcode: Some(reasons.clone()), position_s: Some(101), ..Default::default() };

        // Sticky first, as the collector now does it, and the readings agree: nothing to report.
        let mut new = settled();
        new.play_method = "Transcode".into();
        assert!(diff_events(&transcoding, &mut new, false, false, 1.0, 0).is_empty());
        // The same reading a second later, and the one after that: still nothing.
        let mut later = PlayRecord { position_s: Some(102), ..settled() };
        later.play_method = "Transcode".into();
        assert!(diff_events(&new, &mut later, false, false, 1.0, 0).is_empty());

        // Sticky *after* the comparison, which is what it used to do: every reading is a change.
        let mut unsticky = settled();
        let kinds: Vec<_> = diff_events(&transcoding, &mut unsticky, false, false, 1.0, 0).into_iter().map(|e| e.kind).collect();
        assert_eq!(kinds, ["transcode"], "this is the event that used to repeat for ever");

        // And a play that really does start transcoding still says so, once.
        let direct = PlayRecord { play_method: "DirectPlay".into(), position_s: Some(100), ..Default::default() };
        let mut starts = PlayRecord { position_s: Some(101), ..transcoding.clone() };
        let out = diff_events(&direct, &mut starts, false, false, 1.0, 0);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].detail.as_deref(), Some("Transcode: ContainerBitrateExceedsLimit"));
        // A reason changing is news; the same reason again is not.
        let mut same = PlayRecord { position_s: Some(102), ..transcoding.clone() };
        assert!(diff_events(&starts, &mut same, false, false, 1.0, 0).is_empty());
        let mut other = PlayRecord { transcode: Some(json!({ "reasons": ["VideoCodecNotSupported"] })), position_s: Some(103), ..transcoding.clone() };
        assert_eq!(diff_events(&same, &mut other, false, false, 1.0, 0).len(), 1, "a different reason is worth a line");
    }

    #[test]
    fn jumps_pauses_and_track_switches_are_events() {
        let mut new = rec(900);
        new.streams.subtitle_language = Some("eng".into());
        let kinds: Vec<_> = diff_events(&rec(100), &mut new, false, true, 5.0, 0).into_iter().map(|e| e.kind).collect();
        assert_eq!(kinds, ["seek", "pause", "subtitle"]);
        assert_eq!((new.seek_count, new.pause_count), (1, 1));

        // Sitting on pause does not drift into a "seek".
        let mut still = rec(900);
        assert!(diff_events(&rec(900), &mut still, true, true, 60.0, 0).is_empty());
    }
}
