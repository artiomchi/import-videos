## Why

`GoproSource::scan` (`src/source/gopro.rs`) walks every chapter file on a card — MP4 box metadata for the camera-clock timestamp and HiLight markers, plus a GPMF telemetry search for the session's GPS offset — before `scan`/`import` prints anything. On a full card this can take a perceptible amount of wall time with zero terminal output, which reads as a hang. `import`'s transfer phase already solves this exact problem for the copy step (`Requirement: Transfer progress is shown on interactive terminals`, `cli-core`); scanning has no equivalent.

## What Changes

- `ScanContext` (`src/source/mod.rs`) gains a progress reporter, available to any `ImportSource` implementation the way `ignore`/`tz`/`imported_at` already are. Devices that don't need it (`TeslaSource`) simply never read the field.
- `transfer::Progress` moves to its own `src/progress.rs` (today `source/` would have to import from `transfer.rs`, inverting the crate's existing dependency direction) and gains a count-oriented construction path alongside the existing byte-oriented one, sharing the same enable/hidden/TTY-gating logic and `Option<ProgressBar>`-wrapping no-op-when-disabled shape.
- `GoproSource::scan` reports a **determinate**, per-chapter progress bar. The total comes from the chapter count `discover()` already produces before any per-file parsing starts, so no extra pass is needed. The tick is placed inside `derive_session_offset`'s per-chapter GPS-fix search — the one place scan cost is actually unbounded (worst case: every telemetry sample in every chapter of a session, when GPS never locks). The cheap, bounded work (camera-clock time, HiLight offsets, marker-coordinate lookups) rides along without its own tick; the bar's motion should track where the wall time actually goes, not just "a file was touched."
- Progress visibility is gated identically to transfer progress — interactive TTY and `--json` not set — decided once per command and threaded down through `ScanContext` construction, never re-derived inside device code (mirrors design D6's existing policy for transfer progress).
- Both `scan` and `import` show the scan-phase bar (both call `build_plan`, which calls `ImportSource::scan`). In `import`, the scan bar finishes and clears before the transfer bar appears — sequential, never overlapping, no `MultiProgress` needed.

## Capabilities

### New Capabilities
<!-- None. This extends existing capabilities. -->

### Modified Capabilities
- `cli-core`: broadens progress visibility to cover the scan phase (new requirement alongside the existing transfer-progress one) and establishes that `ScanContext` carries a progress reporter every `ImportSource` implementation may use.
- `gopro-import`: `GoproSource::scan` reports per-chapter progress while building sessions, concentrated on the GPS-offset search where scan cost is unbounded.

## Impact

- **`src/source/mod.rs`**: `ScanContext` gains a progress field.
- **`src/progress.rs`** (new): `Progress` relocated from `src/transfer.rs`; adds a count-oriented style/constructor next to the existing byte-oriented one.
- **`src/transfer.rs`**: uses `Progress` from its new home; the type definition moves out.
- **`src/source/gopro.rs`**: `scan`/`build_session`/`derive_session_offset` report progress; total length comes from `discover()`'s existing chapter count.
- **`src/source/tesla.rs`**: unaffected — ignores the new `ScanContext` field.
- **`src/plan.rs`, `src/lib.rs`**: `ScanContext` construction and progress-visibility gating move upstream of `scan_profile` (today `run_import` builds `transfer::Progress` only after `scan_profile` returns) so both `scan` and `import` get scan-phase progress under the same TTY/`--json` policy.
- **Tests**: a scan-progress "hidden mode constructs no bar" test mirroring `transfer.rs`'s existing one; a `discover()`-based total-count assertion; no change to GPS-offset derivation behavior, only where it reports.

Out of scope: `chapter_civil_time`, `open_chapter_telemetry`, and `chapter_markers` each independently `File::open` the same chapter file — a real but separate performance question (three opens/box-walks instead of one) that a progress bar makes legible without shrinking. Worth a follow-up change; not bundled here.
