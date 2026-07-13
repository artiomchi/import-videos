## MODIFIED Requirements

### Requirement: Scan produces a reviewable plan without side effects

The `scan` command SHALL discover media via each selected profile's device implementation and print a source-only inventory of what would happen on import. Scanning MUST NOT create, modify, or delete any file. Scanning MUST NOT resolve or display a destination or quarantine path for any group, and MUST NOT perform GPS telemetry lookup, regardless of the profile's `gps_lookup` setting (see gopro-telemetry capability, "GPS lookup can be disabled") — `import` (`--dry-run` or real) is the only command that resolves and shows where a group's files will land.

The human-readable inventory SHALL account for every media group, individually or in aggregate:

- Each `Keep` entry SHALL show the group name, its recorded time (short form, in the configured timezone), and its file count and total size. No destination path SHALL be shown.
- A reason clause SHALL appear only where the reason varies per group (`Ignore` verdicts); fixed per-verdict strings SHALL NOT be printed.
- `Quarantine` groups SHALL be rolled up by default into a single line carrying their count and aggregate size (no quarantine path is shown, since `scan` never resolves one). With `-v` each quarantine entry SHALL be listed individually with the same detail as `Keep` entries.
- A group of unrecognized files SHALL list the first 5 file names followed by a "… and <x> more" line when more exist; with `-v` all file names SHALL be listed. With 5 or fewer files, default and verbose output SHALL be identical for that group.
- The inventory SHALL close with a summary line including verdict counts and the total file count and size per verdict class.

With `--summary`, the inventory SHALL omit every per-group and per-verdict listing line — individual `Keep`/`Ignore` entries, the quarantine rollup line, and the unrecognized-files listing — printing only the closing summary line. This applies regardless of `-v` (see "A `--summary` flag collapses per-entry listings across commands" for the precedence rule).

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

#### Scenario: Summary mode prints only the closing line
- **WHEN** `scan --summary` runs against a source with `Keep`, `Quarantine`, and `Ignore` groups
- **THEN** the inventory shows no per-group lines, no quarantine rollup line, and no unrecognized-files listing — only the closing summary line

#### Scenario: Summary overrides verbose listing
- **WHEN** `scan --summary -v` runs against the same source
- **THEN** the inventory output is identical to `scan --summary` alone — no per-group or per-verdict listing appears

### Requirement: Human-readable execution report is summarized by default

After executing a plan, the human-readable report SHALL show individual lines only for notable outcomes — failed transfers, collision-suffixed files, files left on source because quarantine copying is disabled, sidecar write failures, and any group not deleted from the source while deletion was in effect (named, with the reason) — and SHALL always close with a summary line stating the counts of transferred, reflinked (counted distinctly from stream-copied transfers), skipped (already imported, with quick-matched skips counted distinctly), and failed files, and of groups deleted from the source. Routine outcomes (transferred, reflinked, skipped-identical, quick-matched) SHALL NOT be listed per file by default. With `-v` the report SHALL list every file's outcome, grouped per media group with the group's destination shown once as a header. The summary counts SHALL equal those in the JSON report for the same run.

With `--summary`, collision-suffixed and quarantine-copy-disabled per-file lines SHALL also be omitted, with their counts added to the closing summary line instead (a count of files renamed due to a destination collision, and a count left on source because quarantine copying is disabled). Failed transfers, sidecar write failures, and undeleted-group lines SHALL remain individually listed regardless of `--summary` — these are the exceptions the flag is not meant to hide. This applies regardless of `-v`.

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

#### Scenario: Summary mode collapses collisions and disabled-quarantine lines into counts
- **WHEN** `import --summary` executes a plan with two collision-suffixed files and one file left on source because quarantine copying is disabled
- **THEN** no per-file lines appear for those three files, and the closing summary line states counts for both outcomes

#### Scenario: Summary mode still names failures
- **WHEN** `import --summary` executes a plan where one file fails verification
- **THEN** the report still names that failed file with its error, and the summary line counts it as failed

## ADDED Requirements

### Requirement: A --summary flag collapses per-entry listings across commands

A global `--summary` flag SHALL make `scan`, `import`, and `cleanup` (see the cli-maintenance capability) omit per-group and per-entry listings from their human-readable output, replacing them with progress indicators (unaffected by this flag) and a closing summary/tally line, while exceptions that require action (failed transfers, sidecar write failures, delete failures, undeleted-group lines) remain individually listed regardless of `--summary`.

`--summary` and `-v` MAY be combined. The diagnostic-logging effect of `-v` (see "Diagnostic logging is level-gated and never corrupts output") SHALL be unaffected by `--summary`. `--summary` SHALL take precedence over `-v`'s per-entry listing effect: combining the two flags SHALL produce the same collapsed listing `--summary` alone produces, with the addition of `-v`'s diagnostic log lines on stderr.

`--summary` SHALL have no effect in `--json` mode, which is already a single machine-readable document unaffected by either verbosity flag.

#### Scenario: --summary alone collapses listings
- **WHEN** any of `scan`, `import`, or `cleanup` runs with `--summary`
- **THEN** no per-group or per-entry listing appears in the human-readable output, only progress indicators and a closing summary/tally line

#### Scenario: --summary combined with -v adds logs, not listings
- **WHEN** `import --summary -v` runs
- **THEN** stderr carries the INFO-level diagnostic lines `-v` enables, while stdout's report is identical to `import --summary` without `-v`

#### Scenario: --summary is a no-op under --json
- **WHEN** `scan --summary --json` runs
- **THEN** stdout is the same single JSON document `scan --json` alone would produce
