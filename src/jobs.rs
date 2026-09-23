//! Jellyfin's own background jobs: what they are, in plain words, and how far along they are.
//!
//! Jellyfin lists its scheduled tasks under names like "Detect and Analyze Media Segments", which says
//! what the code is called rather than what it is doing to your server. `explain` answers the question
//! the name does not: what it goes through, why you would want it, and whether it is one of the heavy
//! ones. Unknown tasks (a plugin's own) fall back to Jellyfin's description, and say where they came from.
//!
//! **A run is timed by watching it, because Jellyfin does not say when it started.** A task carries a
//! percentage and nothing else — no start time for the run in progress — so `Watch` remembers the
//! percentages this finstats has seen and `eta_s` works the rest out from the rate they moved at, falling
//! back to how long the last run took. It is therefore an estimate that gets better the longer a page is
//! open, and it says so rather than pretending.
//!
//! **Nothing is asked for unless somebody is looking.** The list is read from Jellyfin at most every
//! `MIN_GAP_S`, only when this endpoint is called, and finstats never starts, stops or changes a task
//! there: it is the same read-only relationship as everywhere else.

use std::collections::HashMap;

use serde_json::{Value, json};

use crate::auth::ServerViewer;
use crate::db;
use crate::state::{ApiResult, App};

/// Jellyfin is asked for its task list at most this often, however many people are watching.
const MIN_GAP_S: i64 = 3;
/// .NET ticks: 10 million to the second.
const TICKS_PER_S: i64 = 10_000_000;

// ---------------------------------------------------------------- what a job actually does

/// The plain-English sentence for a task, by Jellyfin's key first (which does not change with the
/// server's language) and by its name second. `None` when finstats has never heard of it.
pub fn explain(key: &str, name: &str) -> Option<&'static str> {
    let k = key.to_ascii_lowercase();
    let n = name.to_ascii_lowercase();
    let is = |needles: &[&str]| needles.iter().any(|needle| k == *needle || n.contains(needle));

    if is(&["refreshlibrary", "scan media library"]) {
        return Some("Looks through your media folders for files that are new, changed or gone and updates Jellyfin's database. This is the job that makes a new film appear. finstats reads your library right after it finishes, so anything it finds is on your statistics within a minute or two.");
    }
    if is(&["mediasegment", "media segments", "intro skip", "introskipper"]) {
        return Some("Goes through your episodes looking for the parts a player can offer to skip — the intro, a recap, the closing credits — and writes down where they are. It reads the audio and video of each file to do it, so it is one of the heaviest things Jellyfin ever runs, and it only looks at what it has not analysed before. Leaving it to run overnight is normal; nothing about it touches your library files.");
    }
    if is(&["refreshtrickplay", "trickplay"]) {
        return Some("Builds the strip of little pictures you see when you drag the progress bar. It decodes every video once and stores the thumbnails next to them, so the first run on a large library takes hours and costs disk space. Later runs only handle what is new.");
    }
    if is(&["refreshchapterimages", "chapter image"]) {
        return Some("Makes the thumbnail for each chapter of a film, so chapter menus have pictures instead of numbers. It seeks into every file that has chapters; new files only, after the first run.");
    }
    if is(&["keyframeextraction", "keyframe"]) {
        return Some("Notes the points in each file a player is allowed to jump to. With them, dragging the progress bar of a video that is being transcoded lands where you let go instead of a few seconds off.");
    }
    if is(&["audionormalization", "audio normalization", "audio normalisation"]) {
        return Some("Measures how loud each file is, so a player can even out the difference between a quiet film and a loud one. It reads the audio of every file it has not measured yet.");
    }
    if is(&["refreshpeople", "refresh people"]) {
        return Some("Fetches the details and pictures of the actors and directors in your library from the metadata providers you have switched on. It talks to the internet; it does not touch your files.");
    }
    if is(&["refreshguide", "refresh guide"]) {
        return Some("Downloads the Live TV programme guide for the next few days from whichever listings provider you have set up. Only does anything if you have Live TV.");
    }
    if is(&["optimizedatabase", "optimize database", "optimise database"]) {
        return Some("Tidies Jellyfin's own database file: reclaims the space deleted rows left behind and rebuilds the indexes. Jellyfin is briefly slower while it runs and quicker afterwards. It has nothing to do with your media.");
    }
    if is(&["cleanactivitylog", "activity log"]) {
        return Some("Deletes old entries from Jellyfin's activity log — the sign-ins and errors you see under Dashboard → Activity. finstats has already copied those into its own history, so nothing on the finstats Server page disappears with them.");
    }
    if is(&["cleanlogs", "log file", "log files"]) {
        return Some("Deletes Jellyfin's own log files once they are older than the number of days set in its settings. Housekeeping; it never touches media.");
    }
    if is(&["cleancache", "cache director", "cache file"]) {
        return Some("Empties the parts of Jellyfin's cache folder nothing needs any more. Safe, quick, and it only frees disk space.");
    }
    if is(&["cleantranscode", "deletetranscodefiles", "transcode director", "transcoding temp"]) {
        return Some("Deletes the temporary files left behind when Jellyfin transcodes something — the pieces of video it made for a player that could not handle the original. They can be many gigabytes on a busy server.");
    }
    if is(&["pluginupdate", "plugin update", "check for plugin"]) {
        return Some("Asks the plugin repositories whether any of your plugins has a newer version, and installs the updates if you asked it to. This one talks to the internet on your server's behalf.");
    }
    if is(&["subtitle"]) {
        return Some("Deals with subtitles: fetching the ones you asked for from the providers you enabled, or pulling the ones already inside a video file out so a player can switch them on without transcoding.");
    }
    if is(&["thumbnail", "image extraction"]) {
        return Some("Pulls a still out of each video to use as its picture where there is no artwork from a metadata provider.");
    }
    if is(&["systemupdate", "check for updates", "update task"]) {
        return Some("Checks whether a newer Jellyfin has been released. It talks to the internet; it installs nothing by itself unless you told it to.");
    }
    if is(&["userdata", "people images", "extract"]) {
        return None; // too vague to guess at: Jellyfin's own description is better than a wrong sentence
    }
    None
}

