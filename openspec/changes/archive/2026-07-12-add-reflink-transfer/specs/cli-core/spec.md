## MODIFIED Requirements

### Requirement: YAML configuration with per-device profiles
The system SHALL load configuration from `~/.config/import-videos/config.yaml` (or the path given by `--config`), containing named profiles. Each profile MUST declare a `type` selecting the device implementation, and MAY set `source` (`auto` or a path), `destination`, `layout`, `ignore` globs, `quarantine`, `copy_quarantine` (boolean, default `true`), `reflink` (boolean, default `true`), and `delete_source`. Configuration errors — unreadable file, invalid YAML, unknown `type`, invalid glob, invalid layout template — MUST be reported at load time with the offending profile and field named, and the process SHALL exit with code 2.

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

#### Scenario: Reflink defaults to enabled
- **WHEN** a profile omits `reflink`
- **THEN** the config loads and the profile behaves as if `reflink: true`

#### Scenario: Reflink can be disabled
- **WHEN** a profile sets `reflink: false`
- **THEN** the config loads and the profile is marked to never attempt a reflink clone

### Requirement: Verified transfer with atomic finalization

For each kept file whose final destination name is unoccupied, the system SHALL read the source exactly once, hashing it (blake3) while stream-copying to a temporary name (`<final>.part`) in the destination directory. The system SHALL then re-read and hash the written temporary file, and only when the read-back hash matches the source stream hash rename it to the final name — so a copy corrupted or truncated in the write path fails verification rather than being finalized. On mismatch or copy failure the temporary file MUST be removed and the source file left untouched.

When the final destination name is already occupied, the system MAY hash the source in a separate pass before copying, in order to resolve the collision without a copy (see "Collisions never overwrite existing footage"); a copy that follows an unresolved collision SHALL use the same single-pass stream-copy and read-back verification at the suffixed name.

When the effective `reflink` is `true`, the system SHALL first attempt a copy-on-write clone (reflink) of the source into the `<final>.part` temporary before falling back to stream-copying. A successful clone SHALL be finalized by the same atomic rename, and — because a copy-on-write clone shares the source's exact extents and is byte-identical by construction — the system SHALL NOT re-read or re-hash it. Any clone failure — a destination on a different filesystem, a filesystem without copy-on-write support, or any other I/O error — SHALL fall through to the stream-copy-and-read-back path above with no change to its guarantees; a failed clone MUST leave no temporary file behind. Reflink is attempted only at the copy step, after quick-match and collision resolution, so it applies equally at a plain or suffixed final name and never bypasses the identical-content skip. When the effective `reflink` is `false`, the system SHALL always take the stream-copy-and-read-back path.

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

#### Scenario: Same-filesystem transfer is reflinked without hashing
- **WHEN** `reflink` is enabled and a kept file's destination is on a copy-on-write filesystem that shares a mount with the source
- **THEN** the file is cloned into place and finalized without reading or hashing either the source or the written file, and is reported as reflinked

#### Scenario: Cross-device transfer falls back to verified copy
- **WHEN** `reflink` is enabled but the source and destination are on different filesystems
- **THEN** the clone attempt fails, no temporary file remains from it, and the file is transferred by the normal stream-copy-and-read-back path

#### Scenario: Reflink disabled always stream-copies
- **WHEN** `reflink` is disabled for the run
- **THEN** no clone is attempted and every kept file is transferred by the stream-copy-and-read-back path

### Requirement: Source deletion only after verification
When the effective `delete_source` is `true` — the profile's value unless overridden for the run by `--delete-source` or `--no-delete-source` — the system SHALL delete a source file only after its verified transfer completed or the file was confirmed already-imported by content hashing. A verified transfer includes a successful reflink clone, whose byte-for-byte identity is guaranteed by construction rather than by re-hashing; a reflinked file SHALL therefore be a source-deletion candidate exactly as a stream-copied-and-verified file is. A file accepted only by `--quick-match` — matched on name, size, and modification time without hashing its contents — SHALL NOT be a source-deletion candidate: trading verification for speed forfeits the right to delete the source. Files whose transfer failed MUST remain on the source. `--delete-source` SHALL force deletion on for the run even when the profile sets `delete_source: false`; forcing it on SHALL NOT bypass the confirmation requirement. `--no-delete-source` SHALL force deletion off for the run; `--keep-source` SHALL be accepted as an undocumented alias of `--no-delete-source`.

