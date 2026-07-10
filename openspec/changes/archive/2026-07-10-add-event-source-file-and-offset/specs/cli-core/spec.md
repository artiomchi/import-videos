## ADDED Requirements

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

## MODIFIED Requirements

### Requirement: Source deletion only after verification
When a profile sets `delete_source: true` (and `--keep-source` is not passed), the system SHALL delete a source file only after its verified transfer completed or the file was confirmed already-imported by content hashing. A file accepted only by `--quick-match` — matched on name, size, and modification time without hashing its contents — SHALL NOT be a source-deletion candidate: trading verification for speed forfeits the right to delete the source. Files whose transfer failed MUST remain on the source. `--keep-source` SHALL override the profile for that run.

#### Scenario: Clean card after successful import
- **WHEN** all of a group's files transfer and verify successfully with `delete_source: true`
- **THEN** those files no longer exist on the source

#### Scenario: Failed transfer keeps source file
- **WHEN** one file's transfer fails verification while others succeed
- **THEN** the failed file remains on the source and the run exits with code 1

#### Scenario: Quick-matched files are never deleted
- **WHEN** `import --quick-match` runs with `delete_source: true` and confirmation given, over a group whose files are all quick-matched
- **THEN** those source files are left in place, because no content was verified for them
