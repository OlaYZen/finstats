# Patch notes

Everything that changed in finstats, newest first. This file is compiled into the
binary and is what the **Patch notes** tab in the app shows.

Format: `## [version] - date`, then `### Added`, `### Changed`, `### Fixed` or `### Removed`
with one bullet per change. An `x.y.0` release carries a short title line under its heading:
that title is what the app uses as the headline of the whole series.

## [1.5.2] - 2026-09-22

### Changed
- A read of Jellyfin's session list and the reset of the clock that asked for it are one call, so the two can no longer come apart — the shape of the runaway-request fault found while 1.5.0 was being built.
- Proving that takes a fraction of a second instead of seven minutes against a real server, so it is checked every time anything changes. The same requests, the same history, the same figures on screen.

## [1.5.1] - 2026-09-22

### Fixed
- A transcode is recorded once, not once a second. When a play starts by transcoding and settles back to direct play, Jellyfin leaves the transcoding details on the session; each reading was compared against a value finstats had just overwritten, so every reading looked like a change and the Activity page filled with "Transcoding" lines reading "Direct play: …" underneath.
- Lines already written are cleaned up on start-up: where a run of them says the same thing, the first is kept. A play that needed transcoding still counts as a transcode.

## [1.5.0] - 2026-09-22

WebSocket session tracking

### Changed
- finstats keeps one connection open to Jellyfin from the moment it starts, and that is how it hears about a play. 1.4.0 shipped it as a setting, off by default, to prove itself against real servers; it is now always on. The same plays, pauses and skips, worked out by the same code.
- Nothing playing: finstats asks Jellyfin nothing at all. An evening when nobody is watching costs no requests.
- Something playing: the session list is read once a second. That is where a pause, a seek or a track change gets its sharpness, and it is what ends a play whose client vanished without saying goodbye.
- Everything loaded paused for three readings in a row: the asking stops and the connection carries it instead, where before a paused film cost the same question every second, around 16 MB an hour. A resume, or somebody else starting something, is answered within about a second, and paused time still never counts as watch time.
- Every outbound read asks for compressed answers and unpacks them (Jellyfin, Sonarr, Radarr and Seerr all compress when asked), which takes more than half off the two largest reads: the session list during a play, and the library read. The geolocation database is still downloaded as it is stored.
- The Jellyfin card in Settings says which of the two halves is running, and the live dot follows whether the connection is carrying rather than which half you are in.
- Only one of the two runs at a time: while finstats is asking it tells Jellyfin to stop sending, and asks it to start again the moment the last play ends. The connection stays open throughout.
- A Jellyfin that answers the subscription with nothing is believed while it answers keep-alives: finstats says so once, reads the session list itself meanwhile, and stays subscribed until the first push settles it. "Cannot push" and "was restarting when we asked" look alike, so silence is never a verdict.
- The fallback is untouched: a connection that closes, goes quiet or turns out not to speak this drops back to asking on a timer on the very next pass and keeps reconnecting, at the **While someone is watching, check every** and **While nothing is playing, check every** intervals.

### Added
- What the collector is doing, on `GET /api/status`: which of the four things it is at (`idle_socket`, `playing_poll`, `paused_socket`, `fallback`), whether the connection is open and subscribed, how often it is asking, how many session reads went out in the last minute, and when that last changed. Answered from memory — no request, no query — and it carries no names, titles or counts.
- Those fields are checked against each other on every pass, and a disagreement that lasts is written to the log.
- One limiter under every session read in the process: at most 2 a second and 70 a minute, with anything above that delayed, counted and logged at most once a minute. Normal watching stays well under it.

### Fixed
- Live updates really replace the polling. Switched on, 1.4.0 still asked for the session list every five seconds and reopened the connection every twenty to forty, because it judged the connection by how recently a list had arrived — and Jellyfin sends one when something *changes* and nothing in between. Liveness is now whether Jellyfin is answering at all, checked with a keep-alive about twice a minute.
- A session list that arrived while finstats was between passes was thrown away, on the assumption that another was a second and a half behind. It is kept and used.
- **Last checked** under Settings, and the Outbound connections card, no longer go stale while the connection is quiet: Jellyfin's answer to a keep-alive is what they show.

