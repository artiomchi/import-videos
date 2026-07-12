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

For each kept file whose final destination name is unoccupied, the system SHALL read the source exactly once, hashing it (blake3) while stream-copying to a temporary name (`<final>.part`) in the destination directory. The system SHALL then re-read and hash the written temporary file, and only when the read-back hash matches the source stream hash rename it to the final name — so a copy corrupted or truncated in the write path fails verification rather than being finalized. On mismatch or copy failure the temporary file MUST be removed and the source file left untouched.

When the final destination name is already occupied, the system MAY hash the source in a separate pass before copying, in order to resolve the collision without a copy (see "Collisions never overwrite existing footage"); a copy that follows an unresolved collision SHALL use the same single-pass stream-copy and read-back verification at the suffixed name.

#### Scenario: Successful verified copy
- **WHEN** a file is transferred and the written temporary file's read-back hash matches the source stream hash
- **THEN** the file exists under its final destination name and no `.part` file remains

#### Scenario: Verification failure preserves the source
- **WHEN** the written temporary file's read-back hash differs from the source stream hash
- **THEN** the temporary file is deleted, the source file remains, and the action is reported as failed

#### Scenario: Source is read once when no collision exists
- **WHEN** a kept file's final destination name is unoccupied
- **THEN** the source file is read exactly once during its transfer

#### Scenario: Write-path corruption is detected
- **WHEN** the bytes persisted in the temporary file differ from the bytes streamed from the source
- **THEN** verification fails, the temporary file is removed, and the source file is untouched

### Requirement: Source deletion only after verification
When the effective `delete_source` is `true` — the profile's value unless overridden for the run by `--delete-source` or `--no-delete-source` — the system SHALL delete a source file only after its verified transfer completed or the file was confirmed already-imported by content hashing. A file accepted only by `--quick-match` — matched on name, size, and modification time without hashing its contents — SHALL NOT be a source-deletion candidate: trading verification for speed forfeits the right to delete the source. Files whose transfer failed MUST remain on the source. `--delete-source` SHALL force deletion on for the run even when the profile sets `delete_source: false`; forcing it on SHALL NOT bypass the confirmation requirement. `--no-delete-source` SHALL force deletion off for the run; `--keep-source` SHALL be accepted as an undocumented alias of `--no-delete-source`.

#### Scenario: Clean card after successful import
- **WHEN** all of a group's files transfer and verify successfully with `delete_source: true`
- **THEN** those files no longer exist on the source

#### Scenario: Failed transfer keeps source file
- **WHEN** one file's transfer fails verification while others succeed
- **THEN** the failed file remains on the source and the run exits with code 1

#### Scenario: Quick-matched files are never deleted
- **WHEN** `import --quick-match` runs with `delete_source: true` and confirmation given, over a group whose files are all quick-matched
- **THEN** those source files are left in place, because no content was verified for them

#### Scenario: CLI forces deletion on against a safe profile
- **WHEN** `import --delete-source --yes` runs on a profile with `delete_source: false` and all files transfer and verify
- **THEN** the source files are deleted, exactly as if the profile had set `delete_source: true`

#### Scenario: Forced deletion still prompts
- **WHEN** `import --delete-source` runs without `--yes` and stdin is not a terminal
- **THEN** files are transferred but source deletion is skipped with an explanatory message

#### Scenario: keep-source alias still works
- **WHEN** `import --keep-source` runs on a profile with `delete_source: true`
- **THEN** no source file is deleted, identically to `--no-delete-source`

### Requirement: Quarantine copying can be disabled per profile
When the effective `copy_quarantine` is `false` — the profile's value unless overridden for the run by `--copy-quarantine`, `--no-copy-quarantine`, or `--quarantine` (which forces it on) — the plan SHALL resolve no quarantine path for `Quarantine` groups, and `import` SHALL transfer nothing for those groups, leaving their source files exactly where they are. The group's verdict SHALL remain `Quarantine` in `scan`, `--dry-run`, and `import` output, distinguished by a note that quarantine copying is disabled in place of a resolved path. Because such a group's files are never transferred or verified, they MUST NOT become source-deletion candidates: even with an effective `delete_source: true` (and confirmation), an un-copied quarantined source file SHALL be left in place. When the effective `copy_quarantine` is `true`, quarantine behavior SHALL be unchanged — `Quarantine` groups are copied to their resolved quarantine path via the same verified transfer as `Keep` groups.

