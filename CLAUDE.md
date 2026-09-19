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
docker build -t finstats:latest .
```

- **Do not run `cargo fmt`.** The code is deliberately not rustfmt-formatted (hundreds of long lines); formatting
  would rewrite every file. Match the surrounding style by hand.
- Debug builds read `web/` from disk at runtime (rust-embed), so UI edits only need a browser refresh.
  Release builds embed it — rebuild to see UI changes. `CHANGELOG.md` is `include_str!`'d, so it always needs a rebuild.
- Env: `FINSTATS_DATA_DIR`, `FINSTATS_BIND`, `FINSTATS_TRUST_PROXY`, `JELLYFIN_URL` + `JELLYFIN_API_KEY` (skip the wizard), `TZ`, `RUST_LOG`.

## Architecture

```
Jellyfin ──/Sessions every 5 s──▶ collector ──▶ SQLite ◀── stats / recap API ◀── embedded SPA (web/)
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

**Profiles (`profile.rs`).** Show progress counts only episodes that exist as files (`path`/`size_bytes` set; the sync
asks Jellyfin to exclude virtual items, and season 0 is skipped). "Seen" merges three sources in order: a recorded
play ≥ 80%, Jellyfin's played flag (`user_items`), a manual mark (`manual_seen`, written via `POST /api/me/seen` for
the caller only — finstats never writes to Jellyfin). Streaks are all-time and share `recap::longest_run`.

**Recap (`recap.rs`)** is one person's year: the caller's own, or, for a Jellyfin administrator only (`is_admin`, not a
permission), the user in `user_id`. There is deliberately no whole-server edition. It excludes Live TV item types and
defaults to the year that is "ready": the current year in December, otherwise the previous one.

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

**HTTP contract.** `docs/api.md` is the contract the UI is written against; change it together with the endpoint.

## Frontend (`web/`)

No build step, no dependencies, no CDN: vanilla ES modules served from the binary, fonts bundled. `dom.js` (`h()`
element builder, icon map, formatters), `api.js`, `router.js` (History API; the server returns `index.html` for any
non-`/api`, non-`/assets` path), `components.js`, `charts.js` (hand-rolled SVG), `pages/*.js`. Rules that hold
everywhere: API/user strings reach the DOM only via `h()`/`textContent` (never `innerHTML` with data); fetches are
aborted and timers cleared on route change; every new card must hide itself when its data is missing (older servers,
non-admins, imported plays). Native `el.append(null)` prints the text "null" — pass possibly-absent nodes through
`h()` or filter them first.

"Now playing" (`nowPlayingView` in `widgets.js`) is polled every 5 s but ticks locally every second. It only re-syncs to the
server's position when that means something (pause, a gap over 15 s, or the server being ahead): clients report their
position to Jellyfin roughly every 10 s, so snapping to every poll would make the clock jump backwards.

Pages load through `dataView()` (`components.js`), which is stale-while-revalidate: it records the GET URLs a page's
`fetch()` issues (they must be fired synchronously when `fetch()` is called), uses them as the cache key, paints a
remembered result at once, refreshes behind it and only re-renders when the JSON differs. Skeletons appear only after
150 ms and then stay at least 300 ms. The cache is in-memory per tab and is cleared by `resetCaches()` (sign-out) and
by any non-GET request. Esc is handled globally in `shell.js` (steps back out of `/libraries/:id`, `/users/:id`,
`/items/:id`); overlays must keep calling `stopPropagation()` on their own Esc.
`ux-rules.md` is the checklist each screen is held to. The look is Obsidian's dark theme via the tokens at the top of
`app.css`; categorical chart colours follow the entity (Movie/Episode/Audio/Other), never rank.

## Releases and patch notes

`CHANGELOG.md` is the single source for the in-app **Patch notes** tab (`changelog.rs` parses it). A test fails if
the top entry's version differs from `Cargo.toml`, or an entry lacks a date/notes/known group — so a version bump
and its changelog entry land together. Format: `## [x.y.z] - YYYY-MM-DD`, optional summary paragraph, then
`### Added | Changed | Fixed | Removed` with one-line bullets (`**bold**` and `` `code` `` are rendered).

History shape: a release is its logical commits (backend before UI, `fix(...)` on their own, then `docs:`),
followed by `chore(release): x.y.z` bumping `Cargo.toml` + `Cargo.lock`, and an annotated tag `vX.Y.Z`. Every
commit that touches Rust must build and pass `cargo test` on its own.

## Licensing

finstats is `GPL-3.0-only` (`LICENSE`, `Cargo.toml`). A new dependency must carry a GPL-3.0-compatible license
(MIT, Apache-2.0, BSD, ISC, Zlib, MPL-2.0 and similar are fine; check with `cargo metadata`). The bundled fonts are
OFL-1.1 and their license texts live next to them in `web/assets/fonts/` — keep them together.

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