/// What finstats says about a task it does not know: Jellyfin's own description, when there is one.
fn fallback(description: Option<&str>) -> String {
    match description.map(str::trim).filter(|d| !d.is_empty()) {
        Some(d) => format!("{d} (Jellyfin's own words: finstats does not know this job, so it is probably from a plugin.)"),
        None => "finstats does not know this job and Jellyfin does not describe it, which usually means a plugin added it.".into(),
    }
}

// ---------------------------------------------------------------- how long is left

/// What is left of a run, in seconds.
///
/// Two ways, in order. **The rate it has actually been moving at** while finstats watched (at least
/// `MIN_GAIN` over `MIN_WINDOW_S`), measured over a recent window rather than the whole run, so a job
/// that sped up or slowed down is believed rather than averaged away. Failing that, **how long the last
/// run took**, applied to the fraction that is left — *minus the time the percentage has already been
/// standing still*, because an estimate that does not age is what makes a stuck job read "about 2
/// minutes left" for a quarter of an hour. When that borrowed time runs out there is nothing honest left
/// to say, and this answers `None`.
pub fn eta_s(progress: f64, gained: f64, over_s: i64, last_duration_s: Option<i64>, stalled_s: i64) -> Option<i64> {
    const MAX_ETA_S: i64 = 30 * 86_400;
    let left = (100.0 - progress.clamp(0.0, 100.0)).max(0.0);
    if left < 0.05 {
        return Some(0);
    }
    if gained >= MIN_GAIN && over_s >= MIN_WINDOW_S {
        let per_s = gained / over_s as f64;
        return Some(((left / per_s).round() as i64).clamp(0, MAX_ETA_S));
    }
    let borrowed = last_duration_s.filter(|d| *d > 0)? ;
    let rest = ((left / 100.0) * borrowed as f64).round() as i64 - stalled_s.max(0);
    (rest > 0).then(|| rest.clamp(0, MAX_ETA_S))
}

// ---------------------------------------------------------------- when it runs

fn hhmm(ticks: i64) -> String {
    let total = (ticks / TICKS_PER_S).rem_euclid(86_400);
    format!("{:02}:{:02}", total / 3600, (total % 3600) / 60)
}