### Removed
- The **Let Jellyfin push what is playing** setting under Settings → Collection, and `live_socket` from the settings API.
- `socket_enabled` from the collector status: there is no "switched off" any more.
- `last_reconcile_at` from the collector status, replaced by `socket_live`.

## [1.4.1] - 2026-09-22

### Changed
- Seerr is only read in full when something has changed: every five minutes finstats asks for one row, the most recently changed request, and the pass ends there if it already knows it — about a kilobyte against about sixty for a page of fifty. Requests still on their way are looked at every quarter of an hour, everything is listed once a day, and a new request still appears within five minutes.
- The Sonarr and Radarr queues are read every five seconds while a page is showing them, every minute while something is in the queue and every five minutes while it is empty, instead of every minute around the clock. Opening the page, connecting a service or a new request in Seerr refreshes them at once.

## [1.4.0] - 2026-09-21

Optional WebSocket sessions, and everywhere finstats can reach

### Added
- **Let Jellyfin push what is playing** under Settings → Collection, off until you turn it on: one connection carries the session list, and about 17,000 requests a day become almost none. The plays, pauses, skips and groups are worked out by exactly the code that worked them out before. While it is live finstats still makes one ordinary request a minute *while something is playing*, and none when nothing is. A connection that closes, goes quiet for fifteen seconds or turns out not to speak it falls back to the timer on the next pass and keeps reconnecting.
- **Outbound connections**, a card in Settings for Jellyfin administrators: every destination finstats can reach — your Jellyfin, the "what is my IP" service, DB-IP's database, and each Sonarr, Radarr or Seerr — with what it is for, whether it is switched on, and when it last answered. Built from what finstats already knows, and it shows addresses only: never a key, never a base path.
- **Look up now** under Settings → Home network, for the day your public address changes.

### Changed
- The "what is my IP" lookup runs once, the first time finstats needs an address, instead of every fifteen minutes.
- The Jellyfin card in Settings says how the collector is being told, and, when the connection is switched on but not carrying, why not.

## [1.3.0] - 2026-09-20

Sonarr, Radarr and Seerr, on one page

### Added
- **Connections** under Settings, for Jellyfin administrators: **Sonarr**, **Radarr** and **Seerr** (or Jellyseerr/Overseerr), several of a kind, each tested before it is saved. Your download client needs no setup of its own, because Sonarr and Radarr already talk to it and finstats reads what they know.
- **Requests**: who asked for what, how long it took to arrive, and whether they ever watched it. Tiles for titles waiting and the typical wait, a chart of how that changed month by month, and lists of what arrived weeks ago and was never played, and who asks for the most.
- **Upcoming**: new episodes and film releases from Sonarr and Radarr as an agenda by day, marked with whether *you* watch that show and, for people who may see everyone, who else does — a show counts as watched when somebody played an episode of it in the last four months. A "Coming up" row on the dashboard, one per profile for the shows that person watches, and what is next for a title on its own page.
- **Downloads**: the live queue of every Sonarr and Radarr — what it is, how far along, how fast, what went wrong on import — with the person who asked for it beside it. A season pack is one line however many episodes it holds, and what needs attention is on top. Below it, what came in over a week, a month or a year by indexer, quality and download client, and which downloads failed. The dashboard shows the same list, shortened.
- **See what is downloading**, a new permission. Without it people still see how far their *own* request has got and how long is left, and nothing else: no release names, no speeds, no other downloads. Other people's requests need *See everyone's activity*.

### Changed
- finstats sends `GET` to these services and nothing else: there is no code in it that could approve a request, start a search, or add, pause or remove a download.
- Their API keys are stored in finstats' own database, never sent back to the browser, never written to a log and never part of a backup. No redirect is followed, so a key cannot travel somewhere you did not enter, and certificates are verified unless you switch that off for one connection.

## [1.2.2] - 2026-09-20

