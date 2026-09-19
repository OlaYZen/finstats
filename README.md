# finstats

Playback statistics for [Jellyfin](https://jellyfin.org) — who watched what, when, on which
device, from where, and whether your server had to transcode it.

One small Rust binary. One SQLite file. No Node, no Postgres, no background job runner.

- **Light** — ~10 MB binary, ~20 MB of RAM, a ~25 MB Docker image. The web UI is embedded
  in the binary and has zero JavaScript dependencies.
- **Sign in with Jellyfin** — your Jellyfin username and password. finstats never stores
  passwords and creates its own API key during setup.
- **Collects what Jellyfin exposes, and what happens in between** — per play: user, item,
  series/season/episode, client, device, app version, IP address (and whether it was on your
  LAN), play method, transcode reasons and hardware acceleration, video codec / resolution /
  HDR range, audio codec / channels / language, subtitles, container, bitrate, time watched
  vs. time paused, where playback resumed from and where it stopped — plus a **timeline of
  the play itself**: every pause, skip, audio/subtitle switch and direct-play→transcode flip.
- **Answers the questions an admin actually has** — How many streams run at once, and how many
  of them transcode? Which clients force transcodes? How much data leaves the house? How far
  do people get before they give up? What is my library made of (resolution, codecs, HDR,
  size per decade), what are the biggest titles, and **which ones has nobody ever watched**
  (checked against both finstats' history and Jellyfin's own played flags)?
- **Knows the server too** — version and pending updates/restarts, disk usage per library
  (Jellyfin 10.11+), plugins, scheduled task results, every registered device, the server
  activity log, and failed sign-ins.
- **Brings your history along** — imports Jellystat backups, including very large ones.

## Quick start

```yaml
# docker-compose.yml
services:
  finstats:
    build: .            # or image: finstats:latest once you've built it
    container_name: finstats
    restart: unless-stopped
    ports:
      - "8080:8080"
    environment:
      TZ: Europe/London   # your timezone — used for "per day" and "hour of day" stats
    volumes:
      - ./data:/data
```

```sh
docker compose up -d --build
```

No Compose plugin? Plain Docker does the same:

```sh
docker build -t finstats:latest .
docker run -d --name finstats --restart unless-stopped \
  -e TZ=Europe/London -p 8080:8080 -v "$PWD/data:/data" finstats:latest
```

Open `http://your-server:8080` and follow the two setup steps:

1. Enter your Jellyfin address (for example `http://jellyfin:8096`) and test the connection.
2. Sign in with a Jellyfin **administrator** account.

That's it. finstats creates an API key named `finstats` in Jellyfin (Dashboard → API Keys),
starts watching sessions, and copies your library and users in the background.

> The container runs as UID/GID 1000. If `./data` is owned by someone else, either
> `chown 1000:1000 data` or add `user: "<uid>:<gid>"` to the service.

## Importing from Jellystat

In Jellystat:

1. Open your Jellystat instance
2. Navigate to **Settings** and select the **Backup** tab
3. Select only **Activity** (it turns purple when selected)
4. Under settings click **Settings**
5. Scroll all the way to the end and start a backup
6. Navigate back to **Backups**
7. Select **Actions** on the backup you just took once it is visible and click **Download**

Then in finstats: **Settings → Import from Jellystat**, and drop the file in.

- Backups that also contain libraries, items and users are fine — finstats uses those tables
  to fill in anything it has not synced from Jellyfin itself.
- Importing the same backup twice is safe. Plays are de-duplicated by their Jellystat id.
- The file is streamed to disk and parsed line by line, so a 350 MB backup imports in a few
  seconds without a memory spike. An import is a single transaction: it either fully
  succeeds or changes nothing.

Headless alternative:

```sh
docker compose run --rm -v /path/to/backup.jsonl:/backup.jsonl:ro finstats import-jellystat /backup.jsonl
```

### How Jellystat data is interpreted

| Jellystat field | finstats |
|---|---|
| `ActivityDateInserted` | The **end** of the play. Start = end − `PlaybackDuration`. |
| `NowPlayingItemId` + `EpisodeId` | For episodes the first is the *series*, the second the episode. |
| `PlayState.PositionTicks` | Not imported as a position — Jellystat usually captures it when the session is first seen, not when it stops. Completion for imported plays is *time watched ÷ runtime* instead. |
| `PlayMethod: Transcode` with video *and* audio copied | Stored as `DirectStream` (a remux), same as for live plays. |
| `jf_playback_reporting_plugin_data` | Skipped: Jellystat already folds these rows into its activity table. |

## Upgrading

Pull/rebuild and restart — the database migrates itself on start-up:

```sh
docker build -t finstats:latest . && docker rm -f finstats
docker run -d --name finstats …        # the same run command as before; ./data is kept
```

Play timelines, pause/skip counts and resume points only exist for plays recorded live by
finstats 0.2+; Jellystat never captured them, so imported plays show none.

Every release is described in [`CHANGELOG.md`](CHANGELOG.md). The same file is compiled into
the binary and shown in the app under **Patch notes**, where a dot on the tab tells you an
update has landed since you last looked.

## Configuration

Everything is optional.

| Variable | Default | |
|---|---|---|
| `TZ` | UTC | Timezone for day buckets and the hour-of-day heatmap. |
| `FINSTATS_BIND` | `0.0.0.0:8080` | Listen address. |
| `FINSTATS_DATA_DIR` | `/data` in Docker, `./data` otherwise | Database and image cache. |
| `FINSTATS_TRUST_PROXY` | off | Set to `1` behind a reverse proxy so sign-in rate limiting sees the real client IP (`X-Forwarded-For`). |
| `JELLYFIN_URL` + `JELLYFIN_API_KEY` | – | Skip the setup wizard. Both or neither. Takes precedence over the wizard's values. |
| `RUST_LOG` | `finstats=info` | Log filter, e.g. `finstats=debug`. |

In the UI (**Settings**, administrators only):

- **Let other users sign in** — off by default. When on, non-admin Jellyfin users can sign in
  and see *only their own* statistics; IP addresses, other users, file paths, the server log
  and settings stay hidden from them. This is enforced by the server, not the UI.
- **Follow Jellyfin's library scan** — on by default. finstats never starts a scan on Jellyfin; it
  only reads. With this on, the (large) library read happens right after Jellyfin's own
  *Scan Media Library* scheduled task finishes — never on a separate timer and never mid-scan —
  so Jellyfin's schedule is the only schedule. A weekly safety-net read covers servers that rely
  on real-time monitoring. Turn it off to use a plain interval instead.
- **Session polling** (default 5 s), **library read interval** (default 6 h; only used when not
  following Jellyfin's scan), **merge window** (default 10 min — a play that resumes on the same
  device within the window continues the same record instead of creating a new one) and
  **ignore plays shorter than**.

## How it works

```
Jellyfin ──/Sessions every 5 s──▶ collector ──▶ SQLite ◀── stats API ◀── embedded web UI
         ──/Users, /Items, /Devices, /System/*, played flags (periodic)──▶ sync ──┘
```

- A play is written the moment it is first seen and refreshed every 30 s while it runs, so a
  restart loses seconds, not the play. Only time spent *not paused* counts as watched.
- Posters are proxied through finstats and cached on disk, so your browser never needs to
  reach Jellyfin directly (it often can't, when Jellyfin only lives on the Docker network).
- Backup: copy `data/finstats.db` (plus `-wal`/`-shm` if present) — or
  `sqlite3 data/finstats.db ".backup backup.db"` while running.

## Security notes

- Sign-in is checked against Jellyfin on every login; the Jellyfin session that check creates
  is ended immediately. finstats sessions are random 256-bit tokens, stored hashed, sent as an
  `HttpOnly; SameSite=Lax` cookie (`Secure` when served over HTTPS via a proxy).
- Sign-in attempts are rate limited per IP (10 per 5 minutes).
- The Jellyfin API key is stored in `finstats.db`. Protect the data directory accordingly.
- Until setup is completed, anyone who can reach the port can start the wizard — but finishing
  it requires a Jellyfin administrator's credentials.
- Writes are refused when the `Origin` header doesn't match; a strict Content-Security-Policy
  is sent with every response; the UI loads nothing from third parties (fonts are bundled).

## Contributing safely

The database (`data/finstats.db`) contains your users, their IP addresses and your Jellyfin
API key, and Jellystat backups contain your whole viewing history. Both are ignored by git and
by the Docker build. For a second lock, enable the bundled hook once per clone — it refuses
to commit databases, backups, `.env` files or anything over 2 MB, even when force-added:

```sh
git config core.hooksPath .githooks
```

## Building from source

```sh
cargo build --release
FINSTATS_DATA_DIR=./data ./target/release/finstats
```

Requires Rust 1.85+ and a C compiler (SQLite is compiled in). In debug builds the web UI is
read from `web/` at runtime, so you can edit and refresh. The HTTP API is documented in
[`docs/api.md`](docs/api.md).

Jellyfin 10.9 and newer are supported (responses are requested in a fixed JSON casing, so
both the 10.x and newer API generations work).

## License

MIT. Bundled fonts: Inter and JetBrains Mono, both SIL Open Font License 1.1.
