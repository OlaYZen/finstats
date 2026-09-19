<p align="center"><img src="web/assets/logo.svg" width="88" height="88" alt=""></p>
<h1 align="center">finstats</h1>

<p align="center">
  <b>See what your Jellyfin server is really doing.</b><br>
  Who is watching, what they watch, how it streams — and your year in review.<br>
  One tiny container. No database server. Set up in two minutes.
</p>

<p align="center">
  <img src="docs/screenshots/dashboard.png" alt="The finstats dashboard: two live streams, watch-time tiles and an activity chart" width="100%">
</p>

## Why finstats

Jellyfin tells you what is playing right now. It does not tell you that one client transcodes
everything it touches, that four people stream at once every Saturday, or that a third of your
disk is films nobody has ever pressed play on.

finstats watches your server quietly in the background and turns that into answers. It is a
lightweight alternative to Jellystat and Streamystats: a single small program with its own
built-in database, using about **20 MB of memory**. Nothing else to install, nothing to maintain.

Already using Jellystat? [Bring your history with you](#moving-from-jellystat) — it takes seconds.

## What you get

### A live view of your server
See every stream as it happens: who, what, on which device and from where, whether it plays
directly or transcodes, and why. A status bar keeps the essentials in sight on every page.

### Answers, not just charts

<img src="docs/screenshots/playback.png" alt="Playback page: play methods, concurrent streams over time, which clients transcode, how far people get" width="100%">

- **How busy does it get?** Peak concurrent streams over time, and how many of them transcode.
- **Which apps cause transcoding?** Every client, split by direct play, remux and transcode, with the reasons Jellyfin reports.
- **How much leaves the house?** Local versus remote plays and an estimate of data streamed.
- **Do people finish what they start?** See how far viewers get before they stop.
- **Want it in a different order?** Every table sorts by any column with a click, and long ones can be filtered as you type.
- **What is my library made of?** Resolutions, codecs, HDR, size per decade, what was added when —
  and the big one: **titles nobody has ever watched**, sorted by how much space they take.

### Every play, down to the pause button

<img align="right" src="docs/screenshots/timeline.png" alt="Play details with a timeline: started, paused, resumed, subtitles switched, skipped ahead, stopped" width="46%">

Other tools store one line per play. finstats records what happened *during* it: every pause and
resume, every skip, audio and subtitle switches, the moment a direct play turned into a transcode,
where playback picked up and where it stopped.

Alongside the usual details — device, app version, IP address and whether it was on your network,
video and audio format, bitrate, time watched versus time paused.

Every film and show lists its cast and crew, and every actor and director has a page of their own:
what they are in on your server, and how much of it has been watched.

Renamed a file? Jellyfin treats it as a new item and orphans its history. finstats notices and
re-attaches the old plays to the new entry.

### Who watches together
When two or more people press play on the same thing at the same time, finstats notices: which
groups watch together, what they watch, and how many hours they have spent doing it. It shows on
the dashboard, on each profile ("most often with"), and as a small mark on every shared play.

### Where you are in every show
Your profile shows each series as a bar with one segment per episode: seen, started, or not yet.
Only episodes that are actually on your server count, so an announced season does not spoil a
finished show. Watched something while nothing was recording? Jellyfin's own played marks fill the
gap, and you can mark episodes, seasons or whole shows as seen yourself. Alongside it: your longest
and current day streak.

<br clear="right">

### Your year in review

<img src="docs/screenshots/recap.png" alt="Recap: Your 2025, replayed — the top posters fanned out beside the headline, above a waveform of the year with one bar per week" width="100%">

A personal recap for every user, in the spirit of Spotify Wrapped: hours watched, top shows, movies,
music and genres, the actors and directors you spent the most time with, and a viewing
personality — night owl, weekend warrior, binge watcher and more. See the whole year as a calendar
of days, find out which weekday took the crown, and collect the records worth bragging about:
biggest binge, longest daily streak, most rewatched title, the oldest film you watched.

Each person sees only their own. Administrators can open another person's recap; nobody else can,
whatever permissions they hold, and there is no recap of the whole server.

### Private by design
- **Sign in with your Jellyfin account.** No new passwords, and finstats never stores yours.
- **You decide who sees what.** Let family and friends sign in if you like. By default they get
  their own statistics and recap and nothing else. From there you grant more, per person or for
  everyone: other people's activity, network details like IP addresses, the server pages, or
  managing finstats itself. No permission opens other people's recaps.
- **Nothing leaves your network.** No telemetry, no external services, no fonts or scripts loaded
  from the internet. Posters are fetched from your own Jellyfin.
- **Read-only.** finstats never changes anything on your Jellyfin server and never starts a library scan.

## Get started

You need Docker and a Jellyfin server (10.9 or newer).

```sh
docker run -d --name finstats --restart unless-stopped \
  -p 8080:8080 \
  -e TZ=Europe/London \
  -v "$PWD/data:/data" \
  finstats:latest
```

<details>
<summary>Prefer Docker Compose?</summary>

```yaml
services:
  finstats:
    image: finstats:latest
    container_name: finstats
    restart: unless-stopped
    ports:
      - "8080:8080"
    environment:
      TZ: Europe/London   # your timezone
    volumes:
      - ./data:/data
```

</details>

There is no published image yet — build it once from this repository with
`docker build -t finstats:latest .`

Then open **http://your-server:8080** and:

1. Enter your Jellyfin address and test the connection.
2. Sign in with a Jellyfin administrator account.

That's it. finstats starts watching immediately and fills in your library in the background.
Set `TZ` to your own timezone so "today" and "evening" mean what you expect.

> Inside a container, `localhost` is the container itself. Use your server's address
> (for example `http://192.168.1.10:8096`) or the Jellyfin container's name.

## Moving from Jellystat

Your history comes with you. In Jellystat:

1. Open **Settings → Backup**
2. Select only **Activity** (it turns purple)
3. Under settings click **Settings**, scroll to the end and start a backup
4. Go back to **Backups**, open **Actions** on the new backup and click **Download**

In finstats, open **Settings → Import from Jellystat** and drop the file in.

Large backups are no problem — a 350 MB file imports in a few seconds — and importing the same
file twice is safe. One thing to know: Jellystat never recorded what happens during a play, so
imported history has no pause-and-skip timelines. Everything finstats records from now on does.
[How imported data is interpreted →](docs/jellystat-import.md)

## Settings

Everything works out of the box. If you want to tune it, **Settings** in the app has:

| | |
|---|---|
| **Access** | Who may sign in and what they may see, for everyone or per person: *sign in*, *see everyone's activity*, *see network details*, *see the server*, *manage finstats*. Jellyfin administrators always have everything, and only they can change this. |
| **Follow Jellyfin's library scan** | On by default. finstats refreshes its copy of your library right after Jellyfin's own scheduled scan — no second schedule to manage. |
| **Check for playback every** | How often finstats looks for streams. Default 5 seconds. |
| **Treat a restart as the same play** | A stream that stops and resumes within 10 minutes counts as one viewing. |
| **Count it as watching together within** | How close together different people must start the same title to count as a group. Default 60 seconds. |
| **Ignore plays shorter than** | Leave accidental clicks out of the statistics. |

<details>
<summary>Environment variables</summary>

| Variable | Default | |
|---|---|---|
| `TZ` | UTC | Your timezone, for per-day and hour-of-day statistics. |
| `FINSTATS_BIND` | `0.0.0.0:8080` | Address to listen on. |
| `FINSTATS_DATA_DIR` | `/data` | Where the database and poster cache live. |
| `FINSTATS_TRUST_PROXY` | off | Set to `1` behind a reverse proxy so sign-in rate limiting sees real client addresses. |
| `JELLYFIN_URL` + `JELLYFIN_API_KEY` | – | Skip the setup wizard. Set both or neither. |
| `RUST_LOG` | `finstats=info` | Log detail, e.g. `finstats=debug`. |

</details>

## Updating

Rebuild or pull the new image and start the container again with the same command. Your data
lives in the `data` folder and upgrades itself on start-up. After an update, the **Patch notes**
tab shows a dot until you have read what changed — the same notes live in [CHANGELOG.md](CHANGELOG.md).

## Questions

**Does it slow Jellyfin down?**
No. It asks Jellyfin one small question every few seconds and reads your library only after
Jellyfin has finished its own scan.

**Where is my data, and how do I back it up?**
In one file: `data/finstats.db`. Copy it while finstats is stopped, or run
`sqlite3 data/finstats.db ".backup backup.db"` while it is running.

**Can I put it behind a reverse proxy?**
Yes. Forward to port 8080 and set `FINSTATS_TRUST_PROXY=1`. Sign-in cookies are marked secure
automatically when the proxy reports HTTPS.

**How do I start over?**
Stop the container, delete the `data` folder, start it again. To clean up fully, also remove the
`finstats` API key in Jellyfin (Dashboard → API Keys).

**Is it safe to expose to the internet?**
It is built for it — Jellyfin-backed sign-in, rate limiting, hashed sessions, a strict content
security policy — but like anything self-hosted, a reverse proxy with HTTPS is strongly
recommended. [Security details →](docs/security.md)

## For developers

finstats is written in Rust with a dependency-free web UI compiled into the binary.

```sh
cargo build --release
FINSTATS_DATA_DIR=./data ./target/release/finstats
cargo test
```

- [HTTP API](docs/api.md) — the contract the web UI is built on
- [How Jellystat data is interpreted](docs/jellystat-import.md)
- [Security model](docs/security.md)
- [Patch notes](CHANGELOG.md)

The database holds your users, their IP addresses and your Jellyfin API key, and a Jellystat
backup is a complete viewing history. Both are ignored by git. Before contributing, switch on the
bundled commit guard as a second lock:

```sh
git config core.hooksPath .githooks
```

## License

finstats is free software under the [GNU General Public License v3.0](LICENSE). You may use,
study, share and change it; if you distribute a modified version, it has to stay under the same
license with its source available. It comes with no warranty.

The bundled fonts, Inter and JetBrains Mono, are under the SIL Open Font License 1.1
([Inter](web/assets/fonts/LICENSE-Inter.txt), [JetBrains Mono](web/assets/fonts/LICENSE-JetBrainsMono.txt)).

<sub>Screenshots show generated demo data: invented users, titles and artwork.</sub>
