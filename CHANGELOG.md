# Patch notes

Everything that changed in finstats, newest first. This file is compiled into the
binary and is what the **Patch notes** tab in the app shows.

Format: `## [version] - date`, an optional one-paragraph summary, then `### Added`,
`### Changed`, `### Fixed` or `### Removed` with one bullet per change.

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
