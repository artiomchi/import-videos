## MODIFIED Requirements

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

## ADDED Requirements

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