### Added
- Audio and subtitle languages on every film and episode ("Audio: Japanese · English").
- How far a dub goes: a show or season says how many of its episodes have each language ("English: 13 of 26 episodes"), and the episode list has an **Audio** column.
- **Audio languages** and **Subtitle languages** on library pages: how many files can be played in each.
- finstats re-reads the library once after this update to pick the languages up. It never says "dubbed", because Jellyfin does not give a title's original language, and a track without a language tag is listed as "Unknown".

## [1.2.1] - 2026-09-20

### Changed
- The pictures in the README are retaken at this version, with a new one of the **Security** page. Nothing in the app changes.

## [1.2.0] - 2026-09-20

Where people watch from, on a map

### Added
- **Security page**, for people who may see network details and everyone's activity: a world map with a dot for every place your users watch from, sized by how much happens there. Home is green, places away are purple, live streams pulse and failed sign-ins from outside are red. Drag to move, zoom with the buttons, the keys or Ctrl + wheel, and overlapping dots merge until you zoom in. Below it, every place with its people, plays, watch time and sign-ins, and the countries by plays.
- **Impossible travel**: an alert when one account is seen in two places no flight connects, or in two distant places at once, with both sightings, the distance, the time between them and the speed that would have taken. **Show on map** draws the trip.
- **New country**: the first time someone plays or signs in from a country they have not been seen in.
- Resolve an alert with a note, or mute a pair of places for that person, for a VPN or a phone whose carrier sits in the capital. Alerts found in old history, after an import or on a fresh database, are filed as resolved rather than flooding the list. Alerts and what you decided about them are part of backups.
- Places come from a city database read locally: no address is ever sent anywhere, and the map is drawn from outlines bundled with finstats rather than a map service. Download DB-IP's free database from the Security page or **Settings → Security**, let finstats refresh it monthly (off until you switch it on), or drop your own `.mmdb` (DB-IP, MaxMind GeoLite2-City) into `data/geoip/`. `FINSTATS_GEOIP_DB` names a file elsewhere.
- **Settings → Security**: the database in use, the monthly update, and how fast (900 km/h) and how far apart (500 km) two sightings must be to count as impossible travel. City databases are often a few hundred kilometres off, which is what the distance is for.

## [1.1.3] - 2026-09-20

### Added
- Ctrl+Space also searches the cast and crew of everything in your library, under **Cast and crew**, with their photo, whether they act or direct, and how many of your titles they are in; choosing one opens their page. As forgiving as the title search — words in any order, accents ignored, a slip of the finger allowed — and when several match equally well, the one in more of your titles comes first.

## [1.1.2] - 2026-09-20

### Added
- **Recently added** on the dashboard, above the Activity chart: the 30 newest arrivals as a row of posters you can scroll sideways, with arrows on a computer. Each says when it arrived and what it is — a film, an album, or "Season 4 · 3 episodes" — and a single new episode says which one, with the season's own poster when it has one.
- **Shift + mouse wheel** and the arrow keys move one poster, Page Up and Page Down a screenful, Home and End go to either end, and swiping works. The plain wheel still scrolls the page.
- New episodes of the same show that arrive on the same day share one entry, and a whole show added at once is one entry ("3 seasons · 60 episodes"), so one big import does not push everything else off the list.
- The row ignores the dashboard's time range and person, since it is about the library and not about plays, and hides itself while the library is empty.

## [1.1.1] - 2026-09-20

### Added
- Pages open at once: a moment after finstats has loaded it quietly fetches what you are likely to open next — the dashboard, your profile and timeline, Activity, Users, Libraries and each library, Playback, the server pages, the recap and the most active people — so opening one shows it immediately and it still refreshes behind the scenes.
- Films, shows and cast members are fetched on intent only, as there are thousands of them: resting the pointer on a link, touching it or reaching it with the keyboard fetches that one page.
- It waits until the page you opened has finished loading, works one page at a time while the browser is idle, pauses in a background tab, does nothing on a data-saver or very slow connection, and is forgotten when you sign out.
- A small **Repo** link with the GitHub mark in the status bar, next to the version.

### Fixed
- The **Timeline** forgot which libraries were switched off when the page was reloaded or opened from a bookmark, although the choice was in the address.

