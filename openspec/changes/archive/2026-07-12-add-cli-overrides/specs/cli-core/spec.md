# cli-core Delta

## ADDED Requirements

### Requirement: Per-invocation profile overrides
The system SHALL accept CLI flags that override individual profile settings for a single run, without modifying the config file. Boolean settings SHALL be overridable in both directions via paired flags named after the config field — `--copy-quarantine` / `--no-copy-quarantine` on `scan` and `import`, and `--delete-source` / `--no-delete-source` on `import` — where passing neither flag uses the profile's value and repeating flags from a pair is resolved last-one-wins. `--quarantine PATH` on `scan` and `import` SHALL override the profile's quarantine directory for the run, resolving a relative path against the effective destination (the same rule as the config field); setting it SHALL also force `copy_quarantine` on for the run. Combining `--quarantine` with `--no-copy-quarantine` MUST be rejected as a usage error (exit 2). Overrides SHALL be applied when the profile is resolved, before planning, so `scan`, `--dry-run`, and `import` all reflect them identically.

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

#### Scenario: Quarantine path override implies copying
- **WHEN** `import --quarantine /tmp/q` runs on a profile with `copy_quarantine: false`
- **THEN** quarantine groups are copied via verified transfer into `/tmp/q`

#### Scenario: Contradictory quarantine flags rejected
- **WHEN** `import --quarantine /tmp/q --no-copy-quarantine` is invoked
- **THEN** the invocation fails as a usage error with exit code 2 before any scanning occurs

#### Scenario: Scan previews overrides
- **WHEN** `scan --quarantine /tmp/q` runs against a source with an unmarked group
- **THEN** the printed plan shows the group's quarantine path under `/tmp/q`, and nothing on disk changes

## MODIFIED Requirements

### Requirement: Source deletion only after verification
When the effective `delete_source` is `true` — the profile's value unless overridden for the run by `--delete-source` or `--no-delete-source` — the system SHALL delete a source file only after its verified transfer completed or the file was confirmed already-imported by content hashing. A file accepted only by `--quick-match` — matched on name, size, and modification time without hashing its contents — SHALL NOT be a source-deletion candidate: trading verification for speed forfeits the right to delete the source. Files whose transfer failed MUST remain on the source. `--delete-source` SHALL force deletion on for the run even when the profile sets `delete_source: false`; forcing it on SHALL NOT bypass the confirmation requirement. `--no-delete-source` SHALL force deletion off for the run; `--keep-source` SHALL be accepted as an undocumented alias of `--no-delete-source`.

#### Scenario: Clean card after successful import
- **WHEN** all of a group's files transfer and verify successfully with `delete_source: true`
- **THEN** those files no longer exist on the source

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
