## ADDED Requirements

### Requirement: Unified import.json sidecar schema
Every kept media group, across all device types, SHALL carry a sidecar named `import.json` written into the group's destination directory after all its files transfer and verify. Scanning MUST NOT write the sidecar. The sidecar SHALL contain a common envelope with `camera` (device model identifier), `source` (the group's source folder path), `imported_at` (the run's import time), `timezone` (the configured IANA zone name), `recorded_at` (the group's recording instant), `time_source` (the provenance of `recorded_at`), and `files` (the imported file names). All timestamp fields SHALL be rendered in the configured-timezone offset format. The sidecar MUST NOT be named `event.json`.

#### Scenario: Common envelope present for every device
- **WHEN** any device's kept group is imported
- **THEN** its `import.json` contains `camera`, `source`, `imported_at`, `timezone`, `recorded_at`, `time_source`, and `files`

#### Scenario: recorded_at recorded for every kept group
- **WHEN** a kept group is imported
- **THEN** `import.json` records the group's `recorded_at` as a configured-timezone offset timestamp

#### Scenario: Sidecar written only after verified transfer
- **WHEN** a group's files transfer and verify successfully
- **THEN** `import.json` is written afterward into the destination directory, and a `scan` of the same source writes no `import.json`

### Requirement: Namespaced event records
The sidecar SHALL carry an `events` array of per-point-in-time records. Each event SHALL have a `type` of the form `<device>:<kind>` (e.g. `gopro:marker`, `tesla:saved`, `tesla:sentry`, `tesla:recent`) and its own `time` rendered in the configured-timezone offset format, and MAY carry `lat`/`lon` and other per-event fields (such as a Tesla trigger `reason`). A group with no discrete events (e.g. a Tesla RecentClips cluster with no `event.json`) SHALL carry an empty `events` array.

#### Scenario: GoPro markers become events
- **WHEN** a GoPro session with two HiLight markers is imported
- **THEN** `import.json`'s `events` has two entries, each with `type` `gopro:marker` and its own `time`

#### Scenario: Tesla trigger becomes one event
- **WHEN** a Tesla saved event with reason `user_interaction_honk` is imported
- **THEN** `import.json`'s `events` has one entry with `type` `tesla:saved`, its `time`, and `reason` `user_interaction_honk`

#### Scenario: Group without discrete events has an empty array
- **WHEN** a Tesla RecentClips cluster with no `event.json` is imported
- **THEN** `import.json`'s `events` array is empty

### Requirement: Device-namespaced block for non-common data
Data that has neither a common-envelope home nor a per-event home SHALL be stored in a single device-namespaced block keyed by the device (`gopro` or `tesla`). This block MUST NOT duplicate any value already present in the envelope or in `events`; in particular, a Tesla group MUST NOT embed a copy of its `event.json` fields that already appear elsewhere, since the raw `event.json` itself travels to the destination unchanged.

#### Scenario: GoPro device block holds session-only fields
- **WHEN** a GoPro session with a GPS clock offset is imported
- **THEN** `import.json` has a `gopro` block containing `session` and `clock_offset_s`, and no `tesla` block

#### Scenario: Tesla device block holds only city
- **WHEN** a Tesla event whose `event.json` has `city: Vilnius` is imported
- **THEN** `import.json` has a `tesla` block equal to `{ "city": "Vilnius" }`, with no nested copy of the event's timestamp, reason, or coordinates

#### Scenario: Raw event.json is not duplicated into the sidecar
- **WHEN** a Tesla event with an `event.json` is imported
- **THEN** the original `event.json` exists in the destination folder and `import.json` does not embed a verbatim copy of it
