# 0008 — Global timezone configuration and unified import.json sidecar

- Status: accepted
- Supersedes: ADR 0006
- Date: 2026-07-10

## Context

Two problems accumulated as device support grew:

**Timezone fragmentation.** ADR 0006 resolved Tesla's civil datetimes using `TimeZone::system()` — the importing machine's timezone. For GoPro, the mvhd civil time was also silently resolved in the system zone. This works when the machine and the camera share a timezone, but breaks silently when they do not (e.g. importing footage shot abroad). There was no way for a user to override the zone without changing the system locale.

**Sidecar proliferation.** GoPro wrote a `markers.json` with a bespoke schema; Tesla wrote the raw `event.json` wholesale and nothing else. Adding a second device meant inventing another schema. There was no single shape a downstream consumer could rely on.

## Decision

**Global `timezone` config key.** `config.yaml` gains an optional `timezone` field accepting an IANA timezone name (e.g. `Europe/Vilnius`). When omitted it falls back to `TimeZone::system()`, preserving current behaviour. The resolved `TimeZone` is threaded through `ScanContext` to every device module and to layout rendering, so all civil-time-to-instant conversions and all `{date:...}` layout fields use one consistent zone.

**Unified `import.json` sidecar (design D6).** Every kept group now writes a single `import.json` file with a common envelope:

```json
{
  "camera": "gopro-hero8",
  "source": "/media/CARD/DCIM/100GOPRO",
  "imported_at": "2026-07-10T12:00:00+03:00",
  "timezone": "Europe/Vilnius",
  "recorded_at": "2026-07-09T23:19:48+03:00",
  "time_source": "gps",
  "files": ["GX010123.MP4"],
  "events": [
    { "type": "gopro:marker", "time": "...", "offset_ms": 500, "lat": 51.5, "lon": -0.12 }
  ],
  "gopro": { "session_id": "0123", "clock_offset_s": -3612.0 }
}
```

Device-specific fields live in a namespaced block (`gopro: {}` or `tesla: {}`). Event types use the format `device:verb` (e.g. `gopro:marker`, `tesla:saved`, `tesla:sentry`). The file is always named `import.json`; `markers.json` and ad-hoc event sidecars no longer exist.

**Timestamp format (design D7).** All timestamps in `import.json` use `"%Y-%m-%dT%H:%M:%S%:z"` — ISO-8601 with a numeric offset and second precision, no IANA zone-name suffix. jiff's `Zoned` `Display` impl appends `[IANA/Name]` which is not standard JSON/ISO; `strftime` with `%:z` avoids that.

**Layout uses `{date:FORMAT}` exclusively.** The old Tesla-specific context keys `event_date` and `event_time` are removed; all layout date/time rendering goes through `{date:FORMAT}` which uses the configured timezone.

Rejected alternatives:
- Per-device timezone override — more precise but far more config surface; global zone covers the primary use-case.
- Keeping bespoke sidecar schemas — makes any tooling that reads sidecars device-specific; unified schema is strictly better.
- jiff `Zoned` Display for timestamps — produces non-standard `[IANA/Name]` suffix.

## Consequences

- `timezone` omitted → `TimeZone::system()`, identical behaviour to before.
- Existing `markers.json` files on already-imported sessions are unaffected (they are in the destination, not re-written).
- Integration tests that pin specific folder names must also pin `timezone: UTC` in their configs to be system-independent.
- A new `src/source/sidecar.rs` module is the single place that renders timestamps and assembles the JSON; device modules call it with typed structs.
