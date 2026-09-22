# Patch notes

Everything that changed in finstats, newest first. This file is compiled into the
binary and is what the **Patch notes** tab in the app shows.

Format: `## [version] - date`, a one-paragraph summary (required for an `x.y.0` release: its first sentence is the
headline of that series in the app; optional otherwise), then `### Added`,
`### Changed`, `### Fixed` or `### Removed` with one bullet per change.

## [1.5.0] - 2026-09-22

Each way of hearing about a play, doing the half it is good at. finstats listens while nothing is playing — an idle server has nothing to report, and asking it every few seconds to be told so was almost all the traffic finstats ever caused — and asks, every second, while something is, because a pause, a skip or a change of audio track is only as sharp as the gap between two sightings. A film left paused goes back to being listened for, since a frozen position is the one thing a server has nothing to say about. None of it is a switch you have to find any more: the connection to Jellyfin is open from the moment finstats starts. Everything finstats reads is compressed now, so the busy half is the cheap half as well, and what the collector is doing at any moment is something you can read off rather than guess at.

### Changed
- **Jellyfin pushes what is playing, always.** finstats keeps one connection open to Jellyfin from the moment it starts, and that is how it hears about a play. It was given 1.4.0 off by default, as a setting, to prove itself against real servers, and it has — so an install that never turned it on is no longer quietly asking Jellyfin seventeen thousand times a day to be told that nobody is watching. Nothing about your history changes: the same plays, the same pauses and skips, worked out by exactly the same code.
- **Nothing playing: finstats asks Jellyfin nothing at all.** It listens, and Jellyfin says when something starts. An evening when nobody is watching costs no requests at all.
- **Something playing: finstats asks, every second.** That is where the detail is — a pause, a seek or a track change is only as sharp as the gap between two sightings, and how often a server volunteers one is its business rather than something finstats can promise. Asking is also what ends a play whose client vanished without saying goodbye.
- **A paused film costs nothing.** Leaving something paused used to mean asking Jellyfin the same question every second for as long as it sat there, which is around 16 MB an hour of asking about a position that had stopped changing. Once everything loaded has been paused for three readings in a row, the asking stops and the live connection carries it instead; a resume, or somebody else starting something, is pushed and answered within about a second. One person watching still means asking, whoever else has paused, and paused time still never counts as watch time.
- **finstats asks for compressed answers now.** Jellyfin, Sonarr, Radarr and Seerr all compress what they send when a client asks for it, and finstats never asked: everything it read came over the network as plain JSON. It now asks and unpacks what comes back, which takes more than half off the reads it makes most — the session list while something is playing, and the library read, the largest single thing it ever fetches. The geolocation database is still downloaded exactly as it is stored, so the progress it shows and the file it writes are unchanged.
- The Jellyfin card in Settings says which of the two halves is happening, and the live dot in the corner follows whether the connection is carrying rather than which half you are in.
- Only one of the two ways of listening ever runs at a time: while finstats is asking it tells Jellyfin to stop sending, and to start again the moment the last play ends. The connection itself stays open throughout, so nothing is any slower to notice.
- A Jellyfin that answers the subscription with nothing is believed as long as it is answering keep-alives: finstats says so once, reads the session list itself meanwhile and keeps the connection subscribed until the first push settles it. "Cannot push" and "was restarting when we asked" look exactly alike, so silence is never taken as a verdict.
- The fallback is untouched. A connection that closes, goes quiet or turns out not to speak this drops finstats back to asking on a timer on the very next pass, and it keeps trying to reconnect — **While someone is watching, check every** and **While nothing is playing, check every** are what it asks at then. A play that is never seen cannot be recovered later, so every doubt still resolves towards asking.

### Added
- **What the collector is doing, on `GET /api/status`.** Which of the four things it is at (`idle_socket`, `playing_poll`, `paused_socket`, `fallback`), whether the live connection is open and subscribed, how often it is asking for the session list, how many session reads went out in the last minute, and when that last changed. Enough to check from outside what used to be visible only in the log, and it costs nothing to answer: no request to Jellyfin, no query, just what is already in memory. It carries no names, no titles and no counts of anything you watch.
- **finstats checks its own work.** The rules that must hold between those fields — listening means subscribed and not asking, asking about a play means not subscribed — are tested against each other on every pass, and a disagreement that lasts is written to the log. A state machine that quietly believes something other than what it is doing is the one fault none of the rest of this would catch.
- **A limit under all of it.** Every read of the session list, wherever in finstats it comes from, takes a slot from one limiter: at most 2 a second and 70 a minute, with anything above that delayed, counted, and written to the log at most once a minute. Normal watching asks once a second and stays well under it.

