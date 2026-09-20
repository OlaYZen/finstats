# finstats HTTP API

All endpoints live under `/api` and speak JSON. Authentication is a session
cookie (`finstats_session`, HttpOnly, SameSite=Lax) issued by `POST /api/auth/login`.

## Conventions

- **Timestamps** are unix seconds (UTC). **Durations** are seconds.
- **Dates** in time series are `YYYY-MM-DD` in the server's local timezone (`TZ` env).
- **IDs** are Jellyfin IDs: 32 lowercase hex chars, no dashes.
- **Errors**: non-2xx status with `{"error": "human readable message"}`.
  `401` = not logged in, `403` = not allowed, `409` = wrong state (e.g. already configured).
- **Common filters** (query string) on every `/api/stats/*`, `/api/activity`, `/api/users*`,
  `/api/libraries*`, `/api/items/*` endpoint:
  - `days` — integer window ending now. `0` or absent = all time.
  - `user_id` — restrict to one user.
  - `library_id` — restrict to one library.
- **Non-admin users** are always scoped to their own `user_id` server-side, never see
  IP addresses, and get `403` on admin endpoints (marked 🔒).

## Bootstrap & auth

| Method | Path | Body | Response |
|---|---|---|---|
| GET | `/api/status` | – | `{configured, version, server_name?}` (public) |
| POST | `/api/setup/test` | `{url}` | `{server_name, version, id}` (public, only while unconfigured) |
| POST | `/api/setup` | `{url, username, password}` | `{user}` — must be a Jellyfin admin. Creates a Jellyfin API key named `finstats`, stores config, logs in, kicks off first sync. Only while unconfigured. |
| POST | `/api/auth/login` | `{username, password}` | `{user}`; `401` bad credentials, `403` user login disabled, `429` too many attempts |
| POST | `/api/auth/logout` | – | `{ok: true}` |
| GET | `/api/auth/me` | – | `{user}` or `401` |

`user` = `{id, name, is_admin, has_image}`.

## Live

`GET /api/now-playing` → `{sessions: [Session]}`

```jsonc
Session = {
  "key": "…",                 // stable while this play lasts
  "user_id": "…", "user_name": "…",
  "item_id": "…", "item_name": "…", "item_type": "Episode",
  "series_id": "…"|null, "series_name": "…"|null,
  "season_number": 1|null, "episode_number": 4|null,
  "image_item_id": "…",       // best item to ask /api/img for (series poster for episodes)
  "position_s": 512, "runtime_s": 1440,
  "is_paused": false,
  "started_at": 1790000000, "watched_s": 498,
  "client": "Jellyfin Web", "device_name": "Firefox", "app_version": "10.11.0",
  "remote_ip": "…"|null,      // null for non-admins
  "play_method": "DirectPlay" | "DirectStream" | "Transcode",
  "video": "HEVC 1080p HDR10"|null, "audio": "EAC3 5.1"|null, "subtitle": "eng (subrip)"|null,
  "container": "mkv"|null, "bitrate": 8500000|null,
  "transcode": null | {
    "video_codec": "h264", "audio_codec": "aac", "container": "ts",
    "is_video_direct": false, "is_audio_direct": false,
    "hw_accel": "nvenc"|null, "reasons": ["ContainerNotSupported"], "progress": 0.42|null
  }
}
```

## Stats

`GET /api/stats/overview`
```jsonc
{
  "totals":   {"plays": 0, "watch_s": 0, "active_users": 0, "distinct_items": 0},
  "previous": {"plays": 0, "watch_s": 0, "active_users": 0, "distinct_items": 0} | null, // same-length window before; null when days=0
  "daily": [ {"date": "2026-01-31", "plays": 3, "watch_s": 5400,
              "by_type": {"Movie": [1, 5000], "Episode": [2, 400], "Audio": [0,0], "Other": [0,0]}} ], // [plays, watch_s]; gap-free, oldest first
  "library": {"movies": 0, "series": 0, "episodes": 0, "tracks": 0, "size_bytes": 0, "users": 0}
}
```
When the window is longer than 120 days `daily` is bucketed per ISO week; each entry then
carries the Monday date and `"bucket": "week"` is set at top level (otherwise `"day"`).

`GET /api/stats/top?kind=<kind>&limit=10` — `kind` ∈ `movies | series | music | users | clients | devices | libraries`
```jsonc
{"rows": [ {"id": "…"|null, "name": "…", "sub": "2019"|null, "plays": 12, "watch_s": 3600,
            "users": 3,               // distinct users (absent for kind=users)
            "image_item_id": "…"|null, "last_played": 1790000000} ]}
```
Sorted by `watch_s` desc unless `&sort=plays`.

