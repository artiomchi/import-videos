## ADDED Requirements

### Requirement: Tesla profile type
The configuration system SHALL accept `type: tesla` in a profile. The profile MAY set `events` (a list drawn from `saved`, `sentry`, `recent`; default `[saved, sentry]`) and MAY set `reasons` containing exactly one of `allow` or `deny` (each a list of trigger-reason strings). A `reasons` block with both `allow` and `deny`, or with neither, SHALL fail configuration loading with an error naming the profile. Tesla-specific fields SHALL NOT be accepted on profiles of other types.

#### Scenario: Tesla profile loads with defaults
- **WHEN** a config defines a profile with `type: tesla` and no `events` or `reasons` fields
- **THEN** the config loads and the profile behaves as if `events: [saved, sentry]` with no reason filtering

#### Scenario: reasons allow and deny are mutually exclusive
- **WHEN** a tesla profile sets both `reasons.allow` and `reasons.deny`
- **THEN** configuration loading fails naming the profile, and the process exits with code 2

#### Scenario: Tesla fields rejected on other types
- **WHEN** a profile with `type: gopro` sets `events` or `reasons`
- **THEN** configuration loading fails

### Requirement: TeslaCam drive detection
The Tesla device implementation's `detect()` SHALL return true if and only if the candidate root contains a `TeslaCam/` directory that itself contains at least one of `SavedClips/`, `SentryClips/`, or `RecentClips/`. Detection MUST NOT modify anything under the candidate root.

#### Scenario: TeslaCam drive detected
- **WHEN** `detect()` runs against a root containing `TeslaCam/SavedClips/`
- **THEN** it returns true

#### Scenario: Bare TeslaCam directory rejected
- **WHEN** `detect()` runs against a root whose `TeslaCam/` contains no clips directories, or against a root with no `TeslaCam/` at all
- **THEN** it returns false

### Requirement: One media group per event folder
Scanning SHALL treat each immediate subdirectory of `TeslaCam/SavedClips/` and `TeslaCam/SentryClips/` as one event, producing one media group containing every file in that folder — camera-angle clips, `event.json`, `thumb.png`, and any unrecognized files — except files matching the profile's `ignore` globs. The event folder is the atomic unit of import: verdicts apply to whole events, never to individual files. Files directly under `TeslaCam/` or its clips directories that belong to no event folder SHALL be reported with an `Ignore` verdict and a reason, and MUST NOT be transferred or deleted.

#### Scenario: Event folder imports as one unit
- **WHEN** `SavedClips/2026-07-04_18-23-51/` contains `event.json`, `thumb.png`, and eight per-angle clips
- **THEN** the scan produces one group containing all ten files

#### Scenario: Unknown file inside an event folder travels with the event
- **WHEN** an event folder contains an unrecognized file `notes.txt` alongside its clips
- **THEN** `notes.txt` is included in that event's group and imported with it

#### Scenario: Stray file outside event folders surfaced but untouched
- **WHEN** `TeslaCam/SavedClips/` directly contains a file `stray.mp4` not inside any event folder
- **THEN** the plan lists it with an Ignore verdict and a reason, and import leaves it in place

### Requirement: Event category filtering
Scanning SHALL evaluate every discovered event against the profile's `events` list. An event whose category (`saved`, `sentry`, `recent`) is not listed SHALL receive an `Ignore` verdict naming the disabled category, and MUST NOT be transferred or deleted. Filtered events SHALL appear in scan output; they MUST NOT be silently omitted.

#### Scenario: Disabled category ignored visibly
- **WHEN** the profile sets `events: [saved]` and the card contains a SentryClips event
- **THEN** the scan lists the sentry event with an Ignore verdict naming the disabled category, and import does not touch it

### Requirement: Trigger-reason filtering
When the profile defines `reasons`, scanning SHALL evaluate each event's `reason` from its `event.json`: with `allow`, only events whose reason is listed SHALL be kept; with `deny`, events whose reason is listed SHALL be ignored. An event whose reason cannot be determined (missing or unparseable `event.json`, or absent `reason` field) SHALL be kept regardless of the `reasons` configuration. Reason-filtered events SHALL receive an `Ignore` verdict naming the reason.

#### Scenario: Deny list filters noisy sentry events
- **WHEN** the profile sets `reasons.deny: [sentry_aware_object_detection]` and an event's reason is `sentry_aware_object_detection`
- **THEN** the event receives an Ignore verdict naming that reason and is not imported

#### Scenario: Allow list keeps only listed reasons
- **WHEN** the profile sets `reasons.allow: [user_interaction_honk]` and an event's reason is `sentry_aware_object_detection`
- **THEN** the event receives an Ignore verdict and is not imported

#### Scenario: Unknown reason fails open
- **WHEN** the profile sets `reasons.allow: [user_interaction_honk]` and an event folder has no `event.json`
- **THEN** the event is kept

