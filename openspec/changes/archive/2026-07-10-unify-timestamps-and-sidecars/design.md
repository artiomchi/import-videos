## Context

Timestamp handling is split two ways today. The layout engine
(`src/config/layout.rs`) only formats `{date...}` in **UTC** from a group's
`Timestamp`. Tesla sidesteps that (ADR 0006) by baking wall-clock strings into
`event_date`/`event_time` context fields so folder names match the car's screen.
Sidecars diverge too: GoPro writes `markers.json`, Tesla writes `import.json`,
with different shapes.

This change introduces one global `timezone` and one sidecar format. The
proposal covers the *what*; this document settles *how* the zone threads through
scan → plan → transfer/report, the unified `import.json` schema, and the
supersession of ADR 0006. It builds on ADR 0003 (scan/plan/execute), ADR 0004
(YAML profiles), and ADR 0005 (`ImportSource` trait).

## Goals / Non-Goals

**Goals:**
- A single config `timezone` (IANA name; default system-local) that both
  *interprets* device wall clocks and *renders* every user-visible timestamp
  (destination paths, sidecar fields, logs).
- `{date:...}` becomes the one date vocabulary for all devices, rendered in the
  configured zone; Tesla's `event_date`/`event_time` context fields go away.
- One `import.json` schema across devices: common envelope + namespaced
  `events[]` + per-device block.
- The internal time representation stays a canonical UTC instant; the zone is a
  *rendering* concern applied at format time.

**Non-Goals:**
- Per-profile timezone overrides or a `--tz` CLI flag (config-global only).
- Cross-timezone reconciliation of a device recorded in a different zone than
  the configured one — we assume device clocks are set to the configured zone
  (the ADR 0006 cross-zone caveat is retained, just relocated to `timezone`).
- Changing keep/quarantine/ignore verdicts — this change only touches
  timestamps and sidecar shape, never what gets kept.
- File *content* changes: mtimes remain the recorded instant; the raw Tesla
  `event.json` still travels untouched.

## Decisions

### D1 — One `timezone`, dual role, resolved once at load

A single `timezone: Option<String>` on the config (not per-profile) is parsed at
load into a `jiff::tz::TimeZone`; unset resolves to `TimeZone::system()`. The
same zone value serves two roles: interpreting device wall clocks (below) and
rendering all output. An invalid IANA name fails at load as `Error::Config`
(exit 2), consistent with existing config validation (ADR 0004).

*Alternatives:* separate interpret/display zones (rejected — the user explicitly
wants one knob; device clocks and the librarian are assumed co-located); a
per-run `--tz` flag (rejected — dropped from scope, config is the single source).

### D2 — Instants stay canonical; the zone is applied only at render time

`MediaGroup.timestamp` and `MediaFile.recorded_at` remain UTC `Timestamp`
values. The zone is threaded to the three *rendering* surfaces — layout, sidecar,
logs — and never stored. This keeps mtimes correct regardless of zone and keeps
the plan/transfer core zone-agnostic.

### D3 — Wall-clock device times are interpreted in `timezone`

Two timestamp provenances exist and must be treated differently:

- **True-UTC instants** — GoPro's GPS-corrected time. Already a real instant;
  rendered in `timezone` gives the correct local reading. Used as-is.
- **Wall-clock civil values** — Tesla's `event.json`/folder time, and GoPro's
  **camera-clock fallback** (local time mislabeled as UTC). These are civil
  readings with no true offset. They are interpreted as being in the configured
  `timezone` (`civil.to_zoned(tz)`) to produce a correct instant.

This generalizes ADR 0006's Tesla-only `resolve_instant` (which used
`TimeZone::system()`) to *the configured zone*, and extends the same treatment
to GoPro's camera-clock path — which today is rendered correctly only because
`{date}` formats in UTC. Once `{date}` renders in a non-UTC zone, the
camera-clock instant **must** be reinterpreted or it double-shifts by the
offset. GPS-corrected sessions are unaffected.

*Alternative:* leave camera-clock formatting in UTC while GPS renders in-zone
(rejected — two rendering rules for one field reintroduces exactly the
inconsistency this change removes).

### D4 — `{date}` renders through the zone; supersede ADR 0006

`LayoutTemplate::resolve` gains the `TimeZone`. `{date:FMT}` becomes
`timestamp.to_zoned(tz).strftime(FMT)` instead of formatting the raw UTC
`Timestamp`. Tesla's `event_date`/`event_time` context fields are removed;
`event_type` stays (it is a category, not a timestamp). The deferred
`{date:local:...}` idea is dropped — `timezone` subsumes it. This supersedes
ADR 0006 via a new **ADR 0008**; no backward compatibility is preserved (the
tool is unreleased).

### D5 — Thread the zone into `ImportSource::scan` via a scan context

Because D3 requires the zone *during scan* (Tesla/GoPro civil→instant) and the
sidecar is assembled during scan, the trait method changes from
`scan(&self, root, ignore)` to carry a small context:

```
struct ScanContext<'a> {
    ignore: &'a GlobSet,
    tz: &'a TimeZone,
    imported_at: Timestamp,   // captured once per run; injected for determinism
}
```

