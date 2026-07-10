## MODIFIED Requirements

### Requirement: YAML configuration with per-device profiles
The system SHALL load configuration from `~/.config/import-videos/config.yaml` (or the path given by `--config`), containing named profiles. Each profile MUST declare a `type` selecting the device implementation, and MAY set `source` (`auto` or a path), `destination`, `layout`, `ignore` globs, `quarantine`, `copy_quarantine` (boolean, default `true`), and `delete_source`. Configuration errors — unreadable file, invalid YAML, unknown `type`, invalid glob, invalid layout template — MUST be reported at load time with the offending profile and field named, and the process SHALL exit with code 2.

#### Scenario: Valid config loads
- **WHEN** a config file defines a profile with a known `type` and valid fields
- **THEN** the profile is available to `scan` and `import` by its name

#### Scenario: Unknown profile type
- **WHEN** a profile declares `type: quadcopter` and no such device implementation exists
- **THEN** loading fails naming the profile and the unknown type, and the process exits with code 2

#### Scenario: Missing config file
- **WHEN** no config file exists at the resolved path
- **THEN** the process exits with code 2 and the error names the path it looked for

#### Scenario: Quarantine copy defaults to enabled
- **WHEN** a profile omits `copy_quarantine`
- **THEN** the config loads and the profile behaves as if `copy_quarantine: true`

#### Scenario: Quarantine copy can be disabled
- **WHEN** a profile sets `copy_quarantine: false`
- **THEN** the config loads and the profile is marked to skip quarantine copying

## ADDED Requirements

### Requirement: Quarantine copying can be disabled per profile
When a profile sets `copy_quarantine: false`, the plan SHALL resolve no quarantine path for `Quarantine` groups, and `import` SHALL transfer nothing for those groups, leaving their source files exactly where they are. The group's verdict SHALL remain `Quarantine` in `scan`, `--dry-run`, and `import` output, distinguished by a note that quarantine copying is disabled in place of a resolved path. Because such a group's files are never transferred or verified, they MUST NOT become source-deletion candidates: even with `delete_source: true` (and confirmation), an un-copied quarantined source file SHALL be left in place. When `copy_quarantine` is `true` or omitted, quarantine behavior SHALL be unchanged — `Quarantine` groups are copied to their resolved quarantine path via the same verified transfer as `Keep` groups.

#### Scenario: Disabled quarantine copy leaves source untouched
- **WHEN** `import` runs a profile with `copy_quarantine: false` over a source containing an unmarked (Quarantine) group
- **THEN** no quarantine directory is created, the group's source files remain byte-for-byte in place, and the plan shows the group as Quarantine with quarantine copying disabled

#### Scenario: Disabled quarantine copy never deletes the source
- **WHEN** the same run also sets `delete_source: true` and confirmation is given
- **THEN** the quarantined group's source files are still not deleted, while eligible Keep groups are cleaned as usual

#### Scenario: Enabled quarantine copy is unchanged
- **WHEN** `import` runs a profile with `copy_quarantine: true` (or omitted) over an unmarked (Quarantine) group
- **THEN** the group's files are copied to the resolved quarantine path via verified transfer, exactly as before