### Fixed
- **Live updates really replace the polling now.** Switched on, 1.4.0 kept asking Jellyfin for the session list every five seconds anyway, and reopened the connection every twenty to forty seconds. It judged the connection by how recently a session list had arrived — but Jellyfin sends one when something *changes* and nothing at all in between, so on a server where nobody was watching, a perfectly healthy connection looked dead every fifteen seconds. finstats now watches whether Jellyfin is still answering at all (it exchanges a keep-alive about twice a minute) rather than whether it has something to say.
- A session list that arrived in the moment finstats was between passes was thrown away, on the assumption that another was a second and a half behind it. With a server that only speaks when something changes, the next one can be hours away, so it is kept and used.
- **Last checked** under Settings, and the Outbound connections card, no longer go stale while the connection is quiet: Jellyfin's answer to a keep-alive is what they now show.

### Removed
- The **Let Jellyfin push what is playing** setting under Settings → Collection, and `live_socket` from the settings API. There is nothing to switch on any more.
- `socket_enabled` from the collector status: it was only there to tell "switched off" apart from "on but not connecting", and there is no "switched off" now.
- `last_reconcile_at` from the collector status, replaced by `socket_live`: there is no separate safety-net read while something plays any more, because every read is one.

## [1.4.1] - 2026-09-22

Less talking to the services around Jellyfin. Asking Seerr every five minutes and reading the Sonarr and Radarr queues every minute made up most of the traffic finstats caused on a quiet server, and almost all of it was the same answer over and over. Both now ask only when there is something to hear; nothing on any page appears any later than it did.

### Changed
- **Seerr is only read properly when something has changed.** Every five minutes finstats asks it for one row — the most recently changed request — and if that is one it already knows, the pass is over there: about a kilobyte, where a page of fifty requests was about sixty. Requests that are still on their way are looked at again every quarter of an hour, because a title becoming available does not always change the request itself, and everything is listed once a day as before. A new request still shows up within five minutes.
- **The download queues are left alone while there is nothing in them.** While a page is showing the queue nothing changes: it is still refreshed every five seconds. Behind the scenes Sonarr and Radarr were asked every minute around the clock, whether or not anything was downloading; now that is every minute only while something is in the queue, and every five minutes while it is empty. Opening the page, connecting a service and a new request in Seerr each refresh it at once.

## [1.4.0] - 2026-09-21

finstats can now be told what is playing instead of asking for it. Jellyfin offers the same session list over a WebSocket, so with one switch finstats keeps a single connection open and hears about a play the moment it starts, pauses or ends — about 17,000 requests a day become almost none, and nothing is noticed late any more. The same release makes the one outside request finstats ever makes by itself a single question rather than a standing one, and adds a card that lists everywhere finstats can reach, so the privacy promises in the README are something you can check rather than something you have to believe.

### Added
- **Let Jellyfin push what is playing** under Settings → Collection, **off until you turn it on**. Everything your history is made of comes from the collector, and a play that is never seen cannot be recovered later, so this transport is given a release to prove itself before it becomes the default. It is a new way of hearing the same thing: the plays, the pauses, the skips and the groups are worked out by exactly the code that worked them out before. While it is live finstats still makes one ordinary request a minute *while something is playing*, which is what ends a play whose client vanished without saying so; when nothing is playing it asks nothing at all. If the connection closes, goes quiet for fifteen seconds, or the server turns out not to speak it, finstats is back to checking on a timer on the very next pass and keeps trying to reconnect in the background.
- **Outbound connections**, a card in Settings for Jellyfin administrators: every destination finstats can reach — your Jellyfin, the "what is my IP" service, DB-IP's database, and each Sonarr, Radarr or Seerr you have connected — with what it is for, whether it is switched on, and when it last answered. It is built from what finstats already knows, so the list itself learns nothing, and it shows addresses only: never a key, never a base path.
- **Look up now** under Settings → Home network, for the day your public address changes.

### Changed
- The **"what is my IP" lookup runs once**, the first time finstats needs an address, instead of every fifteen minutes. A household's address is not news that needs re-checking a hundred times a day; when it does change, the new button asks again.
- The Jellyfin card in Settings says how the collector is being told — pushed and live, or asked every so often — and, when the live connection is switched on but not carrying, why not.

## [1.3.0] - 2026-09-20

Sonarr, Radarr and Seerr, on one page. The new **Pipeline** page says who asked for what, what is about to air and what is arriving right now — and, because finstats knows what people actually watch, whether the thing somebody asked for was ever played.

### Added
- **Connections** under Settings, for Jellyfin administrators: **Sonarr**, **Radarr** and **Seerr** (or Jellyseerr/Overseerr). Several of a kind is normal — a 4K Radarr, an anime Sonarr — and each connection is tested before it is saved, so a mistyped key is found at once instead of in a log next week. Your download client needs no setup of its own: Sonarr and Radarr already talk to it, and finstats reads what they know.
- **Requests**: who asked for what, how long it took to arrive, and whether they ever watched it. Tiles for how many titles are waiting and what the typical wait is, a chart of how that has changed month by month, and two lists that answer the questions people actually ask: what arrived weeks ago and was never played, and who asks for the most.
- **Upcoming**: new episodes and film releases from Sonarr and Radarr as an agenda by day, marked with whether *you* watch that show and, for people who may see everyone, who else does. A show counts as watched when somebody has played an episode of it in the last four months. The dashboard gets a "Coming up" row, a profile "Coming up" for the shows that person watches, and a title page says what is next for it.
- **Downloads**: the live queue of every Sonarr and Radarr — what it is, how far along, how fast, what went wrong on import — with the person who asked for it next to it. A season pack is one line, however many episodes it holds, and what needs attention is on top. Below it: what came in over a week, a month or a year, by indexer, quality and download client, and which downloads failed. The dashboard shows the same list, shortened.
- A new permission, **See what is downloading**. Without it, people still see how far their *own* request has got: how far along and how long is left, and nothing else — no release names, no speeds, no other downloads. Other people's requests need *See everyone's activity*, as before.