### Requirement: Tolerant event metadata parsing
Scanning SHALL parse each event's `event.json` for `timestamp`, `city`, `est_lat`, `est_lon`, and `reason`, tolerating missing or malformed fields. `est_lat`/`est_lon` SHALL be parsed from their JSON string form to coordinates; on failure the group has no geolocation. If the event timestamp cannot be read from `event.json`, it SHALL fall back to parsing the event folder name (`YYYY-MM-DD_HH-MM-SS`); if both fail, the event SHALL receive an `Ignore` verdict with a reason. Metadata parsing MUST NOT modify anything under the source root.

#### Scenario: Coordinates parsed from string fields
- **WHEN** an `event.json` contains `"est_lat": "51.5012"` and `"est_lon": "-0.1246"`
- **THEN** the group's geolocation is (51.5012, -0.1246)

#### Scenario: Corrupt event.json falls back to folder name
- **WHEN** an event folder `2026-07-04_18-23-51/` contains an unparseable `event.json`
- **THEN** the event is kept, with its timestamp taken from the folder name

#### Scenario: No timestamp anywhere
- **WHEN** an event folder has an unparseable name and no usable `event.json` timestamp
- **THEN** the event receives an Ignore verdict with a reason and is not imported

### Requirement: Wall-clock naming with system-timezone instants
Event timestamps SHALL be treated as vehicle-local civil datetimes. Each event group SHALL expose layout-context fields `event_type` (`saved`, `sentry`, or `recent`), `event_date` (`YYYY-MM-DD`), and `event_time` (`HH-MM-SS`) formatted directly from the civil value, so destination paths reproduce the vehicle's wall clock regardless of timezone or DST. The group timestamp and per-file recorded-at instants SHALL be produced by resolving civil times in the system timezone. Each clip's recorded-at SHALL come from its own filename stem; `event.json` and `thumb.png` SHALL use the event timestamp.

#### Scenario: Layout reproduces the vehicle wall clock
- **WHEN** an event with civil timestamp `2026-07-04T18:23:51` is imported with layout `{event_type}/{event_date}/{event_time}`
- **THEN** the destination directory is `saved/2026-07-04/18-23-51` independent of the importing machine's timezone

#### Scenario: Clip mtimes reflect their own start minute
- **WHEN** an event contains clips with stems `2026-07-04_18-18-32` and `2026-07-04_18-19-32`
- **THEN** each imported clip's modification time corresponds to its own stem resolved in the system timezone

### Requirement: RecentClips import is opt-in
Scanning SHALL skip `TeslaCam/RecentClips/` unless the profile's `events` list includes `recent`. When enabled, files in `RecentClips/` sharing a filename-stem timestamp (`YYYY-MM-DD_HH-MM-SS`) SHALL form one group per stem with `event_type` `recent`, wall-clock context derived from the stem, and no reason filtering applied.

#### Scenario: RecentClips skipped by default
- **WHEN** the profile uses the default `events` and `RecentClips/` contains clips
- **THEN** the scan produces no groups from `RecentClips/`

#### Scenario: RecentClips clusters by minute when enabled
- **WHEN** `events` includes `recent` and `RecentClips/` contains four angle clips with stem `2026-07-04_18-40-00` and four with stem `2026-07-04_18-41-00`
- **THEN** the scan produces two Keep groups of four files each

### Requirement: Normalized import sidecar
Each kept event group SHALL carry a sidecar named `import.json` written into the event's destination directory after all its files transfer and verify. The sidecar SHALL record the device type, `event_type`, source folder path, the parsed event metadata (recorded timestamp, city, coordinates, reason where present), the resolved wall-clock and UTC times with the timestamp's provenance (`event.json` or folder name), and the list of imported files. The sidecar SHALL NOT be named `event.json`, which is imported verbatim as an event file.

#### Scenario: Sidecar written alongside imported event
- **WHEN** a saved event with a valid `event.json` is imported
- **THEN** the destination folder contains the original `event.json` plus an `import.json` recording event type, source path, reason, coordinates, and the file list

#### Scenario: Sidecar records timestamp provenance
- **WHEN** an event's timestamp came from its folder name because `event.json` was unreadable
- **THEN** `import.json` records that the timestamp's source was the folder name

### Requirement: Tesla verdicts never quarantine
The Tesla implementation SHALL assign only `Keep` or `Ignore` verdicts. It SHALL NOT assign `Quarantine`: an excluded Tesla event is a deliberate, reversible configuration choice and its footage remains on the card.

#### Scenario: Filtered event is not quarantined
- **WHEN** an event is excluded by category or reason filtering
- **THEN** its verdict is Ignore, nothing is copied to the quarantine directory, and the source files remain on the card
