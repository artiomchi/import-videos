# gopro-import Delta

## ADDED Requirements

### Requirement: require_marker override flags
The `scan` and `import` commands SHALL accept `--gopro-require-marker` and `--no-gopro-require-marker`, overriding the profile's `require_marker` for the run in either direction (paired-flag semantics: neither flag uses the profile value, repeats resolve last-one-wins). Passing either flag when the named profile is not of type `gopro` SHALL fail before scanning with an error naming the constraint — the same wording configuration loading uses for `require_marker` on a non-GoPro profile — and exit code 2.

#### Scenario: Marker requirement disabled for one run
- **WHEN** `scan --no-gopro-require-marker` runs a GoPro profile (with `require_marker` true or omitted) against a card with an unmarked session
- **THEN** the plan shows that session as Keep, while the profile in the config file is untouched

#### Scenario: Marker requirement forced on
- **WHEN** `import --gopro-require-marker` runs a GoPro profile with `require_marker: false` against a card with an unmarked session
- **THEN** the session's verdict is Quarantine, exactly as if the profile had set `require_marker: true`

#### Scenario: Marker flags rejected on non-GoPro profiles
- **WHEN** `import tesla --no-gopro-require-marker` runs where profile `tesla` has `type: tesla`
- **THEN** the run fails before any scanning with an error stating `require_marker` is only valid for profiles of type gopro, and exits with code 2

## MODIFIED Requirements

### Requirement: Marker-driven session verdicts
A session with at least one HiLight marker in any of its chapters SHALL receive a `Keep` verdict. A session with zero markers across all chapters SHALL receive a `Quarantine` verdict. When the effective `require_marker` is `false` — the profile's value unless overridden for the run by `--gopro-require-marker` or `--no-gopro-require-marker` — every session SHALL receive `Keep` regardless of markers, and markers SHALL still be extracted for the sidecar.

#### Scenario: Marker in a later chapter keeps the whole session
- **WHEN** session `0123` has chapters `GX010123.MP4` (no markers) and `GX020123.MP4` (one marker)
- **THEN** the session's verdict is Keep and both files are planned for the destination

#### Scenario: Unmarked session quarantined
- **WHEN** session `0124` has no markers in any chapter
- **THEN** the session's verdict is Quarantine and its files are planned for the quarantine path

#### Scenario: require_marker false keeps everything
- **WHEN** the effective `require_marker` is `false` (from the profile or a CLI override) and session `0124` has no markers
- **THEN** the session's verdict is Keep