#### Scenario: Clean card after successful import
- **WHEN** all of a group's files transfer and verify successfully with `delete_source: true`
- **THEN** those files no longer exist on the source

#### Scenario: Reflinked files are deletion candidates
- **WHEN** all of a group's files are reflinked with `delete_source: true` and confirmation given
- **THEN** those source files are deleted, because a clone is verified by construction

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

### Requirement: Per-invocation profile overrides
The system SHALL accept CLI flags that override individual profile settings for a single run, without modifying the config file. Boolean settings SHALL be overridable in both directions via paired flags named after the config field — `--copy-quarantine` / `--no-copy-quarantine` on `scan` and `import`, `--delete-source` / `--no-delete-source` on `import`, and `--reflink` / `--no-reflink` on `import` — where passing neither flag uses the profile's value and repeating flags from a pair is resolved last-one-wins. `--quarantine PATH` on `scan` and `import` SHALL override the profile's quarantine directory for the run, resolving a relative path against the effective destination (the same rule as the config field); setting it SHALL also force `copy_quarantine` on for the run. Combining `--quarantine` with `--no-copy-quarantine` MUST be rejected as a usage error (exit 2). Overrides SHALL be applied when the profile is resolved, before planning, so `scan`, `--dry-run`, and `import` all reflect them identically.

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

#### Scenario: Reflink override forces cloning off
- **WHEN** `import --no-reflink` runs on a profile with `reflink: true` (or omitted)
- **THEN** the run behaves exactly as if the profile had set `reflink: false`, attempting no clone

#### Scenario: Reflink override forces cloning on
- **WHEN** `import --reflink` runs on a profile with `reflink: false`
- **THEN** the run behaves exactly as if the profile had set `reflink: true`

#### Scenario: Quarantine path override implies copying
- **WHEN** `import --quarantine /tmp/q` runs on a profile with `copy_quarantine: false`
- **THEN** quarantine groups are copied via verified transfer into `/tmp/q`

#### Scenario: Contradictory quarantine flags rejected
- **WHEN** `import --quarantine /tmp/q --no-copy-quarantine` is invoked
- **THEN** the invocation fails as a usage error with exit code 2 before any scanning occurs

#### Scenario: Scan previews overrides
- **WHEN** `scan --quarantine /tmp/q` runs against a source with an unmarked group
- **THEN** the printed plan shows the group's quarantine path under `/tmp/q`, and nothing on disk changes

### Requirement: Human-readable execution report is summarized by default

After executing a plan, the human-readable report SHALL show individual lines only for notable outcomes — failed transfers, collision-suffixed files, files left on source because quarantine copying is disabled, sidecar write failures, and any group not deleted from the source while deletion was in effect (named, with the reason) — and SHALL always close with a summary line stating the counts of transferred, reflinked (counted distinctly from stream-copied transfers), skipped (already imported, with quick-matched skips counted distinctly), and failed files, and of groups deleted from the source. Routine outcomes (transferred, reflinked, skipped-identical, quick-matched) SHALL NOT be listed per file by default. With `-v` the report SHALL list every file's outcome, grouped per media group with the group's destination shown once as a header. The summary counts SHALL equal those in the JSON report for the same run.

#### Scenario: Clean import summarizes to a single line
- **WHEN** `import` transfers every file successfully with no collisions and source deletion completes
- **THEN** the default report is the summary line with the transferred count and deleted-group count, with no per-file lines

#### Scenario: Reflinked files are counted distinctly in the summary
- **WHEN** `import` reflinks some files and stream-copies others in the same run
- **THEN** the summary line reports the reflinked count separately from the stream-copied transferred count, with no per-file lines by default

#### Scenario: A failure is visible without verbosity
- **WHEN** one file fails verification during an otherwise successful `import`
- **THEN** the default report names that file with its error, states why its group was not deleted from the source, and the summary line counts it as failed

#### Scenario: Verbose lists every file grouped by session
- **WHEN** `import -v` executes a plan with several groups
- **THEN** the report shows each group as a header with its destination, its files' outcomes indented beneath it, and the closing summary line
