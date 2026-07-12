## 1. Relocate `Progress`

- [x] 1.1 Move the `Progress` struct and its impl from `src/transfer.rs` to a new `src/progress.rs`; register the module in `src/lib.rs`.
- [x] 1.2 Add `Progress::counted(enabled: bool) -> Self`, identical to `Progress::new` except its `ProgressStyle` template is count-oriented (e.g. `{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {msg}`); factor the shared bar-construction/style-fallback logic so `new` and `counted` don't duplicate it.
- [x] 1.3 Widen `set_length`, `set_message`, `inc`, `finish`, and the `#[cfg(test)] position()` from module-private to `pub(crate)` — they currently compile only because every caller lives in `transfer.rs`; `src/source/gopro.rs` and cross-module tests need them too.
- [x] 1.4 Update `src/transfer.rs` to import `Progress` from `crate::progress` instead of defining it; keep its own call sites (`set_length`, `inc`, `set_message`, `finish`) unchanged.
- [x] 1.5 Update every direct `transfer::Progress::...` reference to `crate::progress::Progress::...` (or the equivalent public path): `src/lib.rs:201` and all 13 `transfer::Progress::hidden()` call sites in `tests/integration.rs`. Do not leave a `pub use` re-export in `transfer.rs` — update the call sites.
- [x] 1.6 Move the existing `hidden_progress_never_constructs_a_bar` test (and any other `Progress`-only tests) to `src/progress.rs`; confirm `cargo test` still passes with no behavior change.

Sections 2-4 change one call chain (`ScanContext` → `build_plan` → `scan_profile`) across several files; they are not independently compilable checkpoints — implement them together before running the quality gates in §7.

## 2. `ScanContext` progress field

- [x] 2.1 Add `progress: &'a Progress` to `ScanContext` in `src/source/mod.rs`.
- [x] 2.2 Update `plan::build_plan` (`src/plan.rs`) to accept the `Progress` it should place on `ScanContext`, rather than constructing `ScanContext` without one.
- [x] 2.3 Update the `ScanContext` test helpers in `src/source/gopro.rs` and `src/source/tesla.rs` to supply a `Progress::hidden()` (or equivalent) alongside the existing `ignore`/`tz` fields. `src/plan.rs` has no such helper — its tests call `build_plan` directly, so they pick up the new `Progress` argument added in 2.2 as an ordinary call-site update, not a `ScanContext`-construction change.

## 3. GoPro per-chapter progress reporting

- [x] 3.1 In `GoproSource::scan`, compute the total chapter count from `discover()`'s output (sum of session chapter-vec lengths) and call `ctx.progress.set_length(total)` before the per-session loop.
- [x] 3.2 Add a `progress: &Progress` parameter to `derive_session_offset` (`src/source/gopro.rs:364`), passed by `build_session` from `ctx.progress`. Inside the loop, tick and `set_message` (session id + current chapter's file name) as the **first statement of every iteration**, before the `telemetry[i].as_mut()` guard and before any `continue` (no telemetry, parse error, no fix, no usable sample) — every iteration the loop executes ticks exactly once, regardless of outcome. Change the return type to `(Option<SessionTelemetry>, usize)`, where the `usize` is the number of iterations executed: `i + 1` on an early return at index `i`, `chapters.len()` if the loop exhausts without a fix. This count must equal the number of ticks already emitted — do not compute it separately from a re-derived index.
- [x] 3.3 In `build_session`, immediately after `derive_session_offset` returns `(session_offset, visited)`, tick `ctx.progress` once via a single `inc(chapters.len() as u64 - visited as u64)` for the chapters the search never reached, so every chapter contributes exactly one tick in total regardless of where (or whether) the GPS fix was found.
- [x] 3.4 Call `ctx.progress.finish()` (clearing the bar) at the end of `GoproSource::scan`, before returning the group list.

## 4. Visibility gating threaded through `scan`/`import`

- [x] 4.1 In `src/lib.rs`, move the TTY/`--json` enabled check so it's computed once per command before `scan_profile` runs; pass the resulting `Progress` (or the enabled bool) into `scan_profile` so it can hand it to `plan::build_plan`.
- [x] 4.2 Confirm `run_import`'s existing transfer-phase `Progress::new(...)` construction (currently after `scan_profile` returns) is unaffected — it remains a separate, byte-oriented `Progress` built after scanning completes.
- [x] 4.3 Confirm `run_scan` and `run_import --dry-run` both show scan-phase progress (both go through `scan_profile`).

## 5. Tests

- [x] 5.1 GoPro scan test: construct `Progress::counted(true)` (drawing to a non-TTY test process is fine — `indicatif` doesn't require a real terminal), scan a multi-session fixture, and assert `position()` (now `pub(crate)`, per 1.3) equals the total chapter count at the end. Cover a session where the GPS search never finds a fix (exhausts all its chapters) and one where it finds a fix on the first chapter (remaining chapters ticked via the 3.3 catch-up path) to exercise both ends of the accounting.
- [x] 5.2 `progress.rs` test: `Progress::counted(false)` and `Progress::hidden()` construct no bar and every method is a no-op (mirrors the existing byte-oriented hidden test).
- [x] 5.3 Integration test (`tests/integration.rs` or `tests/gopro_import.rs`): piped/`--json` `scan` and `import --dry-run` produce no progress or terminal-control bytes on stdout.

## 6. Docs

- [x] 6.1 Update `README.md`'s progress-bar note (near the existing byte-level transfer-progress paragraph) to mention the scan-phase indicator and that it appears before the plan/transfer output.

## 7. Quality gates

- [x] 7.1 Run `cargo test`, `cargo clippy -- -D warnings`, and `cargo fmt --check`; all green.
