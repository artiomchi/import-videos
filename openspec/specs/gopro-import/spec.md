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

### Requirement: Session timestamp from camera clock
A session's timestamp SHALL come from the first chapter's `moov/mvhd` creation time (interpreted as the camera clock), and SHALL drive `{date:...}` layout fields for the session's destination path. If `mvhd` cannot be read, the system SHALL fall back to the file's modification time and log a warning; the import MUST still proceed.

#### Scenario: Layout date from mvhd creation time
- **WHEN** a kept session's first chapter has an mvhd creation time of 2026-07-09 and the layout is `{date:%Y}/{date:%Y-%m-%d}`
- **THEN** the session's resolved destination ends in `2026/2026-07-09`

#### Scenario: Missing mvhd falls back to mtime
- **WHEN** a chapter's mvhd creation time cannot be read
- **THEN** the session timestamp is the file's modification time, a warning is logged, and the scan completes

### Requirement: Markers sidecar planned and written with the import
For every `Keep` session the plan SHALL include a `markers.json` sidecar to be written in the session's destination directory, and the plan output SHALL show it before execution. The sidecar SHALL record the camera model, session number, chapter file names, `"time_source": "camera"`, and each marker's chapter file, millisecond offset, and camera-clock wall time. The transfer engine SHALL write the sidecar only after all of the session's files transferred and verified; a sidecar write failure SHALL mark the session failed, preventing source deletion for it. Scanning MUST NOT write the sidecar.

#### Scenario: Sidecar written next to imported session
- **WHEN** a session with two chapters and one marker at offset 734120 ms imports successfully
- **THEN** the destination directory contains `markers.json` listing both chapter files, `"time_source": "camera"`, and one marker entry with `offset_ms` 734120 and its camera-clock wall time

#### Scenario: Sidecar failure blocks source deletion
- **WHEN** all of a session's files transfer but the sidecar write fails with `delete_source: true`
- **THEN** the session is reported failed, its source files are not deleted, and the process exits with code 1

### Requirement: Unparseable chapters degrade to unmarked
A chapter file whose MP4 structure cannot be parsed SHALL contribute zero markers, produce a logged warning, and MUST NOT abort the scan or the run. The session containing it SHALL be judged by the normal verdict rule, so a fully unparseable session with `require_marker: true` is quarantined — preserved via verified copy, never silently skipped or deleted in place.

#### Scenario: Corrupt chapter quarantines its session
- **WHEN** session `0125`'s only chapter is truncated garbage that fails MP4 parsing
- **THEN** the scan completes with a warning, the session's verdict is Quarantine, and after import the file exists under the quarantine path

