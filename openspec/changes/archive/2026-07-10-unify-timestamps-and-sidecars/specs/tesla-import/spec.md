## REMOVED Requirements

### Requirement: Wall-clock naming with system-timezone instants
**Reason**: The dedicated `event_date`/`event_time` layout-context fields and the system-timezone interpretation of civil times are replaced by the unified `{date:...}` layout resolved in the configured `timezone` (supersedes ADR 0006). Wall-clock reproduction is no longer achieved via bespoke context fields.
**Migration**: Replace `{event_type}/{event_date}/{event_time}` layouts with `{event_type}/{date:%Y-%m-%d}/{date:%H-%M-%S}`, and set the global `timezone` to the vehicle's zone (the default is the importing machine's local zone).

## ADDED Requirements

### Requirement: Tesla timestamps in the configured timezone
Tesla event timestamps SHALL be treated as vehicle civil datetimes interpreted in the configured `timezone` to produce the group timestamp and per-file recorded-at instants. Each event group SHALL expose `event_type` (`saved`, `sentry`, or `recent`) as its only layout-context field; date and time components SHALL be rendered through `{date:...}`, not through dedicated context fields. Each clip's recorded-at SHALL come from its own filename stem interpreted in the configured `timezone`; `event.json` and `thumb.png` SHALL use the event timestamp.

#### Scenario: event_type is available to the layout
- **WHEN** a saved event is imported with layout `{event_type}/{date:%Y-%m-%d}/{date:%H-%M-%S}`
- **THEN** the resolved destination begins with `saved/`

#### Scenario: Layout date reproduces the vehicle clock via the configured zone
- **WHEN** an event with civil timestamp `2026-07-04T18:23:51` is imported with `timezone` set to the vehicle's zone and layout `{event_type}/{date:%Y-%m-%d}/{date:%H-%M-%S}`
- **THEN** the destination directory is `saved/2026-07-04/18-23-51`

#### Scenario: Clip mtimes reflect their own start minute in the configured zone
- **WHEN** an event contains clips with stems `2026-07-04_18-18-32` and `2026-07-04_18-19-32`
- **THEN** each imported clip's modification time corresponds to its own stem interpreted in the configured `timezone`

## MODIFIED Requirements

### Requirement: Normalized import sidecar
Each kept event group SHALL carry the unified `import.json` sidecar (see the `unified-sidecar` capability), written into the event's destination directory after all its files transfer and verify. For Tesla the sidecar SHALL set `camera` to the device identifier, `source` to the event folder path, `recorded_at` to the resolved event instant, and `time_source` to the timestamp's provenance (`event_json` when the time came from `event.json`, or `folder_name` when it fell back to the event folder name). The event's trigger SHALL appear as a single `events` entry with a namespaced `type` (`tesla:saved`, `tesla:sentry`, or `tesla:recent`), its `time`, its `reason` where known, and `lat`/`lon` where known. The event's `city`, when present, SHALL be the only member of the `tesla` device block. The sidecar SHALL NOT be named `event.json`, which is imported verbatim as an event file and is not duplicated into the sidecar.

#### Scenario: Sidecar written alongside imported event
- **WHEN** a saved event with a valid `event.json` is imported
- **THEN** the destination folder contains the original `event.json` plus an `import.json` whose envelope names the source path and whose `events` entry has `type` `tesla:saved` with the trigger reason and coordinates

#### Scenario: Sidecar records timestamp provenance
- **WHEN** an event's timestamp came from its folder name because `event.json` was unreadable
- **THEN** `import.json` records `time_source` as `folder_name`

#### Scenario: City is the only Tesla-block field
- **WHEN** an event whose `event.json` has `city: Vilnius` is imported
- **THEN** `import.json`'s `tesla` block is `{ "city": "Vilnius" }` and the coordinates, reason, and timestamp appear only in the envelope or the `events` entry
