# Proposal: improve-console-output

## Why

The console output answers neither of the two questions a user actually has: *what is the tool doing right now* (progress bars carry no operation label, Tesla scans show no progress at all, warnings garble an active bar) and *what happened* (a full-card import prints hundreds of uniform per-file lines with no closing summary, burying the one `FAILED` line that matters). The `-v`/`-vv` flags exist but gate almost nothing: the report renderers largely ignore them and the codebase has five `warn!` calls and no `info!`/`debug!` at all.

## What Changes

- **Progress bars name their operation.** Both bars gain a phase prefix (`Scanning`, `Importing`) and per-file messages state the action (`copying <name>`, `verifying <name>` — phases introduced by the `single-pass-verified-transfer` change, which this change builds on).
- **Tesla scans report progress**, mirroring GoPro's determinate scan progress: one tick per event folder, folder name as the message.
- **`import` states its intent before transferring**: the plan summary (sessions, files, total size) is printed before the transfer bar starts, so a non-dry-run import is never silent about what it's about to do.
- **Plan output shows time and size instead of boilerplate.** Each entry carries its recorded time and file count/size; the constant "matches profile criteria" reason is dropped (reasons remain where they vary, i.e. `Ignore`). Quarantine gets a one-line rollup with aggregate size and target (it consumes disk when copied). The per-entry sidecar line moves to verbose. Unrecognized files are listed by name — first 5 with "… and x more" by default, all under `-v`.
- **Results output gains a summary and loses the wall of text.** Default shows only notable outcomes (failures, suffixed collisions, files left on source, sidecar failures, groups unexpectedly kept on source) plus a closing summary line with counts; `-v` shows every file, grouped per session with the destination hoisted to the group header.
- **Plan JSON gains a `files` array** per action — today not even `--json` can name an unrecognized file.
- **Diagnostics move to stderr and respect the bar.** `tracing` output currently goes to stdout, violating the JSON-mode contract ("no other stdout output") and garbling active progress bars; it moves to stderr and is routed around a live bar. `-v` additionally unlocks `info!` phase milestones, `-vv` unlocks `debug!` internals.

## Capabilities

### New Capabilities

(none — everything lands in existing capabilities)

### Modified Capabilities

- `cli-core`:
  - "Scan produces a reviewable plan without side effects" — plan entry format (time, sizes), quarantine rollup, sidecar demoted to verbose, unrecognized-file listing with the 5-entry cap.
  - "Import executes exactly the scanned plan" — pre-transfer plan summary requirement.
  - "Transfer progress is shown on interactive terminals" / "Scan progress is shown on interactive terminals" — progress output SHALL identify the running operation and current file/phase.
  - "Machine-readable JSON report output" — per-action `files` array; diagnostics SHALL NOT appear on stdout (compliance fix).
  - New requirement: human-readable execution report with summary line and verbosity gating.
  - New requirement: diagnostic logging levels (`-v`/`-vv`) on stderr, never interleaved with progress rendering.
- `tesla-import`: new requirement — scan progress reflects per-event-folder completion (counterpart of gopro-import's chapter-level progress requirement).

## Impact

- `src/progress.rs` (templates, prefix API), `src/report.rs` (both human renderers, plan JSON view-model), `src/lib.rs` (verbose threading to results, pre-transfer summary), `src/cli.rs` (`init_tracing` writer), `src/transfer.rs` (phase messages), `src/source/tesla.rs` (scan progress), `src/source/gopro.rs` (message wording, `info!`/`debug!` instrumentation).
- Tests: `report.rs` unit tests for both renderers; integration tests asserting stdout cleanliness in JSON mode gain coverage for warnings-on-stderr.
- JSON shape change is additive only (new `files` field); existing consumers unaffected.
- Depends on `single-pass-verified-transfer` for the `copying`/`verifying` phase distinction; implement that change first.
- Learning-project conventions apply (ADR 0001); no new ADR expected — presentation changes, no architectural decision.