fn every(seconds: i64) -> String {
    match seconds {
        86_400 => "every day".into(),
        3_600 => "every hour".into(),
        s if s % 86_400 == 0 && s >= 86_400 => format!("every {} days", s / 86_400),
        s if s % 3_600 == 0 && s >= 3_600 => format!("every {} hours", s / 3600),
        s if s >= 60 => format!("every {} minutes", (s as f64 / 60.0).round() as i64),
        s => format!("every {s} seconds"),
    }
}

const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

/// When Jellyfin will run this, in the words its own settings page uses. A time of day is the *server's*
/// local time, which is why it is printed as a time rather than turned into a countdown finstats cannot
/// promise; an interval is a real countdown, because it is measured from the last run.
pub fn schedule(triggers: &[Value], last_run_at: Option<i64>) -> (Vec<String>, Option<i64>) {
    let mut words = vec![];
    let mut next_at: Option<i64> = None;
    for t in triggers {
        let kind = t["Type"].as_str().unwrap_or("");
        let ticks = |key: &str| t[key].as_i64();
        match kind {
            "DailyTrigger" => words.push(format!("every day at {}", hhmm(ticks("TimeOfDayTicks").unwrap_or(0)))),
            "WeeklyTrigger" => {
                let day = t["DayOfWeek"].as_str().map(str::to_string).or_else(|| t["DayOfWeek"].as_i64().and_then(|d| DAYS.get(d as usize).map(|s| s.to_string())));
                words.push(format!("every {} at {}", day.unwrap_or_else(|| "week".into()), hhmm(ticks("TimeOfDayTicks").unwrap_or(0))));
            }
            "IntervalTrigger" => {
                let seconds = ticks("IntervalTicks").unwrap_or(0) / TICKS_PER_S;
                if seconds > 0 {
                    words.push(every(seconds));
                    if let Some(last) = last_run_at {
                        let due = last + seconds;
                        next_at = Some(next_at.map_or(due, |n: i64| n.min(due)));
                    }
                }
            }
            "StartupTrigger" => words.push("when Jellyfin starts".into()),
            "" => {}
            other => words.push(other.trim_end_matches("Trigger").to_ascii_lowercase()),
        }
    }
    (words, next_at)
}

// ---------------------------------------------------------------- watching a run

/// What finstats has seen of one run. Jellyfin reports a percentage and never says when the run began,
/// so this is the only clock there is: the recent percentages, and when the last one actually changed.
pub struct Run {
    /// When finstats first saw this run — which may be long after Jellyfin started it.
    pub first_at: i64,
    /// When the percentage last moved. Equal to `first_at` until it does.
    pub changed_at: i64,
    /// (when, percentage), oldest first, kept for at most `WINDOW_S`.
    samples: Vec<(i64, f64)>,
}

/// How far back a rate is measured. Long enough for a job that moves a percent every few minutes to be
/// measurable at all, short enough that an hour-old rate is not used to describe what is happening now.
const WINDOW_S: i64 = 900;
/// A percentage that moves by less than this has not moved: Jellyfin's numbers jitter in the last digits.
const MOVED: f64 = 0.01;
/// Movement worth measuring a rate from, and the shortest stretch it may be measured over.
const MIN_GAIN: f64 = 0.5;
const MIN_WINDOW_S: i64 = 5;

impl Run {
    fn new(at: i64, progress: f64) -> Run {
        Run { first_at: at, changed_at: at, samples: vec![(at, progress)] }
    }

    fn note(&mut self, at: i64, progress: f64) {
        if self.samples.last().is_none_or(|(_, p)| (progress - p).abs() > MOVED) {
            self.changed_at = at;
        }
        self.samples.push((at, progress));
        // Only the recent past decides the rate, but never fewer than two points to measure between.
        let cutoff = at - WINDOW_S;
        let keep = self.samples.iter().position(|(t, _)| *t >= cutoff).unwrap_or(0);
        if keep > 0 && self.samples.len() - keep >= 2 {
            self.samples.drain(..keep);
        }
    }

