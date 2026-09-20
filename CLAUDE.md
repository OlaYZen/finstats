# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

finstats is a playback-statistics server for Jellyfin: one Rust binary (axum + tokio + bundled SQLite)
with the web UI compiled into it. It only ever *reads* from Jellyfin; it never triggers scans or writes
anything there except creating its own API key during setup.

## Commands

```sh
cargo build --release                      # release binary embeds web/ and CHANGELOG.md
cargo test                                 # all unit tests
cargo test relink                          # tests whose name/module matches "relink"
cargo test recap::tests::persona_prefers_the_most_distinctive_habit -- --exact
cargo clippy --all-targets                 # not clean: ~12 style warnings exist; add no new ones

FINSTATS_DATA_DIR=./data FINSTATS_BIND=127.0.0.1:8088 cargo run     # serve
cargo run -- import-jellystat <backup.jsonl>                         # headless import, then exit
cargo run -- relink                                                  # re-attach history to renamed items, then exit

for f in web/assets/js/*.js web/assets/js/pages/*.js; do node --check "$f"; done   # the only JS check there is
docker build -t finstats:latest .        # local image; the published one is ghcr.io/olayzen/finstats
```

- **Do not run `cargo fmt`.** The code is deliberately not rustfmt-formatted (hundreds of long lines); formatting
  would rewrite every file. Match the surrounding style by hand.
- Debug builds read `web/` from disk at runtime (rust-embed), so UI edits only need a browser refresh.
  Release builds embed it — rebuild to see UI changes. `CHANGELOG.md` is `include_str!`'d, so it always needs a rebuild.
- Env: `FINSTATS_DATA_DIR`, `FINSTATS_BIND`, `FINSTATS_TRUST_PROXY`, `FINSTATS_PUBLIC_IP_URL`, `JELLYFIN_URL` + `JELLYFIN_API_KEY` (skip the wizard), `TZ`, `RUST_LOG`.

## Architecture

```
Jellyfin ──/Sessions 1 s / 5 s idle─▶ collector ──▶ SQLite ◀── stats / recap API ◀── embedded SPA (web/)
         ──/Users /Items /Devices /System/* (scheduler)──▶ sync ──┘        ▲
Jellystat backup ──▶ import ─────────────────────────────────────┘   relink (after sync/import/start-up)
```

**State & DB access.** `state.rs` holds `AppState` (shared via `Arc` as `App`): DB handle, Jellyfin config,
`Settings` (one JSON blob in the `settings` table, `#[serde(default)]` so old installs load), the in-memory task
registry, the live now-playing snapshot, and a `Notify` (`wake`) that background loops select on. All DB work goes
through `db.call(|conn| …)` (r2d2 pool + `spawn_blocking`); rusqlite is re-exported as `db::rusqlite` — import it from
there, not from the crate, to stay on the version `r2d2_sqlite` uses.

**Migrations** are the `MIGRATIONS` array in `db.rs`, applied by index against `PRAGMA user_version`. Released
migrations are immutable — deployed databases have already run them. Add a new entry; never edit or reorder one.
`Db::open` also refuses a database from the future (`refuse_downgrade`): the settings key `app_version` holds the newest
version that has opened it, and a binary older than that, or one with fewer migrations than `user_version`, bails before
writing anything. Releases up to 1.0.4 predate the check and cannot be stopped. The key is not part of backups on purpose.

**Library reads must ask for real items.** `items_page` passes `CollapseBoxSetItems=false` (otherwise servers with
"group movies into collections" return the BoxSet *instead of* its films, which then get flagged removed) and
`ExcludeLocationTypes=Virtual` (missing/unaired placeholders). Anything a read does not return is marked `removed`,
so a query that silently hides items is a data-loss bug, not a cosmetic one.

**Jellyfin client (`jellyfin.rs`).** Every request asks for `Accept: application/json; profile="PascalCase"` because
10.x servers answer PascalCase and newer ones camelCase by default. All JSON access in the codebase assumes
PascalCase keys. Jellyfin ids are normalised with `db::norm_id` (no dashes, lowercase) everywhere.

**One play-row shape, two producers.** `playback.rs::PlayRecord` + `media.rs` (stream/transcode extraction, labels,
`effective_play_method`) are shared by the live `collector.rs` and the Jellystat `import.rs`, so both sources
produce identical columns. The collector inserts a row the moment a play is first seen (`active = 1`), refreshes
it every 30 s, counts only un-paused time, merges a restart within `merge_window_s` into the same row, and diffs
consecutive sightings into `playback_events` (pause/seek/track/transcode timeline).