Bundling into a struct (vs. adding positional params) keeps the signature stable
as rendering inputs grow, and lets tests pin `imported_at` and `tz` so sidecar
output is deterministic.

*Alternative:* keep `scan` zone-agnostic and defer sidecar rendering to a later
stage (rejected — Tesla's *instant* itself depends on the zone, so the zone has
to reach scan regardless; deferring would split one concern across two stages).

### D6 — Unified `import.json` schema and shared builder

A shared builder (new `src/source/sidecar.rs`) assembles every device's
`import.json`. Device modules hand it structured pieces (envelope facts,
`events`, a device block); the builder renders all timestamps via the zone. The
schema:

```jsonc
{
  "camera": "gopro-hero8" | "tesla",
  "source": "<source folder>",
  "imported_at": "2026-07-10T12:00:00+03:00",   // run time, zone offset
  "timezone": "Europe/Vilnius",
  "recorded_at": "2026-07-04T18:23:51+03:00",    // group instant, zone offset
  "time_source": "gps|camera" | "event_json|folder_name",
  "files": ["…"],
  "events": [
    { "type": "gopro:marker" | "tesla:saved" | "tesla:sentry" | "tesla:recent",
      "time": "…+03:00", "lat": 54.6, "lon": 25.2, "reason": "…" }
  ],
  "gopro": { "session": "0123", "clock_offset_s": 1.2 }
  // or: "tesla": { "city": "Vilnius" }
}
```

Rules the schema enforces:
- **Common envelope** carries anything both devices share.
- **`events[]`** holds per-point-in-time records: GoPro markers (N per group),
  Tesla triggers (one per event folder; empty for RecentClips, which have no
  `event.json`). Each `type` is namespaced `device:kind`.
- **Device block** holds *only* fields with no common and no per-event home
  (GoPro `session`/`clock_offset_s`; Tesla `city`). It never re-wraps data that
  already lives in the envelope or `events[]` — the raw `event.json` already
  travels to the destination, so it is not duplicated into the sidecar.
- GoPro's `markers.json` is removed.

### D7 — Timestamp string format

Every rendered timestamp is ISO-8601 with a numeric offset and no zone-name
suffix, via `strftime("%Y-%m-%dT%H:%M:%S%:z")` on a `Zoned` (jiff's `Zoned`
`Display` appends `[IANA/Name]`, which we do not want in the sidecar). One field
per timestamp — the offset pins the instant, so no redundant `…Z` UTC field.

### D8 — Log and report rendering

`src/report.rs` and `tracing` sites that print instants format them through the
zone (same `%:z` format). The zone reaches the report from `Config`; log sites
that render group times receive it where the plan is walked.

## Risks / Trade-offs

- **Default zone makes output machine-dependent** → integration/unit tests set
  an explicit `timezone` (e.g. `Europe/Vilnius`) rather than relying on
  `TimeZone::system()`, so path/sidecar assertions are stable across machines.
- **`imported_at` is non-deterministic** → injected via `ScanContext` (D5) so
  tests pin it; production captures `Timestamp::now()` once per run.
- **GoPro camera-clock reinterpretation (D3) is a subtle behavior change** →
  without it, camera-clock folders silently shift by the offset. Covered by a
  dedicated test asserting a no-GPS session lands on the expected local day;
  GPS path gets its own test proving it is unaffected.
- **BREAKING sidecar + layout output** → acceptable pre-release (proposal); the
  `markers.json` → `import.json` rename and Tesla context-field removal are
  called out in README and ADR 0008.
- **`scan` signature change ripples to every `ImportSource` impl and its tests**
  → mechanical; the `ScanContext` struct limits future churn.
- **jiff `Zoned` `Display` suffix pitfall** (D7) → mitigated by the explicit
  `%:z` strftime format; a learning note records the gotcha.

## Migration Plan

Pre-release tool — no data migration. Implementation order (each step keeps the
build green):

1. Config: add + validate `timezone`, resolve to `TimeZone` (default system).
2. Introduce `ScanContext`; update the `ImportSource` trait and `GenericSource`.
3. Layout: thread `TimeZone` into `resolve`; `{date}` renders in-zone.
4. Shared `sidecar.rs`: unified `import.json` builder.
5. Tesla: interpret wall clock in configured zone; drop `event_date`/
   `event_time`; emit unified sidecar (`event_type` retained).
6. GoPro: reinterpret camera-clock fallback in-zone; GPS path unchanged; emit
   unified sidecar with `recorded_at`; remove `markers.json`.
7. Report/logs: render instants in-zone.
8. Docs (README GoPro/Tesla/layout, config table), **ADR 0008** superseding
   ADR 0006, and a `docs/learning/` note on jiff zoned rendering.

## Open Questions

- **Logs**: render every logged instant with offset, or a terser local form for
  readability? (Leaning: same `%:z` format everywhere for consistency.)
- **GoPro camera-clock reinterpretation (D3)**: **resolved — in scope.** The
  camera-clock fallback is reinterpreted in the configured zone; the GPS path is
  unchanged.