### Changed
- finstats still only ever reads: it sends `GET` to these services and nothing else, so there is no code in it that could approve a request, start a search, or add, pause or remove a download.
- API keys for these services are stored in finstats' own database, are never sent back to the browser, never written to a log, and are never part of a backup. finstats follows no redirect to a service, so a key cannot travel somewhere you did not enter, and certificates are verified unless you switch that off for one connection.

## [1.2.2] - 2026-09-20

### Added
- **Languages on every title.** A film or episode now lists the languages of all its audio tracks and subtitles ("Audio: Japanese · English"), so you can see at a glance whether there is a dub.
- **How far a dub goes.** A show or a season says how many of its episodes have each language ("English: 13 of 26 episodes"), and the episode list has an **Audio** column, so a dub that stops after season one is visible before you start watching.
- **Audio languages** and **Subtitle languages** on the library pages: how many of your files can be played in each language.
- finstats re-reads your library once after this update to pick the languages up. It does not say "dubbed" by itself, because Jellyfin does not tell it a title's original language; it shows the languages and leaves the conclusion to you. A track without a language tag is listed as "Unknown".

## [1.2.1] - 2026-09-20

### Changed
- The pictures in the README are retaken with this version, and there is a new one of the **Security** page with its map and an impossible-travel alert. Nothing changes in the app itself.

## [1.2.0] - 2026-09-20

See where people watch from. A new Security page puts plays, sign-ins and live streams on a world map and raises an alert when an account is somewhere it cannot be.

