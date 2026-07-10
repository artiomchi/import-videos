# cli-core Delta

## ADDED Requirements

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
