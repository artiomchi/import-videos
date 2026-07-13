## 1. Config and core types

- [x] 1.1 Add `gps_lookup: bool` to `ScanContext` (`src/source/mod.rs`), documented like the existing `imported_at`/`progress` fields a device may simply not read.
- [x] 1.2 Add `gps_lookup: bool` to `SourceKind::Gopro` (`src/config.rs`), `#[serde(default = "default_gps_lookup")]` returning `true`, following `require_marker`'s exact shape; update `SourceKind::build()` to pass it into `GoproSource`.
- [x] 1.3 Add `gps_lookup: bool` to the `GoproSource` struct (`src/source/gopro.rs`).
- [x] 1.4 Add config tests mirroring `gopro_profile_defaults_require_marker_to_true` / the reflink default tests: `gps_lookup` defaults to `true` when omitted, loads correctly when set to `false`.

## 2. GoPro session logic: marker-first ordering and lookup gating (design D2, D4)

- [x] 2.1 Restructure `GoproSource::build_session` so HiLight marker offsets (`chapter_markers`, no telemetry) are collected and the `Keep`/`Quarantine` verdict is decided *before* any telemetry is opened.
- [x] 2.2 Skip `derive_session_offset` / `open_chapter_telemetry` entirely when the effective verdict is `Quarantine`, or when `ctx.gps_lookup` is `false` — both reuse the existing `session_offset: None` / camera-clock fallback, no new code path.
- [x] 2.3 Once telemetry has run (or been skipped), compute marker wall-times/coordinates in a second pass, preserving today's output for sessions where telemetry still runs.
- [x] 2.4 Generalize the progress "catch-up" increment (`ctx.progress.inc(chapters.len() as u64 - visited as u64)`) to cover a session whose telemetry was skipped outright (`visited = 0`), so the total-chapter progress count stays exact.
- [x] 2.5 Update `scan_progress_reaches_total_chapter_count` and any other tick-count-pinned tests in `src/source/gopro.rs` for the new skip paths.
- [x] 2.6 Add unit tests: a `Quarantine`-bound session (no markers, `require_marker: true`) with a `gpmd` track carrying a usable fix never has that fix applied (no offset, no geo) even though the fixture would otherwise produce one; `gps_lookup: false` skips telemetry for a `Keep` session that does have markers.

## 3. CLI flags and overrides (design D3, D7)

- [x] 3.1 Add `--gopro-gps-lookup` / `--no-gopro-gps-lookup` to the `Import` command variant in `src/cli.rs` (not `OverrideFlags`), paired via `overrides_with` like `--reflink`/`--no-reflink`.
- [x] 3.2 Move `quarantine: Option<PathBuf>`, `copy_quarantine`, `no_copy_quarantine` off `OverrideFlags` onto the `Import` variant directly; `OverrideFlags` keeps only `gopro_require_marker`/`no_gopro_require_marker`. Decide whether `OverrideFlags` remains a named struct for that one pair or is inlined directly on `Scan`/`Import` (design's Open Question — implementer's call). (Kept as a named struct — still `#[command(flatten)]`ed on both `Scan` and `Import`, unchanged shape aside from the shrunk field set.)
- [x] 3.3 Add `gps_lookup: Option<bool>` to `Overrides`; wire `quarantine`/`copy_quarantine`'s population into `run_inner`'s `Command::Import` arm (mirroring how `delete_source`/`reflink` are already handled there) instead of `OverrideFlags::to_overrides()`.
- [x] 3.4 Update `apply_overrides` in `src/lib.rs` to apply `gps_lookup` onto `SourceKind::Gopro`, raising the same "only valid for gopro profiles" `Error::Config` as `require_marker` when set on a non-GoPro profile.
- [x] 3.5 Update `src/cli.rs` tests for the new flag pairing (last-flag-wins, `override_pair` coverage) and add a test confirming `scan --quarantine ...`, `scan --copy-quarantine`, `scan --no-copy-quarantine`, and `scan --gopro-gps-lookup` all fail to parse (clap usage error), matching `scan --reflink` today.

## 4. Scan/import plan divergence (design D1, D5)

