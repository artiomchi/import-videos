## 1. Prerequisite

- [x] 1.1 Confirm `single-pass-verified-transfer` is implemented (transfer flow is copy → read-back verify); this change labels those phases and must land after it

## 2. Progress infrastructure (design D1, D8 registry half)

- [x] 2.1 Add `{prefix:.bold}` to both templates in `src/progress.rs`; constructors take the operation label (`Progress::counted(enabled, "Scanning")`, `Progress::new(enabled, "Importing")`); update the construction sites in `src/lib.rs`
- [x] 2.2 Add the `OnceLock<MultiProgress>` registry in `src/progress.rs`; visible bars register on construction and deregister on `finish()`; `hidden()` stays registry-free
- [x] 2.3 Extend `progress.rs` unit tests: prefix present on visible bars, hidden bars still construct nothing and touch no registry

## 3. Diagnostics to stderr, bar-safe (design D8, spec "Diagnostic logging is level-gated and never corrupts output")

- [x] 3.1 Point `init_tracing` at stderr in `src/cli.rs`
- [x] 3.2 Implement the `MakeWriter` that emits inside `MultiProgress::suspend` when a bar is registered, plain stderr otherwise; wire it into `init_tracing`
- [x] 3.3 Integration test: a run that provokes a `warn!` under `--json` still yields exactly one parseable JSON document on stdout, warning text on stderr

## 4. Transfer phase messages (design D2, spec "Progress names the operation and phase")

- [x] 4.1 Set `copying <name>` in the transfer loop and `verifying <name>` around the read-back in `src/transfer.rs`; keep `copy_and_hash`/`hash_file` message-unaware
- [x] 4.2 Unit test: message sequence over one transferred file is copying → verifying

## 5. Tesla scan progress (design D3, tesla-import delta)

- [x] 5.1 In `TeslaSource::scan`, count event folders (+ RecentClips files when enabled) up front, `set_length`, `inc(1)` + name message per unit, `finish()` before returning
- [x] 5.2 Unit test mirroring gopro's `scan_progress_reaches_total_chapter_count`: total equals discovered units, position reaches total, RecentClips units counted only when the category is enabled

## 6. Plan renderer (design D5, D6, spec "Scan produces a reviewable plan")

- [x] 6.1 Rework `render_plan` entry lines: group name, short-form recorded time in the configured timezone, file count + total size, resolved path; reason clause only for `Ignore`
- [x] 6.2 Replace default-mode quarantine suppression with the one-line rollup (count, aggregate size, quarantine root or disabled note); `-v` lists entries individually
- [x] 6.3 List unrecognized-group files: first 5 + "… and <x> more (-v to list all)" by default, all under `-v`; count in the entry line
- [x] 6.4 Move per-entry sidecar lines behind `verbose` (filename only)
- [x] 6.5 Extend the summary line with per-verdict file counts and byte totals
- [x] 6.6 Add `files: Vec<String>` to `PlanActionJson` (uncapped, every action); serialization test proving no truncation for an 8-file unrecognized group
- [x] 6.7 Rewrite `report.rs` plan unit tests for the new format (time/size shown, no boilerplate reason, rollup, cap behavior identical at ≤5 files)

## 7. Results renderer (design D7, spec "Human-readable execution report is summarized by default")

- [x] 7.1 Extract outcome tallying shared by `results_to_json` and the human renderer so counts cannot diverge
- [x] 7.2 Change `render_results` to `(report, verbose)`: default prints notable outcomes only (failed, suffixed, left-on-source, sidecar failures) plus the always-present summary line
- [x] 7.3 Name each group not deleted while deletion was in effect, with its reason
- [x] 7.4 Implement the `-v` grouped listing: group header with destination, indented per-file outcomes
- [x] 7.5 Thread `verbose` through `run_import` to `render_results`
- [x] 7.6 Unit tests: clean run renders one summary line; failure visible without `-v`; verbose grouping; human counts equal `results_to_json` counts on the same report

## 8. Import states intent (design D4, spec "Import states its plan before transferring")

- [x] 8.1 In `run_import` (non-dry-run, human mode) print the plan via `print_plan` before building the transfer bar; JSON mode unchanged
- [x] 8.2 Integration test: non-dry-run `import` stdout shows plan rendering before the execution report; `--json` still emits exactly one document

## 9. Instrumentation (design D9)

- [x] 9.1 Add `info!` milestones: source resolved, scan complete (group count), plan built (verdict counts), deletion decision
- [x] 9.2 Add `debug!` internals: per-chapter telemetry outcome, quick-match hit/miss with compared values, collision resolution
- [x] 9.3 Test (or assert via integration run) that `-v` emits INFO milestones on stderr that a default run does not

## 10. Docs and quality gates

- [x] 10.1 Learning note in `docs/learning/` on the `MakeWriter` trait + `OnceLock` global pattern (first custom tracing writer in the project)
- [x] 10.2 Update README/docs sample output if any shows the old plan or report format
- [x] 10.3 Quality gates: `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`
- [x] 10.4 `openspec validate improve-console-output` passes; scenarios in both delta specs each map to a test added above
