## 1. Configuration

- [x] 1.1 Add `SourceKind::Tesla { events, reasons }` variant: `events` as a `Vec` of a new `EventCategory` enum (`Saved`, `Sentry`, `Recent`) defaulting to `[saved, sentry]`; `reasons` as an optional allow-xor-deny type (spec: "Tesla profile type", design D5)
- [x] 1.2 Enforce `reasons` exclusivity at load — exactly one of `allow`/`deny`, error names the profile, exit code 2 — following the existing `require_marker` validation style
- [x] 1.3 Wire `SourceKind::Tesla` into the profile→`ImportSource` mapping, constructing `TeslaSource`
- [x] 1.4 Unit tests: defaults applied, allow+deny rejected, empty `reasons` block rejected, tesla fields rejected on non-tesla profiles, serde round-trip of the Tesla variant (flatten + tagged-enum quirks, as done for GoPro)

## 2. Detection and event discovery

- [x] 2.1 Create `src/source/tesla.rs` with `TeslaSource`; implement `detect()`: `TeslaCam/` containing at least one of `SavedClips`/`SentryClips`/`RecentClips` (spec: "TeslaCam drive detection", design D1)
- [x] 2.2 Walk `SavedClips/` and `SentryClips/` event folders; build one `MediaGroup` per folder with every contained file (minus `ignore` globs), including unrecognized files (spec: "One media group per event folder", design D2)
- [x] 2.3 Collect stray files outside event folders into an `Ignore("unrecognized file(s)")` group, mirroring the GoPro pattern
- [x] 2.4 Register `pub mod tesla;` in `src/source/mod.rs`

## 3. Event metadata and time handling

- [x] 3.1 Parse `event.json` tolerantly (`timestamp`, `city`, `est_lat`/`est_lon` as strings → `f64`, `reason`); any missing/malformed field degrades individually, never drops the event (spec: "Tolerant event metadata parsing", design D4)
- [x] 3.2 Timestamp resolution chain: `event.json` civil timestamp → folder-name `YYYY-MM-DD_HH-MM-SS` fallback → `Ignore("unparseable event folder")`; record which source won for the sidecar's provenance field
- [x] 3.3 Implement the wall-clock/instant split (design D3): context fields `event_type`/`event_date`/`event_time` formatted from the civil value; `MediaGroup.timestamp` and `recorded_at` resolved via the system timezone with jiff's compatible disambiguation
- [x] 3.4 Per-clip `recorded_at` from each clip's own filename stem; `event.json`/`thumb.png` use the event timestamp (design D8)

## 4. Filtering and verdicts

- [x] 4.1 Category filter: events whose category is not in `events` get `Ignore` naming the disabled category — discovered and reported, not skipped (spec: "Event category filtering")
- [x] 4.2 Reason filter: apply allow/deny only when a reason is known; unknown reason is kept (fail open); `Ignore` verdicts name the filtered reason (spec: "Trigger-reason filtering", design D4/D5)
- [x] 4.3 Confirm no code path can produce `Verdict::Quarantine` for Tesla groups (spec: "Tesla verdicts never quarantine")

## 5. RecentClips (opt-in)

- [x] 5.1 When `events` includes `recent`, group `RecentClips/` files by filename-stem timestamp into per-minute Keep groups with `event_type: recent` and stem-derived wall-clock context; skip the directory entirely otherwise (spec: "RecentClips import is opt-in", design D6)

## 6. Sidecar

- [x] 6.1 Assemble `import.json` per kept event: device type, `event_type`, source folder path, parsed event metadata, resolved wall-clock + UTC times, timestamp provenance, file list — attached via the existing `Sidecar` mechanism (spec: "Normalized import sidecar", design D7)

## 7. Integration tests

- [x] 7.1 Test helper that synthesizes a TeslaCam card in a `tempfile` dir (event folders, `event.json`, dummy clips/thumb) — pure files/JSON, no binary fixtures
- [x] 7.2 Detection: TeslaCam with clips dirs detected; bare `TeslaCam/`, empty root, and GoPro card rejected
- [x] 7.3 End-to-end import: whole event folder lands at `{event_type}/{event_date}/{event_time}`, `event.json` + `thumb.png` + unknown files travel with it, `import.json` written, source deleted only after verify
- [x] 7.4 Filtering: disabled category and denied/not-allowed reasons yield visible Ignore verdicts, untouched sources, nothing in quarantine
- [x] 7.5 Degraded metadata: corrupt `event.json` → folder-name timestamp + provenance recorded in sidecar + kept under allow-list; unparseable folder name and no timestamp → Ignore
- [x] 7.6 RecentClips: skipped by default; per-minute clustering when enabled
- [x] 7.7 Mtime stamping: clip mtimes match their own stems resolved in the system timezone (pin `TZ` in the test for determinism)

## 8. Documentation and quality gates

- [x] 8.1 Write ADR 0006: vehicle-local wall-clock naming vs. system-timezone instants for Tesla event times (design D3)
- [x] 8.2 Add a learning note if implementation surfaces a new-to-this-codebase Rust concept (candidate: modeling allow-xor-deny as an enum vs. two `Option` fields — make illegal states unrepresentable); skip if nothing genuinely new comes up
- [x] 8.3 Update the example config in README/docs with the `tesla` profile (`events`, `reasons.deny` example, `{event_type}/{event_date}/{event_time}` layout)
- [x] 8.4 Quality gates green: `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`
