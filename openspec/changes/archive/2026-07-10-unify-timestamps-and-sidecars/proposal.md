## Why

Timestamp handling is inconsistent across devices and locked to UTC. GoPro
folders format the group's UTC instant via `{date:...}`, while Tesla sidesteps
`{date}` entirely and bakes wall-clock strings into `event_date`/`event_time`
context fields (ADR 0006) so folder names match the car's screen. The result:
two vocabularies for "when did this happen," `{date}` that silently renders UTC
(so late-evening rides land on the "wrong" local day), and no way to ask for a
specific display zone. A single configurable timezone that drives every rendered
timestamp — paths, sidecars, and logs — removes the split and the UTC footgun.

## What Changes

- **BREAKING**: `{date:...}` now renders in the configured display timezone
  instead of UTC. Existing UTC-based layout output changes accordingly.
- Add a global `timezone` config field (IANA name, e.g. `Europe/Vilnius`).
  Unset defaults to the system local zone. It governs every rendered timestamp:
  `{date}` layout fields, sidecar times, and log output.
- **BREAKING**: Tesla's `event_date` and `event_time` layout context fields are
  removed. Tesla timestamps flow through `{date:...}` like every other device.
  `event_type` (`saved`/`sentry`/`recent`) stays — it is a category, not a
  timestamp.
- Tesla's vehicle wall clock is interpreted as being in the configured
  `timezone` (rather than the importing machine's system zone), converting it to
  a correct UTC instant. This supersedes ADR 0006 with a new ADR; no backward
  compatibility is preserved (the tool is not yet released).
- **BREAKING**: consolidate both device sidecars into a single unified
  `import.json` schema. GoPro's `markers.json` is removed; both devices emit
  `import.json` with a common envelope (`camera`, `source`, `imported_at`,
  `timezone`, `recorded_at`, `time_source`, `files`), a device-agnostic
  `events[]` collection (GoPro markers and Tesla triggers, each with a
  namespaced `type` such as `gopro:marker` / `tesla:saved`), and a namespaced
  device block (`gopro: {…}` / `tesla: {…}`) for data that doesn't generalize.
- The unified sidecar records the session/event `recorded_at` (this is the
  GoPro `recorded_at` you asked for) and a new `imported_at` import time; all
  sidecar timestamps are ISO-8601 with the configured zone's offset.
- Remove the deferred `{date:local:...}` idea from the docs — the global
  `timezone` field subsumes it.

## Capabilities

### New Capabilities
- `timezone-rendering`: the global `timezone` config field, its resolution
  (explicit IANA zone or system-local default), and the rule that all rendered
  timestamps — destination paths, sidecar fields, and logs — are formatted in
  that zone.
- `unified-sidecar`: the single cross-device `import.json` schema — common
  envelope, namespaced `events[]` collection, and per-device namespaced block —
  that every device emits, replacing the per-device sidecar formats.

### Modified Capabilities
- `tesla-import`: the vehicle wall clock is interpreted in the configured
  `timezone`; the `event_date`/`event_time` context fields are removed
  (`event_type` retained, date/time via `{date:...}`); the sidecar becomes the
  unified `import.json`. Supersedes ADR 0006.
- `gopro-import`: the camera-clock fallback timestamp is interpreted in the
  configured `timezone` (the GPS-corrected path is unchanged); the sidecar
  becomes the unified `import.json` (markers become `events[]`, session/offset
  move to the `gopro` block) and gains `recorded_at`; `markers.json` is removed.

(The `timezone` config field, its validation, and `{date}` rendering semantics
are owned by the new `timezone-rendering` capability rather than a separate
`cli-core` delta.)

## Impact

- **Config**: `src/config.rs` (new `timezone` field on `RawConfig`/`Config`,
  IANA validation via jiff), README config docs.
- **Layout**: `src/config/layout.rs` (`{date}` resolves through a `TimeZone`
  instead of formatting the raw UTC `Timestamp`).
- **Devices**: `src/source/tesla.rs` (drop `build_context` date fields, reinterpret
  civil time via configured zone, emit unified sidecar), `src/source/gopro.rs`
  (emit unified sidecar with `recorded_at`).
- **Sidecar**: a shared sidecar builder/type (likely `src/source/mod.rs` or a
  new module) producing `import.json` for every device.
- **Rendering surfaces**: report/log output (`src/report.rs`, `tracing` sites)
  that print timestamps.
- **Deps**: `jiff` timezone APIs (already a dependency).
- **Docs/decisions**: new ADR superseding ADR 0006; README GoPro/Tesla and
  layout sections; a `docs/learning/` note if timezone handling introduces a new
  Rust concept.
- **Tests**: layout, config, Tesla, and GoPro sidecar tests; integration tests
  asserting zone-rendered paths.
