# gopro-telemetry Specification

## Purpose
Extract GPS telemetry from GoPro GPMF tracks to derive GPS-corrected session timestamps and per-marker coordinates, degrading gracefully to camera-clock behavior when telemetry is unavailable or unusable.

## Requirements
### Requirement: GPMF track discovery
The system SHALL locate a chapter's GPMF telemetry track by scanning `moov` for a `trak` whose `mdia/hdlr` handler type is `meta` and whose `mdia/minf/stbl/stsd` first entry format is `gpmd`. A file with no such track SHALL yield a clean "no telemetry" result — not an error. Track discovery MUST NOT modify the source file.

#### Scenario: gpmd track found among other tracks
- **WHEN** a chapter contains video, audio, and a `meta`-handler track whose `stsd` entry is `gpmd`
- **THEN** the `gpmd` track is selected and the other tracks are ignored

#### Scenario: File without telemetry yields no-telemetry
- **WHEN** a chapter has no track with a `gpmd` `stsd` entry
- **THEN** the result is "no telemetry" and no error is raised

### Requirement: Telemetry sample index
The system SHALL build a per-sample index of the `gpmd` track from its sample tables: sizes from `stsz`, absolute file offsets from `stsc` + `stco`/`co64` (honoring the sample-to-chunk mapping, not assuming one sample per chunk), and stream-time start/duration from `stts` interpreted with the `mdhd` timescale. Payload bytes SHALL be read on demand per sample, not bulk-loaded. Malformed sample tables SHALL produce an error without panicking.

#### Scenario: Index built from sample tables
- **WHEN** a `gpmd` track declares 3 samples across 2 chunks with a 1000-unit timescale and 1000-unit durations
- **THEN** the index holds 3 entries with correct file offsets, sizes, and stream times 0 s, 1 s, and 2 s

#### Scenario: Corrupt sample table fails cleanly
- **WHEN** `stsz` declares more samples than `stsc`/`stco` can place
- **THEN** index construction returns an error and does not panic

### Requirement: GPMF KLV parsing
The system SHALL parse GPMF payloads as KLV streams: 4-byte key, 1-byte type, 1-byte structure size, 2-byte big-endian repeat count, then values padded to 4-byte alignment; nested containers (type `0x00`) SHALL be traversed. From GPS streams it SHALL extract `GPS5` values scaled by the stream's `SCAL` divisors, `GPSU` timestamps (`yymmddhhmmss.sss`, interpreted as UTC), `GPSF` fix status, and `GPSP` precision. Malformed KLV (truncated values, lengths exceeding the payload) SHALL produce an error without panicking. Unknown keys SHALL be skipped, not rejected.

#### Scenario: GPS5 values scaled by SCAL
- **WHEN** a payload's GPS stream carries `SCAL` divisors of 10000000 for lat/lon and a `GPS5` sample with raw lat 515012340
- **THEN** the parsed latitude is 51.5012340

#### Scenario: GPSU parsed as UTC
- **WHEN** a payload carries `GPSU` value `260709074103.250`
- **THEN** the parsed timestamp is 2026-07-09T07:41:03.250Z

#### Scenario: Unknown streams skipped
- **WHEN** a payload contains accelerometer and gyro streams alongside the GPS stream
- **THEN** parsing succeeds and only GPS values are extracted

#### Scenario: Truncated payload fails without panic
- **WHEN** a payload's last KLV item declares more repeats than the remaining bytes hold
- **THEN** parsing returns an error and does not panic

### Requirement: Fix-quality gating
A telemetry payload SHALL be treated as usable if and only if its `GPSF` value is at least 2 (2D lock) and its `GPSP` value is at most 500. Unusable payloads SHALL be skipped — their coordinates and timestamps MUST NOT feed clock correction or marker positions — and skipping them is not an error.

#### Scenario: Pre-lock payloads ignored
- **WHEN** a chapter's first three payloads report `GPSF` 0 and the fourth reports `GPSF` 3 with `GPSP` 150
- **THEN** clock correction uses the fourth payload

#### Scenario: Poor precision ignored
- **WHEN** a payload reports `GPSF` 3 but `GPSP` 2000
- **THEN** the payload is not used

### Requirement: Session clock offset from first good fix
The system SHALL derive one clock offset per session: scanning chapters in chapter order, the first usable payload carrying `GPSU` yields `offset = GPSU − (chapter mvhd creation time + payload stream time)`. This single offset SHALL be applied session-wide to the session timestamp and every marker wall time. A session where no chapter yields a usable `GPSU` SHALL have no offset and fall back to camera-clock behavior.

