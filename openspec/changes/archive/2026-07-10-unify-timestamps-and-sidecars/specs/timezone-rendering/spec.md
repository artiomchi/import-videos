## ADDED Requirements

### Requirement: Global timezone configuration
The system SHALL accept an optional top-level `timezone` field naming an IANA time zone (e.g. `Europe/Vilnius`). When set, it SHALL be resolved at configuration load; an unrecognized zone name MUST fail loading with an error naming the `timezone` field, and the process SHALL exit with code 2. When unset, the system SHALL resolve the host's local system time zone. This single value governs both the interpretation of device wall clocks and the rendering of every user-visible timestamp; there SHALL be no per-profile timezone override and no timezone command-line flag.

#### Scenario: Explicit timezone loads
- **WHEN** a config sets `timezone: Europe/Vilnius`
- **THEN** the config loads and that zone is used to interpret and render timestamps

#### Scenario: Invalid timezone rejected at load
- **WHEN** a config sets `timezone: Mars/Olympus`
- **THEN** loading fails naming the `timezone` field and the process exits with code 2

#### Scenario: Unset timezone defaults to system local
- **WHEN** a config omits `timezone`
- **THEN** the config loads and timestamps are interpreted and rendered in the host's local time zone

### Requirement: Timestamps rendered in the configured timezone
All user-visible timestamps SHALL be rendered in the configured timezone: `{date:...}` fields in resolved destination and quarantine paths, sidecar timestamp fields, and instants written to logs. `{date:FMT}` SHALL format a group's recording instant converted to the configured zone rather than to UTC. Rendered timestamp strings in sidecars SHALL be ISO-8601 with a numeric UTC offset (e.g. `2026-07-04T18:23:51+03:00`) and SHALL NOT include a zone-name suffix. A file's modification time SHALL remain the recorded instant itself and MUST NOT be altered by the rendering zone.

#### Scenario: Layout date renders in the configured zone
- **WHEN** a group's instant is `2026-07-04T15:23:51Z`, `timezone` is `Europe/Vilnius` (+03:00), and the layout is `{date:%Y-%m-%d}/{date:%H-%M-%S}`
- **THEN** the resolved path ends in `2026-07-04/18-23-51`

#### Scenario: Evening ride keeps its local calendar day
- **WHEN** a group's instant is `2026-07-04T22:30:00Z`, `timezone` is `America/Los_Angeles` (−07:00), and the layout is `{date:%Y-%m-%d}`
- **THEN** the resolved path ends in `2026-07-04`, the local calendar day, not the UTC day

#### Scenario: Sidecar timestamps carry the zone offset
- **WHEN** a group is imported under `timezone: Europe/Vilnius`
- **THEN** its sidecar's `recorded_at` is written as an ISO-8601 string ending in `+03:00` with no `[Europe/Vilnius]` suffix

#### Scenario: File mtime is unaffected by the rendering zone
- **WHEN** the same instant is imported under two different `timezone` settings
- **THEN** the imported file's modification time is identical in both runs

### Requirement: Wall-clock device times interpreted in the configured timezone
Device timestamps that are civil wall-clock readings with no true offset — Tesla event and folder times, and GoPro's camera-clock fallback used when no usable telemetry is available — SHALL be interpreted as being in the configured `timezone` to produce a real instant. Device timestamps that are already true UTC instants — GoPro's GPS-corrected time — SHALL be used unchanged. This interpretation determines both the group instant (and thus file mtimes) and, after rendering, the destination path.

#### Scenario: Tesla civil time resolved via the configured zone
- **WHEN** a Tesla event's civil timestamp is `2026-07-04T18:23:51` and `timezone` is `Europe/Vilnius` (+03:00)
- **THEN** the group instant is `2026-07-04T15:23:51Z`

#### Scenario: GoPro camera-clock fallback resolved via the configured zone
- **WHEN** a GoPro session has no usable telemetry, its first chapter's `mvhd` civil time is `2026-07-04T18:23:51`, and `timezone` is `Europe/Vilnius` (+03:00)
- **THEN** the group instant is `2026-07-04T15:23:51Z` and the rendered `{date:%H-%M-%S}` is `18-23-51`

#### Scenario: GoPro GPS instant used as-is
- **WHEN** a GoPro session has a GPS-corrected instant of `2026-07-04T15:23:51Z`
- **THEN** that instant is used directly and is rendered in the configured zone like any other instant
