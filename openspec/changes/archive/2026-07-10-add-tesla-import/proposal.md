## Why

The Tesla dashcam USB drive fills with SavedClips/SentryClips events that today get triaged by hand — most Sentry events are noise (passers-by), while genuinely useful events (honks, saves, detected collisions) sit mixed in with them. The core pipeline, verified transfer, and GoPro source are done and archived (roadmap changesets 1–3); this changeset delivers the second device the project was scoped around, and the first second implementation of `ImportSource` — proving the trait boundary holds beyond the device it was designed against.

## What Changes

- New `src/source/tesla.rs` implementing `ImportSource` for a TeslaCam USB drive:
  - `detect()`: recognizes a volume by its `TeslaCam/` directory.
  - `scan()`: walks `TeslaCam/SavedClips/<timestamp>/` and `TeslaCam/SentryClips/<timestamp>/`; each event folder becomes one `MediaGroup` (all camera-angle clips + `thumb.png` + `event.json`).
  - Parses `event.json` (`timestamp`, `city`, `est_lat`, `est_lon`, `reason`) for the group's timestamp, geolocation, and trigger reason.
- Event filtering via new `tesla` profile fields:
  - `events: [saved, sentry]` — which event categories to import.
  - Optional `reasons` allow/deny list — e.g. keep `user_interaction_honk`, drop noisy `sentry_aware_object_detection`.
  - Filtered-out events get an `Ignore(reason)` verdict (visible in scan output), not silent omission.
- New `SourceKind::Tesla` config variant carrying those fields, with load-time validation consistent with how `require_marker` is gated to GoPro profiles.
- Normalized sidecar per imported event: superset of `event.json` (adds source path, import context, resolved times), written via the existing `Sidecar` planning mechanism. (No per-file checksums: sidecar content is fixed at plan time while hashes exist only inside the transfer engine, which already verifies every byte — see design D7.)
- Layout template context: event groups supply `event_type` and event time so layouts like `{event_type}/{date:%Y-%m-%d}/{time:%H-%M-%S}` resolve (the `context` map on `MediaGroup` exists for exactly this).
- `TeslaCam/RecentClips/` (rolling buffer, no trigger) ignored by default; importable via config.
- No quarantine flow for Tesla: unlike GoPro, an unwanted event is *filtered* (left on card / cleaned), never quarantined — verdicts are `Keep` or `Ignore`, not `Quarantine`.

## Capabilities

### New Capabilities
- `tesla-import`: TeslaCam card detection, SavedClips/SentryClips event discovery, `event.json` parsing, event-type/reason filtering, whole-event-folder import with normalized sidecar, RecentClips handling.

### Modified Capabilities

None. Tesla plugs in through the existing `ImportSource` trait and profile mechanism; `cli-core`'s requirements (scan/plan/execute, verified transfer, profile config) are unchanged. Device-specific profile fields follow the established pattern of living in the device's own capability spec (as `gopro-import` does for `require_marker`).

## Impact

- **New code**: `src/source/tesla.rs` (detection, event walk, `event.json` parsing, filtering, sidecar assembly).
- **Modified code**:
  - `src/config.rs`: `SourceKind::Tesla` variant + validation for tesla-only fields on non-tesla profiles.
  - `src/source/mod.rs`: register the module (trait itself unchanged).
- **Dependencies**: none new — `serde_json` (event.json), `jiff` (timestamps), `globset` already in the tree.
- **Tests**: Tesla card layouts are pure directories + JSON + arbitrary files — fully synthesizable in `tempfile` integration tests (no binary fixtures needed, unlike GoPro). Cover: detection, event discovery, reason filtering, verdicts, destination layout, sidecar contents.
- **Docs**: ADR only if a real decision surfaces during design (roadmap flags RecentClips handling as the candidate); learning note if the implementation introduces a new-to-this-codebase Rust concept.
