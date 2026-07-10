# gopro-import Specification

## Purpose
TBD - created by archiving change add-gopro-import. Update Purpose after archive.
## Requirements
### Requirement: GoPro profile type
The configuration system SHALL accept `type: gopro` in a profile. The profile MAY set `require_marker` (boolean, default `true`). `require_marker` SHALL NOT be accepted on profiles of other types.

#### Scenario: GoPro profile loads
- **WHEN** a config defines a profile with `type: gopro` and no `require_marker` field
- **THEN** the config loads and the profile behaves as if `require_marker: true`

#### Scenario: require_marker rejected on other types
- **WHEN** a profile with a non-gopro `type` sets `require_marker`
- **THEN** configuration loading fails naming the profile, and the process exits with code 2

### Requirement: GoPro card detection
The GoPro device implementation's `detect()` SHALL return true if and only if the candidate root contains a `DCIM/` directory with at least one subdirectory matching `1*GOPRO` that contains at least one chapter-pattern file (`G[XH]ccnnnn.MP4`, case-insensitive extension). Detection MUST NOT modify anything under the candidate root.

#### Scenario: HERO8 card layout detected
- **WHEN** `detect()` runs against a root containing `DCIM/100GOPRO/GX010123.MP4`
- **THEN** it returns true

#### Scenario: Non-GoPro storage rejected
- **WHEN** `detect()` runs against a root containing `TeslaCam/` or an empty `DCIM/`
- **THEN** it returns false

### Requirement: Chapter files group into sessions
Scanning SHALL treat every file matching `G[XH]ccnnnn.MP4` under `DCIM/1*GOPRO/` directories as a chapter, where `cc` is the chapter number and `nnnn` the session number. All chapters sharing a session number — including across multiple `1*GOPRO` directories — SHALL form one media group, with chapters ordered by chapter number. The group SHALL expose the session number as the `session` layout-context field. The session is the atomic unit of import: verdicts apply to whole sessions, never to individual chapters.

#### Scenario: Multi-chapter session groups as one unit
- **WHEN** a card contains `GX010123.MP4` and `GX020123.MP4`
- **THEN** the scan produces one group containing both files, in chapter order

#### Scenario: Distinct sessions stay separate
- **WHEN** a card contains chapters for sessions `0123` and `0124`
- **THEN** the scan produces two groups, one per session

#### Scenario: Session split across DCIM subdirectories
- **WHEN** chapters `GX010200.MP4` (in `100GOPRO/`) and `GX020200.MP4` (in `101GOPRO/`) share session `0200`
- **THEN** both files belong to the same group

### Requirement: Ignored and unrecognized files
Files matching the profile's `ignore` globs SHALL be excluded from scanning entirely — never opened, parsed, transferred, or deleted. Files under `DCIM/1*GOPRO/` that match neither the chapter pattern nor an ignore glob SHALL be reported in the plan with an `Ignore` verdict and a reason, and MUST NOT be transferred or deleted.

#### Scenario: LRV and THM files skipped via globs
- **WHEN** a profile ignores `*.LRV` and `*.THM` and the card contains `GX010123.MP4`, `GL010123.LRV`, `GX010123.THM`
- **THEN** the resulting plan mentions only `GX010123.MP4` and the ignored files are untouched by import

#### Scenario: Unrecognized file surfaced but untouched
- **WHEN** the card contains `GOPR0042.JPG` and no ignore glob matches it
- **THEN** the plan lists it with an Ignore verdict and a reason, and import leaves it in place

### Requirement: HiLight marker extraction from HMMT
The system SHALL extract HiLight markers from each chapter's `moov/udta/HMMT` box, parsed as a big-endian u32 count followed by that many big-endian u32 millisecond offsets. A chapter without an `HMMT` box, or with a count of zero, SHALL contribute no markers. Extraction MUST NOT modify the source file.

#### Scenario: Markers parsed from HMMT
- **WHEN** a chapter's `HMMT` payload encodes count 2 with offsets 5000 and 73000
- **THEN** the chapter contributes two markers at those millisecond offsets

#### Scenario: No HMMT box means no markers
- **WHEN** a chapter's `moov/udta` contains no `HMMT` box
- **THEN** the chapter contributes zero markers and no error is raised

### Requirement: Marker-driven session verdicts
A session with at least one HiLight marker in any of its chapters SHALL receive a `Keep` verdict. A session with zero markers across all chapters SHALL receive a `Quarantine` verdict. When the profile sets `require_marker: false`, every session SHALL receive `Keep` regardless of markers, and markers SHALL still be extracted for the sidecar.

#### Scenario: Marker in a later chapter keeps the whole session
- **WHEN** session `0123` has chapters `GX010123.MP4` (no markers) and `GX020123.MP4` (one marker)
- **THEN** the session's verdict is Keep and both files are planned for the destination

#### Scenario: Unmarked session quarantined
- **WHEN** session `0124` has no markers in any chapter
- **THEN** the session's verdict is Quarantine and its files are planned for the quarantine path

#### Scenario: require_marker false keeps everything
- **WHEN** the profile sets `require_marker: false` and session `0124` has no markers
- **THEN** the session's verdict is Keep

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

### Requirement: Unparseable chapters degrade to unmarked
A chapter file whose MP4 structure cannot be parsed SHALL contribute zero markers, produce a logged warning, and MUST NOT abort the scan or the run. The session containing it SHALL be judged by the normal verdict rule, so a fully unparseable session with `require_marker: true` is quarantined — preserved via verified copy, never silently skipped or deleted in place.

#### Scenario: Corrupt chapter quarantines its session
- **WHEN** session `0125`'s only chapter is truncated garbage that fails MP4 parsing
- **THEN** the scan completes with a warning, the session's verdict is Quarantine, and after import the file exists under the quarantine path

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

