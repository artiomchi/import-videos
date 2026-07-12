## ADDED Requirements

### Requirement: Scan progress reflects per-event completion

During `scan`, TeslaCam scanning SHALL report progress as a determinate count of event units processed: one unit per `SavedClips`/`SentryClips` event folder, plus one per `RecentClips` file when that category is enabled. The total SHALL be known from directory listings before any per-event parsing begins, and the indicator SHALL advance once per unit as it is processed, showing the current unit's name.

#### Scenario: Progress total matches discovered events
- **WHEN** a TeslaCam scan begins over a drive with saved and sentry event folders
- **THEN** the progress indicator's total equals the number of event folders the scan will process, known before any `event.json` is read

#### Scenario: Progress advances per event folder
- **WHEN** the scan processes each event folder
- **THEN** the progress indicator advances by one and shows that folder's name

#### Scenario: RecentClips files count when enabled
- **WHEN** a profile enables the `recent` category and the scan runs over a drive with RecentClips files
- **THEN** the progress total includes those files and the indicator advances as each is grouped
