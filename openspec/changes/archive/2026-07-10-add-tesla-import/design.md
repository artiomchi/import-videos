## Context

The core pipeline (scan → plan → execute, ADR 0003), verified transfer, and the GoPro source are archived and green. This changeset adds the second `ImportSource` implementation (ADR 0005): `src/source/tesla.rs`, driven entirely by filesystem layout and one JSON file per event — no binary parsing, unlike GoPro.

A TeslaCam USB drive looks like:

```
TeslaCam/
  SavedClips/2026-07-04_18-23-51/     # user-triggered (honk, dashcam save, ...)
    event.json                        # {"timestamp":"2026-07-04T18:23:51","city":"London",
                                      #  "est_lat":"51.5012","est_lon":"-0.1246",
                                      #  "reason":"user_interaction_honk", "camera":"0", ...}
    thumb.png
    2026-07-04_18-18-32-front.mp4     # per-minute, per-angle clips
    2026-07-04_18-18-32-back.mp4
    2026-07-04_18-18-32-left_repeater.mp4
    ...
  SentryClips/<same shape>/           # reason e.g. sentry_aware_object_detection
  RecentClips/                        # flat rolling buffer, no event.json
    2026-07-04_18-40-00-front.mp4 ...
```

Two facts about the existing core shape this design:

- **Layout templates** (`src/config/layout.rs`) apply strftime only to the reserved `date` field, which formats the group timestamp *in UTC*; every other `{field}` is a plain lookup in `MediaGroup.context`. The roadmap sketch `{time:%H-%M-%S}` therefore cannot work as written.
- **Sidecar content** is a fixed `serde_json::Value` attached at scan time; the transfer engine writes it verbatim after the group verifies. Transfer-time blake3 hashes are not fed back into it.

## Goals / Non-Goals

**Goals:**

- Detect a TeslaCam drive, discover SavedClips/SentryClips events, and import whole event folders as atomic units.
- Filter by event category and trigger reason with verdicts visible in `scan` output (`Ignore(reason)`, never silent omission).
- Name destination folders by the *vehicle's wall clock* (matching what the car UI and the card's own folder names show) while stamping machine-accurate mtimes.
- Normalized `import.json` sidecar per event.
- RecentClips importable when explicitly enabled.

**Non-Goals:**

- No quarantine flow — Tesla verdicts are `Keep` or `Ignore` only (proposal). Quarantine exists for "footage we might want but can't tell"; a filtered Tesla event is a deliberate, reversible config choice and the footage stays on the card.
- No video/binary parsing, no stitching of camera angles, no dedup of footage that appears in both RecentClips and a saved event.
- No changes to `cli-core` requirements: layout template semantics, transfer engine, and the `ImportSource` trait are untouched.
- Single-drive semantics only; multi-vehicle libraries are a config concern (separate profiles), not a code one.

## Decisions

### D1 — Detection: `TeslaCam/` with at least one clips directory

`detect()` returns true iff `root/TeslaCam/` exists and contains at least one of `SavedClips`, `SentryClips`, `RecentClips`. Requiring a clips dir avoids claiming a freshly-formatted (empty) drive that another profile might legitimately own; requiring media *files* would be overkill — an empty-but-structured card is still a Tesla card, and scanning it yields an empty plan harmlessly.

### D2 — One `MediaGroup` per event folder, contents imported verbatim

Each `SavedClips/<ts>/` or `SentryClips/<ts>/` folder becomes one group containing *every* file in it — camera angles, `event.json`, `thumb.png`, and anything unrecognized. The event folder is an atomic unit of evidence; unlike GoPro's DCIM (a grab-bag of session chapters plus camera cruft), everything Tesla put in an event folder belongs to that event. Profile `ignore` globs still apply (trait contract) but the Tesla profile defaults to none. Files under `TeslaCam/` but outside any recognized structure are reported as one `Ignore("unrecognized file(s)")` group, mirroring the GoPro pattern.

### D3 — Vehicle-local wall clock for names, system timezone for machine time

`event.json` timestamps (and folder names) are civil datetimes in the vehicle's local time, with no UTC offset recorded. Two consumers want different things:

- **Folder naming** should reproduce the wall clock — the user correlates imported footage with what the car UI showed and with the card's own folder names.
- **`recorded_at`/mtime** should be a real instant.

Resolution: parse the civil datetime with `jiff::civil::DateTime`, then