## [1.1.0] - 2026-09-20

A timeline of everything you have watched

### Added
- **Timeline**, next to **Overview** on every profile: your watching as one trail from today back to the first play finstats knows about, with an evening of episodes folded into a single stop. Each stop is a poster with what was watched and when — "Season 2 · Episodes 3-6", a film and whether it took more than one sitting, or an album and how many tracks — and opens the title.
- The trail follows the width of the window: three stops to a row on a wide screen, two on a narrower one, every other row running backwards with a bend joining it to the next, down to a single straight line on a phone. The first stop of each month carries the month.
- Older stops load by themselves as you scroll, all the way back to where the history starts.
- **Libraries** switches above the trail leave out what you do not want to see, such as music, and the choice is part of the address.
- Something that is still playing says **Playing now**.
- You see your own timeline; other people's need the "see everyone" permission.

## [1.0.5] - 2026-09-20

### Added
- Downgrade protection: the database remembers the newest finstats version that has opened it, and an older version refuses to start on it instead of quietly working on data it does not fully understand. The message says what to do. Versions up to 1.0.4 were released before this check existed.

## [1.0.4] - 2026-09-20

### Fixed
- In a play's details, the copy button next to a long **Device ID** had dropped onto a line of its own.
- On the Playback page the last column of **Which clients transcode** was cut off ("Transc…"); two-word headers wrap now.

### Changed
- The screenshots in the README are current again, having still shown version 0.5.0.

## [1.0.3] - 2026-09-20

### Added
- An episode page shows that episode's own picture, the one Jellyfin shows in its episode list, instead of the show's poster. An episode Jellyfin has no picture for keeps the poster.

## [1.0.2] - 2026-09-20

### Fixed
- In **Patch notes**, the v0.9 and v0.10 groups had no headline next to their version. A group's headline is now always the first sentence of what its first release was about, so a long introduction is not cut off mid-sentence.

## [1.0.1] - 2026-09-20

### Fixed
- The published image would not start on a fresh machine: "unable to open database file: /data/finstats.db", after hanging for half a minute. Docker creates a missing `data` folder as root and finstats runs as an ordinary user. The container now makes the folder its own and then drops to that user, so `docker run` works on the first try. For files owned by someone other than user 1000, set `PUID` and `PGID`; `--user` works as before.
- A data folder that really cannot be written is reported at once, with the command that fixes it, instead of 30 seconds later as a database error.

## [1.0.0] - 2026-09-20

Ready for everyone

### Added
- A ready-made image at `ghcr.io/olayzen/finstats`, for 64-bit Intel/AMD and ARM machines (a Raspberry Pi 4 or 5 works): `:latest` is the newest release, `:1` follows every 1.x update, `:1.0.0` stays where it is, and `:edge` is the development version.
- Every release appears on the project's GitHub releases page with these patch notes.

### Changed
- Version numbers now mean what they say: 1.x updates will not break your data, your backups or your settings.
- The Docker Compose file and all instructions use the published image. A self-built `finstats:latest` only needs the image name changed; the `data` folder carries over untouched.

## [0.10.2] - 2026-09-20

### Fixed
- On a phone, Settings and the page of a show could scroll sideways: a label meant only for screen readers escaped its table, and the episode lists and the cast row did the same. They scroll inside themselves now.
- The recap no longer scrolls sideways when a chapter's background word is wider than the page.
- A long "watching on…" line under **Now playing** ends in an ellipsis instead of being cut mid-letter.
- The poster in front of each show on a profile was a second, nameless link for screen readers; it is hidden from them and the title next to it is the link.

## [0.10.1] - 2026-09-20

### Fixed
- Downloading a backup failed in the browser ("the source file could not be read" in Firefox) with nothing on the page to say why: finstats was compressing a backup, which is already compressed, a second time, and the doubly-packed transfer broke off before the end. Backups are sent as they are, with their size up front, so the browser can show real progress.

## [0.10.0] - 2026-09-20

Backups, and moving to a new install