- [x] 4.1 Add a shared helper (e.g. `scan_nonempty`) that calls `ImportSource::scan()` and drops any `(group, verdict)` with `group.files.is_empty()`, used by both `build_plan` and the new scan summary builder — the empty-group guarantee lives in exactly one place.
- [x] 4.2 Add `ScanSummary` / `ScanEntry` types (per design D1) — name, verdict, file count, total size, `recorded_at`, and the unrecognized-files listing — distinct from `ImportPlan`/`PlannedAction`.
- [x] 4.3 Add `plan::build_scan_summary(profile, source_impl, source_root, progress)`: resolves the source, builds a `ScanContext` with `gps_lookup: false` unconditionally, calls the shared helper from 4.1, and tallies into `ScanSummary` — no destination/quarantine path resolution.
- [x] 4.4 Update `build_plan` to use the shared empty-group helper, and to set `ctx.gps_lookup` from the profile's effective GoPro `gps_lookup` (post-override).
- [x] 4.5 Update `lib.rs::run_scan` to call `build_scan_summary` instead of sharing `scan_profile`/`build_plan` with `run_import`; `run_import` (dry-run and real) keeps using `build_plan`.
- [x] 4.6 Add `plan.rs` unit tests: a zero-file group is excluded from both `build_plan` and `build_scan_summary`; `build_scan_summary` never populates a destination or quarantine path for any entry.

## 5. Reporting (design D1)

- [x] 5.1 Add `report::render_scan_summary` (human-readable) following the existing verdict-tally / unrecognized-files-cap conventions in `render_plan`, but with no per-entry path.
- [x] 5.2 Add `ScanSummaryJson` view-model and `report::scan_summary_to_json`, distinct from `PlanJson` — no `path` field on any entry.
- [x] 5.3 Wire `run_scan` to the new renderer/JSON functions instead of `print_plan`.
- [x] 5.4 `report.rs` unit tests: scan summary shows time, file count, and size with no destination path; quarantine rollup shows count and size with no quarantine path; unrecognized-files cap-at-5 behavior is unchanged; JSON summary has no `path` field anywhere.

## 6. Directory pruning after verified deletion (design D6)

- [x] 6.1 Add a `source_root: &Path` parameter to `transfer::execute` / `execute_inner`; thread it from `lib.rs::run_import`, which already has it from `plan::resolve_source`.
- [x] 6.2 Implement a prune-upward-while-empty helper in `transfer.rs`: canonicalize both the candidate directory and `source_root` before comparing, stop at the first non-empty directory or at `source_root` (never removing it), and treat any removal failure as "stop climbing" rather than an error.
- [x] 6.3 Call the helper after a group's files are deleted, once per distinct parent directory of the files just removed.
- [x] 6.4 `transfer.rs` tests: an emptied subdirectory is removed after deletion; the source root is never removed even when everything under it is deleted; pruning stops at a directory that still holds a file from a different, undeleted group; a pruning failure does not fail the import or get reported as a transfer error.

## 7. Update existing tests for the new scan/import split

- [x] 7.1 `tests/cli_overrides.rs::scan_previews_the_quarantine_override_without_touching_disk` — replace with a test asserting `scan --quarantine <path>` now fails as a usage error.
- [x] 7.2 `tests/integration.rs::scan_and_dry_run_perform_no_filesystem_changes` — split into a case exercising `build_scan_summary`'s read-only guarantee and a case exercising `build_plan`/dry-run's read-only guarantee.
- [x] 7.3 `tests/gopro_import.rs`: review and update `scan_and_dry_run_are_read_only`, `piped_scan_and_dry_run_import_produce_no_progress_bytes`, and `non_dry_run_import_prints_the_plan_before_the_execution_report` for scan's new output shape. (Also fixed `verbose_flag_unlocks_info_milestones_a_default_run_does_not_emit`, which pinned scan logging "plan built" — scan now logs "scan complete" only; "plan built" moved to `import --dry-run`.)
- [x] 7.4 `tests/gopro_gps.rs`: review `unmarked_session_without_telemetry_is_still_quarantined` and related cases against the marker-first reordering; add a case confirming a quarantine-bound session's `gpmd` track is never opened even when present.
- [x] 7.5 Add an end-to-end integration test: import + delete a Tesla `SavedClips` event, confirm the event folder no longer exists, then confirm a subsequent `scan` and `import` report no phantom zero-file group for it.

## 8. Documentation

- [x] 8.1 Update `docs/adr/0003-scan-plan-execute-safety-model.md`'s "Plan review" language — only `import --dry-run` is now the exact preview of what `import` will do.
- [x] 8.2 Add a learning note under `docs/learning/` for whichever new concept reads as least obvious in review (candidates: canonicalized-ancestor-directory pruning, or structuring one shared helper so two call sites can't drift on an invariant). (Added `canonicalized-path-boundaries.md`.)
- [x] 8.3 Update README/CLI help text that documents `scan`'s current output shape or the flags moving to `import`-only.

## 9. Quality gates

- [x] 9.1 `cargo test`
- [x] 9.2 `cargo clippy -- -D warnings`
- [x] 9.3 `cargo fmt --check`