`GET /api/stats/heatmap` → `{"plays": [[24 ints] × 7], "watch_s": [[24 ints] × 7]}` — outer index 0 = Monday, inner = hour of day (server TZ).

`GET /api/stats/playback`
```jsonc
{
  "methods":           [{"name": "DirectPlay", "plays": 0, "watch_s": 0}],
  "transcode_reasons": [Bucket], "hw_accel": [Bucket],
  "video_codecs": [Bucket], "audio_codecs": [Bucket], "resolutions": [Bucket],
  "video_ranges": [Bucket], "containers": [Bucket], "audio_channels": [Bucket],
  "clients": [Bucket], "subtitles": [Bucket]   // subtitle language, "None" when off
}
// Bucket = {"name": "hevc", "plays": 0, "watch_s": 0}; sorted by plays desc, max 12, tail folded into "Other"
```

## Activity

`GET /api/activity?page=1&per_page=50&q=&method=&type=&item_id=&series_id=` (+ common filters)
```jsonc
{"total": 2918, "page": 1, "per_page": 50, "rows": [Play]}

Play = {
  "id": 123, "source": "live" | "jellystat", "active": false,
  "user_id": "…", "user_name": "…",
  "item_id": "…", "item_name": "…", "item_type": "Episode",
  "series_id": null, "series_name": null, "season_number": null, "episode_number": null,
  "image_item_id": "…", "item_exists": true,     // false → item no longer in library, don't link
  "started_at": 0, "ended_at": 0, "duration_s": 0, "paused_s": 0,
  "position_s": 0|null, "runtime_s": 0|null, "completion": 0.93|null,
  "client": "…", "device_name": "…", "app_version": "…", "remote_ip": "…"|null,
  "play_method": "…", "container": "mkv"|null,
  "video": "HEVC 1080p SDR"|null, "audio": "AAC 2.0 jpn"|null, "subtitle": "eng"|null
}
```

`GET /api/activity/{id}` → `Play` plus
`{"device_id", "bitrate", "video_codec", "width", "height", "video_range", "bit_depth", "audio_codec", "audio_channels", "audio_language", "subtitle_codec", "subtitle_language", "transcode": {…same as Session.transcode, plus "bitrate","width","height","audio_channels"} | null}`

🔒 `DELETE /api/activity/{id}` → `{ok: true}`

## Users

`GET /api/users` (non-admin: only self)
```jsonc
{"users": [ {"id","name","is_admin","is_disabled","removed","has_image",
             "last_login_at","last_activity_at","plays","watch_s","last_played_at",
             "last_item_name","last_client"} ]}
```

`GET /api/users/{id}`
```jsonc
{
  "user": {…as above},
  "totals": {"plays","watch_s","distinct_items","movies","episodes","tracks"},
  "daily": [...same as overview.daily],  "bucket": "day",
  "heatmap": {"plays": [[…]], "watch_s": [[…]]},
  "top_series": [TopRow], "top_movies": [TopRow],
  "clients": [Bucket], "methods": [Bucket],
  "devices": [{"device_id","device_name","client","app_version","plays","last_seen"}],
  "ips": [{"ip","plays","first_seen","last_seen","is_local"}]   // 🔒 admins only, else []
}
```

## Libraries & items

`GET /api/libraries`
```jsonc
{"libraries": [ {"id","name","collection_type","removed","item_count","series_count","episode_count",
                 "size_bytes","plays","watch_s","last_played_at"} ]}
```

`GET /api/libraries/{id}` → `{"library": {…}, "top": [TopRow], "recently_added": [ItemCard], "daily": [...], "bucket": "day"}`

`ItemCard = {"id","name","type","year","sub","image_item_id","date_created"}`

`GET /api/items/{id}`
```jsonc
{
  "item": {"id","name","type","year","overview":null,"genres":[…],"community_rating","official_rating",
           "runtime_s","premiere_date","date_created","library_id","library_name","removed",
           "series_id","series_name","season_number","episode_number",
           "container","size_bytes","bitrate","video":"HEVC 1080p"|null,"audio":null,"path":"…"|null /* 🔒 */,
           "has_image": true /* own primary image; an episode without one shows its series' poster */, "has_backdrop": true},
  "totals": {"plays","watch_s","users","last_played_at"},
  "watchers": [{"user_id","user_name","plays","watch_s","last_played_at"}],
  "seasons": [ {"id","name","season_number","episodes":[{"id","name","episode_number","runtime_s","plays","watch_s"}]} ], // Series only, else []
  "daily": [...], "bucket": "day"
}
```
Recent plays for an item come from `/api/activity?item_id=` (episode/movie) or `?series_id=`.