### Added
- A backup every week, newest five kept: every play with its pause-and-skip timeline, seen marks, permissions, home addresses, the server log and your settings, in one small file — a few hundred KB for thousands of plays. **Settings → Backups** lists them with a **Download** button each, makes one on demand, and sets how often and how many. They live in the `backups` folder of your data directory. A backup holds no Jellyfin API key and no sign-in session, but it is a complete viewing history with IP addresses, so keep it somewhere private.
- Restoring merges rather than overwrites: plays already there are skipped, so it is safe to do twice or into an instance that has been running for a while. Untick "Also restore settings and permissions" for the history only. Backups work across versions in both directions. From a terminal: `finstats backup` and `finstats restore <file>`.

### Changed
- Collection checks every second while someone is watching and every 5 seconds while nobody is, where it used to be every 5 seconds throughout, so pauses, skips and track changes are recorded to the second. Both are under **Settings → Collection**; a changed old interval needs setting again.

### Fixed
- An IP address in a play's details broke across two lines in the middle of the address. The Local/Remote mark moves underneath when there is no room.

## [0.9.1] - 2026-09-20

### Fixed
- People at home were shown as remote. A device on your own network that reaches Jellyfin through its public name, such as a reverse proxy or a domain, arrives with the household's public IP, and only private addresses counted as local. finstats now learns the public address and counts those plays as local for the whole history, and it remembers earlier addresses.

### Added
- **Settings → Home network**: the addresses finstats treats as home, adding your own (an earlier address, a second home, a VPN exit), or switching the lookup off. The lookup asks a plain "what is my IP" service every 15 minutes, trying several (Amazon, Cloudflare and others) because ad-blocking DNS such as Pi-hole often blocks them, and one of them needs no DNS at all. The request contains nothing about you or your server, and with the switch off finstats talks to nothing but Jellyfin. `FINSTATS_PUBLIC_IP_URL` sets a service of your own.

## [0.9.0] - 2026-09-20

Every table, in the order you want

### Added
- Every table sorts: click a header to sort by it, again to turn it around, a third time for the original order. Cell meaning is understood, so "3d 2h" sorts as time and "1.4 GB" as size, and empty cells go last. It covers the user list, watchers, episodes, devices, plugins, scheduled tasks, the table view of every chart, the permission matrix, and the bar lists (codecs, resolutions, clients and the rest), which get a slim header of their own.
- Activity and the Server log sort across all their pages rather than the rows on screen, and the order is kept in the address, so it survives a reload or a shared link.
- Tables of ten rows or more get a small filter field above them.

## [0.8.2] - 2026-09-20

### Changed
- The recap's year picker is the same switch as every other range picker in finstats (7d · 30d · 90d …).

## [0.8.1] - 2026-09-19

### Added
- Pages for actors and directors, from the recap's "Most watched people" or the new **Cast & crew** row on any film, show or episode: everything they are in on your server, how much has been watched and by whom, and what is still waiting. It follows the time range you pick, people who may only see their own statistics see only their own watching there, and Esc takes you back.

## [0.8.0] - 2026-09-19

The recap has a new look, and more to say

### Added
- **Most watched people**: the actors you spent the most time with and the directors behind what you watched, as a row of portraits. Cast and crew are read along with the library and appear after the first library read following this update.
- **Your year in days**: every day of the year as one square, brighter the more you watched, with your longest streak and biggest day above it. Hover a day, or walk the calendar with the arrow keys, to see what it held.
- **By the numbers**: plays, watch time, different titles, days watched, longest streak and how much was a **rewatch**, plus how the year splits between episodes, movies and music.
- The genre chapter says how many genres you touched and how large a share the top one took.

### Changed
- A new design: it opens on your year "replayed", the headline beside the posters that filled it, over a waveform of the year with one bar per week and the loudest lit, and you pick the year right above it. Every chapter has a headline with its key word in violet, a sentence that carries the figures, and the chapter's word standing large and faint behind it. It ends the way films do, with the credits: starring, directed by, screened on, running time.
- Hours, weekdays and months share one **Activity patterns** chapter with a switch between them, and the headline follows what you are looking at ("Friday took the crown").

