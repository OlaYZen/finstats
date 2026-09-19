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
           "has_backdrop": true},
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
 "poll_interval_s": 5,             // session polling, 2..60
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
