## MODIFIED Requirements

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

## ADDED Requirements

### Requirement: Empty media groups are excluded from every plan

Neither `scan`'s inventory nor `import`'s plan SHALL include a media group with zero files, regardless of which device produced it, its verdict, or why it has no files (a leftover empty directory from a prior deletion, a manually cleared folder, or any other cause). This filter SHALL apply uniformly across device implementations, at the point where `ImportSource::scan()`'s results are consumed, so no device module needs its own empty-group logic.

#### Scenario: Leftover empty directory does not resurface as a group
- **WHEN** a device's `scan()` reports a group with zero files (for example, a directory left behind by a prior import)
- **THEN** that group does not appear in `scan`'s inventory or in `import`'s plan, in any verdict

#### Scenario: Empty-group filtering applies identically to scan and import
- **WHEN** the same source containing a zero-file group is scanned and then imported
- **THEN** neither command's output lists that group

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
