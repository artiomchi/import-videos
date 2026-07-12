## ADDED Requirements

### Requirement: Scan progress reflects chapter-level completion

During `scan`, GoPro card scanning SHALL report progress as a determinate count of chapter files processed against the total chapter count discovered on the card, updating as each chapter's session-building work completes rather than only once per session.

#### Scenario: Progress total matches discovered chapters

- **WHEN** a card scan begins
- **THEN** the progress indicator's total equals the number of chapter files the scan will process, known before any chapter's metadata is read

#### Scenario: Progress advances during GPS offset search

- **WHEN** a session's chapters are searched in order for a usable GPS fix (gopro-telemetry)
- **THEN** the progress indicator advances as each searched chapter is examined, rather than only after that session's session-building completes

#### Scenario: Progress reaches full completion

- **WHEN** a card scan finishes
- **THEN** the progress indicator has advanced exactly once per chapter file scanned, regardless of how many chapters actually contributed to GPS offset derivation
