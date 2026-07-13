## Why

`source: auto` profiles resolve to a single drive today: `plan::resolve_source` walks `mount_roots`, finds the first mounted volume whose contents match the profile's device type, and returns it — any other matching drive is silently ignored, with no trace even at `-vv`. This is a real gap, not a hypothetical one: plugging in two GoPro cards via a USB hub (or a GoPro card plus a Tesla drive on their respective profiles run back to back) means one card is imported and the other vanishes from output entirely. The fix is to make `scan` and `import` process every matching drive in one invocation, naming each one so the user always knows which physical drive a plan or report belongs to.

## What Changes

- `plan::resolve_source` (single `Option<PathBuf>`, first-match-wins) becomes a multi-drive resolver that collects every matching mounted volume across all `mount_roots`, sorted by path for deterministic, reproducible ordering. Scope: `source: auto` profiles only — an explicit `--source <path>` or a profile with `source: <path>` remains single-drive and is unaffected by this change.
- `scan` loops over every detected drive, printing a name/path header before each drive's existing scan-summary rendering. Read-only, so there is no ordering or failure-handling concern beyond display.
- `import` processes drives sequentially, one fully at a time: scan → plan → print plan → confirmation prompt (today's existing delete-source prompt, unchanged in its own semantics) → execute → print report — repeated per drive, so the prompt fires once per drive rather than once per invocation.
- **BREAKING (JSON contract)**: `--json` output for `scan`/`import` against a `source: auto` profile changes shape from a single flat document to a document containing a `drives` array, one entry per detected drive (each carrying that drive's `name`, `path`, and its existing scan-summary/execution-report payload). The "exactly one JSON document per invocation" guarantee (cli-core, "Import executes exactly the scanned plan") is preserved — this only changes what that one document contains.
- A drive-level hard error (e.g. a device's `scan()` fails outright, not an individual file transfer failure) is caught per drive rather than propagating and aborting the whole run: it's recorded as that drive's outcome (`status: "error"`) and the batch continues to the next drive. Existing per-file soft failures (`TransferOutcome::Failed`) are unaffected — they already don't abort a run.
- Overall process exit code reflects failure if **any** drive had a hard error or any per-file failure, same aggregate-failure semantics as today's single-drive run, just widened across drives.
- No new detection mechanism: a drive's displayed name is its mount-point directory basename, exactly what `resolve_source` already reads today (this is how Linux automounters like udisks2 name a volume by its label) — no `lsblk`/`blkid`/udev dependency added.

## Capabilities

### New Capabilities
- `multi-drive-import`: detecting and sequentially processing every matching mounted drive for a `source: auto` profile in one `scan`/`import` invocation — drive enumeration and ordering, per-drive naming and display, per-drive confirmation, continue-past-a-failed-drive semantics, and the multi-drive JSON document shape.

### Modified Capabilities
- `cli-core`: "Scan produces a reviewable plan without side effects" and "Import executes exactly the scanned plan" currently assume one source per profile per run (e.g. "No sources found" scenario, the single-document JSON guarantee). These requirements need amending so "no sources" and "exactly one document" are stated correctly for zero-or-more drives rather than zero-or-one. "Source deletion only after verification" currently describes one confirmation prompt per `import` run; this needs amending to describe one prompt per drive.

## Impact

- `src/plan.rs`: `resolve_source` signature and behavior change (its two callers: `scan_profile` and `run_scan` in `src/lib.rs`).
- `src/lib.rs`: `run_scan` and `run_import` restructure their bodies around a per-drive loop instead of a single resolve/scan/plan/execute sequence; error handling for a single drive's failure moves from `?`-propagation to being caught and recorded so later drives still run.
- `src/report.rs`: new JSON view types wrapping today's `ScanSummaryJson`/`ResultsJson`/`PlanJson` in a `drives` array; human-readable rendering gains a per-drive name/path header, reusing existing `render_scan_summary`/`render_plan`/`render_results`.
- No config schema changes — `mount_roots` and `source: auto` already exist (ADR 0004); this changes what `resolve_source` does with them, not the config surface.
- No change to the scan → plan → execute safety model per drive (ADR 0003) or to `ImportSource`/device implementations (ADR 0005) — each drive still goes through the exact same single-drive pipeline that exists today, just invoked once per detected drive instead of once per run.
