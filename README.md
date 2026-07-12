# import-videos

A CLI that imports footage from camera storage (SD cards, USB drives) into a
date-organized video library. It understands device-specific metadata to
decide what's worth keeping — for example, only pulling GoPro clips that
have a HiLight marker, or Tesla dashcam clips tied to a sentry/honk event —
and quarantines everything else instead of deleting it outright.

Every import is **scan → plan → execute**: a read-only scan produces a plan
(what will be kept, quarantined, or ignored, and why); nothing is copied,
moved, or deleted until you review that plan and run `import`.

GoPro HERO8 and Tesla dashcam/sentry footage are supported today; other
devices follow in later changesets. See "What gets kept" below for each
device's keep/quarantine (or, for Tesla, keep/filter) rule.

## Install

```sh
cargo install --path .
```

## Configuration

Profiles live in a YAML file at `~/.config/import-videos/config.yaml`
(override with `--config`). Each profile selects a device `type` and
where its footage goes:

```yaml
# Optional: where `source: auto` looks for mounted cards.
# Defaults to /run/media/<user>, /media, /mnt.
mount_roots:
  - /run/media/alice
  - /media

profiles:
  commute:
    type: gopro               # selects the device implementation
    source: auto               # or an explicit path, e.g. /media/alice/GOPRO
    destination: ~/Videos/commute
    layout: "{date:%Y}/{date:%Y-%m-%d}_{date:%H-%M}"
    ignore:
      - "*.THM"
      - "*.LRV"
    quarantine: ~/Videos/commute/_quarantine
    delete_source: true
    require_marker: true      # gopro-specific: see "What gets kept" below

  dashcam:
    type: tesla
    source: auto               # or an explicit path, e.g. /media/alice/TESLA
    destination: ~/Videos/tesla
    layout: "{event_type}/{date:%Y-%m-%d}/{date:%H-%M-%S}"
    events: [saved, sentry]     # tesla-specific: default shown; add `recent` to import RecentClips too
    reasons:
      deny: [sentry_aware_object_detection]   # or `allow: [...]` — not both
    delete_source: true
```

Common fields, available to every profile:

| Field           | Meaning                                                         |
| --------------- | ---------------------------------------------------------------- |
| `type`          | Selects the device implementation (e.g. `gopro`, `tesla`)        |
| `source`        | `auto` (probe mount roots) or an explicit path                   |
| `destination`   | Where kept footage lands                                         |
| `layout`        | Path template under `destination`, resolved per media group      |
| `timezone`      | IANA timezone name for `{date:...}` layout fields and mtime stamping (e.g. `Europe/Vilnius`); defaults to the system timezone |
| `ignore`        | Glob patterns for files to skip entirely                         |
| `quarantine`    | Where footage that doesn't meet the keep criteria goes; defaults to `{destination}/_quarantine`. Purged with `cleanup` |
| `delete_source` | Delete source files after a verified transfer (per-run: `--keep-source` overrides) |
| `copy_quarantine` | Copy quarantined footage to the quarantine folder (default `true`). Set to `false` to leave it on the source untouched — it is still reported as `QUARANTINE` in `scan` output, but no copy is made and no quarantine directory is created. A file left in place is never a deletion candidate, so `delete_source` cannot remove it. |

Device-specific fields (only valid on their own `type`; e.g. `require_marker`
on a non-`gopro` profile, or `events`/`reasons` on a non-`tesla` profile,
fails config loading):

| Field            | Type    | Meaning                                                                 |
| ---------------- | ------- | ------------------------------------------------------------------------ |
| `require_marker` | `gopro` | Whether a session needs a HiLight marker to be kept (default `true`)     |
| `events`         | `tesla` | Event categories to import: any of `saved`, `sentry`, `recent` (default `[saved, sentry]`) |
| `reasons`        | `tesla` | `allow: [...]` or `deny: [...]` (not both) — filters by `event.json`'s trigger `reason` |

### What gets kept — GoPro

A HERO8 card's `DCIM/1*GOPRO/` chapter files (`GX01nnnn.MP4`, `GX02nnnn.MP4`,
...) are grouped by session — one commute is one session, even if it spans
several chapters. A HiLight marker (the side-button press) anywhere in the
session keeps the *whole* session; a session with no markers is quarantined,
not deleted. Set `require_marker: false` to keep every session regardless.
Set `copy_quarantine: false` to leave unmarked sessions on the card entirely
— they are still recognized and reported as `QUARANTINE` in `scan` output,
but no copy is made and no quarantine folder is created.
Kept sessions get an `import.json` sidecar recording the camera model,
session id, and chapter files. HERO8 chapters carry a GPMF telemetry track
(`gpmd`) with GPS fixes and GPS-derived UTC; when it's present and usable
(at least a 2D lock, DOP ≤ 5.0), the session's timestamp — and so its
`{date:...}` destination folder — is the GPS-corrected UTC instant rather
than the camera's clock, which drifts and (on GoPros) is local time
mismarked as UTC. The sidecar then records `"time_source": "gps"`, the
session's `clock_offset_s` in the `gopro` device block, and each marker's
corrected timestamp plus `lat`/`lon` (omitted for a marker with no nearby
fix), plus the chapter `file` it was pressed in and a human-readable `offset`
string (`M:SS.mmm`), in the `events[]` array. Example marker event:

