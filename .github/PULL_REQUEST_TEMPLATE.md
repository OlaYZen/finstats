<!-- Thanks! Keep it short. CONTRIBUTING.md explains each line of the checklist. -->

## What a user will notice

<!-- One or two plain sentences. This becomes the patch note. "Nothing" is a fine answer for a refactor. -->

## Why this is the right change

<!-- What was wrong, what you found, what you decided against. Link the issue: Fixes #123 -->

## How it was checked

<!-- What you ran and looked at. Screenshots for UI changes, with invented or blurred data. -->

## Checklist

- [ ] `cargo test` passes, and a test was added if this fixes a bug
- [ ] the web UI still parses (`node --check` loop), and I did **not** run `cargo fmt`
- [ ] UI changes were looked at on a desktop *and* at phone width, as an administrator *and* as a user without permissions
- [ ] an endpoint change comes with its `docs/api.md` change
- [ ] nothing from a real server is in the code, tests, docs, screenshots or commit messages
- [ ] `CHANGELOG.md` and the version in `Cargo.toml` are untouched
- [ ] commits are conventional, one logical change each, and their bodies say why