`GET /api/search?q=att&limit=12` → `{"items": [ItemCard], "users": [{"id","name"}]}` — items limited to Movie/Series/MusicAlbum/Audio top-level hits.

## Images

- `GET /api/img/item/{id}?kind=primary|backdrop&w=300` — proxied + cached from Jellyfin. `404` when Jellyfin has none.
- `GET /api/img/user/{id}?w=96`

Both send long-lived `Cache-Control`. Use as `<img loading="lazy">` with an `onerror` fallback.

## Admin 🔒

`GET /api/settings`
```jsonc
{"jellyfin_url","server_name","server_version",
 "allow_user_login": false,        // let non-admin Jellyfin users sign in and see their own stats
 "active_interval_s": 1,           // session polling while something plays, 1..60 (replaced poll_interval_s in 0.10)
 "idle_interval_s": 5,             // …and while nothing does, 1..60
 "follow_jellyfin_scan": true,     // read the library when Jellyfin's own scan task finishes, not on a timer
 "sync_interval_h": 6,             // library read interval, 1..168 — only used when not following Jellyfin's scan
 "merge_window_s": 600,            // resume the same play if it restarts within this window
 "min_play_s": 0}                  // stats ignore plays shorter than this
```
`PUT /api/settings` — partial object of the mutable keys above (not `jellyfin_url`/`server_*`) → full settings.

`GET /api/tasks`
```jsonc
{"tasks": [ {"id": "sync_users" | "sync_libraries" | "sync_events" | "import",
             "state": "idle" | "running" | "ok" | "error",
             "message": "Fetching items 4,500 / 13,900", "progress": 0.32|null,
             "started_at","finished_at","error": null} ],
 "collector": {"connected": true, "last_poll_at": 0, "active_sessions": 1, "error": null},
 "db": {"size_bytes","plays","items","oldest_play_at"}}
```
`POST /api/tasks/{id}/run` (not for `import`) → `202 {ok:true}`; `409` if already running.

`POST /api/import/jellystat` — **raw request body** is the `.jsonl` (or legacy `.json`) backup
(`Content-Type: application/octet-stream`; can be hundreds of MB — use XHR for upload progress).
Returns `202 {ok:true}` once the upload is stored; parsing continues as task `import`
(poll `/api/tasks`). On finish the task `message` summarises, and `result` holds
`{"plays_imported","plays_skipped","users","libraries","items","seasons","episodes","item_info"}`.
Re-importing the same backup is safe: plays are de-duplicated by their Jellystat id.

`GET /api/events?page=1&per_page=50&q=&type=` → Jellyfin server activity log
```jsonc
{"total", "page", "per_page",
 "rows": [{"id","date","name","overview","type","severity","user_id","user_name","item_id"}]}
```

## Light status (any signed-in user)

`GET /api/summary` → `{"active_sessions": 1, "plays_total": 2918, "last_sync_at": 0, "collector_ok": true, "version": "0.1.0"}` — cheap; poll for the status bar.

---

# v0.2 additions

## Richer plays

The collector now keeps a timeline per live play and counts interruptions.

- `Play` rows (list + detail) gain `"pause_count": 0`, `"seek_count": 0`, and — admins only, else `null` —
  `"is_local": true|false|null` (LAN / remote, derived from the IP).
- `GET /api/activity/{id}` additionally returns `"start_position_s": 0|null` (where playback resumed from) and
  ```jsonc
  "events": [ {"at": 1790000000, "kind": "start"|"pause"|"resume"|"seek"|"audio"|"subtitle"|"transcode"|"stop",
               "position_s": 512|null,
               "detail": null | "12:40 → 31:05" | "EAC3 5.1 eng" | "Off" | "Transcode: ContainerNotSupported"} ]
  ```
  Oldest first. Empty for imported plays (Jellystat never recorded this).

## `GET /api/stats/insights` (common filters)

