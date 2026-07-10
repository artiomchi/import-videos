## MODIFIED Requirements

### Requirement: Markers sidecar planned and written with the import
For every `Keep` session the plan SHALL include the unified `import.json` sidecar (see the `unified-sidecar` capability) to be written in the session's destination directory, and the plan output SHALL show it before execution. For GoPro the sidecar SHALL set `camera` to the camera model, `recorded_at` to the session instant, and `time_source` to `"gps"` when the session has a clock offset or `"camera"` otherwise. The session number and, when `time_source` is `"gps"`, the `clock_offset_s` (fractional seconds) SHALL be stored in the `gopro` device block. Each HiLight marker SHALL appear as an `events` entry with `type` `gopro:marker`, the base name of the chapter it was pressed in as `file`, its millisecond position as `offset_ms`, that same position as a human-readable `min:sec.ms` string in `offset`, and its `time`: with `"gps"` the corrected time and `lat`/`lon` when the marker has coordinates (omitted otherwise); with `"camera"` the camera-clock time and no coordinates. The `offset` string SHALL render whole minutes (never wrapped into hours), two-digit seconds, and three-digit milliseconds, e.g. `12:14.120` for 734120 ms and `0:05.000` for 5000 ms. The transfer engine SHALL write the sidecar only after all of the session's files transferred and verified; a sidecar write failure SHALL mark the session failed, preventing source deletion for it. Scanning MUST NOT write the sidecar. No `markers.json` sidecar SHALL be produced.

#### Scenario: GPS sidecar written next to imported session
- **WHEN** a session with one marker at offset 734120 ms in chapter `GX020123.MP4` imports successfully with a usable GPS fix and a clock offset of −12.4 s
- **THEN** the destination's `import.json` records `"time_source": "gps"`, a `gopro` block with `clock_offset_s` −12.4 and the session number, and an `events` entry with `type` `gopro:marker`, `file` `GX020123.MP4`, `offset_ms` 734120, `offset` `12:14.120`, its corrected `time`, and `lat`/`lon`

#### Scenario: Camera sidecar without telemetry
- **WHEN** a session with two chapters and one marker at offset 734120 ms imports successfully and no telemetry is available
- **THEN** the destination's `import.json` lists both chapter files, `"time_source": "camera"`, and one `events` entry with `type` `gopro:marker`, the marker's chapter as `file`, `offset_ms` 734120, `offset` `12:14.120`, and its camera-clock `time`

#### Scenario: Marker attributed to its own chapter
- **WHEN** a session's chapter `GX010123.MP4` holds one marker and chapter `GX020123.MP4` holds another
- **THEN** each marker's `events` entry names its own chapter in `file`

#### Scenario: Marker without a fix omits coordinates
- **WHEN** a session imports with `"time_source": "gps"` but one marker found no usable fix nearby
- **THEN** that marker's `events` entry has a corrected `time`, its `file`, `offset_ms`, and `offset`, and no `lat`/`lon` fields

#### Scenario: No markers.json is written
- **WHEN** any GoPro session is imported
- **THEN** the destination directory contains `import.json` and no `markers.json`

#### Scenario: Sidecar failure blocks source deletion
- **WHEN** all of a session's files transfer but the sidecar write fails with `delete_source: true`
- **THEN** the session is reported failed, its source files are not deleted, and the process exits with code 1
