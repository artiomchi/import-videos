## ADDED Requirements

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
