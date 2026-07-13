# cli-core Specification

## Purpose

The device-agnostic foundation of `import-videos`: the CLI surface (`scan` / `import`), YAML profile configuration, the scan → plan → execute pipeline contract (the `ImportSource` trait and plan types), and the verified-transfer engine with quarantine and collision semantics. Device-specific support (GoPro, Tesla) plugs into this core via the `ImportSource` trait and does not live here.
## Requirements
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

The `scan` command SHALL discover media via each selected profile's device implementation and print a source-only inventory of what would happen on import. Scanning MUST NOT create, modify, or delete any file. Scanning MUST NOT resolve or display a destination or quarantine path for any group, and MUST NOT perform GPS telemetry lookup, regardless of the profile's `gps_lookup` setting (see gopro-telemetry capability, "GPS lookup can be disabled") — `import` (`--dry-run` or real) is the only command that resolves and shows where a group's files will land.

The human-readable inventory SHALL account for every media group, individually or in aggregate:

- Each `Keep` entry SHALL show the group name, its recorded time (short form, in the configured timezone), and its file count and total size. No destination path SHALL be shown.
- A reason clause SHALL appear only where the reason varies per group (`Ignore` verdicts); fixed per-verdict strings SHALL NOT be printed.
- `Quarantine` groups SHALL be rolled up by default into a single line carrying their count and aggregate size (no quarantine path is shown, since `scan` never resolves one). With `-v` each quarantine entry SHALL be listed individually with the same detail as `Keep` entries.
- A group of unrecognized files SHALL list the first 5 file names followed by a "… and <x> more" line when more exist; with `-v` all file names SHALL be listed. With 5 or fewer files, default and verbose output SHALL be identical for that group.
- The inventory SHALL close with a summary line including verdict counts and the total file count and size per verdict class.

#### Scenario: Scan is read-only
- **WHEN** `scan` runs against a source containing media
- **THEN** the inventory is printed and the source and destination filesystems are byte-for-byte unchanged

#### Scenario: No sources found
- **WHEN** `scan` runs and no configured profile detects a source
- **THEN** the command reports that no sources were found and exits with code 0

#### Scenario: Scan entries show time and size but no destination path
- **WHEN** `scan` prints an inventory containing a `Keep` group
- **THEN** the group's line shows its recorded time, file count, and total size, contains no fixed "matches profile criteria" reason text, and shows no destination path

#### Scenario: Quarantine rolls up by default
- **WHEN** `scan` runs without `-v` over a source with many unmarked (Quarantine) groups
- **THEN** the inventory shows one quarantine rollup line with the group count and aggregate size, and no per-group quarantine entries or quarantine path

#### Scenario: Unrecognized files are listed with a cap
- **WHEN** `scan` runs without `-v` over a source with 8 unrecognized files
- **THEN** the inventory lists the first 5 file names followed by a line stating 3 more exist, and `-v` lists all 8

#### Scenario: Scan never performs GPS telemetry lookup
- **WHEN** `scan` runs against a GoPro card whose chapters carry usable GPS fixes, regardless of the profile's `gps_lookup` setting
- **THEN** no `gpmd` track is opened, and every session's recorded time in the inventory is its camera-clock time

### Requirement: Empty media groups are excluded from every plan

Neither `scan`'s inventory nor `import`'s plan SHALL include a media group with zero files, regardless of which device produced it, its verdict, or why it has no files (a leftover empty directory from a prior deletion, a manually cleared folder, or any other cause). This filter SHALL apply uniformly across device implementations, at the point where `ImportSource::scan()`'s results are consumed, so no device module needs its own empty-group logic.

#### Scenario: Leftover empty directory does not resurface as a group
- **WHEN** a device's `scan()` reports a group with zero files (for example, a directory left behind by a prior import)
- **THEN** that group does not appear in `scan`'s inventory or in `import`'s plan, in any verdict

#### Scenario: Empty-group filtering applies identically to scan and import
- **WHEN** the same source containing a zero-file group is scanned and then imported
- **THEN** neither command's output lists that group

### Requirement: Import executes exactly the scanned plan

The `import` command SHALL resolve a plan — every group's destination or quarantine path, and (for GoPro profiles, unless disabled) its GPS-corrected timestamp — independently of `scan`. `scan`'s source-only inventory (see "Scan produces a reviewable plan without side effects") MUST NOT be relied on to predict `import`'s resolved paths, since it never resolves one. With `--dry-run` the system MUST print this resolved plan and stop, performing no filesystem changes; a `--dry-run` run and a real run of the same invocation against an unchanged source MUST resolve and print the identical plan. Without `--dry-run`, in human output mode, `import` SHALL print its plan (the same rendering `--dry-run` produces, honoring verbosity) before executing it, so the run states its intent before any transfer starts; in JSON mode `import` SHALL still emit exactly one document, the execution report. Executed actions SHALL match the printed plan: no file may be transferred, quarantined, or deleted that the plan did not list.

