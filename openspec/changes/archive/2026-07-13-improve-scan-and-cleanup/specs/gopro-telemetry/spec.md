## ADDED Requirements

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