### Fixed
- **Now playing** on the dashboard was left blank when nobody was watching, so the heading seemed to belong to the filters under it. Its "Nothing is playing right now" box is back.

## [0.7.10] - 2026-09-19

### Added
- Library artwork: each library card, and the library's own page, shows the picture Jellyfin uses for it, through finstats' image proxy like the posters. A library without one keeps its icon.

## [0.7.9] - 2026-09-19

### Changed
- The Genres card opens as the **list** again, with the radar as the second option on the switch. An existing choice is kept.

## [0.7.8] - 2026-09-19

### Added
- A radar view for the Genres card, on the dashboard and on profiles: one spoke per genre, switched between **Radar** and **List** in the card's corner, and the choice is remembered. Hovering a point, or using the arrow keys, shows that genre's watch time and share.

## [0.7.7] - 2026-09-19

### Fixed
- Movies that belong to a collection were shown as "No longer in library". With *group movies into collections* switched on, Jellyfin hands out the collection instead of the films inside it, so finstats never saw them — on one server that hid 556 of 1,238 items in the Movies library. finstats now asks for the films themselves, and the read this update triggers brings them back and re-attaches their plays.

## [0.7.6] - 2026-09-19

### Changed
- On profiles the **Shows** card moved down to just above Genres, so a long list of shows no longer pushes the watch time, activity chart and top titles out of sight. The day streaks stay at the top, and a show you have opened stays open when you change the time range.

## [0.7.5] - 2026-09-19

### Changed
- Search matches word by word, in any order, with anything in between, instead of needing the exact phrase, so "alya hides" finds *Alya Sometimes Hides Her Feelings in Russian*. Case, accents and punctuation do not matter, a leading "The" is ignored, and a slip of the finger is forgiven ("mentalsit" finds *The Mentalist*); words of three letters or fewer still have to be typed right. Better matches rank first, and music is also found by its artist.
- The search boxes on Activity and the Server log match word by word across all their columns: "alya opera" finds plays of that show in Opera.

## [0.7.4] - 2026-09-19

### Changed
- Patch notes are grouped by series: every `0.x` series is one fold holding its releases, with what the series was about, how many there are and when. Only the newest starts open, and a closed fold still shows which one you are running.

## [0.7.3] - 2026-09-19

### Fixed
- The Now playing clock hesitated every few seconds: the 5-second refresh was also repainting it, so whenever the server was a fraction ahead the number moved early and then stood still for a second. Only the once-a-second tick moves it now, by exactly one second (measured 999 to 1001 ms apart), and the refresh still corrects after a pause, a skip or real drift, landing on the tick.

## [0.7.2] - 2026-09-19

### Added
- Group watching, live: each Now playing card says "With maria" while people are watching the same thing together, where a group used to be recognised only once the plays had ended. Live, people count as together when they are on the same title and either started within the group window or are at nearly the same position, which still works if finstats was restarted mid-stream.

## [0.7.1] - 2026-09-19

### Changed
- Now playing counts every second: the clock ticks and the bar glides in between, instead of jumping each time the server is asked. The server is still asked every 5 seconds and stays the source of truth — the display re-syncs on a pause, a skip or when the server is ahead, but not for a position merely a few seconds stale, since clients report only every ten seconds or so. Paused streams stand still.

## [0.7.0] - 2026-09-19

Who watches together

### Added
- Group watching: when different people start the same title at the same time, finstats counts it as watching together. A **Watched together** card on the dashboard shows the groups, the titles they share and their hours together, each profile shows who that person watches with most, shared plays carry a small people mark in Activity, and the play details say who it was watched with. It works on your whole history, imported plays included.
- A setting for how close together the starts must be, 60 seconds by default: about a third of genuine group sessions start 6 to 60 seconds apart. People also have to keep watching alongside each other for a couple of minutes, so two people opening the same episode by coincidence is not a group.

### Changed
- Search opens with **Ctrl + Space** instead of Ctrl/⌘ + K.

## [0.6.1] - 2026-09-19

### Added
- Administrators can open another person's recap, at the top of the Recap page or with **Open recap** on their profile. It then reads about them by name instead of "you".