### Added
- **Security page** (for people who may see network details and everyone's activity): a world map with a dot for every place your users watch from, sized by how much happens there. Home is one green dot, places away from home are purple, streams running right now pulse, and failed sign-ins from outside are red. Drag to move, zoom with the buttons, the keys or Ctrl + wheel; dots that overlap merge until you zoom in. Below it: every place with its people, plays, watch time and sign-ins, and the countries by plays.
- **Impossible travel.** When the same account is seen in two places that no flight connects (home at eight, New York twenty minutes later) or in two distant places at once, finstats raises an alert with both sightings, the distance, the time between them and the speed that would have taken. **Show on map** draws the trip.
- **New country.** The first time someone plays or signs in from a country they have never been seen in.
- **Resolve, with a memory.** Resolve an alert with a note, or tick "never report these two places for this person again" for a VPN or a phone whose carrier is "in" the capital. Alerts found in old history (after an import, or the first time a database is installed) are filed as resolved instead of flooding the list. Alerts and what you decided about them are part of backups.
- **Places come from a file on your machine.** Addresses are looked up in a city database finstats reads locally; no address is ever sent anywhere, and the map is drawn from outlines bundled with finstats, not from a map service. Download DB-IP's free database with one click on the Security page or under **Settings → Security**, let finstats refresh it monthly (off until you switch it on), or drop your own `.mmdb` file (DB-IP, MaxMind GeoLite2-City) into `data/geoip/`. `FINSTATS_GEOIP_DB` names a file elsewhere.
- **Settings → Security**: the database in use, the monthly update, and how fast (900 km/h) and how far apart (500 km) two sightings must be to count as impossible travel. City databases are often a few hundred kilometres off, which is what the distance is for.

## [1.1.3] - 2026-09-20

### Added
- **Search finds actors and directors.** Ctrl+Space now also looks through the cast and crew of everything in your library and lists them under **Cast and crew**, with their photo, whether they act or direct, and how many of your titles they are in. Choosing one opens their page. It is as forgiving as the title search: words in any order, accents ignored, and a slip of the finger allowed, so "scarlet johanson" still finds her. When several people match equally well, the one in more of your titles comes first.

## [1.1.2] - 2026-09-20

### Added
- **Recently added** on the dashboard, above the Activity chart: the 30 newest arrivals in your libraries as a row of posters you can scroll sideways, with arrows on a computer. Each says when it arrived and what it is: a film, an album, or "Season 4 · 3 episodes" of a show. A single new episode says which one it is, and the poster is the season's own when it has one.
- The row moves the way you expect: **Shift + mouse wheel** and the arrow keys one poster at a time, Page Up/Down a screenful, Home and End to either end, plus swiping and the arrows. The plain wheel still scrolls the page.
- New episodes of the same show that arrive on the same day share one entry, and a whole show added at once is one entry too ("3 seasons · 60 episodes"), so one big import does not push everything else off the list.
- The row is the same whatever time range or person the dashboard is filtered to, since it is about the library and not about plays. It hides itself while the library is empty.

## [1.1.1] - 2026-09-20

### Added
- **Pages open at once.** A moment after finstats has loaded, it quietly fetches what the pages you are likely to open next need: the dashboard, your profile and timeline, Activity, Users, Libraries and each library, Playback, the server pages, the recap and the most active people. Opening one of them then shows it immediately instead of a loading skeleton, and it still refreshes behind the scenes, so nothing you see is older than before.
- Films, shows and cast members are not fetched in advance, as there are thousands of them. Instead, resting the pointer on a link (or touching it, or reaching it with the keyboard) fetches that one page, so it is usually there by the time you have clicked.
- This stays out of the way: it waits until the page you opened has finished loading, works through the list one page at a time while the browser is idle, pauses in a background tab, and does nothing on a data-saver or very slow connection. Everything is forgotten when you sign out.
- A small **Repo** link with the GitHub mark in the status bar, next to the version, opens the project on GitHub.

### Fixed
- The **Timeline** forgot which libraries were switched off when the page was reloaded or opened from a bookmark, although the choice was in the address.

## [1.1.0] - 2026-09-20

A timeline of everything you have watched. Every profile has a new **Timeline** tab: your watching as one trail from today back to the first play finstats knows about, with an evening of episodes folded into a single stop. On a wide screen the trail winds across the page like a snake, three stops to a row; on a phone it is one straight line.

### Added
- **Timeline**, next to **Overview** on every profile. Each stop is a poster with what was watched and when: "Season 2 · Episodes 3–6", a film (and whether it took more than one sitting), an album and how many tracks of it. Episodes of the same season that were watched one after the other are one stop; anything watched in between starts a new one. A stop opens the show, film or track.
- The trail follows the width of the window: three stops in a row on a wide screen, two on a narrower one, every other row running backwards with a bend joining it to the next, down to a single straight line on a phone. The first stop of each month carries the month.
- Older stops load by themselves as you scroll, all the way back to where the history starts.
- **Libraries** switches above the trail leave out what you do not want to see, such as music. The choice is part of the address, so it survives a reload and can be bookmarked.
- Something that is still playing says **Playing now**.
- The same rules as everywhere else apply: you see your own timeline, and other people's only with the "see everyone" permission.

## [1.0.5] - 2026-09-20

### Added
- **Downgrade protection.** The database now remembers the newest finstats version that has opened it, and an older version refuses to start on it instead of quietly working on data it does not fully understand. The message says what to do: run the newer version again, or start the older one on an empty data folder and restore a backup. Updating works as before. Versions up to 1.0.4 were released before this check existed, so it protects from 1.0.5 onwards.

## [1.0.4] - 2026-09-20

### Fixed
- In a play's details, the copy button next to a long **Device ID** had dropped onto a line of its own. It sits beside the ID again.
- On the Playback page, the last column of **Which clients transcode** was cut off ("Transc…"). The column headers got wider when tables became sortable and no longer fitted the card; the two-word headers now wrap instead.

### Changed
- The screenshots in the README are current again (they still showed version 0.5.0).

## [1.0.3] - 2026-09-20

### Added
- **Episode pictures.** The page of an episode now shows that episode's own picture, the same one Jellyfin shows in its episode list, instead of the show's poster, which was identical for every episode. An episode Jellyfin has no picture for keeps the show's poster.

## [1.0.2] - 2026-09-20

### Fixed
- In **Patch notes**, the v0.9 and v0.10 groups had no headline next to their version, unlike every other group. They have one now, and a group's headline is always the first sentence of what its first release was about, so a long introduction no longer gets cut off mid-sentence.

## [1.0.1] - 2026-09-20

### Fixed
- **The published image would not start on a fresh machine**: "unable to open database file: /data/finstats.db", after hanging for half a minute. When the `data` folder does not exist yet, Docker creates it as root, and finstats runs as an ordinary user that may not write there. The container now makes the folder its own when it starts and then drops to that ordinary user before finstats runs, so `docker run` works on the first try. If your files belong to someone other than user 1000, set `PUID` and `PGID`; `--user` still works as before.
- If the data folder really cannot be written, finstats now says so at once, with the command that fixes it, instead of waiting 30 seconds and blaming the database.

## [1.0.0] - 2026-09-20

Ready for everyone. This is finstats 1.0: everything a Jellyfin server owner needs from a statistics tool is here and has settled: live sessions and every play down to the pause button, the library and playback insights, group watching, profiles with show progress, the yearly recap, people pages, permissions, sortable tables, the Jellystat import, and backups that move between installs. From here on, version numbers mean what they say: 1.x updates will not break your data, your backups or your settings.

### Added
- **A ready-made image.** finstats is now published at `ghcr.io/olayzen/finstats`, for 64-bit Intel/AMD and ARM machines (a Raspberry Pi 4 or 5 works). `docker pull ghcr.io/olayzen/finstats:latest` replaces building it yourself. `:latest` is the newest release, `:1` follows every 1.x update, `:1.0.0` stays exactly where it is, and `:edge` is the development version.
- Every release now appears on the project's GitHub releases page with these patch notes.

### Changed
- The Docker Compose file and all instructions use the published image. If you built `finstats:latest` yourself, switch the image name in your `docker run` or Compose file; your `data` folder carries over untouched.

## [0.10.2] - 2026-09-20

### Fixed
- **On a phone, Settings and the page of a show could scroll sideways.** A label meant only for screen readers escaped the table it belongs to and stretched the whole page; the episode lists and the cast row did the same on narrow screens. Tables and the cast row now scroll inside themselves.
- The recap no longer scrolls a few pixels sideways when a chapter's background word is wider than the page.
- A long "watching on…" line under **Now playing** ends in an ellipsis instead of being cut mid-letter.
- The poster in front of each show on a profile was a second, nameless link to the same page for screen readers. It is hidden from them now; the title next to it is the link.

## [0.10.1] - 2026-09-20

### Fixed
- **Downloading a backup failed in the browser** ("the source file could not be read" in Firefox), with nothing on the page to say why. finstats compresses what it sends, and it was compressing the backup, which is a compressed file already, a second time; that doubly-packed transfer broke off before the end. Backups are now sent as they are, with their size up front, so the browser can also show real progress.

## [0.10.0] - 2026-09-20

Your history, backed up and ready to move.

### Added
- **Backups.** finstats now writes a backup of itself every week and keeps the newest five: every play with its pause-and-skip timeline, seen marks, permissions, home addresses, the server log and your settings, in one small file (a few hundred KB for thousands of plays). **Settings → Backups** lists them with a **Download** button each, makes one on demand, and lets you change how often and how many. They live in the `backups` folder of your data directory. A backup never contains your Jellyfin API key or anyone's sign-in session, but it is a complete viewing history with IP addresses, so keep it somewhere private.
- **Move to a new finstats.** Set up the new instance, open **Settings → Backups** and choose the file, or pick one from the list and press **Restore**. Restoring merges rather than overwrites: plays that are already there are skipped, so it is safe to do twice or into an instance that has been running for a while. Untick "Also restore settings and permissions" to bring back the history only. Backups work across versions in both directions. From a terminal: `finstats backup` and `finstats restore <file>`.

### Changed
- **finstats now checks every second while someone is watching, and every 5 seconds while nobody is** (it used to be every 5 seconds throughout). Pauses, skips and track changes are recorded to the second, and the idle beat keeps the load on Jellyfin low. Both are under **Settings → Collection**. If you had changed the old single interval, set your preference again: it has been replaced by these two.

### Fixed
- An IP address in a play's details broke across two lines in the middle of the address. It stays whole now; the Local/Remote mark moves underneath when there is no room.

## [0.9.1] - 2026-09-20

### Fixed
- **People at home were shown as remote.** A device on your own network that reaches Jellyfin through its public name (a reverse proxy, a domain) arrives with your household's public IP, and finstats only recognised private addresses as local. It now learns your public address and counts plays from it as local, for the whole history, not just from now on. It remembers earlier addresses too, since most connections get a new one now and then.

### Added
- **Settings → Home network.** See the addresses finstats treats as home, add your own (an earlier address, a second home, a VPN exit), or switch the lookup off. To learn your public address finstats asks a plain "what is my IP" service every 15 minutes; it tries several (Amazon, Cloudflare and others), because ad-blocking DNS such as Pi-hole often blocks them, and one of them needs no DNS at all. The request contains nothing about you or your server, and with the switch off finstats talks to nothing but Jellyfin. `FINSTATS_PUBLIC_IP_URL` lets you use a service of your own.

## [0.9.0] - 2026-09-20

Every table, in the order you want.

### Added
- **Every table can be sorted.** Click a column header to sort by it, click again to turn it around, and a third time to get the original order back: users A–Z or Z–A, progress 0–100% or 100–0%, watch time, sizes, dates, anything. It understands what the cells mean, so "3d 2h" sorts as time and "1.4 GB" as size, and empty cells always go last. This covers the user list, watchers, episodes, devices, plugins, scheduled tasks, the table view of every chart, the permission matrix, and the bar lists (codecs, resolutions, clients and the rest), which now have a slim header of their own.
- **Activity and the Server log sort across all their pages**, not just the rows on screen, and the chosen order is kept in the address so it survives a reload or a shared link.
- **A quick filter on long tables.** Tables with ten rows or more get a small filter field above them; type a few words to narrow the rows down.

## [0.8.2] - 2026-09-20

### Changed
- The recap's year picker is the same switch as every other range picker in finstats (7d · 30d · 90d …), instead of a style of its own.

## [0.8.1] - 2026-09-19

### Added
- **Pages for actors and directors.** Click a face in the recap's "Most watched people", or in the new **Cast & crew** row on any film, show or episode, to open that person's page: everything they are in on your server, how much of it has been watched and by whom, and what is still waiting. Like every other page it follows the time range you pick, and people who may only see their own statistics see only their own watching there. Esc takes you back to where you came from.

## [0.8.0] - 2026-09-19

The recap has a new look, and more to say.

### Added
- **Most watched people.** The actors you spent the most time with, and the directors behind what you watched, as a row of portraits. finstats now reads cast and crew for films and shows along with the library; they appear after the first library read following this update, which finstats does by itself on start.
- **Your year in days.** Every day of the year as one square, brighter the more you watched, with your longest streak and biggest day called out above it. Hover a day, or walk the calendar with the arrow keys, to see what it held.
- **By the numbers.** Plays, watch time, different titles, days watched, longest streak and how much of your watching was a **rewatch**, plus how the year splits between episodes, movies and music.
- The genre chapter now says how many genres you touched and how large a share the top one took.

### Changed
- **A new design for the recap.** It opens on your year "replayed": the headline beside the posters that filled it, over a waveform of the year with one bar per week and the loudest week lit. You pick the year right above it. Every chapter now has a headline with its key word in violet, a sentence that carries the figures, and the chapter's word standing large and faint behind it. It ends the way films do, with the credits: starring, directed by, screened on, running time.
- Hours, weekdays and months now share one **Activity patterns** chapter with a switch between them; the headline follows what you are looking at ("Friday took the crown").

### Fixed
- **Now playing** on the dashboard was left blank when nobody was watching, so the heading seemed to belong to the filters under it. It shows its "Nothing is playing right now" box again.

## [0.7.10] - 2026-09-19

### Added
- **Library artwork.** Each library card now shows the picture Jellyfin uses for that library on its home screen, in place of the plain icon on the left; the library's own page shows it too. It comes through finstats' image proxy like the posters do. A library without a picture keeps its icon.

## [0.7.9] - 2026-09-19

### Changed
- The Genres card opens as the **list** again, with the radar as the second option on the switch. If you had already picked a view, that choice is kept.

## [0.7.8] - 2026-09-19

### Added
- **A radar view for Genres.** The Genres card, on the dashboard and on profiles, can now be shown as a radar: one spoke per genre, so the shape of what gets watched is visible at a glance. Switch between **Radar** and **List** in the card's corner; your choice is remembered. Hovering a point (or using the arrow keys) shows the genre's watch time and share, and the list is still there for reading exact numbers.

## [0.7.7] - 2026-09-19

### Fixed
- **Movies that belong to a collection were shown as "No longer in library".** On servers with *group movies into collections* switched on, Jellyfin hands out the collection instead of the films inside it, so finstats never saw those films and marked them as removed; on one server that hid 556 of 1,238 items in the Movies library. finstats now asks for the films themselves. The next library read brings them back and re-attaches their plays; the update triggers that read right away.

## [0.7.6] - 2026-09-19

### Changed
- On profiles the **Shows** card moved down, to just above Genres, so a long list of shows no longer pushes the watch time, activity chart and top titles out of sight. The day streaks stay at the top. A show you have opened stays open when you change the time range.

## [0.7.5] - 2026-09-19

### Changed
- **Search is forgiving.** It used to need the exact phrase; now it matches word by word, in any order, with anything in between: "alya hides" finds *Alya Sometimes Hides Her Feelings in Russian*. Case, accents and punctuation don't matter ("pokemon" finds *Pokémon*, "spider man" finds *Spider-Man*), a leading "The" is ignored, and a slip of the finger is forgiven ("mentalsit" finds *The Mentalist*). Words of three letters or fewer still have to be typed right, so short queries don't turn into guesses. Better matches rank first, and music is also found by its artist.
- The search boxes on Activity and the Server log match word by word as well, across all their columns: "alya opera" finds plays of that show in Opera.

## [0.7.4] - 2026-09-19

### Changed
- **Patch notes are grouped by series.** Every `0.x` series is one fold holding all of its releases, so `v0.7` contains 0.7.0 to 0.7.4, with what the series was about, how many releases it has and when. Only the newest series starts open; older ones are a click away, and a closed fold still shows which one you are running.

## [0.7.3] - 2026-09-19

### Fixed
- The Now playing clock no longer hesitates every few seconds. The 5-second refresh was also repainting the clock, so whenever the server was a fraction of a second ahead the number moved early, between two ticks, and then appeared to stand still for a second. Now only the once-a-second tick moves it, by exactly one second each time; measured, every change lands 999 to 1001 ms after the last. The refresh still corrects it after a pause, a skip or real drift, and even that correction now lands on the tick.

## [0.7.2] - 2026-09-19

### Added
- **Group watching, live.** When people are watching the same thing together right now, each of their Now playing cards says so: "With maria". Until now a group was only recognised once the plays had ended. Live, people count as together when they are on the same title and either started within the group window or are at nearly the same position, which still works if finstats was restarted mid-stream.

## [0.7.1] - 2026-09-19

### Changed
- **Now playing counts every second.** The clock and the progress bar used to jump forward each time the server was asked, every 5 seconds. They now tick once a second in between and the bar glides, like a player. The server is still only asked every 5 seconds and stays the source of truth: the display re-syncs when someone pauses, skips or when the server turns out to be ahead, but not when Jellyfin's position is merely a few seconds stale (clients report it only every ten seconds or so), so the clock never stutters backwards. Paused streams stand still.

## [0.7.0] - 2026-09-19

Who watches together.

### Added
- **Group watching.** When different people start the same title at the same time, finstats counts it as watching together. A **Watched together** card on the dashboard shows the groups, the titles they share and their hours together; each profile shows who that person watches with most; shared plays carry a small people mark in Activity, and the play details say who it was watched with. It works on your whole history, imported plays included.
- A setting for how close together the starts must be (default 60 seconds). Real history shows why not 5: about a third of genuine group sessions start 6 to 60 seconds apart, because someone always presses play a moment late. People also have to keep watching alongside each other for a couple of minutes, so two people opening the same episode by coincidence and one leaving at once is not a group.

### Changed
- Search now opens with **Ctrl + Space** instead of Ctrl/⌘ + K.

## [0.6.1] - 2026-09-19

### Added
- Administrators can open another person's recap: pick them at the top of the Recap page, or use **Open recap** on their profile. It then reads about them by name instead of "you".

### Changed
- Everyone else still only ever sees their own recap, whatever permissions they hold, and there is still no recap of the whole server.

## [0.6.0] - 2026-09-19

Your profile, and where you are in every show.

### Added
- **Show progress on profiles.** Every series you have touched as a bar with one segment per episode: seen, started or not yet. Open a show to see it season by season. Sorted by how close you are to finishing, with separate views for finished shows and everything.
- **Mark as seen.** On your own profile, click an episode, or mark a whole season or show, for what you watched while nothing was recording. Jellyfin's own played marks are used as well, so most gaps fill themselves. Marks stay in finstats and never change anything in Jellyfin; a recorded play can't be unmarked, your own marks can.
- **Day streaks** on profiles: your longest run of days in a row with a play, your current streak, and how many days you have watched something.
- **My profile** in the sidebar.
- Activity shows **where playback stopped** (for example `44:15 / 45:00`) under the progress bar, and the actual **date and time** under "3h ago". Plays imported from Jellystat have no stop position, because Jellystat never recorded one.

### Changed
- Episodes that are missing or have not aired yet are no longer treated as part of your library. Jellyfin lists them as placeholders without a file; they used to count towards episode totals.

## [0.5.1] - 2026-09-19

### Added
- finstats has a logo: a play button sliced into three chart bars. It replaces the generic pulse icon in the sidebar and on the sign-in screens, and is the new browser-tab icon, as a sharp SVG with a classic multi-size `favicon.ico` for browsers and bookmark bars that want one, plus a home-screen icon for phones and tablets.

## [0.5.0] - 2026-09-19

You decide who sees what.

### Added
- **Permissions.** Under Settings → Access, grant people more than their own statistics, for everyone at once or person by person: **see everyone's activity** (other people's statistics and history, the Users page, every live stream), **see network details** (IP addresses, device ids, local or remote), **see the server** (the Server page, the server log, failed sign-ins, file paths) and **manage finstats** (settings, tasks, import, deleting plays).
- **Sign in, per person.** Let just a few people in without opening finstats to every Jellyfin user.
- Changes apply immediately, including taking access away, and everything is enforced by the server rather than by hiding buttons.