```jsonc
{
  "concurrency": {"peak": 4, "peak_at": 1790000000|null, "peak_transcodes": 2,
                  "series": [{"date": "2026-01-31", "peak": 3}], "bucket": "day"|"week"},   // gap-free, same bucketing as overview.daily
  "network": [Bucket],            // names "Local" / "Remote" / "Unknown"; [] for non-admins
  "data_bytes": 1234567890,       // estimated bytes sent to clients (stream bitrate × time watched)
  "genres": [Bucket],             // by watch time; episodes count towards their series' genres; max 12 + "Other"
  "client_methods": [ {"client": "Jellyfin Web", "direct_play": 10, "direct_stream": 2, "transcode": 30, "watch_s": 0} ], // plays; sorted by total desc, max 12
  "completion": [ {"name": "Under 10%", "plays": 0}, {"name": "10–50%", ...}, {"name": "50–90%", ...}, {"name": "Finished (90%+)", ...} ], // movies + episodes; fixed order
  "behaviour": {"plays_measured": 120, "avg_pauses": 1.4, "avg_seeks": 0.8, "resumed_share": 0.31}, // live plays only; plays_measured = 0 → hide
  "failed_logins": [ {"date": 0, "overview": "…", "user_name": null} ]   // 🔒 newest 10 in window; [] for non-admins
}
```

## `GET /api/library/insights?library_id=` — what the library is made of (no time window)

```jsonc
{
  "totals": {"files": 0, "size_bytes": 0, "runtime_s": 0, "movies": 0, "series": 0, "episodes": 0, "tracks": 0},
  "resolutions": [LibBucket], "video_codecs": [LibBucket], "video_ranges": [LibBucket],
  "containers": [LibBucket], "audio_codecs": [LibBucket],
  "genres": [LibBucket],          // movies + series, size_bytes omitted (0)
  "decades": [LibBucket],         // name "1990s", oldest first
  "added": [ {"month": "2026-01", "count": 12} ],   // last 24 months, gap-free, oldest first
  "largest": [LibItem],           // 15 biggest movies / series (series = sum of episodes)
  "unwatched": {"count": 0, "size_bytes": 0, "items": [LibItem]}   // never played by anyone, per finstats history AND Jellyfin's own played flags; 25 biggest
}
// LibBucket = {"name", "count", "size_bytes"};   sorted by count desc (decades/added excepted), max 12 + "Other"
// LibItem   = {"id","name","type","year","size_bytes","date_created","image_item_id"}
```

## `GET /api/server` 🔒 — the Jellyfin server itself

```jsonc
{
  "fetched_at": 0|null,           // null → not fetched yet (run task sync_server)
  "info": {"server_name","version","operating_system","architecture","has_update_available","has_pending_restart",
           "transcoding_temp_path","cache_path","program_data_path","log_path","encoder_location"} | null,
  "storage": [ {"label": "Shows" | "Program data" | "Cache" | "Transcodes" | …, "path", "free_bytes", "used_bytes", "kind": "library"|"system"} ], // [] on servers older than 10.11
  "plugins": [ {"name","version","status","description"} ],
  "scheduled_tasks": [ {"name","category","state","last_result": "Completed"|"Failed"|…|null,"last_run_at": 0|null,"last_duration_s": 0|null} ],
  "devices": [ {"device_id","name","app","app_version","last_user_id","last_user_name","last_seen"} ]   // newest first
}
```
New task ids in `/api/tasks`: `sync_server` (server info, plugins, tasks, devices) and `sync_userdata` (per-user played / favourite flags). Both runnable via `POST /api/tasks/{id}/run`.

## Small additions to existing responses

