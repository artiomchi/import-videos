# gopro-import — Delta Specification

## RENAMED Requirements

- FROM: `### Requirement: Session timestamp from camera clock`
- TO: `### Requirement: Session timestamp prefers GPS-corrected time`

## MODIFIED Requirements

### Requirement: Session timestamp prefers GPS-corrected time
A session's timestamp SHALL be the GPS-corrected UTC time (the first chapter's `moov/mvhd` creation time plus the session clock offset from `gopro-telemetry`) when telemetry is available, and SHALL drive `{date:...}` layout fields for the session's destination path. When no telemetry is available for the session, the timestamp SHALL be the first chapter's `mvhd` creation time interpreted as the camera clock. If `mvhd` cannot be read, the system SHALL fall back to the file's modification time and log a warning; the import MUST still proceed.

#### Scenario: Layout date from GPS-corrected time
- **WHEN** a kept session's camera clock reads 2026-07-10T00:20 but GPS correction places the recording at 2026-07-09T23:20Z, with layout `{date:%Y}/{date:%Y-%m-%d}`
- **THEN** the session's resolved destination ends in `2026/2026-07-09`

#### Scenario: Layout date from mvhd creation time without telemetry
- **WHEN** a kept session's first chapter has an mvhd creation time of 2026-07-09 and no usable telemetry, and the layout is `{date:%Y}/{date:%Y-%m-%d}`
- **THEN** the session's resolved destination ends in `2026/2026-07-09`

#### Scenario: Missing mvhd falls back to mtime
- **WHEN** a chapter's mvhd creation time cannot be read
- **THEN** the session timestamp is the file's modification time, a warning is logged, and the scan completes

### Requirement: Markers sidecar planned and written with the import
For every `Keep` session the plan SHALL include a `markers.json` sidecar to be written in the session's destination directory, and the plan output SHALL show it before execution. The sidecar SHALL record the camera model, session number, chapter file names, and a `time_source` of `"gps"` when the session has a clock offset or `"camera"` otherwise. With `"time_source": "gps"` the sidecar SHALL record the session's `clock_offset_s` (fractional seconds), and each marker entry SHALL carry its chapter file, millisecond offset, corrected `utc` time, and `lat`/`lon` when the marker has coordinates (omitted otherwise). With `"time_source": "camera"` each marker entry SHALL carry its chapter file, millisecond offset, and camera-clock wall time. The transfer engine SHALL write the sidecar only after all of the session's files transferred and verified; a sidecar write failure SHALL mark the session failed, preventing source deletion for it. Scanning MUST NOT write the sidecar.

#### Scenario: GPS sidecar written next to imported session
- **WHEN** a session with one marker at offset 734120 ms imports successfully with a usable GPS fix and a clock offset of −12.4 s
- **THEN** the destination's `markers.json` records `"time_source": "gps"`, `"clock_offset_s": -12.4`, and a marker entry with `offset_ms` 734120, its corrected `utc` time, and `lat`/`lon`

#### Scenario: Camera sidecar unchanged without telemetry
- **WHEN** a session with two chapters and one marker at offset 734120 ms imports successfully and no telemetry is available
- **THEN** the destination's `markers.json` lists both chapter files, `"time_source": "camera"`, and one marker entry with `offset_ms` 734120 and its camera-clock wall time

#### Scenario: Marker without a fix omits coordinates
- **WHEN** a session imports with `"time_source": "gps"` but one marker found no usable fix nearby
- **THEN** that marker entry has a corrected `utc` time and no `lat`/`lon` fields

#### Scenario: Sidecar failure blocks source deletion
- **WHEN** all of a session's files transfer but the sidecar write fails with `delete_source: true`
- **THEN** the session is reported failed, its source files are not deleted, and the process exits with code 1

## ADDED Requirements

### Requirement: Imported files carry the recorded time as mtime
After a file's verified copy is renamed into place, the transfer engine SHALL set the destination file's modification time to the file's recorded time when the media group provides one (GPS-corrected when telemetry is available, camera-clock otherwise). This applies to destination and quarantine transfers alike. File content MUST remain byte-identical to the source — mtime is the only metadata touched, and only after checksum verification. A failure to set mtime SHALL log a warning and MUST NOT fail the transfer or block source deletion. Files skipped as already-imported (identical content at the destination) SHALL NOT have their mtime altered.

#### Scenario: Destination mtime matches corrected recording time
- **WHEN** a chapter with a GPS-corrected recording time of 2026-07-09T07:41:03Z transfers and verifies successfully
- **THEN** the destination file's mtime is 2026-07-09T07:41:03Z and its content hash equals the source's

#### Scenario: mtime failure does not fail the import
- **WHEN** the destination filesystem rejects setting the modification time
- **THEN** a warning is logged, the transfer is reported successful, and source deletion proceeds normally

#### Scenario: Already-imported file left untouched
- **WHEN** a transfer is skipped because identical content already exists at the destination
- **THEN** the existing file's mtime is not modified