### Changed
- Jellyfin administrators still have everything, and only they can change permissions. Someone who may manage finstats cannot grant anything, to themselves or anyone else.
- The year recap stays personal whatever is granted.
- Nothing changes for existing installs until you grant something: people who could sign in before still see only their own statistics.

## [0.4.5] - 2026-09-19

### Changed
- finstats is now licensed under the **GNU General Public License v3.0**. No earlier version was ever published, so nothing was released under different terms. The licenses of the bundled fonts ship alongside them.

## [0.4.4] - 2026-09-19

### Added
- **Esc steps back.** On an opened library, user or title, pressing Esc returns you to where you came from, with the list exactly as you left it. Anything that is open closes first (play details, search, a dropdown, the mobile menu), and Esc is left alone while you are typing or using a chart with the keyboard.

## [0.4.3] - 2026-09-19

Pages you have already seen open instantly.

### Changed
- finstats remembers what each page last showed. Going back to a profile, a library or a time range you have already looked at paints it immediately, profile picture and all, while fresh numbers load quietly behind it; the page only redraws if something actually changed. What is remembered lives in the browser tab only and is dropped when you sign out or change anything.
- Loading placeholders now appear only when loading is actually slow. Before, every page showed them for at least a third of a second, even when the server had answered at once.

### Fixed
- Libraries that were deleted in Jellyfin and never had a single play are no longer listed. A deleted library with history is still shown, marked as removed.