#### Scenario: Disabled quarantine copy leaves source untouched
- **WHEN** `import` runs a profile with `copy_quarantine: false` over a source containing an unmarked (Quarantine) group
- **THEN** no quarantine directory is created, the group's source files remain byte-for-byte in place, and the plan shows the group as Quarantine with quarantine copying disabled

#### Scenario: Disabled quarantine copy never deletes the source
- **WHEN** the same run also sets `delete_source: true` and confirmation is given
- **THEN** the quarantined group's source files are still not deleted, while eligible Keep groups are cleaned as usual

#### Scenario: Enabled quarantine copy is unchanged
- **WHEN** `import` runs a profile with `copy_quarantine: true` (or omitted) over an unmarked (Quarantine) group
- **THEN** the group's files are copied to the resolved quarantine path via verified transfer, exactly as before

#### Scenario: CLI disables quarantine copying for one run
- **WHEN** `import --no-copy-quarantine` runs on a profile that omits `copy_quarantine`, over an unmarked (Quarantine) group
- **THEN** the group's source files stay in place and the plan notes quarantine copying is disabled, while the profile in the config file is untouched

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

### Requirement: Per-invocation profile overrides
The system SHALL accept CLI flags that override individual profile settings for a single run, without modifying the config file. Boolean settings SHALL be overridable in both directions via paired flags named after the config field — `--copy-quarantine` / `--no-copy-quarantine` on `scan` and `import`, and `--delete-source` / `--no-delete-source` on `import` — where passing neither flag uses the profile's value and repeating flags from a pair is resolved last-one-wins. `--quarantine PATH` on `scan` and `import` SHALL override the profile's quarantine directory for the run, resolving a relative path against the effective destination (the same rule as the config field); setting it SHALL also force `copy_quarantine` on for the run. Combining `--quarantine` with `--no-copy-quarantine` MUST be rejected as a usage error (exit 2). Overrides SHALL be applied when the profile is resolved, before planning, so `scan`, `--dry-run`, and `import` all reflect them identically.

#### Scenario: Unset flags use the profile value
- **WHEN** `import` runs without any override flags on a profile with `delete_source: false` and `copy_quarantine: true`
- **THEN** behavior is identical to a run before this change: no source deletion, quarantine groups copied

#### Scenario: Override forces a boolean off
- **WHEN** `import --no-copy-quarantine` runs on a profile with `copy_quarantine: true` (or omitted)
- **THEN** the run behaves exactly as if the profile had set `copy_quarantine: false`

#### Scenario: Override forces a boolean on
- **WHEN** `import --copy-quarantine` runs on a profile with `copy_quarantine: false`
- **THEN** the run behaves exactly as if the profile had set `copy_quarantine: true`

#### Scenario: Last flag of a pair wins
- **WHEN** `import --no-copy-quarantine --copy-quarantine` runs
- **THEN** the effective value is `copy_quarantine: true` and no error is raised

#### Scenario: Quarantine path override implies copying
- **WHEN** `import --quarantine /tmp/q` runs on a profile with `copy_quarantine: false`
- **THEN** quarantine groups are copied via verified transfer into `/tmp/q`

#### Scenario: Contradictory quarantine flags rejected
- **WHEN** `import --quarantine /tmp/q --no-copy-quarantine` is invoked
- **THEN** the invocation fails as a usage error with exit code 2 before any scanning occurs

#### Scenario: Scan previews overrides
- **WHEN** `scan --quarantine /tmp/q` runs against a source with an unmarked group
- **THEN** the printed plan shows the group's quarantine path under `/tmp/q`, and nothing on disk changes

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

### Requirement: Quick match skips content hashing when opted in
The `import` command SHALL accept a `--quick-match` flag. When it is set, before hashing a kept file the system SHALL compare that file against its resolved final destination path; if the destination path exists, its byte size equals the source file's size, and its modification time equals the recording time this run would stamp on it, the file SHALL be accepted as already-imported without reading or hashing either file's contents. The modification-time comparison SHALL allow a tolerance of 0.1 second so that filesystems which truncate sub-second timestamps still match. A quick-matched file SHALL be counted and reported as skipped, reported distinctly from a blake3-verified already-imported skip. Any quick-match miss — no destination file, or a differing size or modification time — SHALL fall through to the normal verified transfer, including the existing collision handling. When `--quick-match` is absent, transfer and verification behavior SHALL be unchanged.

