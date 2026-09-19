# How Jellystat data is imported

finstats reads Jellystat's backup format directly (`.jsonl`, and the older single-file `.json`).
The backup is streamed to disk and parsed line by line, so its size barely matters, and the whole
import is one transaction: it either fully succeeds or changes nothing. Plays are de-duplicated by
their Jellystat id, which makes importing the same backup twice safe.

Backups that also contain libraries, items and users are welcome — finstats uses those tables to
fill in anything it has not yet read from Jellyfin itself, and never overwrites fresher data.

## What the fields mean

Learned from real exports rather than documentation:

| Jellystat field | How finstats reads it |
|---|---|
| `ActivityDateInserted` | The **end** of the play. Start = end − `PlaybackDuration`. |
| `NowPlayingItemId` + `EpisodeId` | For episodes the first is the *series*, the second the episode. |
| `PlayState.PositionTicks` | Not imported. Jellystat usually captures it when a session is first seen, not where playback stopped. Completion for imported plays is *time watched ÷ runtime* instead. |
| `PlayMethod: Transcode` with video *and* audio copied | Stored as a direct stream (a remux), the same as for live plays. |
| *(no item type exists)* | Episodes are recognised by `EpisodeId`; other items by the library. Something with video that is not in the library and has no file container is a **Live TV** channel. |
| `jf_playback_reporting_plugin_data` | Skipped — Jellystat has already folded these rows into its activity table. |

## What imported history cannot have

Jellystat stores one row per play, so imported plays have no timeline of pauses, skips and track
switches, no pause or skip counts, and no resume point. Everything finstats records itself does.

## Without the browser

```sh
docker run --rm -v "$PWD/data:/data" -v /path/to/backup.jsonl:/backup.jsonl:ro \
  ghcr.io/olayzen/finstats:latest import-jellystat /backup.jsonl
```