## [0.4.2] - 2026-09-19

### Fixed
- On the Users page the **Admin** tag sat on its own line and made that row taller than the others. Tags now sit beside the name, so every row is the same height.

## [0.4.1] - 2026-09-19

### Fixed
- The word "null" no longer appears under the dashboard title outside December and January, when there is no recap banner to show.

## [0.4.0] - 2026-09-19

See what changed without leaving the app.

### Added
- **Patch notes** tab showing every release, with the version you are running marked.
- The version in the status bar links to the patch notes, and the tab shows a dot after an update until you have looked.

### Changed
- Re-attaching history to renamed items is far faster (from 18 seconds to a third of a second with 666 orphaned titles), so start-up stays instant as history grows.

## [0.3.3] - 2026-09-19

finstats now follows Jellyfin's own schedule instead of keeping a second one.

### Changed
- The library is re-read right after Jellyfin's **Scan Media Library** task finishes, never on a separate timer and never while a scan is running. finstats has always been read-only towards Jellyfin; it never starts a scan there.
- A weekly safety-net read covers servers that rely on real-time monitoring, where the scan task may never run.
- The old interval is now only a fallback, used when following is switched off or the server reports no scan task.

### Added
- **Follow Jellyfin's library scan** switch under Settings → Collection (on by default).

