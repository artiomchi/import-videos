## MODIFIED Requirements

### Requirement: Session timestamp prefers GPS-corrected time
A session's timestamp SHALL be the GPS-corrected UTC time (the first chapter's `moov/mvhd` creation time plus the session clock offset from `gopro-telemetry`) when telemetry is available, and SHALL drive `{date:...}` layout fields for the session's destination path. When no telemetry is available for the session, the timestamp SHALL be the first chapter's `mvhd` creation time interpreted as a camera wall clock **in the configured `timezone`** — the camera records local time, so its civil reading is resolved through the configured zone to a real instant (rather than being taken as UTC). If `mvhd` cannot be read, the system SHALL fall back to the file's modification time and log a warning; the import MUST still proceed. In all cases the resulting instant is rendered in the configured `timezone` for `{date:...}` fields.

#### Scenario: Layout date from GPS-corrected time
- **WHEN** a kept session's camera clock reads 2026-07-10T00:20 but GPS correction places the recording at 2026-07-09T23:20Z, `timezone` is `UTC`, with layout `{date:%Y}/{date:%Y-%m-%d}`
- **THEN** the session's resolved destination ends in `2026/2026-07-09`

#### Scenario: Layout date from camera clock without telemetry
- **WHEN** a kept session's first chapter has an `mvhd` civil time of 2026-07-09T18-40 and no usable telemetry, `timezone` is `Europe/Vilnius`, and the layout is `{date:%Y}/{date:%Y-%m-%d}_{date:%H-%M}`
- **THEN** the session's resolved destination ends in `2026/2026-07-09_18-40`, the camera's own wall reading, not a value shifted by the zone offset

#### Scenario: Missing mvhd falls back to mtime
- **WHEN** a chapter's `mvhd` creation time cannot be read
- **THEN** the session timestamp is the file's modification time, a warning is logged, and the scan completes

### Requirement: Markers sidecar planned and written with the import
For every `Keep` session the plan SHALL include the unified `import.json` sidecar (see the `unified-sidecar` capability) to be written in the session's destination directory, and the plan output SHALL show it before execution. For GoPro the sidecar SHALL set `camera` to the camera model, `recorded_at` to the session instant, and `time_source` to `"gps"` when the session has a clock offset or `"camera"` otherwise. The session number and, when `time_source` is `"gps"`, the `clock_offset_s` (fractional seconds) SHALL be stored in the `gopro` device block. Each HiLight marker SHALL appear as an `events` entry with `type` `gopro:marker`, its millisecond `offset_ms`, and its `time`: with `"gps"` the corrected time and `lat`/`lon` when the marker has coordinates (omitted otherwise); with `"camera"` the camera-clock time and no coordinates. The transfer engine SHALL write the sidecar only after all of the session's files transferred and verified; a sidecar write failure SHALL mark the session failed, preventing source deletion for it. Scanning MUST NOT write the sidecar. No `markers.json` sidecar SHALL be produced.

#### Scenario: GPS sidecar written next to imported session
- **WHEN** a session with one marker at offset 734120 ms imports successfully with a usable GPS fix and a clock offset of −12.4 s
- **THEN** the destination's `import.json` records `"time_source": "gps"`, a `gopro` block with `clock_offset_s` −12.4 and the session number, and an `events` entry with `type` `gopro:marker`, `offset_ms` 734120, its corrected `time`, and `lat`/`lon`

#### Scenario: Camera sidecar without telemetry
- **WHEN** a session with two chapters and one marker at offset 734120 ms imports successfully and no telemetry is available
- **THEN** the destination's `import.json` lists both chapter files, `"time_source": "camera"`, and one `events` entry with `type` `gopro:marker`, `offset_ms` 734120, and its camera-clock `time`

#### Scenario: Marker without a fix omits coordinates
- **WHEN** a session imports with `"time_source": "gps"` but one marker found no usable fix nearby
- **THEN** that marker's `events` entry has a corrected `time` and no `lat`/`lon` fields

#### Scenario: No markers.json is written
- **WHEN** any GoPro session is imported
- **THEN** the destination directory contains `import.json` and no `markers.json`