```json
{
  "type": "gopro:marker",
  "time": "2026-07-04T15:23:51+03:00",
  "lat": 54.6872,
  "lon": 25.2797,
  "offset_ms": 734120,
  "offset": "12:14.120",
  "file": "GX010123.MP4"
}
```

Imported files' mtime is
set to this corrected recording time after the verified copy completes —
file content is untouched either way. Without usable telemetry (no `gpmd`
track, no fix, or unparseable data), everything falls back to today's
behavior: camera-clock timestamp, `"time_source": "camera"`, each marker's
timestamp in the configured timezone. A telemetry problem is logged and
never fails or requeues an import — it only ever affects timestamps, never
the Keep/Quarantine verdict.

Destination dates are resolved in the configured `timezone` (default: system
timezone). Set `timezone: UTC` in the config to get UTC-based folder names,
or set it to your local IANA name (e.g. `Europe/Vilnius`) to get local dates.

### What gets kept — Tesla

A TeslaCam drive's `SavedClips/<timestamp>/` and `SentryClips/<timestamp>/`
folders each become one event: every file inside — camera-angle clips,
`event.json`, `thumb.png`, anything else — travels together as one atomic
unit. `events` picks which categories are even considered (default `saved`
and `sentry`; add `recent` to also import the flat `RecentClips/` rolling
buffer, clustered into one group per shared per-minute filename stem).
Within an enabled category, `reasons` optionally filters by `event.json`'s
trigger `reason` (e.g. keep `user_interaction_honk`, drop the noisy
`sentry_aware_object_detection`) — `allow` keeps only listed reasons,
`deny` drops only listed reasons (not both). An event whose reason can't
be determined at all (missing/corrupt `event.json`) is always kept: a
filter miss costs disk space, a false drop costs evidence. Unlike GoPro,
filtered-out Tesla events are never quarantined — they get a visible
`Ignore` verdict in `scan` output and are left untouched on the card,
since excluding them is a deliberate, reversible config choice, not
uncertainty about whether the footage matters.

Event timestamps are the vehicle's own local wall clock: destination
folders reproduce that wall clock via `{date:...}` layout fields rendered
in the configured timezone (e.g. `{event_type}/{date:%Y-%m-%d}/{date:%H-%M-%S}`
→ `saved/2026-07-04/18-23-51` when the vehicle's wall clock and the
configured timezone agree). Set `timezone` in the config to match where the
vehicle was driven; it defaults to the importing machine's system timezone.
A corrupt or missing `event.json` falls back to the event folder's own name
for the timestamp; if neither is parseable, the event is `Ignore`d rather
than imported with a guessed time. Each kept event gets a unified
`import.json` sidecar: common envelope (camera, source, times, files) +
`events[]` array with the trigger reason + optional `tesla` device block.