**Jellystat import semantics** (learned from real exports; documented at the top of `import.rs` and in the README):
`ActivityDateInserted` is the *end* of a play; for episodes `NowPlayingItemId` is the series and `EpisodeId` the
episode; `PositionTicks` is unreliable and not imported (completion = watched ÷ runtime for imported rows);
there is no item type, so Live TV is inferred (video, not in library, no container → `TvChannel`). The import is a
single transaction, de-duplicated by `source_id = "jellystat:<id>"`, and streams a multi-hundred-MB file line by line.

**Sync scheduling (`sync.rs`).** Small reads (users, activity log, server details/devices) run every 15 min. The
expensive library read *follows Jellyfin's own "Scan Media Library" task* (`library_scan_status`): it runs after
that task finishes, never mid-scan, with a weekly safety net; the `sync_interval_h` timer is only a fallback
(setting `follow_jellyfin_scan`). After a library read, `backfill_playbacks` links plays to libraries and
`relink.rs` re-attaches orphaned plays to renamed items (Jellyfin ids derive from the path): provider-id match
first, then cleaned title + year, episodes by series + S/E number — only when unambiguous.

**Stats layer (`stats.rs`).** Every query goes through `Scope` → `Cond`: the time window ("last N days" = N full
local days, so chart buckets and totals agree), user/library filters, `min_play_s`, and the rule that **non-admins
without `see_everyone` are force-scoped to their own `user_id`, and IPs/device ids/file paths are only sent with `see_network`/`see_server` — enforced server-side**.
`row_json` maps SQL rows to JSON by column name (`BOOL_COLS` / `JSON_COLS` decide bool and JSON columns), so adding a
field is usually just adding a column to a SELECT. Local-time bucketing relies on SQLite's `'localtime'` and the
process `TZ` (the Docker image ships tzdata for this). Do not use `#[serde(flatten)]` in `Query` structs —
serde_urlencoded then hands numbers over as strings and every numeric filter 400s.

**Group watching (`groups.rs`).** Inferred, because `/Sessions` exposes no SyncPlay groups: plays of one item by ≥ 2
different users starting within `group_window_s` (default 60 — real data shows a third of genuine groups start 6–60 s
apart) and overlapping ≥ 2 min share `playbacks.group_id` (= lowest play id in the group). `detect()` re-runs per item
when a play ends, fully after import/start-up, and fully when the setting changes. "Time together" is the
second-longest stay in a session. Running streams are grouped separately by `mark_live` (same title,
different users, starts within the window *or* positions within `max(window, 30)` s), before `/api/now-playing` narrows
the list to the caller.

**Search (`fuzzy.rs`).** `/api/search` scores every library title in Rust instead of using `LIKE`: normalised (case,
accents, punctuation, leading article), every typed word must match some word of the title (exact > prefix > substring
> typo; typos only for words of 4+ letters, with swapped letters as one slip). ~70 ms over 5.5k titles; the palette
debounces. The Activity and Server-log `q` filters stay in SQL but are word-by-word too.

**Profiles (`profile.rs`).** Show progress counts only episodes that exist as files (`path`/`size_bytes` set; the sync
asks Jellyfin to exclude virtual items, and season 0 is skipped). "Seen" merges three sources in order: a recorded
play ≥ 80%, Jellyfin's played flag (`user_items`), a manual mark (`manual_seen`, written via `POST /api/me/seen` for
the caller only — finstats never writes to Jellyfin). Streaks are all-time and share `recap::longest_run`.

**Timeline (`timeline.rs`, `/users/:id/timeline`).** One person's plays, newest first, folded by the pure `fold()`: plays that follow
each other with the same key (series + season, album + artist, else the item) are one stop. Pages use a `(started_at, id)` cursor
and only give out a stop once the play after it has been read, so a stop is never split and the stops do not depend on the page
size. In the UI (`pages/timeline.js`) the column count comes from a `ResizeObserver`: rows alternate direction by being `direction: rtl`
(their cards reset to `ltr`), a bend joins each row to the next, and below 620 px it becomes one straight line (`.is-line`). DOM
order stays chronological, so keyboard and screen readers follow the trail.

**Recap (`recap.rs`)** is one person's year: the caller's own, or, for a Jellyfin administrator only (`is_admin`, not a
permission), the user in `user_id`. There is deliberately no whole-server edition. It excludes Live TV item types and
defaults to the year that is "ready": the current year in December, otherwise the previous one.
Its "most watched people" read `item_people`: actors (first 12 billed) and directors of films and shows only, filled by a
second, small `/Items` pass per library in `sync_libraries` (`Fields=People`) — never add `People` to the main item read.
The same table feeds the Cast & crew row on `/items/:id` and the person pages (`/people/:id`, `stats::person_detail`), which are
scoped like any other stats query.

**Auth (`auth.rs`).** Login forwards credentials to Jellyfin's `AuthenticateByName`, immediately logs that Jellyfin
session out, and mints an own opaque session token (stored hashed, HttpOnly SameSite=Lax cookie).