#### Scenario: Quick match skips hashing
- **WHEN** `import --quick-match` runs and a kept file's destination exists with matching name, size, and modification time
- **THEN** the file is reported as skipped without either file's contents being hashed

#### Scenario: Sub-second truncation still matches
- **WHEN** the destination's stored modification time differs from the recording time by less than 0.1 second because the filesystem truncated it
- **THEN** the file is still treated as a quick match

#### Scenario: Size mismatch falls through to verified transfer
- **WHEN** `import --quick-match` runs and the destination exists but its size differs from the source
- **THEN** the file is not quick-matched and the normal verified transfer (with collision handling) runs

#### Scenario: No quick match without the flag
- **WHEN** `import` runs without `--quick-match` over a source whose files already exist at the destination
- **THEN** each file is verified by hashing exactly as before

### Requirement: Machine-readable JSON report output

A global `--json` flag SHALL make every subcommand emit its result as a single
JSON document on stdout: `scan` and `import --dry-run` emit the plan, `import`
emits the execution report. In JSON mode no other stdout output SHALL be
produced (informational lines and progress are suppressed); errors still go to
stderr and exit codes are unchanged. Timestamps SHALL be RFC 3339 strings
rendered in the configured timezone; paths SHALL be strings. The JSON shape is
produced by dedicated view-model types, not by serializing internal domain types
directly.

#### Scenario: Scan emits a JSON plan

- **WHEN** `scan gopro --json` runs against a card with keep, quarantine, and
  ignore verdicts
- **THEN** stdout parses as one JSON document containing every planned action
  (including quarantined entries, which the human output hides by default) with
  verdict, group name, resolved paths, and summary counts

#### Scenario: Import emits a JSON execution report

- **WHEN** `import gopro --json --yes` executes a plan
- **THEN** stdout parses as one JSON document with per-file transfer outcomes,
  sidecar outcomes, source-deletion results, and any deletion-skipped reason

#### Scenario: JSON mode does not bypass confirmation

- **WHEN** `import` runs with `--json` but without `--yes` where source deletion
  requires confirmation
- **THEN** the confirmation rules apply exactly as without `--json`

#### Scenario: No sources found in JSON mode

- **WHEN** `scan gopro --json` finds no sources
- **THEN** stdout is a JSON document stating that (not a bare human string), and
  the exit code is 0

### Requirement: Transfer progress is shown on interactive terminals

During plan execution, byte-level transfer progress SHALL be displayed when the
session is an interactive terminal and `--json` is not set. Progress output
SHALL be absent when stdout is not a TTY or JSON mode is active, and SHALL NOT
interleave with the report output consumed by pipes.

#### Scenario: Interactive import shows progress

- **WHEN** `import` transfers files with stdout attached to a terminal
- **THEN** a progress indicator reflects bytes transferred during copy and
  verification, and the final report renders after it

#### Scenario: Piped output stays clean

- **WHEN** `import` runs with stdout redirected to a file or pipe (or with
  `--json`)
- **THEN** the captured output contains only the report, with no progress or
  terminal-control bytes

### Requirement: Scan progress is shown on interactive terminals

The system SHALL display a progress indicator during the scan phase of `scan` and `import` (the `ImportSource::scan` call within plan building) when the session is an interactive terminal and `--json` is not set. Progress output SHALL be absent when stdout is not a TTY or JSON mode is active, and SHALL NOT interleave with the printed plan or, for `import`, the subsequent transfer-progress indicator.

#### Scenario: Interactive scan shows progress

- **WHEN** `scan` runs against a source with media, with stdout attached to a terminal
- **THEN** a progress indicator reflects scan progress before the plan is printed, and clears before the plan appears

#### Scenario: Interactive import shows both phases in sequence

- **WHEN** `import` runs with stdout attached to a terminal
- **THEN** the scan-phase progress indicator appears and clears, followed (after any plan output) by the transfer-phase progress indicator; the two indicators never appear at the same time

#### Scenario: Piped or JSON output stays clean

- **WHEN** `scan` or `import` runs with stdout redirected to a file or pipe, or with `--json`
- **THEN** the captured output contains no progress or terminal-control bytes from the scan phase

