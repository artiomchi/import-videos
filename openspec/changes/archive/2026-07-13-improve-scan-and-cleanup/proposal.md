## Why

`scan` is slow because it pays the full cost of GoPro GPS telemetry extraction — and often for footage that gets quarantined and discarded anyway — while producing destination paths that can silently disagree with where `import` actually files a session once GPS corrects a drifted camera clock. Separately, deleting a group's source files during `import --delete-source` never removes the now-empty directory it leaves behind, and Tesla's scanner then treats that leftover empty folder as a legitimate 0-file event on every subsequent scan. Both are papercuts in the same pipeline (scan → plan → execute, ADR 0003) worth fixing together: one is about scan being trustworthy, the other about the source card staying clean after import.

## What Changes

- **BREAKING**: `scan` no longer produces the same plan shape as `import`/`import --dry-run`. `scan` becomes a lightweight, source-only inventory (session/file counts and verdicts) — it never resolves destination paths and never runs GPS telemetry lookup. `import` (dry-run or real) keeps the full, accurate plan: resolved destination paths and (per the new flag below) GPS-corrected timestamps. This replaces the current guarantee that `scan` and `import --dry-run` share `build_plan` verbatim; `import --dry-run` becomes the sole "exact preview of what import will do." `scan`'s text and JSON output shapes both change (existing `PlanJson`/`render_plan` consumers of `scan` output need to move to the new scan summary shape).
- Add a GoPro-only, two-state config field (default `true`, preserves today's behavior) plus a paired CLI override (`--gopro-gps-lookup` / `--no-gopro-gps-lookup`, following the existing `require_marker`/ADR 0011 pattern) to disable GPS telemetry lookup entirely for `import`. When disabled, sessions take the same already-tested "no usable fix" fallback to camera-clock time.
- **BREAKING**: `--quarantine`, `--copy-quarantine`, and `--no-copy-quarantine` become `import`-only, moving off the shared `scan`/`import` flag set alongside the new GPS flag — since `scan` no longer resolves or shows any path, these flags have no observable effect there either, and now raise a usage error on `scan` instead of being silently accepted and ignored.
- Reorder `GoproSource::build_session` to decide the Keep/Quarantine verdict (from HiLight markers, which telemetry never influences) before running GPS telemetry — sessions that end up `Quarantine`d under `require_marker: true` never pay the telemetry cost, since their destination path doesn't use the timestamp. Always on, independent of the flag above.
- After `transfer::execute` deletes a group's verified source files, prune the now-empty directories they leave behind, walking up from each file's parent but never removing the source root itself.
- After any device's `scan()` returns, drop groups with zero files before they become part of a plan — a device-agnostic backstop so a leftover empty directory (from this tool or any other cause) never resurfaces as a phantom 0-file `Keep` group on a later scan.

## Capabilities

### New Capabilities
(none — everything below is a requirement change to an existing capability)

### Modified Capabilities
- `cli-core`: `scan` becomes a source-only inventory distinct from `import`'s plan (changes "Scan produces a reviewable plan without side effects" and "Import executes exactly the scanned plan"); adds a scan-specific JSON summary shape (extends "Machine-readable JSON report output"); adds the `--gopro-gps-lookup`/`--no-gopro-gps-lookup` override (extends "Per-invocation profile overrides"); adds empty-directory pruning after verified source deletion (extends "Source deletion only after verification"); adds the zero-file-group filter as a plan-building guarantee (extends "Scan produces a reviewable plan without side effects").
- `gopro-telemetry`: adds the ability to disable GPS lookup outright (new requirement, degrades via the existing camera-clock fallback path); adds skipping telemetry for sessions that verdict `Quarantine` before it would run (extends "Telemetry failures degrade to camera clock" / verdict-independence guarantee).

## Impact

- `src/transfer.rs`: source-deletion loop gains directory pruning after each group's files are removed.
- `src/plan.rs`: `build_plan` drops zero-file groups; gains (or is replaced by, for the `scan` path) a lightweight source-only inventory builder that skips destination/quarantine path resolution.
- `src/source/gopro.rs`: `build_session` reordered (markers/verdict before telemetry); new `gps_time_lookup`-style field read from `ScanContext` or `GoproSource`.
- `src/config.rs`: new field on `SourceKind::Gopro`, defaulted and validated like `require_marker`.
- `src/cli.rs`, `src/lib.rs`: new paired override flag wired through `Overrides`/`apply_overrides`; `--quarantine`/`--copy-quarantine`/`--no-copy-quarantine` move off the shared `OverrideFlags` onto `Import` directly, alongside it; `OverrideFlags` shrinks to just the `gopro-require-marker` pair; `run_scan` diverges from `run_import` instead of sharing `scan_profile` verbatim.
- `src/report.rs`: new scan-summary renderer and JSON view-model, separate from `render_plan`/`PlanJson`.
- `docs/adr/0003-scan-plan-execute-safety-model.md`: the "Plan review: `scan` and `import --dry-run` print the plan without touching anything" language needs updating to reflect that only `import --dry-run` is now the exact preview.
- No change to `src/source/tesla.rs` — the zero-file-group fix is a generic, device-agnostic filter in `plan.rs`.
