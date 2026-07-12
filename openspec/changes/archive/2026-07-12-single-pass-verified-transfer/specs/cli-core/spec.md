## MODIFIED Requirements

### Requirement: Verified transfer with atomic finalization

For each kept file whose final destination name is unoccupied, the system SHALL read the source exactly once, hashing it (blake3) while stream-copying to a temporary name (`<final>.part`) in the destination directory. The system SHALL then re-read and hash the written temporary file, and only when the read-back hash matches the source stream hash rename it to the final name — so a copy corrupted or truncated in the write path fails verification rather than being finalized. On mismatch or copy failure the temporary file MUST be removed and the source file left untouched.

When the final destination name is already occupied, the system MAY hash the source in a separate pass before copying, in order to resolve the collision without a copy (see "Collisions never overwrite existing footage"); a copy that follows an unresolved collision SHALL use the same single-pass stream-copy and read-back verification at the suffixed name.

#### Scenario: Successful verified copy
- **WHEN** a file is transferred and the written temporary file's read-back hash matches the source stream hash
- **THEN** the file exists under its final destination name and no `.part` file remains

#### Scenario: Verification failure preserves the source
- **WHEN** the written temporary file's read-back hash differs from the source stream hash
- **THEN** the temporary file is deleted, the source file remains, and the action is reported as failed

#### Scenario: Source is read once when no collision exists
- **WHEN** a kept file's final destination name is unoccupied
- **THEN** the source file is read exactly once during its transfer

#### Scenario: Write-path corruption is detected
- **WHEN** the bytes persisted in the temporary file differ from the bytes streamed from the source
- **THEN** verification fails, the temporary file is removed, and the source file is untouched
