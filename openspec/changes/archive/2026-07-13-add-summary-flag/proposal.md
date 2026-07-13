## Why

`scan`, `import`, and `cleanup` all print one line per group/entry by default (`render_plan`'s `[KEEP]` lines, `render_cleanup_plan`'s `[PURGE]`/`[KEEP]` lines). That's fine for a handful of sessions but unusable on a card with dozens to hundreds of them — the listing scrolls past faster than it can be read, and the only existing escape hatch is `--json`, which also kills progress bars and human formatting entirely. Users who just want to watch progress and confirm the outcome need a middle ground: progress bars plus a final tally, nothing per-entry.

## What Changes

- New global `--summary` flag, alongside the existing `--json`/`-v`/`--config`, applying to `scan`, `import`, and `cleanup`.
- `render_plan`/`render_scan_summary`: every verdict (not just `Quarantine`, which already rolls up) collapses to one rollup line (count, file count, size, shared destination/quarantine root); the unrecognized-files group collapses to a count with no names. The "`-v` to list" hints are dropped under `--summary` since `-v` no longer unlocks a listing.
- `render_results`: `Suffixed` (collision) and `SkippedQuarantineDisabled` per-file lines collapse into new counts on the summary line. `FAILED`, `SIDECAR FAILED`, and "not deleted from source" lines are unaffected — they stay individually listed regardless of `--summary`, since they're the actionable exceptions the flag is not meant to hide.
- `render_cleanup_plan` gains a rollup mode (purge count/size, keep count/size) in place of its current unconditional per-entry listing. `render_cleanup_report` gains a summary line it doesn't have today (deleted count/size, failed count) and collapses routine `deleted: <path>` lines under `--summary`; `FAILED to delete` lines stay individually listed.
- `--summary` and `-v` may be combined: `-v`'s effect on diagnostic log verbosity (stderr, via `tracing`) is independent of rendering and keeps working normally. `-v`'s effect on per-entry render detail is overridden off whenever `--summary` is set — `--summary -v` reads as "collapsed listing, plus parsing/diagnostic logs on stderr."
- `--summary` is a no-op under `--json`, which is already the maximally compressed, non-interactive mode.

No behavior changes to progress bars themselves — they continue to run under `--summary` exactly as they do today; the flag only affects what prints after they clear.

## Capabilities

### New Capabilities
(none — this is a reporting-detail change to existing commands, not a new capability)

### Modified Capabilities
- `cli-core`: scan/plan rendering, transfer-report rendering, and the `-v`/`--json` interaction rules gain a third `--summary` detail tier and its interaction with `-v`.
- `cli-maintenance`: cleanup's plan and post-execution report gain rollup/summary rendering they don't currently have.

## Impact

- `src/cli.rs`: new `--summary` global flag.
- `src/report.rs`: `render_plan`, `render_scan_summary`, `render_results` (+ `ResultsTally`/`summary_line`), `render_cleanup_plan`, `render_cleanup_report` (new tallying, since it has none today).
- `src/lib.rs`: thread the new flag through `run_scan`, `run_import_cycle`, `run_scan_cycle`, `run_cleanup`, and compute the effective render-verbose bool (`cli.verbose > 0 && !cli.summary`) at the call site so `report.rs` functions don't need to know about `-v` directly.
- Tests: `tests/cli_overrides.rs` (flag parsing), `tests/integration.rs` (rendering behavior, `--summary`/`-v` combination, cleanup rollup/summary).