#### Scenario: Dry run changes nothing
- **WHEN** `import --dry-run` runs against a source with media
- **THEN** the plan is printed and no filesystem changes occur

#### Scenario: Execution follows the plan
- **WHEN** `import` executes a plan listing group A as Keep and group B as Quarantine
- **THEN** afterwards A's files exist under the resolved destination, B's files exist under the quarantine path, and nothing else changed

#### Scenario: Import states its plan before transferring
- **WHEN** `import` runs without `--dry-run` and without `--json` against a source with media
- **THEN** the plan rendering appears on stdout before any transfer output, followed by the execution report

#### Scenario: Dry-run plan matches real execution
- **WHEN** `import --dry-run` and `import` (without `--dry-run`) run in sequence against the same unchanged source
- **THEN** the destination paths and verdicts the dry run printed are exactly what the real run resolves and executes

#### Scenario: Scan's inventory does not predict import's destination path
- **WHEN** a GoPro session's camera clock is drifted enough that GPS correction moves it across a day boundary, and `gps_lookup` is enabled
- **THEN** `scan`'s inventory may name a different day than the destination path `import` actually resolves and uses

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

### Requirement: Empty source directories are pruned after verified deletion

When the effective `delete_source` is `true` and a group's files are deleted after verification (see "Source deletion only after verification"), the system SHALL remove each deleted file's parent directory if it is left empty, and continue removing each successive ancestor directory while it is also empty, stopping strictly before the scanned source root — the source root itself MUST NEVER be removed, even if it becomes empty. A directory that cannot be removed (not actually empty, or a filesystem error) SHALL simply stop the climb at that directory; a pruning failure MUST NOT fail the import or be reported as a transfer error.

#### Scenario: Empty source directory is removed after deletion
- **WHEN** a group's only files live in one source subdirectory and all are deleted with `delete_source: true`
- **THEN** that subdirectory no longer exists on the source afterward

#### Scenario: Pruning never removes the source root
- **WHEN** every file discovered under the source root is deleted with `delete_source: true`
- **THEN** the source root directory itself still exists afterward

#### Scenario: Pruning stops at a non-empty ancestor
- **WHEN** a group's files are deleted but their parent directory also contains a file belonging to a different, undeleted group
- **THEN** that parent directory, and everything above it, remains untouched

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
Before deleting source files, the system SHALL prompt for confirmation unless `--yes` is passed. If stdin is not a terminal and `--yes` is absent, the destructive step MUST be skipped with an explanatory message rather than assumed confirmed. When `import` processes more than one drive in a single invocation (a `source: auto` profile matching multiple volumes — see the multi-drive-import capability), this confirmation SHALL be required independently before each drive's deletion step; confirming or declining one drive's prompt SHALL have no effect on any other drive's prompt. `--yes` SHALL skip every drive's prompt for the run, exactly as it skips the single prompt in a one-drive run.

#### Scenario: Non-interactive run without --yes
- **WHEN** `import` runs with stdin not attached to a terminal and no `--yes`
- **THEN** files are transferred but source deletion is skipped, with a message explaining why

#### Scenario: Declined prompt
- **WHEN** the user answers no at the deletion prompt
- **THEN** all transfers remain in place and no source file is deleted

#### Scenario: Each drive in a multi-drive run prompts independently
- **WHEN** `import` runs against a `source: auto` profile matching two drives, with the effective `delete_source` true and no `--yes`
- **THEN** the system prompts once before deleting drive 1's source files and, after drive 1's report is printed, prompts again before deleting drive 2's source files

#### Scenario: --yes skips every drive's prompt
- **WHEN** `import --yes` runs against a `source: auto` profile matching three drives
- **THEN** no confirmation prompt appears for any of the three drives

### Requirement: Per-invocation profile overrides

