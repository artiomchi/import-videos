## MODIFIED Requirements

### Requirement: Namespaced event records
The sidecar SHALL carry an `events` array of per-point-in-time records. Each event SHALL have a `type` of the form `<device>:<kind>` (e.g. `gopro:marker`, `tesla:saved`, `tesla:sentry`, `tesla:recent`) and its own `time` rendered in the configured-timezone offset format, and MAY carry `lat`/`lon` and other per-event fields (such as a Tesla trigger `reason`). An event MAY carry a `file` field naming the base name of the single source file it originated from; it SHALL be present when exactly one of the group's files owns the event (e.g. the chapter a GoPro marker was pressed in) and SHALL be omitted when no single file applies (e.g. a Tesla trigger whose clips are all synchronized around one moment). Where an event has a positional offset from the start of its recording, that offset SHALL be recorded both as `offset_ms` (integer milliseconds) and as `offset`, the same position rendered as a human-readable `min:sec.ms` string. A group with no discrete events (e.g. a Tesla RecentClips cluster with no `event.json`) SHALL carry an empty `events` array.

#### Scenario: GoPro markers become events
- **WHEN** a GoPro session with two HiLight markers is imported
- **THEN** `import.json`'s `events` has two entries, each with `type` `gopro:marker`, its own `time`, and the `file` it came from

#### Scenario: Tesla trigger becomes one event
- **WHEN** a Tesla saved event with reason `user_interaction_honk` is imported
- **THEN** `import.json`'s `events` has one entry with `type` `tesla:saved`, its `time`, and `reason` `user_interaction_honk`, and no `file` field

#### Scenario: Positional offset recorded in both forms
- **WHEN** an event has a positional offset of 734120 ms from the start of its recording
- **THEN** its entry carries `offset_ms` 734120 and `offset` `12:14.120`

#### Scenario: Group without discrete events has an empty array
- **WHEN** a Tesla RecentClips cluster with no `event.json` is imported
- **THEN** `import.json`'s `events` array is empty
