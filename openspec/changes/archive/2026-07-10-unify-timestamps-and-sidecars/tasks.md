# Implementation Tasks

## 1. Config: global timezone

- [x] 1.1 Add `timezone: Option<String>` to `RawConfig` in `src/config.rs` (serde default `None`)
- [x] 1.2 Resolve it during `load` into a `jiff::tz::TimeZone` on `Config`: unset → `TimeZone::system()`; unrecognized IANA name → `Error::Config` (exit 2), message naming the `timezone` field
- [x] 1.3 Confirm `timezone` is a top-level field only (no per-profile field, no CLI flag); add a unit test for each of the three load outcomes (explicit valid, invalid, unset)

## 2. Scan context and trait plumbing

- [x] 2.1 Add `struct ScanContext<'a> { ignore: &'a GlobSet, tz: &'a TimeZone, imported_at: Timestamp }` in `src/source/mod.rs`
- [x] 2.2 Change `ImportSource::scan` to `scan(&self, root: &Path, ctx: &ScanContext) -> Result<...>`; update `GenericSource`
- [x] 2.3 Update the caller (scan/plan entry point) to build `ScanContext`, capturing `imported_at = Timestamp::now()` once per run and passing the resolved `tz`

## 3. Layout: render `{date}` in the configured zone

- [x] 3.1 Add a `TimeZone` parameter to `LayoutTemplate::resolve` in `src/config/layout.rs`
- [x] 3.2 Format `{date:FMT}` via `timestamp.to_zoned(tz).strftime(FMT)` instead of UTC formatting
- [x] 3.3 Update `resolve` call sites (plan) to pass the zone; add unit tests asserting an evening instant lands on the local calendar day under a non-UTC zone

## 4. Unified sidecar builder

- [x] 4.1 Create `src/source/sidecar.rs` with a builder that assembles `import.json`: common envelope (`camera`, `source`, `imported_at`, `timezone`, `recorded_at`, `time_source`, `files`), `events[]`, and one optional device block
- [x] 4.2 Render all timestamp fields via `zoned.strftime("%Y-%m-%dT%H:%M:%S%:z")` (offset form, no zone-name suffix)
- [x] 4.3 Expose typed inputs (envelope facts, event records with namespaced `type`, device block) so device modules hand structured pieces, not raw JSON
- [x] 4.4 Unit-test the builder: offset format, empty `events`, device-block-only fields, and that no `event.json` copy is embedded

## 5. Tesla: configured-zone interpretation + unified sidecar

- [x] 5.1 Interpret the event/folder civil time in `ctx.tz` (replace `TimeZone::system()` in `resolve_instant`)
- [x] 5.2 Remove `event_date`/`event_time` from `build_context`; keep only `event_type`
- [x] 5.3 Emit the unified `import.json` via the shared builder: `events[]` trigger (`tesla:saved|sentry|recent`) with `time`/`reason`/`lat`/`lon`, `tesla.city` as the sole device-block field, `time_source` = `event_json|folder_name`
- [x] 5.4 Ensure the raw `event.json` still travels untouched and is not duplicated into the sidecar

## 6. GoPro: camera-clock reinterpretation + unified sidecar

- [x] 6.1 When falling back to the camera clock, interpret the `mvhd` civil value as a wall clock in `ctx.tz` (not UTC); leave the GPS-corrected path unchanged
- [x] 6.2 Emit the unified `import.json`: markers → `events[]` (`gopro:marker`, `offset_ms`, `time`, `lat`/`lon` when present), `gopro` block with `session` and `clock_offset_s`, `time_source` = `gps|camera`, `recorded_at` present
- [x] 6.3 Remove the `markers.json` sidecar path
- [x] 6.4 Confirm plan visibility, write-after-verify, and sidecar-write-failure-blocks-deletion semantics still hold

## 7. Report and logs

- [x] 7.1 Render instants in `src/report.rs` through the zone (same `%:z` format); thread the zone from `Config` to the report
- [x] 7.2 Update `tracing` sites that print group instants to render in-zone

## 8. Docs and decision record

- [x] 8.1 Add ADR 0008 superseding ADR 0006 (global `timezone`, unified `{date}`, unified sidecar); update ADR 0006 with a superseded-by pointer
- [x] 8.2 Update README: config `timezone` field + table, GoPro/Tesla "What gets kept" sections (drop `event_date`/`event_time`, `markers.json` → `import.json`, remove the `{date:local:...}` note), layout section
- [x] 8.3 Add a `docs/learning/` note on jiff zoned rendering (the `Zoned` `Display` `[IANA/Name]` suffix pitfall and the `%:z` fix)

## 9. Tests and quality gates

- [x] 9.1 Update all `ImportSource` scan callers/tests for the new `ScanContext` signature
- [x] 9.2 Pin an explicit `timezone` (e.g. `Europe/Vilnius`) in integration tests; assert Tesla paths/mtimes, GoPro GPS path (unaffected), and GoPro no-GPS camera-clock path land on the expected local day and sidecar shape
- [x] 9.3 Add an integration assertion that each device writes `import.json` (never `markers.json`, never `event.json` as the sidecar name) with the unified schema
- [x] 9.4 Run `cargo build`, `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check` green