1. **context fields** `event_type` (`saved`/`sentry`/`recent`), `event_date` (`YYYY-MM-DD`), `event_time` (`HH-MM-SS`) are formatted directly from the *civil* value — pure wall clock, immune to timezone/DST. Documented default layout: `{event_type}/{event_date}/{event_time}`.
2. **`MediaGroup.timestamp`** and per-file `recorded_at` come from resolving the civil time in the **system timezone** (`jiff`'s tzdb, compatible disambiguation for DST gaps/folds) — correct instants whenever the vehicle and the importing machine share a timezone, which is the overwhelmingly common case for a daily-driver dashcam.

Alternatives rejected: `{date:%H-%M-%S}` in layouts (formats in UTC → names shift ±1h across DST vs. what the car showed, dates flip near midnight); interpreting civil time *as* UTC (names right, mtimes silently wrong by the offset). This split — name by wall clock, stamp by system zone — is a real decision worth **ADR 0006**.

### D4 — Tolerant `event.json` parsing; fail open

Missing or corrupt `event.json`, or missing fields, never drops footage:

- Timestamp falls back to parsing the event **folder name** (`YYYY-MM-DD_HH-MM-SS`); if that fails too, the group is `Ignore("unparseable event folder")` — a folder with neither is not evidence of an event.
- `est_lat`/`est_lon` arrive as JSON *strings*; parse to `f64` for `MediaGroup.geo`, `None` on failure.
- Unknown/absent `reason`: the reason filter (D5) is only evaluated when a reason is known — an event with unknown reason is **kept**. Bias toward preserving footage: a filter miss costs disk space, a false drop costs evidence.

### D5 — Filtering config: `events` list + mutually exclusive `reasons.allow`/`reasons.deny`

```yaml
tesla:
  type: tesla
  events: [saved, sentry]        # default; add `recent` to enable RecentClips
  reasons:
    deny: [sentry_aware_object_detection]   # or `allow: [...]` — not both
```

- `events` selects categories; a category not listed yields `Ignore("event type 'sentry' not enabled")` per event group (visible, countable in scan output) rather than skipping the directory walk.
- `reasons` is an enum of `allow` (import only these) xor `deny` (import all but these); both present → config load error, following the existing load-time-validation style (`require_marker` gating). Applies to any event with a known reason regardless of category; RecentClips has no reasons so is unaffected.
- These fields live on `SourceKind::Tesla`, so serde's internally-tagged enum rejects them on other profile types for free; explicit cross-field validation is only needed for the allow/deny exclusivity.

### D6 — RecentClips: opt-in, one group per clip-timestamp cluster

When `recent` is in `events`, files in `RecentClips/` are grouped by their filename stem timestamp (`YYYY-MM-DD_HH-MM-SS` prefix shared by all angles of one minute) — each cluster is one group with `event_type: recent` and wall-clock context from the stem. Per-minute groups are the honest unit: there is no event boundary in a rolling buffer, and inventing sessions from gaps would be guesswork. Not enabled by default; the buffer is noise by design.

### D7 — Sidecar `import.json`, no checksums

Named `import.json` — `event.json` is already among the imported files (verbatim, D2) and must not be shadowed. Content: `camera: "tesla"`, `event_type`, source folder path, the parsed event fields (timestamp as recorded, city, lat/lon, reason), resolved wall-clock and UTC times with a `time_source` note, and the file list.

**No per-file checksums**, dropping that item from the proposal: sidecar content is fixed at plan time (core mechanism), while blake3 hashes exist only inside the transfer engine. Feeding them back would be a `cli-core` requirement change — out of scope and low value, since transfer already *verifies* every byte; a checksum in the sidecar would only re-state that. The proposal will be amended accordingly.

### D8 — Timestamp fields on files

Per-file `recorded_at` for clips comes from each clip's own filename stem (each minute-clip carries its start time), resolved via D3; `event.json`/`thumb.png` use the event timestamp. This gives every imported file an mtime matching when its footage actually started, consistent with the GoPro module's mtime stamping.

## Risks / Trade-offs

- [Vehicle and importing machine in different timezones] → mtimes skew by the zone difference, but folder names (wall clock) stay correct; documented in the profile docs. Accepted: correct handling would require the user to configure the vehicle's zone, complexity not justified for a commute dashcam.
- [Tesla firmware changes `event.json` schema or folder layout] → D4's tolerant parsing degrades to folder-name timestamps and unknown reasons (kept, not dropped). Integration tests pin the currently-known shape.
- [Sentry deny-list typos silently keep noise] → scan output shows per-event verdicts with reasons, so a miss is visible before import; no silent behavior to debug.
- [DST gap/fold at event time] → jiff's compatible disambiguation picks a deterministic instant; names are unaffected (wall clock).
- [RecentClips cluster grouping produces many small groups] → accepted; it is opt-in and reflects the actual structure of the buffer.

## Open Questions

None blocking. ADR 0006 (D3's wall-clock/machine-time split) is written as part of this changeset, when the decision lands in code.