- `GET /api/users/{id}` gains `"genres": [Bucket]` and `"jellyfin": {"played_movies": 0, "played_episodes": 0, "favorites": 0} | null` (Jellyfin's own flags; covers history from before finstats).
- `GET /api/items/{id}` → `item` gains `"studios": ["…"]`, `"external": [{"label": "IMDb", "url": "https://…"}]`, `"bit_depth"`, `"framerate"`; top level gains
  `"played_by": [{"user_id","user_name","last_played_at": 0|null,"is_favorite": false}]` (Jellyfin's played flags; admins see everyone, others only themselves).

---

# v0.3 — Recap (the year in review)

`GET /api/recap?year=2026` — a recap is personal: everyone, administrators included, only ever gets their **own**.
`year` is a calendar year (server TZ) or `last12` (the last 12 full months plus the current one).
Default: the year whose recap is "ready" — the current year during December, otherwise the previous year
(2026 becomes the default in December 2026 and stays it until December 2027). If that year has no plays
(a new install), the newest year that has.

```jsonc
{
  "years": [2026, 2025],                 // years that have plays in this scope, newest first
  "year": 2026 | "last12", "from": 1790000000, "to": 1790000000,   // [from, to)
  "scope": {"user_id": "…", "user_name": "…", "server_name": "…"},          // always the signed-in user
  "empty": false,                        // true → nothing was played in this period; all lists empty

  "totals": {"plays", "watch_s", "distinct_items", "movies", "episodes", "tracks",   // plays per type
             "series_count", "active_days"},
  "rank": {"position": 2, "of": 8, "share": 0.31} | null,  // by watch time among users that played anything (no names)

  "top_series": [RecapTitle],  "top_movies": [RecapTitle],  "top_tracks": [RecapTitle],   // up to 5 each, by watch time
  "top_genres": [Bucket],                // up to 6, by watch time, no "Other"
  // RecapTitle = {"id","name","sub","image_item_id","plays","watch_s","episodes": 12 /* distinct episodes, series only, else null */, "item_exists": true}

  "months": [ {"month": "2026-01", "watch_s", "plays",
               "top": {"id","name","image_item_id","watch_s"} | null} ],   // every month of the period, oldest first; top = most watched series-or-movie
  "hours": [24 × watch_s], "weekdays": [7 × watch_s /* 0 = Monday */],

  "persona": {"key": "night_owl" | "early_bird" | "weekend_warrior" | "binge_watcher" | "movie_buff" | "music_lover" | "creature_of_habit",
              "title": "Night owl", "line": "43% of the watching happened after 22:00"},

  "records": {                           // any entry may be null
    "biggest_day":    {"date": "2026-03-14", "watch_s", "plays"},
    "biggest_binge":  {"date", "series_id", "series_name", "image_item_id", "episodes", "watch_s"},
    "longest_streak": {"days": 9, "from": "2026-02-01", "to": "2026-02-09"},
    "longest_play":   {"item_id", "name", "image_item_id", "duration_s", "date"},
    "most_rewatched": {"id", "name", "type", "image_item_id", "plays"},      // a movie or episode played on ≥ 2 different days
    "first_play":     {"item_id", "name", "image_item_id", "at": 1790000000},
    "oldest_title":   {"id", "name", "year": 1957, "image_item_id"}
  },

  "discovery": {"new_series": 14,                                 // shows whose first ever play falls in the period
                "one_and_done": [{"id","name","image_item_id"}],  // up to 5 shows with exactly one episode started, ever
                "finished_movies": 31, "finished_episodes": 402}, // ≥ 90% complete

  "clients": [Bucket]                    // top 3
}
```

---

# v0.4 — Patch notes

`GET /api/changelog` (any signed-in user) — `CHANGELOG.md`, compiled into the binary and parsed.

```jsonc
{
  "current": "0.4.0",                       // the running version
  "releases": [                             // newest first
    {"version": "0.4.0", "date": "2026-09-19" | null, "summary": "One paragraph." | null,
     "groups": [ {"kind": "Added" | "Changed" | "Fixed" | "Removed", "items": ["One change. May contain **bold** and `code`."]} ]}
  ]
}
```

---

# v0.5 — Permissions

`user` (from `/api/auth/login`, `/api/auth/me`, `/api/setup`) gains
`"permissions": {"see_everyone": false, "see_network": false, "see_server": false, "manage": false}` — all `true` for
Jellyfin administrators. They are evaluated on every request, so a change applies at once.

What each one gates, server-side:

| Permission | Effect |
|---|---|
| *(none)* | Every list and statistic is pinned to the caller's own `user_id` (a `user_id` filter is ignored); `/api/users` returns only them; `/api/users/{other}` → `403`. |
| `see_everyone` | `user_id` filters are honoured, `/api/users` and `/api/users/{id}` for anyone, all sessions in `/api/now-playing`, users in `/api/search`, everyone in `watchers` / `played_by`. |
| `see_network` | `remote_ip`, `device_id`, `is_local` in plays and sessions; `ips` on user pages; `network` in insights; IP search in `/api/activity?q=`. Otherwise `null` / `[]`. |
| `see_server` | `/api/server`, `/api/events`, `failed_logins` in insights, `item.path`. Otherwise `403` / `[]` / `null`. |
| `manage` | `/api/settings`, `/api/tasks*`, `/api/import/jellystat`, `DELETE /api/activity/{id}`. Otherwise `403`. |

`/api/recap` is never widened: it is always the caller's own. `PUT /api/settings` rejects `allow_user_login` and
`default_permissions` with `403` unless the caller is a Jellyfin administrator.

## Managing permissions — Jellyfin administrators only (🔒 `403` for everyone else, including `manage`)

`GET /api/permissions`
```jsonc
{"available": [{"key": "sign_in" | "see_everyone" | "see_network" | "see_server" | "manage", "label": "…", "description": "…"}],
 "defaults": ["sign_in"],                  // what every non-admin has; "sign_in" here = sign-in is open to everyone
 "users": [{"id","name","is_admin","is_disabled","has_image","permissions": ["see_everyone"]}]}   // own grants only, defaults not included
```
`PUT /api/permissions/defaults` `{"permissions": [...]}` → `{"defaults": [...]}`
`PUT /api/permissions/users/{id}` `{"permissions": [...]}` → `{"permissions": [...]}`; `400` for an unknown key or an
administrator (they already have everything), `404` for an unknown user. Effective = defaults ∪ own grants.

---

# v0.6 — Profiles: show progress, manual marks, streaks

`GET /api/users/{id}/shows` — own id, or anyone's with `see_everyone` (`403` otherwise). All time; no filters.
```jsonc
{
  "editable": true,                       // true only on your own profile
  "streaks": {"longest": {"days": 29, "from": "2026-04-17", "to": "2026-05-15"} | null,
              "current": {"days": 3, "since": "2026-09-17" | null, "includes_today": true},   // still alive if it reached yesterday
              "active_days": 189},
  "shows": [ {"id", "name", "year", "removed", "image_item_id", "last_played_at": 0|null,
              "total": 26, "seen": 25, "started": 0,
              "seasons": [ {"season_number": 1, "total": 12, "seen": 12,
                            "episodes": [ {"id", "episode_number": 1|null, "name",
                                           "state": "seen" | "started" | "none",
                                           "source": "played" | "jellyfin" | "manual" | null} ]} ]} ]
}
```
Only episodes that exist as files count: Jellyfin's virtual items (missing or unaired episodes) and specials
(season 0) are left out, so an announced season does not drag a finished show below 100%. For a series that has
left the library its removed episodes are used instead. An episode is `seen` when a recorded play reached 80%
(`played`), Jellyfin has it marked as played (`jellyfin`), or the user marked it (`manual`), in that order of
precedence; `started` = played, but not that far. Shows with nothing seen or started are omitted.
The same `streaks` object is also part of `GET /api/users/{id}`.

`POST /api/me/seen` `{"item_ids": ["…"], "seen": true|false}` → `{"changed": 12}` — marks episodes as seen for the
caller only (1–5000 ids; non-episodes are ignored). Marks live in finstats alone: nothing is written to Jellyfin,
and `seen: false` only removes manual marks, never a recorded play.

`Play` rows were already carrying `position_s` and `runtime_s`; the Activity table now shows them as the stop position.

---

# v0.6.1 — Recap for administrators

`GET /api/recap?year=&user_id=` — `user_id` is honoured for **Jellyfin administrators** only and selects one other
user's recap (`scope` then names them). Everyone else always gets their own, whatever permissions they hold
(`see_everyone` included); the parameter is ignored rather than refused. There is no whole-server recap.

---

# v0.7 — Group watching

Plays of one item by at least two different users that start within `group_window_s` (setting, default 60, 5–600) of
each other and overlap for at least two minutes share a `group_id`. Jellyfin's sessions do not expose SyncPlay groups,
so this is inferred; it is recomputed for an item whenever one of its plays ends, after an import, at start-up, and for
the whole history when the setting changes.

- `Play` rows gain `"group_id": 123|null` and `"group_size": 3|null` (distinct people).
- `GET /api/activity/{id}` gains `"watched_with": [{"user_id","user_name"}]` — the other people in the group.

`GET /api/stats/groups` (common filters; without `see_everyone` only groups the caller was part of; with a `user_id`
filter only that person's groups)
```jsonc
{
  "totals": {"sessions": 150, "together_s": 0, "person_s": 0, "people": 3},
  // together_s: per session, how long at least two people were watching (the second-longest stay), summed.
  // person_s: everyone's watch time in those sessions, summed.
  "companions": [ {"members": [{"user_id","user_name","has_image"}], "sessions", "together_s", "last_at"} ],  // by exact set of people, top 8
  "titles":     [ {"id","name","image_item_id","sessions","together_s"} ],                                   // series or movie, top 8
  "recent":     [ {"item_id","item_name","item_type","series_id","series_name","season_number","episode_number","image_item_id",
                   "started_at","together_s","members": [{"user_id","user_name","has_image","duration_s"}]} ]   // newest 10
}
```
`GET/PUT /api/settings` gains `"group_window_s": 60`.

`Session` (from `GET /api/now-playing`) gains `"group": {"size": 2, "with": [{"user_id","user_name"}]}` when other
people are playing the same title right now, having started within `group_window_s` or being within
`max(group_window_s, 30)` seconds of the same position; absent otherwise. It is computed before the list is narrowed
to the caller, so someone without `see_everyone` still sees who they are watching with.

---

# v0.7.5 — Forgiving search

`GET /api/search?q=` matches word by word instead of by exact phrase: every word of `q` has to be found in the title
(any order; case, accents, punctuation and a leading article ignored; one slip allowed in words of 4–7 letters, two in
longer ones; music also matches on its artist). Results are ranked, best first. The `q` filters of `/api/activity` and
`/api/events` are word-by-word as well: each word must occur in at least one searched column.

---

# v0.8 — Recap: days, people, rewatches

`GET /api/recap` gains, all within the same period and scope as the rest of the response:

```jsonc
{
  "totals": { …, "movie_watch_s": 0, "episode_watch_s": 0, "track_watch_s": 0 },
  "genres": {"count": 16, "plays": 480, "watch_s": 0} | null,   // distinct genres; plays and time of titles that have any genre
                                                                  // (top_genres[0].watch_s / genres.watch_s = the top genre's share)
  "people": {                                                     // most watched cast and crew, 5 each, by watch time
    "actors":    [{"id": "…", "name": "…", "has_image": true, "plays": 0, "watch_s": 0, "titles": 3, "top_title": "…"}],
    "directors": [ … same shape … ]
  },
  "rewatch": {"sittings": 0, "rewatches": 0, "share": 0.12} | null,
  "days": [{"date": "2026-03-14", "plays": 3, "watch_s": 0}]      // only days with plays, oldest first, server TZ
}
```

- **People** come from the films and shows themselves (an episode counts towards its show's cast). Only actors — the
  first 12 billed per title — and directors are kept. A person's portrait is `GET /api/img/item/{person id}`.
  They are read with the library, in a second, smaller request per library (`IncludeItemTypes=Movie,Series`,
  `Fields=People`); until the first library read after upgrading, both lists are empty.
- A **sitting** is one film or episode on one local day (plays of at least 5 minutes). A **rewatch** is a sitting with
  something that already had one in the period: `rewatches = sittings − distinct titles`, `share = rewatches / sittings`.
  Picking a play up again on the same day is not a rewatch.

---

# v0.8.1 — People

`GET /api/items/{id}` gains `"people": [{"id","name","kind": "Actor"|"Director","role": "…"|null,"has_image": true}]` —
directors first, then the cast in billing order. An episode or season answers with its show's. Empty until the
library has been read by 0.8.0 or newer.

`GET /api/people/{id}` (common filters) — one actor or director. `404` for an id nobody in the library carries.

```jsonc
{
  "person": {"id": "…", "name": "…", "has_image": true, "is_actor": true, "is_director": false, "titles": 13},
  "totals": {"plays": 0, "watch_s": 0, "users": 0, "titles_watched": 0, "last_played_at": 0|null},
  "titles": [{"id","name","type": "Movie"|"Series","year","removed": false,"kinds": "Actor"|"Director"|"Actor,Director",
              "role": "…"|null,"plays": 0,"watch_s": 0,"last_played_at": 0|null}],   // every title they are in; most watched first
  "watchers": [{"user_id","user_name","plays","watch_s","last_played_at"}]
}
```

Everything counted is within the caller's scope, exactly like `/api/items/{id}`: without `see_everyone` the totals,
per-title figures and `watchers` cover the caller's own plays only. A play of an episode counts towards its show's
people; a play counts once even when the person both acts in and directs the title.

---

# v0.9 — Sorting the paginated lists

`GET /api/activity` and `GET /api/events` take `sort` and `dir` (`asc` | `desc`, default `desc`). Rows without a value
for the sorted column come last in either direction, and the default order is always the tiebreaker, so pages stay stable.
An unknown `sort` is ignored (default order), never an error.

| Endpoint | `sort` values | Default order |
|---|---|---|
| `/api/activity` | `when`, `user`, `title` (show name for episodes), `watched`, `progress` (the same rule as `completion`), `client`, `method`, `ip` (ignored without `see_network`) | newest first |
| `/api/events` | `when`, `event`, `type`, `user` | newest first |

Every other table is sorted in the browser (`web/assets/js/tables.js`); those endpoints are unchanged.

---

# v0.9.1 — Home network

`is_local` on plays and on a user's address list now means "a private address **or** a known home address". Home
addresses are this network's public IP (looked up, every one ever seen) plus any added by hand; changing either
re-decides `is_local` for the whole history.

`GET/PUT /api/settings` gain `"public_ip_lookup": true` and `"home_addresses": ["203.0.113.7"]` (IP addresses only, at
most 50; anything else is a `400`). The response also carries, read-only:

```jsonc
"known_home_addresses": [{"ip": "203.0.113.7", "source": "lookup"|"manual", "first_seen": 0, "last_seen": 0}],
"public_ip_services": ["https://checkip.amazonaws.com", "…"]      // who is asked, in order
```

---

# v0.10 — Backups, and the polling intervals

`poll_interval_s` is gone from the settings. In its place: `"active_interval_s": 1` (while something plays) and
`"idle_interval_s": 5` (while nothing does), both 1..60. New: `"backup_every_d": 7` (0 = off, 0..365) and `"backup_keep": 5` (1..100).

All of the following are for **Jellyfin administrators** (`403` otherwise): a backup is everyone's history, and a restore
can bring permissions back. `{name}` must look exactly like `finstats-backup-YYYYMMDD-HHMMSS.jsonl.gz`; anything else is a `404`.

| | |
|---|---|
| `GET /api/backups` | `{"backups": [{"name","size_bytes","created_at"}], "every_d": 7, "keep": 5, "next_at": 0\|null}` — newest first |
| `POST /api/backups` | Start writing one now. `202`; progress is task `backup` in `/api/tasks`. `409` while one is running. |
| `GET /api/backups/{name}` | The file (`application/gzip`, `Content-Disposition: attachment`), streamed. |
| `DELETE /api/backups/{name}` | Remove it. |
| `POST /api/backups/{name}/restore?settings=true` | Restore a stored backup. `202`; task `restore`. |
| `POST /api/backups/restore?settings=true` | The same from an uploaded file: raw request body, no size limit. |

`settings` (default `true`) also restores the settings and the permissions; `false` merges history only. The finished
`restore` task carries `result: {"plays_imported","plays_skipped","events","other_rows","settings_restored","from_version"}`.

**The file** is gzip-compressed JSON Lines. Line 1: `{"finstats_backup": 1, "app_version", "created_at", "server_name", "counts": {table: rows}}`.
Every other line: `{"t": "<table>", "r": {column: value}}` for `settings` (the one settings row), `playbacks`, `playback_events`,
`manual_seen`, `user_permissions`, `home_addresses`, `server_events`, `devices`. Rows are matched by column name in both
directions, so backups move between versions. Never in it: the Jellyfin address and API key, sessions, the library.
Restoring merges: a play already present (same `source_id`, or same user, item and start) is skipped with its timeline;
restored plays are never `active`, and groups, local/remote and library links are worked out again afterwards.
CLI: `finstats backup`, `finstats restore <file>`.

# v1.1 — Timeline

`GET /api/users/{id}/timeline?before=&limit=24&libraries=` — own id, or anyone's with `see_everyone` (`403` otherwise; `404`
for an unknown user). All time, newest first; `min_play_s` applies.
```jsonc
{
  "user": {"id", "name", "has_image"},
  "libraries": [{"id", "name", "collection_type"}],   // the ones this person has played from, for the filter
  "next": "1789675170.3245" | null,                    // pass as `before` for the next page; null = the history ends here
  "stops": [ {"kind": "season"|"album"|"item", "type": "Episode", "id": "<series, track or item id>", "name": "The Rookery",
              "sub": "Season 2" | "<year>" | "<album artist>" | null, "image_item_id",   // the season's poster if it has one, else the show's
              "from": 0, "to": 0,                        // start of the oldest play, end of the newest
              "plays": 4, "titles": 4, "watch_s": 5520, "active": false,   // titles = different episodes/tracks; active = still playing
              "episode_from": 1, "episode_to": 4} ]    // only when the episodes are an unbroken run, else null
}
```
A stop is a run of plays that follow each other and belong together: episodes of one season of one show, tracks of one album,
or one title played again. Anything else in between starts a new stop, so a show can appear many times. A page never ends in
the middle of a stop, and the stops are the same whatever `limit` (1..60) is. `libraries` is a comma-separated list of library
ids (absent = all); ids that are not ids and cursors that are not cursors are a `400`.
