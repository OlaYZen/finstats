# Patch notes

Everything that changed in finstats, newest first. This file is compiled into the
binary and is what the **Patch notes** tab in the app shows.

Format: `## [version] - date`, an optional one-paragraph summary, then `### Added`,
`### Changed`, `### Fixed` or `### Removed` with one bullet per change.

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