    /// (how much the percentage gained, over how many seconds), measured against the *most recent* point
    /// far enough back to say anything: a job moving quickly is described by the last few seconds, a slow
    /// one by as much of the window as it takes to see it move at all. Averaging over the whole run
    /// instead would describe a job that has changed pace by the pace it no longer has.
    fn measured(&self) -> (f64, i64) {
        let Some(&(t1, p1)) = self.samples.last() else { return (0.0, 0) };
        let anchor = self.samples.iter().rev().find(|(t, p)| p1 - p >= MIN_GAIN && t1 - t >= MIN_WINDOW_S);
        match anchor.or_else(|| self.samples.first()) {
            Some(&(t0, p0)) => (p1 - p0, t1 - t0),
            None => (0.0, 0),
        }
    }

    /// A run whose percentage went backwards is the next run, not this one running in reverse.
    fn restarted(&self, progress: f64) -> bool {
        self.samples.last().is_some_and(|(_, p)| progress + MOVED < *p)
    }
}

/// What finstats remembers between two reads: the last answer, and how each running job has moved.
#[derive(Default)]
pub struct Watch {
    pub fetched_at: i64,
    pub answer: Option<Value>,
    runs: HashMap<String, Run>,
}

impl Watch {
    /// Note where a running job is now, and hand back what has been seen of this run so far.
    fn note(&mut self, id: &str, progress: f64, now: i64) -> &Run {
        let run = self.runs.entry(id.to_string()).or_insert_with(|| Run::new(now, progress));
        if run.restarted(progress) {
            *run = Run::new(now, progress);
        } else {
            run.note(now, progress);
        }
        run
    }

    fn forget(&mut self, running: &[String]) {
        self.runs.retain(|id, _| running.iter().any(|r| r == id));
    }
}

// ---------------------------------------------------------------- the answer

fn job_json(t: &Value, watch: &mut Watch, now: i64) -> Value {
    let key = t["Key"].as_str().unwrap_or("");
    let name = t["Name"].as_str().unwrap_or("Unnamed job");
    let id = t["Id"].as_str().unwrap_or(key);
    let state = t["State"].as_str().unwrap_or("Idle");
    let running = state != "Idle";
    let last = &t["LastExecutionResult"];
    let (started, ended) = (last["StartTimeUtc"].as_str().and_then(db::parse_ts), last["EndTimeUtc"].as_str().and_then(db::parse_ts));
    let last_duration_s = started.zip(ended).map(|(s, e)| (e - s).max(0));
    let (schedule_words, next_at) = schedule(t["Triggers"].as_array().map(Vec::as_slice).unwrap_or_default(), ended.or(started));
    let progress = running.then(|| t["CurrentProgressPercentage"].as_f64().unwrap_or(0.0).clamp(0.0, 100.0));

    let (eta, watched_since, unchanged_for) = match progress {
        Some(p) => {
            let run = watch.note(id, p, now);
            let (gained, over_s) = run.measured();
            let (first_at, stalled) = (run.first_at, now - run.changed_at);
            (eta_s(p, gained, over_s, last_duration_s, stalled), Some(first_at), Some(stalled.max(0)))
        }
        None => (None, None, None),
    };

    json!({
        "id": id, "key": key, "name": name, "category": t["Category"],
        "state": state, "running": running, "hidden": t["IsHidden"].as_bool().unwrap_or(false),
        "progress": progress,
        "eta_s": eta,
        // When finstats first saw this run. It may have been going for hours before anybody opened the page.
        "watching_since": watched_since,
        // How long the percentage has been standing still. A slow job is not a broken page.
        "unchanged_for_s": unchanged_for,
        "what": explain(key, name).map(str::to_string).unwrap_or_else(|| fallback(t["Description"].as_str())),
        "known": explain(key, name).is_some(),
        "description": t["Description"],
        "schedule": schedule_words,
        "next_at": next_at,
        "last_result": last["Status"],
        "last_error": last["ErrorMessage"],
        "last_run_at": ended.or(started),
        "last_duration_s": last_duration_s,
    })
}