The system SHALL accept CLI flags that override individual profile settings for a single run, without modifying the config file. Boolean settings SHALL be overridable in both directions via paired flags named after the config field — `--gopro-require-marker` / `--no-gopro-require-marker` on `scan` and `import`, and `--copy-quarantine` / `--no-copy-quarantine`, `--delete-source` / `--no-delete-source`, `--reflink` / `--no-reflink`, and `--gopro-gps-lookup` / `--no-gopro-gps-lookup`, all on `import` only — where passing neither flag uses the profile's value and repeating flags from a pair is resolved last-one-wins. `--quarantine PATH` on `import` SHALL override the profile's quarantine directory for the run, resolving a relative path against the effective destination (the same rule as the config field); setting it SHALL also force `copy_quarantine` on for the run. Combining `--quarantine` with `--no-copy-quarantine` MUST be rejected as a usage error (exit 2). Overrides SHALL be applied when the profile is resolved, before planning. Because `scan` never resolves or displays a destination or quarantine path and never performs GPS lookup (see "Scan produces a reviewable plan without side effects"), `scan` SHALL reject `--quarantine`, `--copy-quarantine` / `--no-copy-quarantine`, and `--gopro-gps-lookup` / `--no-gopro-gps-lookup` as usage errors — they are all `import`-only, exactly like `--reflink`/`--no-reflink` and `--delete-source`/`--no-delete-source`. `--gopro-require-marker`/`--no-gopro-require-marker` remains valid on both, since it changes the verdict counts `scan`'s inventory reports.

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

#### Scenario: Quarantine flags are import-only
- **WHEN** `scan --quarantine /tmp/q`, `scan --copy-quarantine`, or `scan --no-copy-quarantine` is invoked
- **THEN** the invocation fails as a usage error, since `scan` never resolves or displays a quarantine path

#### Scenario: GPS lookup override forces telemetry off
- **WHEN** `import --no-gopro-gps-lookup` runs on a profile with `gps_lookup: true` (or omitted)
- **THEN** the run behaves exactly as if the profile had set `gps_lookup: false`, attempting no GPS telemetry lookup

#### Scenario: GPS lookup override forces telemetry on
- **WHEN** `import --gopro-gps-lookup` runs on a profile with `gps_lookup: false`
- **THEN** the run behaves exactly as if the profile had set `gps_lookup: true`

#### Scenario: GPS lookup flag is import-only
- **WHEN** `scan --gopro-gps-lookup` is invoked
- **THEN** the invocation fails as a usage error, since `scan` never performs GPS lookup

### Requirement: Source resolution via explicit path or mount probing
A profile with `source: <path>` SHALL use exactly that path, and only that path — this SHALL NOT change regardless of how many other volumes happen to be mounted. A profile with `source: auto` SHALL probe mounted volumes under the configured mount roots (default: `/run/media/<user>`, `/media`, `/mnt`) and select every volume accepted by the device implementation's `detect()`, not merely the first — each accepted volume becomes one independently processed drive (see the multi-drive-import capability for how `scan` and `import` handle more than one). Selected volumes SHALL be ordered by their resolved path, ascending, so drive enumeration and numbering are reproducible across repeated runs regardless of filesystem directory-listing order. `--source PATH` on the command line SHALL override the profile for that run, always resolving to exactly that one path — this bypasses `auto` probing entirely, including when the profile's own `source` is `auto`.

#### Scenario: Explicit source overrides auto-detection
- **WHEN** `scan gopro --source /tmp/fake-card` runs
- **THEN** only `/tmp/fake-card` is scanned, and mount roots are not probed

#### Scenario: Explicit source path does not exist
- **WHEN** a run specifies a source path that does not exist
- **THEN** the run fails with an error naming the path and exits with code 1

#### Scenario: Every matching volume is selected, not just the first
- **WHEN** a profile has `source: auto` and two mounted volumes both satisfy the device implementation's `detect()`
- **THEN** both volumes are selected as drives, not only the first one found

#### Scenario: Selected volumes are ordered by path
- **WHEN** `source: auto` matches multiple volumes, whether under one mount root or spread across several
- **THEN** they are processed in ascending path order, regardless of the order the filesystem happened to list them in

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

A global `--json` flag SHALL make every subcommand emit its result as a single JSON document on stdout: `scan` emits its source-only inventory summary, `import --dry-run` emits its resolved plan, and `import` (without `--dry-run`) emits the execution report. `scan`'s JSON summary SHALL use a shape distinct from `import`'s plan JSON and MUST NOT include a path field for any entry, since `scan` never resolves a destination or quarantine path. In JSON mode no other stdout output SHALL be produced (informational lines and progress are suppressed); errors still go to stderr and exit codes are unchanged. Timestamps SHALL be RFC 3339 strings rendered in the configured timezone; paths SHALL be strings. Each `import` plan action SHALL include a `files` array naming the group's files, so entries the human output truncates or aggregates (unrecognized files, quarantined groups) are fully enumerable in JSON; `scan`'s inventory entries SHALL do the same for their file listings. The JSON shape is produced by dedicated view-model types, not by serializing internal domain types directly.

