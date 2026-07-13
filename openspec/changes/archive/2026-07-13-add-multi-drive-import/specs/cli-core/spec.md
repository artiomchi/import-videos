## MODIFIED Requirements

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
