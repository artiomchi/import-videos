# cli-maintenance Specification

## Purpose

The maintenance and debugging surface of `import-videos`: the `cleanup` command for purging quarantined footage on a schedule, and the `inspect` command for dumping a single file's or event's device metadata without running a full import. These commands operate alongside the core scan → plan → execute pipeline but are not part of it.

## Requirements

### Requirement: Cleanup builds a reviewable purge plan before deleting

`cleanup PROFILE` SHALL scan the profile's resolved quarantine directory and
produce a purge plan (entries with name, age in quarantine, and size) without
modifying anything. With `--dry-run` the command SHALL print the plan and exit 0
without deleting. Deletion SHALL happen only when the plan is executed.

By default, the plan SHALL list every entry individually (`[PURGE]`/`[KEEP]`,
its age, and its size), closed by a summary line stating the purge count and
size and the kept count and size. With `--summary` (see the cli-core
capability's "A `--summary` flag collapses per-entry listings across
commands"), the per-entry listing SHALL be omitted; only the quarantine root
and the closing summary line SHALL be printed.

#### Scenario: Dry run deletes nothing

- **WHEN** `cleanup gopro --dry-run` runs against a quarantine directory with
  entries
- **THEN** the plan listing each entry is printed, no file or directory is
  removed, and the exit code is 0

#### Scenario: Empty quarantine

- **WHEN** `cleanup` runs and the quarantine directory is empty or absent
- **THEN** the command reports nothing to clean and exits 0

#### Scenario: Default plan lists every entry

- **WHEN** `cleanup gopro --dry-run` runs against a quarantine directory with
  several entries
- **THEN** each entry is printed individually with its age and size, followed
  by the closing summary line

#### Scenario: Summary mode omits the entry listing

- **WHEN** `cleanup gopro --dry-run --summary` runs against the same
  quarantine directory
- **THEN** no per-entry lines are printed — only the quarantine root and the
  closing summary line with purge/keep counts and sizes

### Requirement: Cleanup deletes only within the quarantine directory

Cleanup SHALL remove only the immediate entries of the profile's resolved
quarantine directory (the same resolution used by import planning:
`profile.quarantine` or `{destination}/_quarantine`). It SHALL refuse to run,
with a config error (exit 2), if the resolved quarantine root equals or contains
the profile's destination root.

#### Scenario: Imported footage is untouched

- **WHEN** cleanup purges a quarantine directory that lives under the
  destination (default `{destination}/_quarantine`)
- **THEN** only entries inside the quarantine directory are removed; sibling
  imported footage under the destination is untouched

#### Scenario: Misconfigured quarantine root is refused

- **WHEN** a profile resolves its quarantine directory to the destination root
  itself (or a parent of it)
- **THEN** cleanup exits 2 with an error naming the conflict and deletes nothing

### Requirement: Age filtering measures time in quarantine

Cleanup SHALL, when given `--older-than <span>` (jiff friendly format, e.g.
`30d`, `2w`), purge only entries whose quarantine age exceeds the span. Age SHALL be
measured from the quarantine entry's own directory mtime (when it landed in
quarantine), NOT from the mtimes of files inside it, which are stamped to
recording time. Without `--older-than`, all entries are candidates.

#### Scenario: Recent entry is retained

- **WHEN** `cleanup gopro --older-than 30d --yes` runs and a group directory was
  created in quarantine 5 days ago (regardless of the recording-time mtimes of
  the files inside it)
- **THEN** that entry is not purged and the plan marks it as kept

#### Scenario: Old entry is purged

- **WHEN** `cleanup gopro --older-than 30d --yes` runs and a group directory's
  own mtime is 45 days old
- **THEN** that entry is deleted

#### Scenario: Invalid span is a usage error

- **WHEN** `cleanup gopro --older-than banana` runs
- **THEN** the command exits 2 with a parse error and deletes nothing

### Requirement: Cleanup deletion requires confirmation

Executing a purge SHALL prompt for confirmation unless `--yes` is given,
following the same rules as import's source deletion: a non-interactive session
without `--yes` SHALL fail rather than block; a declined prompt SHALL abort with
nothing deleted.

#### Scenario: Non-interactive run without --yes