/// `GET /api/jellyfin/jobs` — what Jellyfin is doing right now, and what it will do later. Read-only, and
/// asked of Jellyfin at most every few seconds however many people are watching the page.
pub async fn jobs(axum::extract::State(app): axum::extract::State<App>, _viewer: ServerViewer) -> ApiResult {
    let now = db::now();
    {
        let watch = app.jf_jobs.lock().unwrap();
        if let Some(answer) = &watch.answer
            && now - watch.fetched_at < MIN_GAP_S
        {
            return Ok(axum::Json(answer.clone()));
        }
    }
    let Some(jf) = app.jellyfin() else {
        return Ok(axum::Json(json!({ "jobs": [], "running": 0, "fetched_at": now, "error": "Not connected to Jellyfin" })));
    };
    let tasks = match jf.scheduled_tasks_all().await {
        Ok(tasks) => tasks,
        Err(e) => {
            tracing::debug!("could not read Jellyfin's scheduled tasks: {e:#}");
            let stale = app.jf_jobs.lock().unwrap().answer.clone();
            let mut answer = stale.unwrap_or_else(|| json!({ "jobs": [], "running": 0 }));
            answer["error"] = json!("Jellyfin did not answer");
            answer["fetched_at"] = json!(now);
            return Ok(axum::Json(answer));
        }
    };
    let mut watch = app.jf_jobs.lock().unwrap();
    let mut jobs: Vec<Value> = tasks.iter().map(|t| job_json(t, &mut watch, now)).collect();
    watch.forget(&jobs.iter().filter(|j| j["running"] == true).filter_map(|j| j["id"].as_str().map(str::to_string)).collect::<Vec<_>>());
    // Running first, then whatever ran most recently: the page is read from the top.
    jobs.sort_by(|a, b| {
        let key = |j: &Value| (!j["running"].as_bool().unwrap_or(false), -j["last_run_at"].as_i64().unwrap_or(0));
        key(a).cmp(&key(b))
    });
    let running = jobs.iter().filter(|j| j["running"] == true).count();
    let answer = json!({ "jobs": jobs, "running": running, "fetched_at": now });
    watch.fetched_at = now;
    watch.answer = Some(answer.clone());
    Ok(axum::Json(answer))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_jobs_people_ask_about_are_explained_in_words_that_are_not_their_name() {
        // The one that started this: a name that says what the code is called.
        let segments = explain("MediaSegmentDetect", "Detect and Analyze Media Segments").expect("media segments");
        assert!(segments.contains("skip") && segments.contains("heaviest"), "{segments}");
        assert!(explain("RefreshLibrary", "Scan Media Library").is_some_and(|s| s.contains("finstats reads your library")));
        assert!(explain("", "Generate Trickplay Images").is_some(), "matched by name when the key is unknown");
        assert!(explain("OptimizeDatabase", "").is_some(), "matched by key when the name is in another language");
        assert!(explain("SomePluginTask", "Do the thing").is_none());
        assert!(fallback(Some("What the plugin says")).starts_with("What the plugin says"));
        assert!(fallback(None).contains("plugin"));
        for (key, name) in [("RefreshLibrary", "Scan Media Library"), ("CleanTranscode", "Clean Transcode Directory"), ("PluginUpdates", "Check for plugin updates")] {
            let text = explain(key, name).unwrap();
            assert!(text.len() > 80 && text.ends_with('.'), "{key}: {text}");
        }
    }

    #[test]
    fn what_is_left_comes_from_the_rate_it_has_actually_been_moving_at() {
        // Ten percent in twenty seconds, nine tenths to go: three minutes.
        assert_eq!(eta_s(10.0, 10.0, 20, None, 0), Some(180));
        // Nothing measured yet: how long it took last time, for the part that is left.
        assert_eq!(eta_s(25.0, 0.0, 2, Some(400), 0), Some(300));
        assert_eq!(eta_s(0.0, 0.0, 0, None, 0), None, "nothing to go on is said, not guessed");
        assert_eq!(eta_s(100.0, 0.0, 0, None, 0), Some(0));
        // A window too short to believe falls back rather than extrapolating from noise.
        assert_eq!(eta_s(50.0, 0.4, 3, Some(600), 0), Some(300));
    }

    #[test]
    fn an_estimate_cannot_stand_still_while_the_job_does() {
        // The fault this was written for: a percentage that does not move left "about 2 minutes" on the
        // screen for a quarter of an hour, because the estimate was worked out afresh from the last run
        // every time and never noticed how long it had been saying it.
        let same = |stalled| eta_s(90.0, 0.0, stalled, Some(1_200), stalled);
        assert_eq!(same(0), Some(120));
        assert_eq!(same(30), Some(90), "half a minute of standing still is half a minute less to wait");
        assert_eq!(same(119), Some(1));
        assert_eq!(same(120), None, "the borrowed time ran out: there is nothing honest left to say");
        assert_eq!(same(600), None);
        // A run that is moving is never affected by it, because the rate is measured and believed first.
        assert_eq!(eta_s(90.0, 5.0, 50, Some(1_200), 50), Some(100));
    }

    #[test]
    fn a_schedule_reads_the_way_jellyfins_own_settings_page_reads_it() {
        let daily = json!([{ "Type": "DailyTrigger", "TimeOfDayTicks": 3i64 * 3600 * TICKS_PER_S }]);
        let (words, next) = schedule(daily.as_array().unwrap(), Some(1_000));
        assert_eq!(words, vec!["every day at 03:00"]);
        assert_eq!(next, None, "a time of day is the server's own, so finstats does not turn it into a countdown");

        let interval = json!([{ "Type": "IntervalTrigger", "IntervalTicks": 86_400i64 * TICKS_PER_S }, { "Type": "StartupTrigger" }]);
        let (words, next) = schedule(interval.as_array().unwrap(), Some(1_000));
        assert_eq!(words, vec!["every day", "when Jellyfin starts"]);
        assert_eq!(next, Some(1_000 + 86_400), "an interval is measured from the last run, so it is a real countdown");
    }

    #[test]
    fn a_run_is_timed_by_watching_it_and_a_restart_starts_the_watch_again() {
        let mut w = Watch::default();
        assert_eq!(w.note("a", 10.0, 100).first_at, 100);
        let run = w.note("a", 30.0, 140);
        let (gained, over_s) = run.measured();
        assert_eq!((gained, over_s, run.first_at, run.changed_at), (20.0, 40, 100, 140));
        assert_eq!(eta_s(30.0, gained, over_s, None, 0), Some(140));

        // Standing still: the window still spans the same seconds, and `changed_at` stops moving.
        let run = w.note("a", 30.0, 200);
        assert_eq!((run.changed_at, run.measured()), (140, (20.0, 100)));

        // The percentage went back to the beginning: that is the next run, not the same one going backwards.
        let run = w.note("a", 2.0, 260);
        assert_eq!((run.first_at, run.changed_at, run.measured()), (260, 260, (0.0, 0)));

        w.forget(&["b".to_string()]);
        assert_eq!(w.note("a", 5.0, 300).first_at, 300, "a job that stopped is not remembered");
    }

    #[test]
    fn the_rate_is_measured_over_the_recent_past_not_the_whole_run() {
        let mut w = Watch::default();
        // An hour of one percent a minute, then three minutes of ten times that.
        for m in 0..60 {
            w.note("a", m as f64, m * 60);
        }
        for m in 1..=3 {
            w.note("a", 60.0 + (m as f64) * 10.0, 3_600 + m * 60);
        }
        let (gained, over_s) = w.runs["a"].measured();
        let per_min = gained / (over_s as f64 / 60.0);
        assert!(per_min > 8.0, "the hour that came before is still being averaged in: {per_min:.2}%/min");
        assert!(over_s <= WINDOW_S, "the window is not allowed to grow for ever: {over_s}s");

        // …and a job creeping a percent every five minutes is still measurable, which is why the window
        // reaches back as far as it does.
        let mut slow = Watch::default();
        for m in 0..10 {
            slow.note("b", m as f64, m * 300);
        }
        let (gained, over_s) = slow.runs["b"].measured();
        assert!(gained >= MIN_GAIN && over_s >= MIN_WINDOW_S, "a slow job cannot be measured at all: {gained}% over {over_s}s");
        assert_eq!(eta_s(9.0, gained, over_s, None, 0), Some(27_300), "about seven and a half hours to go, at a percent every five minutes");
    }
}