**Permissions.** `AuthUser.perms` (`Perms`: `see_everyone`, `see_network`, `see_server`, `manage`) is rebuilt on every
request from `user_permissions` ∪ `Settings.default_permissions`; `sign_in` (or `allow_user_login`) gates access at
all; Jellyfin admins always get `Perms::ALL`. Grants only add, there are no denies. Extractors: `AuthUser` (anyone
signed in), `ServerViewer`, `Manager`, and `JellyfinAdmin` — the only one allowed to edit permissions, and
`put_settings` refuses the access keys from anyone else, so a manager cannot self-promote. In `stats.rs` decide by
the specific permission (`scope.perms.see_network` for IPs, `see_server` for paths, `see_everyone` for whose rows),
never by `is_admin`. The recap ignores permissions: own for everyone, any one user for Jellyfin administrators. The UI mirrors this with
`can('perm')` from `state.js`; it is cosmetic — every rule is enforced server-side. `api.rs` adds an Origin check on writes and a strict CSP
(`style-src 'self'` — the UI must not use inline `<style>`/`style=""`; `el.style.x` via JS is fine).

**Local vs remote (`network.rs`).** `is_local` = private range (`db::is_local_ip`) or a row in `home_addresses`: this network's own
public IP, looked up with the light syncs from a plain-text service (the only non-Jellyfin request finstats makes; setting
`public_ip_lookup`, override `FINSTATS_PUBLIC_IP_URL`), plus the manual `home_addresses` setting. Always go through
`network::classify(conn, ip)`; after the set changes call `network::reclassify`, which re-decides the whole history. The lookup
must stay anonymous (no version, no ids in the request) and the docs' privacy claims must stay true to it.

**Backups (`backup.rs`).** gzip JSON Lines, one row per line tagged with its table, matched *by column name* both ways so files move
between versions; a new table that holds something Jellyfin cannot give back must be added to `backup::TABLES`. Secrets (Jellyfin
URL/API key, sessions) and the library are never exported; a test asserts the key is absent. Restore merges (dedupe on `source_id`
or user+item+start), remaps timeline rows to the new play ids, and re-derives groups, `is_local` and library links. The scheduler
writes one when the newest file is older than `backup_every_d`; endpoints are `JellyfinAdmin`-only and names go through `valid_name`.

The response compression layer skips `application/gzip`: re-compressing a backup broke the download in browsers. Anything served
pre-compressed needs the same exemption.

**HTTP contract.** `docs/api.md` is the contract the UI is written against; change it together with the endpoint.