`layout` is a small template language: `{date:%Y}/{date:%Y-%m-%d}` resolves
`{date...}` against the media group's timestamp via
[jiff strftime](https://docs.rs/jiff) conversion specifiers; any other
`{field}` resolves from context the device module supplies (vocabulary is
defined per device). A malformed template is rejected when the config
loads, not partway through an import.

## Usage

### `scan` / `import`

Always scan before importing — it's read-only and shows exactly what
`import` would do:

```sh
import-videos scan commute
```

```sh
import-videos import commute
```

Both `scan` and `import` show a per-chapter/session scan-phase progress
indicator while a card is being read, appearing before the plan is printed
and clearing once scanning finishes. `import` additionally shows a byte-level
progress bar for the transfer phase, after the plan is built — the two never
appear at once. Both are shown only while stdout is an interactive terminal
and `--json` is off; either is silent (no progress, no terminal-control
bytes) when stdout is piped or `--json` is set, so scripted and redirected
runs stay clean.

Useful flags:

- `--source PATH` — use this path instead of the profile's configured source
- `--dry-run` — print the plan and stop (same as `scan`, but via `import`)
- `--keep-source` — never delete source files, even if the profile requests it
- `--yes` — skip the confirmation prompt before deleting source files
- `--quick-match` — skip content hashing when the destination file's name,
  size, and mtime match within 0.1 s of the source's recording time. Useful
  for regenerating `import.json` on already-imported footage without
  re-hashing gigabytes of video. Files accepted this way are never deletion
  candidates (ADR 0009). Recipe: `import <profile> --quick-match --keep-source`
  re-imports metadata and rewrites sidecars cheaply.

### `cleanup`

Purges a profile's quarantine directory — the operational tail to `scan`'s
`QUARANTINE` verdicts. Same plan/confirm/execute safety model as `import`
(ADR 0003): a purge plan is always built and shown before anything is
deleted.

```sh
import-videos cleanup commute --dry-run
import-videos cleanup commute --older-than 30d --yes
```

- `--older-than <span>` — only purge entries that have sat in quarantine
  longer than this (jiff friendly-format span, e.g. `30d`, `2w`, `1mo`).
  Without it, every entry is a candidate. Age is measured from each entry's
  own arrival in quarantine — the group directory's mtime, not the
  recording-stamped mtimes of the files inside it (ADR 0010) — so footage
  recorded months ago but only quarantined yesterday is not purged
  immediately.
- `--dry-run` — print the purge plan and exit without deleting anything
- `--yes` — skip the confirmation prompt before deleting

`cleanup` only ever touches the profile's resolved quarantine directory
(`quarantine`, or `{destination}/_quarantine`); it refuses to run (exit 2,
nothing deleted) if that directory would equal or contain the destination.

### `inspect`

Dumps one file's device metadata for debugging and card triage — no profile
and no config file required:

```sh
import-videos inspect /media/alice/GOPRO/DCIM/100GOPRO/GX010123.MP4
import-videos inspect /media/alice/TESLA/TeslaCam/SavedClips/2026-07-04_18-23-51/
```

For a GoPro `.mp4`: HiLight marker count and per-marker offset/timestamp,
the camera's creation time, and — when a `gpmd` telemetry track is present —
a GPS summary (first usable fix, clock offset from the camera clock, sample
count). For a Tesla event folder (or an `event.json` path directly): the
parsed `timestamp`/`reason`/`city`/coordinates plus the clip files present
alongside it. Parsing is read-only and never modifies the file. A section
that fails to parse (e.g. a corrupt `gpmd` track) still lets the rest of the
dump print — the command exits 1 to signal the partial failure. An
unsupported path (anything that's neither) is a usage error, exit 2.

### Global flags

- `--json` — emit the result as a single JSON document on stdout instead of
  human-readable text, for every subcommand (`scan`, `import`, `cleanup`,
  `inspect`). No other stdout output is produced in this mode; errors still
  go to stderr and exit codes are unchanged. Confirmation prompts still
  apply — `--json` does not imply `--yes`. See "JSON output" below for the
  shape.
- `-v` / `-vv` — increase log verbosity; also expands the plan output (`scan` /
  `import --dry-run`): shows quarantined sessions and per-marker details,
  which are otherwise collapsed into the closing summary line
- `--config PATH` — use a config file other than the default

Exit codes: `0` success (including "nothing to import" / "nothing to
clean"), `1` if any planned action failed (or, for `inspect`, if a section
of the dump failed to parse), `2` on a configuration or usage error.

### JSON output

**v0 — the field set may still evolve.** No breaking changes are planned,
but this hasn't been used against real scripts long enough to freeze yet;
treat unfamiliar fields as forward-compatible additions rather than errors.

Every JSON document is built from dedicated view-model types (`src/report.rs`),
not by serializing internal domain types directly, so the shape is
deliberate rather than an accident of refactoring. Timestamps are RFC 3339
strings in the configured timezone; paths are strings.

- `scan --json` / `import --dry-run --json` — the plan: `actions[]` (group,
  verdict, reason, path, markers, sidecar path) and a `summary` (kept/
  quarantined/ignored/total counts). Unlike the human output, quarantined
  entries are always included.
- `import --json` — the execution report: `groups[]` (per-file outcomes,
  sidecar outcome, whether the group was deleted from source),
  `deletion_skipped_reason`, and a `summary` (transferred/failed/deleted
  counts).
- `cleanup --json` — the purge plan (`entries[]` with name, age, size,
  purge flag, and a summary) for a dry run, or the deletion results
  (`results[]`, `aborted_reason`) once executed.
- `inspect --json` — the metadata dump: raw millisecond offsets alongside
  rendered timestamps for GoPro, or the parsed Tesla event fields; any
  section that failed to parse carries its own `..._error` field instead of
  data.
- A "nothing found" outcome (`scan`/`import` with no sources; `cleanup`
  with an empty quarantine) is still a JSON document — `{"status":
  "no_sources", "profile": "..."}` for the former — never a bare string.

## Development

```sh
cargo build
cargo test
cargo clippy -- -D warnings
cargo fmt --check
```

See `AGENTS.md` for project conventions and `docs/adr/` for the design
decisions behind the scan/plan/execute model, YAML profiles, and the
single-crate + trait-based extensibility approach.
