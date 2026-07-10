# cli-core Specification

## Purpose

The device-agnostic foundation of `import-videos`: the CLI surface (`scan` / `import`), YAML profile configuration, the scan → plan → execute pipeline contract (the `ImportSource` trait and plan types), and the verified-transfer engine with quarantine and collision semantics. Device-specific support (GoPro, Tesla) plugs into this core via the `ImportSource` trait and does not live here.

## Requirements

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

### Requirement: Layout templates validated at load
The system SHALL parse `layout` path templates (literals and `{field}` / `{field:%strftime}` tokens) when configuration loads. An unclosed brace, empty field name, or invalid strftime spec MUST fail configuration loading; template errors SHALL NOT surface for the first time during import execution.

#### Scenario: Valid template accepted
- **WHEN** a profile sets `layout: "{date:%Y}/{date:%Y-%m-%d}"`
- **THEN** the config loads and destination paths resolve using the group's date

#### Scenario: Malformed template rejected at load
- **WHEN** a profile sets `layout: "{date:%Y"` (unclosed brace)
- **THEN** loading fails naming the profile, the field, and the position of the error

#### Scenario: Unknown field at resolution time
- **WHEN** a plan is built for a group whose context does not define a field used in the template
- **THEN** planning fails for that profile with an error naming the missing field, and no files are transferred

### Requirement: Scan produces a reviewable plan without side effects
The `scan` command SHALL discover media via each selected profile's device implementation and print an import plan — every media group with its verdict (`Keep`, `Quarantine`, or `Ignore`), the reason, and the fully resolved destination or quarantine path. Scanning MUST NOT create, modify, or delete any file.

#### Scenario: Scan is read-only
- **WHEN** `scan` runs against a source containing media
- **THEN** the plan is printed and the source and destination filesystems are byte-for-byte unchanged

#### Scenario: No sources found
- **WHEN** `scan` runs and no configured profile detects a source
- **THEN** the command reports that no sources were found and exits with code 0

### Requirement: Import executes exactly the scanned plan
The `import` command SHALL build the same plan `scan` would and execute it. With `--dry-run` it MUST print the plan and stop, performing no filesystem changes. Executed actions SHALL match the printed plan: no file may be transferred, quarantined, or deleted that the plan did not list.

#### Scenario: Dry run changes nothing
- **WHEN** `import --dry-run` runs against a source with media
- **THEN** the plan is printed and no filesystem changes occur

#### Scenario: Execution follows the plan
- **WHEN** `import` executes a plan listing group A as Keep and group B as Quarantine
- **THEN** afterwards A's files exist under the resolved destination, B's files exist under the quarantine path, and nothing else changed

### Requirement: Verified transfer with atomic finalization
For each kept file the system SHALL stream-copy to a temporary name (`<final>.part`) in the destination directory while hashing the source (blake3), then hash the written temporary file, and only on hash match rename it to the final name. On mismatch or copy failure the temporary file MUST be removed and the source file left untouched.

#### Scenario: Successful verified copy
- **WHEN** a file is transferred and both hashes match
- **THEN** the file exists under its final destination name and no `.part` file remains

#### Scenario: Verification failure preserves the source
- **WHEN** the destination hash differs from the source hash
- **THEN** the temporary file is deleted, the source file remains, and the action is reported as failed

### Requirement: Source deletion only after verification
When a profile sets `delete_source: true` (and `--keep-source` is not passed), the system SHALL delete a source file only after its verified transfer completed (or the file was confirmed already-imported). Files whose transfer failed MUST remain on the source. `--keep-source` SHALL override the profile for that run.

#### Scenario: Clean card after successful import
- **WHEN** all of a group's files transfer and verify successfully with `delete_source: true`
- **THEN** those files no longer exist on the source

#### Scenario: Failed transfer keeps source file
- **WHEN** one file's transfer fails verification while others succeed
- **THEN** the failed file remains on the source and the run exits with code 1

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

### Requirement: Collisions never overwrite existing footage
If the final destination path exists with identical content (matching blake3), the file SHALL be treated as already imported: skipped, counted, and still eligible for source deletion. If it exists with different content, the incoming file SHALL be written under a numeric-suffixed name (`name-1.ext`, `name-2.ext`, …) and a warning reported. The system MUST NOT overwrite an existing destination file.

#### Scenario: Re-running an import is idempotent
- **WHEN** `import` runs twice over the same source with `delete_source: false`
- **THEN** the second run transfers nothing and the destination is unchanged

#### Scenario: Same name, different content
- **WHEN** an incoming file's destination exists with a different hash
- **THEN** the incoming file is stored under a suffixed name and both files exist afterwards

### Requirement: Destructive steps require confirmation
Before deleting source files, the system SHALL prompt for confirmation unless `--yes` is passed. If stdin is not a terminal and `--yes` is absent, the destructive step MUST be skipped with an explanatory message rather than assumed confirmed.

#### Scenario: Non-interactive run without --yes
- **WHEN** `import` runs with stdin not attached to a terminal and no `--yes`
- **THEN** files are transferred but source deletion is skipped, with a message explaining why

#### Scenario: Declined prompt
- **WHEN** the user answers no at the deletion prompt
- **THEN** all transfers remain in place and no source file is deleted

### Requirement: Source resolution via explicit path or mount probing
A profile with `source: <path>` SHALL use exactly that path. A profile with `source: auto` SHALL probe mounted volumes under the configured mount roots (default: `/run/media/<user>`, `/media`, `/mnt`) and select volumes accepted by the device implementation's `detect()`. `--source PATH` on the command line SHALL override the profile for that run.

#### Scenario: Explicit source overrides auto-detection
- **WHEN** `scan gopro --source /tmp/fake-card` runs
- **THEN** only `/tmp/fake-card` is scanned, and mount roots are not probed

#### Scenario: Explicit source path does not exist
- **WHEN** a run specifies a source path that does not exist
- **THEN** the run fails with an error naming the path and exits with code 1

### Requirement: Device implementations plug in via the ImportSource trait
The system SHALL define an `ImportSource` trait through which device modules provide card detection and scanning (media discovery, grouping, verdicts). The core pipeline — planning, path resolution, transfer, quarantine, reporting — MUST NOT contain device-specific logic; adding a device type SHALL require only a new trait implementation and a new profile `type`.

#### Scenario: Profiles map to registered implementations
- **WHEN** configuration contains profiles for several device types
- **THEN** each profile is served by its type's `ImportSource` implementation, with core pipeline code shared

### Requirement: Process exit codes reflect outcome
The process SHALL exit 0 on success (including "nothing to import"), 1 when any planned action failed, and 2 on configuration or usage errors.

#### Scenario: Partial failure
- **WHEN** at least one file fails to transfer during `import`
- **THEN** the process exits with code 1 and the report lists the failed action
