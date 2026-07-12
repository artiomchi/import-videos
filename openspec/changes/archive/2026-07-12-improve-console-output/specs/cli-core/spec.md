## MODIFIED Requirements

### Requirement: Scan produces a reviewable plan without side effects

The `scan` command SHALL discover media via each selected profile's device implementation and print an import plan. Scanning MUST NOT create, modify, or delete any file.

The human-readable plan SHALL account for every media group, individually or in aggregate:

- Each `Keep` entry SHALL show the group name, its recorded time (short form, in the configured timezone), its file count and total size, and the fully resolved destination path.
- A reason clause SHALL appear only where the reason varies per group (`Ignore` verdicts); fixed per-verdict strings SHALL NOT be printed.
- `Quarantine` groups SHALL be rolled up by default into a single line carrying their count, aggregate size, and the quarantine root (or the quarantine-copying-disabled note). With `-v` each quarantine entry SHALL be listed individually with the same detail as `Keep` entries.
- A group of unrecognized files SHALL list the first 5 file names followed by a "… and <x> more" line when more exist; with `-v` all file names SHALL be listed. With 5 or fewer files, default and verbose output SHALL be identical for that group.
- Per-entry sidecar details SHALL be shown only with `-v`.
- The plan SHALL close with a summary line including verdict counts and the total file count and size per verdict class.

#### Scenario: Scan is read-only
- **WHEN** `scan` runs against a source containing media
- **THEN** the plan is printed and the source and destination filesystems are byte-for-byte unchanged

#### Scenario: No sources found
- **WHEN** `scan` runs and no configured profile detects a source
- **THEN** the command reports that no sources were found and exits with code 0

#### Scenario: Plan entries show time and size instead of boilerplate
- **WHEN** `scan` prints a plan containing a `Keep` group
- **THEN** the group's line shows its recorded time, file count, and total size, and contains no fixed "matches profile criteria" reason text

#### Scenario: Quarantine rolls up by default
- **WHEN** `scan` runs without `-v` over a source with many unmarked (Quarantine) groups
- **THEN** the plan shows one quarantine rollup line with the group count, aggregate size, and quarantine root, and no per-group quarantine entries

#### Scenario: Unrecognized files are listed with a cap
- **WHEN** `scan` runs without `-v` over a source with 8 unrecognized files
- **THEN** the plan lists the first 5 file names followed by a line stating 3 more exist, and `-v` lists all 8

### Requirement: Import executes exactly the scanned plan

The `import` command SHALL build the same plan `scan` would and execute it. With `--dry-run` it MUST print the plan and stop, performing no filesystem changes. Without `--dry-run`, in human output mode, `import` SHALL print the plan (the same rendering `scan` produces, honoring verbosity) before executing it, so the run states its intent before any transfer starts; in JSON mode `import` SHALL still emit exactly one document, the execution report. Executed actions SHALL match the printed plan: no file may be transferred, quarantined, or deleted that the plan did not list.

#### Scenario: Dry run changes nothing
- **WHEN** `import --dry-run` runs against a source with media
- **THEN** the plan is printed and no filesystem changes occur

#### Scenario: Execution follows the plan
- **WHEN** `import` executes a plan listing group A as Keep and group B as Quarantine
- **THEN** afterwards A's files exist under the resolved destination, B's files exist under the quarantine path, and nothing else changed

#### Scenario: Import states its plan before transferring
- **WHEN** `import` runs without `--dry-run` and without `--json` against a source with media
- **THEN** the plan rendering appears on stdout before any transfer output, followed by the execution report

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

### Requirement: Machine-readable JSON report output

A global `--json` flag SHALL make every subcommand emit its result as a single
JSON document on stdout: `scan` and `import --dry-run` emit the plan, `import`
emits the execution report. In JSON mode no other stdout output SHALL be
produced (informational lines and progress are suppressed); errors still go to
stderr and exit codes are unchanged. Timestamps SHALL be RFC 3339 strings
rendered in the configured timezone; paths SHALL be strings. Each plan action
SHALL include a `files` array naming the group's files, so entries the human
output truncates or aggregates (unrecognized files, quarantined groups) are
fully enumerable in JSON. The JSON shape is produced by dedicated view-model
types, not by serializing internal domain types directly.

#### Scenario: Scan emits a JSON plan

- **WHEN** `scan gopro --json` runs against a card with keep, quarantine, and
  ignore verdicts
- **THEN** stdout parses as one JSON document containing every planned action
  (including quarantined entries, which the human output hides by default) with
  verdict, group name, resolved paths, and summary counts

#### Scenario: Plan JSON names every file

- **WHEN** `scan --json` runs over a source with an unrecognized-files group
- **THEN** that action's `files` array names every unrecognized file, with no truncation

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

## ADDED Requirements

### Requirement: Human-readable execution report is summarized by default

After executing a plan, the human-readable report SHALL show individual lines only for notable outcomes — failed transfers, collision-suffixed files, files left on source because quarantine copying is disabled, sidecar write failures, and any group not deleted from the source while deletion was in effect (named, with the reason) — and SHALL always close with a summary line stating the counts of transferred, skipped (already imported, with quick-matched skips counted distinctly), and failed files, and of groups deleted from the source. Routine outcomes (transferred, skipped-identical, quick-matched) SHALL NOT be listed per file by default. With `-v` the report SHALL list every file's outcome, grouped per media group with the group's destination shown once as a header. The summary counts SHALL equal those in the JSON report for the same run.

#### Scenario: Clean import summarizes to a single line
- **WHEN** `import` transfers every file successfully with no collisions and source deletion completes
- **THEN** the default report is the summary line with the transferred count and deleted-group count, with no per-file lines

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
