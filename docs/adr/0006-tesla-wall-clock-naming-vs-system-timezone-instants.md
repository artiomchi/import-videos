# 0006 — Tesla: vehicle-local wall-clock naming vs. system-timezone instants

- Status: superseded by ADR 0008
- Date: 2026-07-10

## Context

Tesla's `event.json` and folder/clip names carry civil datetimes (`2026-07-04T18:23:51`, `2026-07-04_18-23-51`) with no UTC offset — they are the vehicle's local wall clock. Two consumers want different things from the same value: destination folder names should reproduce what the user saw on the car's screen and on the card's own folders, while each imported file's `recorded_at`/mtime should be a real, unambiguous instant.

The existing `layout` mini-language only formats `{date...}` in UTC via the group's `timestamp` (`src/config/layout.rs`), so a naive `{date:%H-%M-%S}` layout field would shift names by the UTC offset and could flip the date near midnight — visibly wrong against the vehicle's own folder names.

## Decision

Parse Tesla timestamps as `jiff::civil::DateTime` (no timezone attached), then split by consumer:

- **Layout-context fields** (`event_type`, `event_date`, `event_time`) are formatted directly from the civil value — pure wall clock, immune to timezone and DST.
- **`MediaGroup.timestamp` and per-file `recorded_at`** are produced by resolving that civil value in the **system timezone** (`jiff::tz::TimeZone::system()`, compatible disambiguation for DST gaps/folds) — a correct instant whenever the vehicle and the importing machine share a timezone, the common case for a daily-driver dashcam.

Rejected alternatives:
- `{date:%H-%M-%S}` layout fields — formats in UTC, so names drift from the vehicle's own folder names by the UTC offset and can land on the wrong calendar day.
- Interpreting the civil time as UTC directly — folder names stay correct, but every mtime is silently wrong by the local UTC offset.

## Consequences

- If the vehicle and the importing machine are in different timezones, mtimes skew by the zone difference; folder names (wall clock) are unaffected. Not solved here — would require configuring the vehicle's own zone, not justified for this project's scope.
- DST gaps/folds at an event's civil time resolve deterministically via jiff's compatible disambiguation; names are never affected since they never resolve through a timezone.
- `resolve_instant` (`src/source/tesla.rs`) is now the one place Tesla converts civil time to a real instant; any future device needing the same wall-clock/instant split can reuse the pattern.