### Changed
- Everyone else still sees only their own recap, whatever permissions they hold, and there is still no recap of the whole server.

## [0.6.0] - 2026-09-19

Your profile, and where you are in every show

### Added
- Show progress on profiles: every series you have touched as a bar with one segment per episode — seen, started or not yet — openable season by season, sorted by how close you are to finishing, with separate views for finished shows and everything.
- **Mark as seen** on your own profile, per episode, season or show, for what you watched while nothing was recording. Jellyfin's own played marks are used as well. Marks stay in finstats and never change anything in Jellyfin; a recorded play cannot be unmarked, your own marks can.
- Day streaks on profiles: your longest run of days in a row with a play, your current streak, and how many days you have watched something.
- **My profile** in the sidebar.
- Activity shows where playback stopped (for example `44:15 / 45:00`) under the progress bar, and the date and time under "3h ago". Plays imported from Jellystat have no stop position, because Jellystat never recorded one.

### Changed
- Episodes that are missing or have not aired yet are no longer treated as part of your library. Jellyfin lists them as placeholders without a file, and they used to count towards episode totals.

## [0.5.1] - 2026-09-19

### Added
- A logo: a play button sliced into three chart bars, in the sidebar and on the sign-in screens, as a sharp SVG with a multi-size `favicon.ico` for browsers and bookmark bars that want one, plus a home-screen icon for phones and tablets.

## [0.5.0] - 2026-09-19

You decide who sees what

### Added
- Permissions under Settings → Access, for everyone at once or person by person: **see everyone's activity** (other people's statistics and history, the Users page, every live stream), **see network details** (IP addresses, device ids, local or remote), **see the server** (the Server page, the server log, failed sign-ins, file paths) and **manage finstats** (settings, tasks, import, deleting plays).
- Sign-in per person, to let a few people in without opening finstats to every Jellyfin user.
- Changes apply immediately, including taking access away, and everything is enforced by the server rather than by hiding buttons.

### Changed
- Jellyfin administrators still have everything and are the only ones who can change permissions; someone who may manage finstats cannot grant anything, to themselves or anyone else.
- The year recap stays personal whatever is granted.
- Nothing changes for existing installs until you grant something.

## [0.4.5] - 2026-09-19

### Changed
- finstats is licensed under the **GNU General Public License v3.0**. No earlier version was ever published, so nothing was released under different terms. The licenses of the bundled fonts ship alongside them.

## [0.4.4] - 2026-09-19

### Added
- Esc steps back: on an opened library, user or title it returns you to where you came from, with the list exactly as you left it. Anything open closes first (play details, search, a dropdown, the mobile menu), and Esc is left alone while you are typing or using a chart with the keyboard.

## [0.4.3] - 2026-09-19

### Changed
- finstats remembers what each page last showed: going back to a profile, a library or a time range you have already looked at paints it immediately, profile picture and all, while fresh numbers load behind it, and the page only redraws if something changed. What is remembered lives in the browser tab and is dropped when you sign out or change anything.
- Loading placeholders appear only when loading is actually slow. Before, every page showed them for at least a third of a second.

### Fixed
- Libraries that were deleted in Jellyfin and never had a play are no longer listed. A deleted library with history is still shown, marked as removed.

## [0.4.2] - 2026-09-19

### Fixed
- On the Users page the **Admin** tag sat on its own line and made that row taller than the others. Tags sit beside the name now.

## [0.4.1] - 2026-09-19

### Fixed
- The word "null" no longer appears under the dashboard title outside December and January, when there is no recap banner to show.

## [0.4.0] - 2026-09-19

See what changed without leaving the app

### Added
- **Patch notes** tab showing every release, with the version you are running marked.
- The version in the status bar links to it, and the tab shows a dot after an update until you have looked.

### Changed
- Re-attaching history to renamed items is far faster: 18 seconds to a third of a second with 666 orphaned titles, so start-up stays instant as history grows.

## [0.3.3] - 2026-09-19

