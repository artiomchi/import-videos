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
    layout: "{event_type}/{event_date}/{event_time}"
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
| `ignore`        | Glob patterns for files to skip entirely                         |
| `quarantine`    | Where footage that doesn't meet the keep criteria goes           |
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
Kept sessions get a `markers.json` sidecar recording the camera model,
session id, and chapter files. HERO8 chapters carry a GPMF telemetry track
(`gpmd`) with GPS fixes and GPS-derived UTC; when it's present and usable
(at least a 2D lock, DOP ≤ 5.0), the session's timestamp — and so its
`{date:...}` destination folder — is the GPS-corrected UTC instant rather
than the camera's clock, which drifts and (on GoPros) is local time
mismarked as UTC. The sidecar then records `"time_source": "gps"`, the
session's `clock_offset_s`, and each marker's corrected `utc` plus `lat`/
`lon` (omitted for a marker with no nearby fix). Imported files' mtime is
set to this corrected recording time after the verified copy completes —
file content is untouched either way. Without usable telemetry (no `gpmd`
track, no fix, or unparseable data), everything falls back to today's
behavior: camera-clock timestamp, `"time_source": "camera"`, each marker's
`camera_time`. A telemetry problem is logged and never fails or requeues an
import — it only ever affects timestamps, never the Keep/Quarantine
verdict.

Destination dates stay UTC-based even with GPS correction: a session that
crosses midnight UTC lands in the UTC calendar date, which can read as the
"wrong" local day for a late-evening ride. A `{date:local:...}` layout
field to resolve against local time instead is a possible future addition,
not something this changeset does.

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

Event timestamps are the vehicle's own local wall clock (see ADR 0006):
destination folders reproduce that wall clock via the `event_type`,
`event_date`, and `event_time` context fields (e.g.
`{event_type}/{event_date}/{event_time}` → `saved/2026-07-04/18-23-51`,
matching what the car's screen and the card's own folder names show),
while each clip's `recorded_at`/mtime is resolved as a real instant in
the importing machine's system timezone. If the vehicle and the
importing machine are in different timezones, mtimes skew by the
difference but folder names stay correct. A corrupt or missing
`event.json` falls back to the event folder's own name for the
timestamp; if neither is parseable, the event is `Ignore`d rather than
imported with a guessed time. Each kept event gets a normalized
`import.json` sidecar: device type, event type, source path, the parsed
`event.json` fields, resolved wall-clock and UTC times with the
timestamp's provenance (`event_json` or `folder_name`), and the file
list.

`layout` is a small template language: `{date:%Y}/{date:%Y-%m-%d}` resolves
`{date...}` against the media group's timestamp via
[jiff strftime](https://docs.rs/jiff) conversion specifiers; any other
`{field}` resolves from context the device module supplies (vocabulary is
defined per device). A malformed template is rejected when the config
loads, not partway through an import.

## Usage

Always scan before importing — it's read-only and shows exactly what
`import` would do:

```sh
import-videos scan commute
```

```sh
import-videos import commute
```

Useful flags:

- `--source PATH` — use this path instead of the profile's configured source
- `--dry-run` — print the plan and stop (same as `scan`, but via `import`)
- `--keep-source` — never delete source files, even if the profile requests it
- `--yes` — skip the confirmation prompt before deleting source files
- `-v` / `-vv` — increase log verbosity; also expands the plan output (`scan` /
  `import --dry-run`): shows quarantined sessions and per-marker details,
  which are otherwise collapsed into the closing summary line
- `--config PATH` — use a config file other than the default

Exit codes: `0` success (including "nothing to import"), `1` if any
planned action failed, `2` on a configuration or usage error.

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
