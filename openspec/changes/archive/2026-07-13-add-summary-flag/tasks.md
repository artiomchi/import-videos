## 1. CLI surface

- [x] 1.1 Add a global `--summary: bool` flag to `Cli` in `src/cli.rs`, alongside the existing `--json`/`-v`/`--config`, with a doc comment describing its scope (scan, import, cleanup) and its precedence over `-v`'s render-detail effect.
- [x] 1.2 Add parsing tests in `tests/cli_overrides.rs` (or `src/cli.rs`'s own `#[cfg(test)]` module, matching existing convention) confirming `--summary` parses on `scan`, `import`, and `cleanup`, and combines with `-v` and `--json` without a clap-level conflict.

## 2. report.rs: Detail enum foundation

- [x] 2.1 Add `pub enum Detail { Summary, Normal, Verbose }` (`Copy, Clone, Debug, PartialEq`) to `src/report.rs`, replacing the `verbose: bool` parameter design used today.
- [x] 2.2 Replace `verbose: bool` with `detail: Detail` across `render_plan`, `render_plan_entry`, `render_scan_summary`, `render_scan_entry`, `render_unrecognized_files`, `render_scan_unrecognized_files`, `render_file_name_list`, `render_results`, `render_group_notable`, `render_group_verbose`, and `render_cleanup_plan` — update every internal branch (`if verbose` → `if detail == Detail::Verbose`, etc.) and every call site, including existing tests that currently pass a literal `true`/`false`.

## 3. Scan/plan rendering: summary tier

- [x] 3.1 In `render_plan` and `render_scan_summary`, under `Detail::Summary`, skip per-action rendering entirely — no `render_plan_entry`/`render_scan_entry`/`render_quarantine_rollup` calls — while still accumulating `VerdictTotals` from every action, so the closing `Summary: ...` line is unaffected.
- [x] 3.2 Drop the "`-v` to list" / "`-v` to list all" hint text whenever `Detail::Summary`, since `-v` no longer unlocks a listing in that mode.

## 4. Execution report rendering: summary tier

- [x] 4.1 Extend `ResultsTally` / `summary_line` with `suffixed` and `skipped_quarantine_disabled` counts (the struct already computes both; `summary_line` doesn't read them yet), appended as new trailing clauses only when `Detail::Summary` and the corresponding count is nonzero — default and verbose output must render byte-for-byte unchanged.
- [x] 4.2 In `render_group_notable`, gate the `Suffixed` and `SkippedQuarantineDisabled` per-file lines on `detail != Detail::Summary`. Keep `Failed`, `SIDECAR FAILED`, and the undeleted-group line unconditional in every `Detail` value.

## 5. Cleanup rendering: summary tier

- [x] 5.1 Add a `detail: Detail` parameter to `render_cleanup_plan`; under `Detail::Summary` skip the per-entry `[PURGE]`/`[KEEP]` loop, keeping the `Quarantine: <root>` header line and the existing closing `Summary: ...` line unchanged.
- [x] 5.2 Check `src/cleanup.rs` for what `CleanupReport`/`CleanupResult` already carry (size, per-entry outcome), then add a deleted-count/deleted-size/failed-count tally to `render_cleanup_report`, emitted only under `Detail::Summary`, replacing the per-entry `deleted: <path>` lines. Keep `FAILED to delete` lines unconditional.

## 6. Wire Detail through lib.rs

- [x] 6.1 Replace `let verbose = cli.verbose > 0;` in `src/lib.rs` with a `Detail` computation: `Detail::Summary` when `cli.summary`, else `Detail::Verbose` when `cli.verbose > 0`, else `Detail::Normal`.
- [x] 6.2 Thread `detail: Detail` through `run_scan`, `run_scan_cycle`, `run_import_cycle`, `scan_drives`, the multi-drive import loop, `print_plan`, `print_scan_summary`, `print_cleanup_plan`, and `run_cleanup`, replacing every `verbose: bool` parameter along these call chains.
- [x] 6.3 Confirm every `--json` branch reads neither `detail` nor `cli.summary` — `--summary` needs no `conflicts_with` against `--json`, per design.

## 7. Tests

- [x] 7.1 Update existing `src/report.rs` unit tests and `tests/integration.rs` cases that construct `verbose: true`/`false` to construct the appropriate `Detail` variant instead.
- [x] 7.2 Add coverage for: scan `--summary` prints only the closing summary line (no per-group, rollup, or unrecognized-files listing); import `--summary` collapses `Suffixed`/`SkippedQuarantineDisabled` lines into summary-line counts while still naming a `FAILED` file individually; `import --summary -v` produces stdout identical to `import --summary` alone, with additional stderr diagnostic lines from `-v`; `--summary --json` produces output identical to `--json` alone; `cleanup --dry-run --summary` omits per-entry `[PURGE]`/`[KEEP]` lines; `cleanup --yes --summary` tallies deletions while still naming a delete failure individually.

## 8. Quality gates

- [x] 8.1 `cargo test`
- [x] 8.2 `cargo clippy -- -D warnings`
- [x] 8.3 `cargo fmt --check`