- **WHEN** cleanup runs with stdin not a terminal and without `--yes`
- **THEN** it exits non-zero with an error telling the user to pass `--yes`, and
  deletes nothing

#### Scenario: Declined prompt

- **WHEN** the user answers anything other than yes at the confirmation prompt
- **THEN** cleanup aborts and deletes nothing

#### Scenario: --yes skips the prompt

- **WHEN** `cleanup gopro --yes` runs interactively
- **THEN** the purge executes without prompting

### Requirement: Cleanup's execution report names failures individually and, under --summary, tallies deletions

After executing a purge, the human-readable report SHALL, by default, print a
line for every deleted entry and every entry that failed to delete. With
`--summary`, the per-entry `deleted` lines SHALL be omitted and replaced by a
closing tally line stating the count and total size deleted; entries that
failed to delete SHALL remain individually listed regardless of `--summary`,
naming the entry and the error.

#### Scenario: Default report lists every deletion

- **WHEN** `cleanup gopro --yes` executes a purge plan with three entries, all
  deleted successfully
- **THEN** the report prints one "deleted" line per entry

#### Scenario: Summary mode tallies deletions but still names failures

- **WHEN** `cleanup gopro --yes --summary` executes a purge plan where one
  entry fails to delete and two succeed
- **THEN** no per-entry "deleted" lines appear, a closing tally line states
  the two successful deletions and their total size, and the failed entry is
  still individually named with its error

### Requirement: Inspect dumps a GoPro MP4's device metadata

`inspect FILE` on an MP4 SHALL print the file's HiLight markers (count and
per-marker offsets with derived timestamps), the MP4 creation time, and — when a
`gpmd` telemetry track is present — a GPS summary (first usable fix coordinates,
GPS-vs-camera clock offset, sample count). Parsing SHALL be read-only.

#### Scenario: MP4 with HiLights and GPS

- **WHEN** `inspect GX010123.MP4` runs on a file containing HMMT markers and a
  gpmd track
- **THEN** the output includes the marker count, each marker's offset and
  timestamp, the creation time, and the GPS summary, and the file is unmodified

#### Scenario: Partial metadata still prints

- **WHEN** `inspect` runs on an MP4 whose gpmd track is corrupt but whose HMMT
  box parses
- **THEN** the HiLight and creation-time sections are printed, the GPS section
  reports the parse error, and the exit code is 1

### Requirement: Inspect dumps a Tesla event's metadata

`inspect PATH` SHALL, for a Tesla event folder (a directory containing
`event.json`) or an `event.json` file itself, print the parsed event fields
(timestamp, reason, city, coordinates) and list the clip files present in the
folder.

#### Scenario: Event folder

- **WHEN** `inspect TeslaCam/SavedClips/2026-07-09_08-15-30/` runs
- **THEN** the parsed `event.json` fields and the per-camera clip files are
  listed

### Requirement: Inspect works without configuration

`inspect` SHALL NOT require a config file or profile; it operates on the given
path alone. Timestamps are rendered in the system timezone when available, UTC
otherwise. A path that is neither an MP4 nor a Tesla event SHALL be a usage
error.

#### Scenario: No config file present

- **WHEN** `inspect clip.mp4` runs on a machine with no
  `~/.config/import-videos/config.yaml`
- **THEN** the metadata dump succeeds

#### Scenario: Unsupported input

- **WHEN** `inspect notes.txt` runs
- **THEN** the command exits 2 explaining what inputs are supported

### Requirement: Maintenance commands honor JSON output mode

With the global `--json` flag, `cleanup` SHALL emit its purge plan (and, when
executed, per-entry results) as a single JSON document on stdout, and `inspect`
SHALL emit the metadata dump as a single JSON document, including raw marker
millisecond offsets alongside rendered timestamps. No other stdout output SHALL
be produced in JSON mode.

#### Scenario: Cleanup dry run as JSON

- **WHEN** `cleanup gopro --dry-run --json` runs
- **THEN** stdout is exactly one JSON document describing the purge plan

#### Scenario: Inspect as JSON

- **WHEN** `inspect GX010123.MP4 --json` runs
- **THEN** stdout is exactly one JSON document with marker offsets (raw ms and
  rendered timestamps), creation time, and GPS summary fields