#### Scenario: Scan emits a JSON inventory
- **WHEN** `scan gopro --json` runs against a card with keep, quarantine, and ignore verdicts
- **THEN** stdout parses as one JSON document containing every group (including quarantined entries, which the human output hides by default) with verdict, group name, file count, and summary counts, and no path field on any entry

#### Scenario: Inventory JSON names every file
- **WHEN** `scan --json` runs over a source with an unrecognized-files group
- **THEN** that group's `files` array names every unrecognized file, with no truncation

#### Scenario: Import dry-run emits a JSON plan with resolved paths
- **WHEN** `import gopro --dry-run --json` runs against a card with keep and quarantine verdicts
- **THEN** stdout parses as one JSON document containing every planned action with verdict, group name, resolved destination or quarantine path, and summary counts

#### Scenario: Import emits a JSON execution report
- **WHEN** `import gopro --json --yes` executes a plan
- **THEN** stdout parses as one JSON document with per-file transfer outcomes, sidecar outcomes, source-deletion results, and any deletion-skipped reason

#### Scenario: JSON mode does not bypass confirmation
- **WHEN** `import` runs with `--json` but without `--yes` where source deletion requires confirmation
- **THEN** the confirmation rules apply exactly as without `--json`

#### Scenario: No sources found in JSON mode
- **WHEN** `scan gopro --json` finds no sources
- **THEN** stdout is a JSON document stating that (not a bare human string), and the exit code is 0

### Requirement: Transfer progress is shown on interactive terminals

During plan execution, byte-level transfer progress SHALL be displayed when the
session is an interactive terminal and `--json` is not set. The progress
indicator SHALL name the running operation and SHALL show the current file
together with its transfer phase (copying or verifying). Progress output
SHALL be absent when stdout is not a TTY or JSON mode is active, and SHALL NOT
interleave with the report output consumed by pipes.

#### Scenario: Interactive import shows progress

- **WHEN** `import` transfers files with stdout attached to a terminal
- **THEN** a progress indicator reflects bytes transferred during copy and
  verification, and the final report renders after it

#### Scenario: Progress names the operation and phase

- **WHEN** `import` is copying a file and then verifying its written copy
- **THEN** the progress indicator identifies the operation (importing) and shows the current file name with `copying` while streaming and `verifying` during the read-back

#### Scenario: Piped output stays clean

- **WHEN** `import` runs with stdout redirected to a file or pipe (or with
  `--json`)
- **THEN** the captured output contains only the report, with no progress or
  terminal-control bytes

### Requirement: Scan progress is shown on interactive terminals

The system SHALL display a progress indicator during the scan phase of `scan` and `import` (the `ImportSource::scan` call within plan building) when the session is an interactive terminal and `--json` is not set. The indicator SHALL name the running operation (scanning) alongside the current item being processed. Progress output SHALL be absent when stdout is not a TTY or JSON mode is active, and SHALL NOT interleave with the printed plan or, for `import`, the subsequent transfer-progress indicator.

#### Scenario: Interactive scan shows progress

- **WHEN** `scan` runs against a source with media, with stdout attached to a terminal
- **THEN** a progress indicator naming the scan operation reflects scan progress before the plan is printed, and clears before the plan appears

#### Scenario: Interactive import shows both phases in sequence

- **WHEN** `import` runs with stdout attached to a terminal
- **THEN** the scan-phase progress indicator appears and clears, followed (after any plan output) by the transfer-phase progress indicator; the two indicators never appear at the same time

#### Scenario: Piped or JSON output stays clean

- **WHEN** `scan` or `import` runs with stdout redirected to a file or pipe, or with `--json`
- **THEN** the captured output contains no progress or terminal-control bytes from the scan phase

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

### Requirement: Diagnostic logging is level-gated and never corrupts output

Diagnostic log output (tracing) SHALL be written to stderr, never stdout, in every output mode. The default level SHALL be WARN; `-v` SHALL enable INFO (phase milestones such as source resolution and scan completion) and `-vv` SHALL enable DEBUG (per-item decisions). While a progress indicator is active, an emitted log line SHALL NOT corrupt the indicator's rendering: the line appears intact and the indicator continues drawing after it.

#### Scenario: Warnings do not pollute JSON output
- **WHEN** `import --json` runs and a diagnostic warning fires mid-scan
- **THEN** stdout still parses as exactly one JSON document and the warning text appears on stderr

#### Scenario: A warning during an active progress bar stays legible
- **WHEN** a warning fires while the scan progress indicator is drawing on an interactive terminal
- **THEN** the warning appears as an intact line and the progress indicator resumes rendering below it

#### Scenario: Verbosity unlocks milestones
- **WHEN** `scan -v` runs against a source with media
- **THEN** stderr carries INFO-level phase milestones that a run without `-v` does not emit