For people: `CONTRIBUTING.md` is the one guide (what fits the project, running from source, the local Docker setup with a throwaway
Jellyfin, rules for a change, pull requests); `SECURITY.md` covers private reporting, `CODE_OF_CONDUCT.md` behaviour (it forbids posting other people's viewing data), and
`.github/` holds the issue forms, the discussion forms (file name = category slug: `q-a`, `ideas`, `show-and-tell`) and the PR template. Keep their rules in step with this file, and do
not add a second developer guide next to them.

## Frontend (`web/`)

No build step, no dependencies, no CDN: vanilla ES modules served from the binary, fonts bundled. `dom.js` (`h()`
element builder, icon map, formatters), `api.js`, `router.js` (History API; the server returns `index.html` for any
non-`/api`, non-`/assets` path), `components.js`, `charts.js` (hand-rolled SVG), `pages/*.js`. Rules that hold
everywhere: API/user strings reach the DOM only via `h()`/`textContent` (never `innerHTML` with data); fetches are
aborted and timers cleared on route change; every new card must hide itself when its data is missing (older servers,
non-admins, imported plays). Native `el.append(null)` prints the text "null" — pass possibly-absent nodes through
`h()` or filter them first.

"Now playing" (`nowPlayingView` in `widgets.js`) is polled every 5 s, but **only the 1 s ticker moves a clock** (+1 whole
second per beat). A poll never repaints it; it only corrects the position when that means something (pause, server > 3 s
ahead or > 15 s behind — clients report to Jellyfin roughly every 10 s), and even then by setting it one short so the
change lands on the next beat. Painting from the poll is what made the clock stutter.

Tables go through `tables.js`: `dataTable()` / `plainTable()` / `chartTable()` wrap a built `<table>` in its scroll box and make every
header a sort button (`sortable()` for tables without a scroll box, like the bar lists). Sorting reads cell meaning (durations, sizes, %,
`<time>`, switches; `data-sort` overrides, `data-nosort` opts a header out, `data-pin` keeps a row on top). Paginated lists must not be
sorted in the browser: they pass `server: {key, dir, onSort}` with `data-key` headers, and the endpoint whitelists `sort` via
`stats::order_by`. A new table that skips this helper is a bug.

Pages load through `dataView()` (`components.js`), which is stale-while-revalidate: it records the GET URLs a page's
`fetch()` issues (they must be fired synchronously when `fetch()` is called), uses them as the cache key, paints a
remembered result at once, refreshes behind it and only re-renders when the JSON differs. Skeletons appear only after
150 ms and then stay at least 300 ms. The cache is in-memory per tab and is cleared by `resetCaches()` (sign-out) and
by any non-GET request. Esc is handled globally in `shell.js` (steps back out of `/libraries/:id`, `/users/:id`,
`/items/:id`); overlays must keep calling `stopPropagation()` on their own Esc.
Each screen is held to the UX patterns from <https://uxgoodpatterns.com>. A generated copy, `ux-rules.md`, may sit in the working
tree for reference; it is someone else's work, is git-ignored and must never be committed. The look is Obsidian's dark theme via the tokens at the top of
`app.css`; categorical chart colours follow the entity (Movie/Episode/Audio/Other), never rank.

## Local QA suite

`qa/` (git-ignored, so it may not exist in a fresh clone) holds a local release gate: `qa/run.sh` runs static checks, an API suite
and a real-browser suite against a throwaway instance built from generated data. Run it before every release and add a check for
every bug fixed. It is a local tool and must never be committed; the same goes for `ux-rules.md`.

## Releases and patch notes

`CHANGELOG.md` is the single source for the in-app **Patch notes** tab (`changelog.rs` parses it). A test fails if
the top entry's version differs from `Cargo.toml`, or an entry lacks a date/notes/known group — so a version bump
and its changelog entry land together. Format: `## [x.y.z] - YYYY-MM-DD`, a summary paragraph (required for `x.y.0`, where its first sentence becomes that series'
headline in the app, and a test enforces it; optional otherwise), then
`### Added | Changed | Fixed | Removed` with one-line bullets (`**bold**` and `` `code` `` are rendered).

History shape: a release is its logical commits (backend before UI, `fix(...)` on their own, then `docs:`),
followed by `chore(release): x.y.z` bumping `Cargo.toml` + `Cargo.lock`, and an annotated tag `vX.Y.Z`. Every
commit that touches Rust must build and pass `cargo test` on its own.

## Licensing

finstats is `GPL-3.0-only` (`LICENSE`, `Cargo.toml`). A new dependency must carry a GPL-3.0-compatible license
(MIT, Apache-2.0, BSD, ISC, Zlib, MPL-2.0 and similar are fine; check with `cargo metadata`). The bundled fonts are
OFL-1.1 and their license texts live next to them in `web/assets/fonts/` — keep them together.

## Publishing

The image has no `USER` line on purpose: `docker-entrypoint.sh` starts as root only to make the data directory belong to `PUID:PGID`
(default 1000:1000; Docker creates a missing bind-mount folder as root, which is what broke 1.0.0 on fresh machines), then `su-exec`s
to that user; with `--user` it changes nothing. finstats itself never runs as root. `main.rs::ensure_writable` fails fast with the fix.
Test the image on folders Docker creates (`qa/run.sh docker`), not on a data folder that already exists on the dev machine.

The repository is `github.com/OlaYZen/finstats`; images go to `ghcr.io/olayzen/finstats`. `.github/workflows/docker.yml` runs the unit
tests, builds amd64 and arm64 on native runners (no QEMU), and publishes `:edge` from `main` and `:X.Y.Z`, `:X.Y`, `:X`, `:latest` from a
`vX.Y.Z` tag, then creates the GitHub release from that version's `CHANGELOG.md` section. It refuses a tag that does not match
`Cargo.toml` or has no changelog entry, so the release commit and its tag must be pushed together. Docs always point at the published
image, never at a locally built tag.

## Git conventions

- Conventional-commit subjects, with a scope where one fits (`feat(recap):`, `fix(import):`, `refactor`, `docs`,
  `chore`). Subject says what changed; body says why.
- No LLM attribution of any kind: no `Co-Authored-By:` for Claude or any model, no session links, no
  "Generated with…" line, in commits or PR text, regardless of tool defaults.
- Commit as work lands; each commit is one logical change and stands on its own. Docs may follow as their own `docs:` commit.
- Never push unless explicitly told to.

## This repository is public

`data/` (the SQLite database: users, IP addresses, the Jellyfin API key) and Jellystat backups (`*.jsonl`,
`backup_*`) sit next to the source on development machines and must never be committed; `.gitignore`,
`.dockerignore` and `.githooks/pre-commit` (enable with `git config core.hooksPath .githooks`) guard this. Stage
explicit paths rather than `git add -A`. Tests, docs, examples and commit messages use invented data only
(e.g. "alice", "Big Buck Bunny", `Europe/London`, `192.168.1.10`) — never values from a real server.