#### Scenario: Offset corrects a drifted clock
- **WHEN** a chapter's mvhd time is 2026-07-09T08:41:15 (camera clock, one hour and 12 seconds ahead of UTC) and its first usable payload at stream time 2 s carries GPSU 2026-07-09T07:41:05Z
- **THEN** the session clock offset is −3612 s and the corrected session timestamp is 2026-07-09T07:41:03Z

#### Scenario: Offset from a later chapter
- **WHEN** chapter 1 has no usable fix but chapter 2 does
- **THEN** the offset is derived from chapter 2 and still applied to the whole session

### Requirement: Marker coordinates from nearest GPS sample
For each HiLight marker the system SHALL select the payload whose stream-time interval covers the marker's offset, then the `GPS5` sample within it nearest to the marker's position assuming uniform sample spacing across the payload's duration. If the covering payload is unusable, the system SHALL search the nearest usable payload within ±2 payloads; if none qualifies, the marker SHALL carry no coordinates while still receiving the corrected UTC time.

#### Scenario: Marker mapped to in-payload sample
- **WHEN** a marker sits at offset 1500 ms and the covering payload spans 1–2 s with 10 uniformly spaced GPS5 samples
- **THEN** the marker's coordinates come from the sample nearest the payload's midpoint

#### Scenario: No usable fix near marker
- **WHEN** every payload within ±2 of the marker's covering payload is unusable
- **THEN** the marker has no coordinates but its corrected UTC time is still recorded

### Requirement: Telemetry failures degrade to camera clock
Any telemetry failure for a session — no `gpmd` track, malformed sample tables or KLV, or no usable fix — SHALL log a warning and leave the session on camera-clock behavior. Telemetry MUST NOT influence Keep/Quarantine verdicts, and an import MUST NOT fail because of telemetry. All telemetry reads SHALL be read-only over source files.

#### Scenario: Malformed telemetry does not abort the scan
- **WHEN** a session's chapters contain a `gpmd` track with corrupt KLV
- **THEN** the scan completes with a warning and the session uses its camera-clock timestamp

#### Scenario: Verdict unaffected by telemetry
- **WHEN** a session has HiLight markers but no usable GPS fix
- **THEN** the session's verdict is Keep, exactly as without telemetry

### Requirement: GPS lookup can be disabled

A GoPro profile SHALL support disabling telemetry-based session correction entirely, via a `gps_lookup` profile field (boolean, default `true`) and a per-invocation `--gopro-gps-lookup` / `--no-gopro-gps-lookup` override on `import`. When the effective `gps_lookup` is `false`, telemetry extraction (GPMF track discovery, sample-table parsing, fix-quality gating) SHALL NOT run for that session; the session SHALL fall back to camera-clock behavior exactly as when telemetry is attempted and no chapter yields a usable fix. The `scan` command SHALL NEVER perform telemetry lookup, independent of this setting — it does not accept the override at all (`cli-core`: "Per-invocation profile overrides").

#### Scenario: Disabled lookup skips telemetry entirely
- **WHEN** `gps_lookup: false` and `import` runs over a session whose chapters carry usable GPS fixes
- **THEN** no `gpmd` track is opened for the session, and its recorded time is its camera-clock time

#### Scenario: CLI override disables lookup for one run
- **WHEN** `import --no-gopro-gps-lookup` runs on a profile with `gps_lookup: true` (or omitted)
- **THEN** telemetry is skipped for every session in the run, without editing the profile

#### Scenario: Scan never performs telemetry lookup
- **WHEN** `scan` runs against a GoPro card whose chapters carry usable GPS fixes, regardless of the profile's `gps_lookup` setting
- **THEN** no `gpmd` track is opened, and the inventory shows only camera-clock-derived times

### Requirement: Telemetry is skipped for sessions that cannot use it

Since telemetry MUST NOT influence Keep/Quarantine verdicts and a `Quarantine` group's destination does not use the session timestamp, the system SHALL decide a session's verdict from its HiLight markers before attempting telemetry, and SHALL skip telemetry entirely for a session whose verdict is `Quarantine`. This SHALL hold regardless of the `gps_lookup` setting (when GPS lookup is already disabled, there is nothing to skip).

#### Scenario: Quarantine-bound session skips telemetry
- **WHEN** `require_marker: true` and `import` runs over a session with no HiLight markers, whose chapters carry a `gpmd` track with usable fixes
- **THEN** the session's verdict is `Quarantine` and no `gpmd` track is opened for it

#### Scenario: Keep-bound session still uses telemetry
- **WHEN** `require_marker: true` and `import` runs over a session with HiLight markers and the effective `gps_lookup` is `true`
- **THEN** telemetry is attempted for the session exactly as before this change