## [0.3.2] - 2026-09-19

### Fixed
- History survives renames. Jellyfin gives a renamed file a new id, which used to strand its earlier plays under the raw folder name (`Title (2010) [tmdbid-…] [imdbid-…]`) with no poster. Those plays are now re-attached to the current library entry: by TMDB/IMDb/TVDB id when the old name carries one, otherwise by cleaned title and year; episodes by series plus season and episode number. A match has to be unambiguous, so titles that were really deleted stay as they are.
- Live TV channels imported from Jellystat were counted as movies, because Jellystat records no item type. They are now recognised as Live TV, and new Live TV plays are recorded with Jellyfin's own type.

### Changed
- Live TV is left out of the recap entirely: top lists, totals, persona, records and rank.

### Added
- `finstats relink` command to re-attach history on demand. It also runs at start-up and after every library read.

## [0.3.1] - 2026-09-19

### Changed
- The recap is strictly personal. Everyone, administrators included, only ever sees their own; the "Everyone" view, the user switcher and the server chapter are gone.
- The recap opens on the year that is "ready": the current year during December, otherwise the previous one. 2026 becomes the default in December 2026 and stays it until December 2027. Other years and "Last 12 months" remain one click away.

## [0.3.0] - 2026-09-19

Your year in review.

### Added
- **Recap** tab: a scrolling story of a year. Hours watched, top shows, movies, tracks and genres, and a viewing persona (night owl, early bird, weekend warrior, binge watcher, movie buff, music lover, creature of habit) with hour-of-day and weekday charts.
- The year month by month, with the title that defined each month.
- Records: biggest day, biggest binge, longest daily streak, longest single play, most rewatched title, first play of the year and the oldest title watched.
- Discovery: new shows started, movies and episodes finished, and the shows that got exactly one episode.
- A one-line banner on the dashboard in December and January when a recap is ready.

