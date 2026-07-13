## 1. Source resolution: detect every matching drive

- [x] 1.1 In `src/plan.rs`, add a `DetectedSource { name: String, path: PathBuf }` type next to `resolve_source`.
- [x] 1.2 Add `resolve_sources` (auto-mode only): walks `mount_roots` exactly as today's `resolve_source` does, but collects *every* directory where `source_impl.detect()` returns true instead of returning at the first match, then sorts the collection by `path` ascending before returning it (design D1, D5).
- [x] 1.3 Leave `resolve_source` (explicit path: CLI `--source` or profile `source: <path>`) untouched — it keeps returning `Option<PathBuf>` for exactly one path, unaffected by 1.1/1.2 (design D1's explicit/auto split).
- [x] 1.4 Unit tests for `resolve_sources` against a tempdir standing in for `mount_roots`: two matching subdirectories both returned; result sorted by path regardless of `read_dir` order; a non-matching subdirectory excluded; zero matches returns an empty `Vec`, not an error.

## 2. Per-drive scan/plan/execute cycle

- [x] 2.1 In `src/lib.rs`, factor the existing single-drive `scan → build plan → print plan → confirm → execute → print report` sequence (currently inline in `run_scan`/`run_import`) into a function taking one `DetectedSource`-shaped `(name, path)`, so it can be called once for explicit sourcing and once per drive for auto (design D1's shared-logic mitigation, avoiding duplicating the sequence).
- [x] 2.2 `run_scan`: when the profile is `source: auto` (and no `--source` override), call `resolve_sources`, then for each detected drive print its name/path header (spec: multi-drive-import, "Each drive is identified by name and path before its output") followed by that drive's existing scan-summary rendering; zero-groups drives print the header plus a "no media found" line instead of the summary (spec: "A detected drive with nothing to import is reported distinctly").
- [x] 2.3 `run_import`: same branch, looping sequentially — each drive completes its full cycle (plan printed, confirmation prompt via the existing `transfer::execute`-internal `confirm()`, execution, report printed) before the next drive's cycle starts (spec: "Import processes drives sequentially with independent confirmation"); `--dry-run` runs the same per-drive loop through printing the plan, with no confirm/execute call for any drive.
- [x] 2.4 A drive whose plan/scan summary has zero entries is treated as `empty`: header printed, no confirmation prompt, no `transfer::execute` call, loop continues to the next drive (spec: "A detected drive with nothing to import is reported distinctly").
- [x] 2.5 Explicit sourcing (`--source`, or profile `source: <path>`) keeps calling the factored single-drive function exactly once, with no header, no loop, and today's exact output — confirm this by diffing existing single-source test output/fixtures, not just by inspection.

## 3. Continue past a failed drive

- [x] 3.1 Define a per-drive outcome capturing one of: `Completed`, `CompletedWithFailures` (derived the same way `run_import` already checks `any_failed` today, scoped to one drive's `ExecuteReport`), `Empty`, or `Error(String)` (design D3).
- [x] 3.2 In the per-drive loop, catch a hard `Err` from that drive's scan/plan/execute step (instead of propagating it with `?`), record it as `Error(message)` using the error's existing `Display` (`src/error.rs`), and continue to the next drive rather than aborting the run.
- [x] 3.3 After all drives are processed, compute the run's exit code as `Failure` if any drive is `Error` or `CompletedWithFailures`, else `Success` — same aggregate rule `run_import` already applies within one drive's report, now folded across drives (design D7).
- [x] 3.4 Human-mode output for an `Error` drive: print its header, then the error message, then continue — do not print a partial/garbled report for that drive.
- [x] 3.5 Integration test: three fake drives under a tempdir `mount_roots`, drive 2 configured so its scan/plan step returns a hard error (e.g. a device stub that errors on a specific path) — assert drive 1 and drive 3 both still produce their normal output and drive 3's files are actually transferred, and the process exits 1.
- [x] 3.6 Integration test: drive 2 has one file that fails verification (reuse the existing corruption-injection pattern from `transfer_failure_keeps_source_and_does_not_block_other_groups` in `tests/integration.rs`) while drives 1 and 3 succeed — assert all three drives' reports are printed and the exit code is 1.

## 4. Multi-drive JSON output

- [x] 4.1 In `src/report.rs`, add a `DriveJson { name: String, path: String, status: &'static str, error: Option<String>, summary/plan/results: Option<...> }`-shaped view type (or one per command, if a shared type doesn't fit `scan`/`plan`/`results` payload differences cleanly) reusing the existing `ScanSummaryJson`/`PlanJson`/`ResultsJson` types as the payload — no restructuring of those types themselves.
- [x] 4.2 Wrap `scan --json` output for `source: auto` in a top-level document carrying a `drives` array of `DriveJson`, ordered exactly as processed (spec: "Multi-drive JSON output enumerates every drive").
- [x] 4.3 Wrap `import --json` (both `--dry-run` and real execution) output for `source: auto` the same way, adding the aggregate `any_failed` boolean alongside `drives`.
- [x] 4.4 Confirm explicit-source JSON output (`--source`, or profile `source: <path>`) is byte-for-byte unchanged — no `drives` key, same flat shape as today — with a regression test asserting this directly.
- [x] 4.5 JSON integration tests: two-drive `scan --json` produces a `drives` array of length 2 with correct `name`/`path`/`summary`; an `error`-status drive's entry has no `summary`/`plan`/`results` key and does have `error`; `any_failed` is `true` when any drive failed and `false` when all succeeded.

## 5. Documentation

- [x] 5.1 Add an ADR (`docs/adr/0014-...md`, following the existing numbered format) capturing the continue-past-a-failed-drive decision and why it was chosen over stopping the batch — mirrors the shape of existing decision ADRs like 0009 (quick-match) and 0011 (CLI overridability).
- [x] 5.2 Update `docs/ROADMAP.md` if its architecture/config description references `resolve_source` or single-source assumptions that this change makes stale. (Checked: the "source: auto" config line only says "probe mounted volumes with detect()" — never claimed single-drive/first-match — so nothing is stale; no edit needed.)

## 6. Quality gates

- [x] 6.1 `cargo test` passes, including new unit tests (`src/plan.rs`) and integration tests (`tests/integration.rs`).
- [x] 6.2 `cargo clippy -- -D warnings` passes.
- [x] 6.3 `cargo fmt --check` passes.
- [x] 6.4 Manually exercise `scan`/`import` against a tempdir standing in for two+ mounted drives (via `--config` pointing `mount_roots` at it) to confirm the human-readable output reads well end-to-end, not just that assertions pass. (Verified: two-drive scan/import human output shows each drive's header before its content, sequential per-drive plan/confirm/report; `--json` wraps both drives correctly with `any_failed`; zero-drive case still prints "no sources found" and exits 0.)
