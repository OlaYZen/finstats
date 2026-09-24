# Contributing to finstats

Thanks for wanting to help. finstats is a small project with a narrow idea of itself, and this page tells you
how to report a problem, suggest something, or send a change that is likely to be merged. Everyone taking part
is expected to follow the [code of conduct](CODE_OF_CONDUCT.md).

- [Ways to help](#ways-to-help)
- [What finstats is, and what it will not become](#what-finstats-is-and-what-it-will-not-become)
- [Reporting a bug](#reporting-a-bug)
- [Suggesting a feature](#suggesting-a-feature)
- [Reporting a security problem](#reporting-a-security-problem)
- [Your first change](#your-first-change)
- [A local Docker setup](#a-local-docker-setup)
- [Rules for a change](#rules-for-a-change)
- [Commit messages](#commit-messages)
- [Opening a pull request](#opening-a-pull-request)
- [Licence](#licence)

## Ways to help

You do not have to write Rust. Useful contributions, roughly in order of how often they are needed:

- **A good bug report.** Especially from setups unlike the maintainer's: another Jellyfin version, ARM
  hardware, a reverse proxy, rootless Docker, a NAS, a big library, unusual clients.
- **Telling us where the docs lost you.** If the README made you guess, that is a bug in the README.
- **A fix** for something in the [issue list](https://github.com/OlaYZen/finstats/issues).
- **A feature**, after talking about it first (see below).
- **Answering a question** in [Discussions](https://github.com/OlaYZen/finstats/discussions), or showing how you
  run it. Other people's setups are the best documentation.

A question about installing or running finstats belongs in
[Discussions → Q&A](https://github.com/OlaYZen/finstats/discussions/categories/q-a), not in an issue.

## What finstats is, and what it will not become

Knowing this saves you from building something that cannot be merged.

- **Read-only towards Jellyfin.** finstats never changes anything on your server and never starts a scan.
  The one write it ever makes is creating its own API key during setup.
- **Light.** One binary, a bundled SQLite database, about 20 MB of memory. No database server, no queue, no
  cache service. A change that needs a second container will not be merged.
- **Private.** No telemetry, no accounts, nothing loaded from other hosts. By default the only request it makes
  to anything but your Jellyfin is an anonymous "what is my IP" lookup, made once and switchable off; the one other, downloading
  a geolocation database for the Security map, stays off until the owner asks for it, and addresses are always looked up
  locally. Notifications are the one thing finstats sends rather than reads, and only to destinations the owner enters, only the
  events ticked for each, and without addresses unless that destination asked for them. Settings → Outbound connections lists every
  destination, so a new one cannot be added quietly: it shows up there. The README and
  [`docs/security.md`](docs/security.md) state these things as promises; a change that would make one of
  those sentences untrue has to change the sentence too, and will be looked at very hard.
- **No front-end build step.** The web UI is plain ES modules with no dependencies. Please do not add a
  framework, a bundler or a CDN.
- **Few dependencies.** Every crate is something to audit and keep alive. Adding one needs a reason, and a
  GPL-3.0-compatible licence. It also needs its licence recorded: run `python3 tools/make-third-party.py`
  (after `cargo fetch`) and commit the updated `THIRD-PARTY.json` with the change. `cargo test` fails while
  that file does not cover every package in `Cargo.lock`, and the app shows the result under
  **Settings → Licences**.
- **Personal where it matters.** The recap is one person's year and there is deliberately no whole-server
  edition; permissions are enforced on the server, never only in the UI.

## Reporting a bug

[Open a bug report](https://github.com/OlaYZen/finstats/issues/new?template=bug_report.yml). The form asks
for what is needed; the two things that matter most:

- **The finstats version** (bottom right of every page, or the Patch notes tab) and **how you run it**
  (which image tag, or from source).
- **The log** around the moment it went wrong: `docker logs finstats`. For more detail, start it with
  `-e RUST_LOG=finstats=debug`.

**Look at what you paste before you post it.** Logs and screenshots contain user names, titles, device names
and IP addresses of the people on your server. Blur or replace them. **Never attach** `finstats.db`, a
finstats backup or a Jellystat export: each is a complete viewing history, and the database also contains
your Jellyfin API key. If a maintainer needs data to reproduce something, they will ask for the smallest
piece that shows it.

## Suggesting a feature

[Open a feature request](https://github.com/OlaYZen/finstats/issues/new?template=feature_request.yml) and
describe **the question you could not answer** ("which of my users still use the old Android app?") rather
than the screen you imagine. There is often a smaller way to answer it, sometimes with data finstats
already has.

Not sure yet whether it is a feature? Think out loud in
[Discussions → Ideas](https://github.com/OlaYZen/finstats/discussions/categories/ideas) first. For anything bigger
than a bug fix, please **talk before you build**. It is no fun to review, or to write, a
large pull request that does not fit.

## Reporting a security problem

Please **do not open a public issue** for a vulnerability. See [`SECURITY.md`](SECURITY.md) for how to report
it privately.

## Your first change

```sh
# fork on GitHub, then:
git clone https://github.com/<you>/finstats.git && cd finstats
git config core.hooksPath .githooks        # refuses databases, backups and secrets; do this once per clone
git switch -c fix/what-you-are-fixing

FINSTATS_DATA_DIR=./data FINSTATS_BIND=127.0.0.1:8088 cargo run
```

Open <http://127.0.0.1:8088> and point it at a Jellyfin server. You need a current stable Rust (edition 2024,
so 1.85 or newer); SQLite is bundled; Node.js is only used for a syntax check. There is nothing to
`npm install`.

With `cargo run`, changes to the web UI (`web/`) need only a browser refresh: debug builds read it from disk.
Rust changes need a restart. Release builds and the Docker image compile the UI and `CHANGELOG.md` in, so
they need a rebuild. To start over, stop finstats and delete the data directory. `RUST_LOG=finstats=debug`
shows what the collector and the syncs are doing.

[`CLAUDE.md`](CLAUDE.md) explains how the code is organised and why; [`docs/api.md`](docs/api.md) is the
HTTP contract.

## A local Docker setup

finstats only reads from Jellyfin, so developing against your real server is safe. A throwaway one is still
nicer: you can create users, break things and reset it without anyone noticing. This runs your working tree
as an image next to a throwaway Jellyfin, on a private network, with its own data folder. Nothing here
touches a production container or its data. Compose is not needed.

```sh
# 1. a network, so the containers find each other by name
docker network create finstats-dev

# 2. a throwaway Jellyfin (two or three short media files are plenty)
docker run -d --name jellyfin-dev --network finstats-dev -p 8097:8096 \
  -v jellyfin-dev-config:/config -v jellyfin-dev-cache:/cache \
  -v /path/to/a/few/media/files:/media:ro \
  jellyfin/jellyfin

# 3. your working tree as an image
docker build -t finstats:dev .

# 4. run it: another port, another data folder, debug logging
docker run -d --name finstats-dev --network finstats-dev -p 8089:8080 \
  -e TZ=Europe/London -e RUST_LOG=finstats=debug \
  -v "$PWD/data-dev:/data" \
  finstats:dev
```

1. Open <http://localhost:8097> and click through Jellyfin's own first-run wizard: create an administrator,
   add `/media` as a library.
2. Open <http://localhost:8089>. In finstats' setup the Jellyfin address is **`http://jellyfin-dev:8096`**:
   the container's name and Jellyfin's *internal* port, not `localhost` and not 8097. Inside a container,
   `localhost` is the container itself; this is the most common way to get "Could not connect".
3. Play something in the Jellyfin web client. It appears on the finstats dashboard within five seconds.
   Pause, skip and switch subtitles to get a timeline.

After a change: `docker build -t finstats:dev . && docker rm -f finstats-dev`, then step 4 again, and
`docker logs -f finstats-dev`. Dependencies are cached in a layer of their own, so a rebuild compiles only
finstats unless you touched `Cargo.toml` or `Cargo.lock`.

Things that trip people up:

- Jellyfin accepts connections a few seconds before it is ready. "Did not answer like a Jellyfin server"
  right after starting it means: wait a moment.
- `data-dev/` is created by Docker as root; the container takes it over on start and then runs as an ordinary
  user. Start it with `-e PUID=$(id -u) -e PGID=$(id -g)` if you want the files to be yours. It is not in
  `.gitignore`: add it to `.git/info/exclude` if you keep it inside the repository folder.
- When you change the image or its entrypoint, test on a data folder that **does not exist yet**. A folder
  that has had the right owner for weeks hides exactly the bugs strangers will hit.
- Without `TZ`, "per day" and "hour of day" are in UTC.
- An empty install is a poor place to work on charts. A Jellystat export imports headless too:
  `docker run --rm -v "$PWD/data-dev:/data" -v /path/to/backup.jsonl:/backup.jsonl:ro finstats:dev import-jellystat /backup.jsonl`.
  Never commit such a file.

Tear down: `docker rm -f finstats-dev jellyfin-dev && docker network rm finstats-dev`, plus
`docker volume rm jellyfin-dev-config jellyfin-dev-cache` and `data-dev/` for a clean slate.

## Rules for a change

Short, because each one exists for a reason that has already cost somebody an evening:

1. **Do not run `cargo fmt`.** The code is deliberately not rustfmt-formatted; it would rewrite every file
   and bury your change. Match the style around you.
2. **Tests pass, and you add one when you fix a bug.** `cargo test`, and
   `for f in web/assets/js/*.js web/assets/js/pages/*.js; do node --check "$f"; done`. `cargo clippy
   --all-targets` has about 20 style warnings; add no new ones. Every commit that touches Rust builds and
   passes on its own.
3. **Look at it like a user.** UI changes: at desktop width *and* around 390 px, as an administrator *and* as
   a user without permissions. Anything a browser downloads: test it the way a browser asks
   (`curl -H 'Accept-Encoding: gzip'`), not with bare `curl`. Anything about the image: on a data folder that
   does not exist yet, not on one that has had the right owner for weeks.
4. **Permissions are decided on the server.** Hiding a button is cosmetic. Statistics go through `Scope` →
   `Cond` in `stats.rs`; IP addresses need `see_network`, file paths `see_server`, other people's rows
   `see_everyone`.
5. **Released migrations are immutable.** Add a new entry to `MIGRATIONS`; never edit or reorder one. A new
   table holding something Jellyfin cannot give back also goes into `backup::TABLES`.
6. **Library reads must ask for real items** (`CollapseBoxSetItems=false`, `ExcludeLocationTypes=Virtual`).
   Whatever a read does not return is marked as removed, so a query that hides items loses data.
7. **Text reaches the page through `h()` or `textContent`**, never `innerHTML`. No inline styles in markup
   (the Content-Security-Policy forbids them). New tables go through `tables.js`. Reuse the existing
   controls before inventing one.
8. **An endpoint and [`docs/api.md`](docs/api.md) change together.**
9. **Invented data only.** Code, tests, docs, screenshots and commit messages never contain anything from a
   real server. Use "alice", "Big Buck Bunny", `Europe/London`, `192.168.1.10`, and the documentation ranges
   (`203.0.113.0/24`, `198.51.100.0/24`) for public addresses. This repository is public and its history is
   permanent.
10. **Leave `CHANGELOG.md` and the version in `Cargo.toml` alone.** A test ties the two together, and the
    changelog is the in-app Patch notes, written at release time. Say in your pull request what a user will
    notice, in one or two plain sentences; that becomes the patch note.

## Commit messages

[Conventional commits](https://www.conventionalcommits.org), with a scope where one fits:

```
fix(import): stop counting Live TV channels as movies

Jellystat has no item type, so imported channels were guessed to be films and
showed up in "top movies". A video that is not in the library and has no
container is a channel.
```

- Types in use: `feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `build`, `ci`, `chore`.
- The **subject** says what changed, in the imperative, without a full stop. The **body** says *why*: what was
  wrong, what you found, what you decided against. The diff already says how.
- **One logical change per commit**, however many files it touches. A fix you noticed on the way is its own
  commit. Documentation may follow as a separate `docs:` commit.
- No attribution lines for tools or models (`Co-Authored-By`, "Generated with …"). You are the author of
  what you send, however you wrote it, and you answer for it.

## Opening a pull request

1. Push your branch to your fork and open the pull request against `main`. The template has a short
   checklist.
2. **Say what a user will notice and why the change is right**, and link the issue. Screenshots for UI
   changes (with invented or blurred data).
3. CI runs the unit tests and the JavaScript check, then builds the image for amd64 and arm64. Nothing is
   published from a pull request.
4. Expect questions, and possibly a request to split or rebase. The history is kept readable on purpose: it
   is how the next person finds out why something is the way it is. Small, focused pull requests are
   reviewed first.
5. Maintainers cut releases (version, patch notes, tag); you do not need to.

## Licence

finstats is licensed under the [GNU General Public License v3.0](LICENSE) (`GPL-3.0-only`). By contributing
you agree that your contribution is licensed under the same terms, and you confirm that you wrote it or
otherwise have the right to submit it under that licence. You keep the copyright to your work.