## [0.2.0] - 2026-09-19

More than Jellystat ever recorded.

### Added
- A **timeline for every play**: start, pause, resume, skip, audio and subtitle switches, direct play turning into a transcode, and stop. Plus pause and skip counts, where playback resumed from, and whether the viewer was on the local network.
- **Server** tab: Jellyfin version, pending updates and restarts, disk usage per library (Jellyfin 10.11+), plugins, scheduled task results and every registered device.
- **Playback** insights: concurrent streams over time, which clients force transcodes, how far people get before stopping, local versus remote plays and viewing behaviour.
- **What your library is made of**: resolutions, codecs, dynamic range, containers, titles per decade, additions per month, the largest titles, and the titles nobody has ever watched, checked against both finstats' history and Jellyfin's own played flags.
- Dashboard: peak concurrent streams, estimated data streamed, share of remote plays, genres by watch time and failed sign-ins.
- Item pages: IMDb and TMDB links, studios, bit depth and frame rate, and who has the title marked as played in Jellyfin.
- User pages: genres, and the movies, episodes and favourites Jellyfin has on record.

### Fixed
- The time label on a now-playing card no longer spills outside the card.
- Long labels on the Playback page were cut off too early.
- A series that has left the library still shows its season and episode list.

## [0.1.0] - 2026-09-19

First release.

### Added
- Live collection from Jellyfin's sessions: user, title, client, device, IP address, play method, transcode reasons and hardware acceleration, video, audio and subtitle details, and time watched versus time paused.
- Sign-in with your Jellyfin account. A two-step setup creates finstats' own API key; passwords are never stored. Optionally let non-admin users sign in to see only their own statistics.
- Dashboard, Activity, Users, Libraries, Playback and Server log pages, a command palette (Ctrl/⌘ K) and a live status bar.
- Jellystat import: large backups stream to disk and import in seconds, and importing the same file twice is safe.
- A single small binary with an embedded web UI and one SQLite file, plus a Docker image.