### Changed
- The library is re-read right after Jellyfin's **Scan Media Library** task finishes, never on a separate timer and never while a scan is running. finstats has always been read-only towards Jellyfin, and it never starts a scan there.
- A weekly safety-net read covers servers that rely on real-time monitoring, where the scan task may never run.
- The old interval is now only a fallback, for when following is switched off or the server reports no scan task.

### Added
- **Follow Jellyfin's library scan** switch under Settings → Collection, on by default.

## [0.3.2] - 2026-09-19

### Fixed
- History survives renames. Jellyfin gives a renamed file a new id, which used to strand its earlier plays under the raw folder name (`Title (2010) [tmdbid-…] [imdbid-…]`) with no poster. Those plays are re-attached to the current library entry: by TMDB/IMDb/TVDB id when the old name carries one, otherwise by cleaned title and year, and episodes by series plus season and episode number. A match has to be unambiguous, so titles that were really deleted stay as they are.
- Live TV channels imported from Jellystat were counted as movies, because Jellystat records no item type. They are recognised as Live TV, and new Live TV plays are recorded with Jellyfin's own type.

### Changed
- Live TV is left out of the recap entirely: top lists, totals, persona, records and rank.

### Added
- `finstats relink` to re-attach history on demand. It also runs at start-up and after every library read.

## [0.3.1] - 2026-09-19

### Changed
- The recap is strictly personal: everyone, administrators included, sees only their own, and the "Everyone" view, the user switcher and the server chapter are gone.
- The recap opens on the year that is "ready": the current year during December, otherwise the previous one. Other years and "Last 12 months" remain one click away.

## [0.3.0] - 2026-09-19

Your year in review

### Added
- **Recap** tab, a scrolling story of a year: hours watched, top shows, movies, tracks and genres, and a viewing persona (night owl, early bird, weekend warrior, binge watcher, movie buff, music lover, creature of habit) with hour-of-day and weekday charts.
- The year month by month, with the title that defined each month.
- Records: biggest day, biggest binge, longest daily streak, longest single play, most rewatched title, first play of the year and the oldest title watched.
- Discovery: new shows started, movies and episodes finished, and the shows that got exactly one episode.
- A one-line banner on the dashboard in December and January when a recap is ready.

## [0.2.0] - 2026-09-19

More than Jellystat ever recorded

### Added
- A timeline for every play: start, pause, resume, skip, audio and subtitle switches, direct play turning into a transcode, and stop. Plus pause and skip counts, where playback resumed from, and whether the viewer was on the local network.
- **Server** tab: Jellyfin version, pending updates and restarts, disk usage per library (Jellyfin 10.11+), plugins, scheduled task results and every registered device.
- **Playback** insights: concurrent streams over time, which clients force transcodes, how far people get before stopping, local versus remote plays and viewing behaviour.
- What your library is made of: resolutions, codecs, dynamic range, containers, titles per decade, additions per month, the largest titles, and the titles nobody has ever watched, checked against both finstats' history and Jellyfin's own played flags.
- Dashboard: peak concurrent streams, estimated data streamed, share of remote plays, genres by watch time and failed sign-ins.
- Item pages: IMDb and TMDB links, studios, bit depth and frame rate, and who has the title marked as played in Jellyfin.
- User pages: genres, and the movies, episodes and favourites Jellyfin has on record.

### Fixed
- The time label on a now-playing card no longer spills outside the card.
- Long labels on the Playback page were cut off too early.
- A series that has left the library still shows its season and episode list.

## [0.1.0] - 2026-09-19

First release

### Added
- Live collection from Jellyfin's sessions: user, title, client, device, IP address, play method, transcode reasons and hardware acceleration, video, audio and subtitle details, and time watched versus time paused.
- Sign-in with your Jellyfin account. A two-step setup creates finstats' own API key, and passwords are never stored. Non-admin users can optionally sign in to see only their own statistics.
- Dashboard, Activity, Users, Libraries, Playback and Server log pages, a command palette (Ctrl/⌘ K) and a live status bar.
- Jellystat import: large backups stream to disk and import in seconds, and importing the same file twice is safe.
- A single small binary with an embedded web UI and one SQLite file, plus a Docker image.
