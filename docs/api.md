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
 "sync_interval_h": 6,             // library/user sync, 1..168
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
