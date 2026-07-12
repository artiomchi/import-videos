## Context

`cli-core`, `gopro-import`, and `gopro-telemetry` are implemented and green. `transfer::Progress` (`src/transfer.rs`) already solves this exact problem for the copy phase: an `Option<ProgressBar>` wrapper whose methods are no-ops when disabled, constructed once by `run_import` (`src/lib.rs`) from a TTY/`--json` check, never re-derived downstream.

`GoproSource::scan` (`src/source/gopro.rs`) is the slow, silent phase this change targets. Per session, `build_session` does, in order:

1. `chapter_civil_time` over every chapter (cheap — MP4 box-header walk, bytes read scale with box count, not file size).
2. `open_chapter_telemetry` over every chapter (cheap — same box-walk, plus a `gpmd` sample-table read that scales with telemetry sample count, ~1/sec, not file size).
3. `derive_session_offset`: iterates chapters *in order*, decoding GPMF payloads in each until one yields a usable GPS fix, then stops. This is the one unbounded step — worst case (GPS never locks) decodes every telemetry payload of every chapter in the session before giving up.
4. A marker-extraction loop over every chapter (cheap — bounded by marker count, a handful of payload decodes each).

So the wall-clock cost is concentrated in step 3, and step 3 does not necessarily visit every chapter (it stops at the first fix). A naive "tick once per chapter, anywhere in `build_session`" would either miss the part that actually takes time (if ticked in step 1/2/4) or land short of the true total (if ticked only in step 3, since it can exit early).

## Goals / Non-Goals

**Goals:**

- A determinate progress bar during `scan` (and during `import`'s scan phase) that reaches 100% exactly at `discover()`'s chapter count, with no second pass needed to compute the total.
- The bar's motion tracks where wall time is actually spent (step 3), not just "a chapter was touched."
- Progress plumbing lives where any current or future `ImportSource` can use it, without imposing cost on implementations that don't (`TeslaSource`).
- Same visibility policy as transfer progress: TTY and not `--json`, decided once per command.

**Non-Goals:**

- Reducing scan *cost* — the three-separate-`File::open`-per-chapter redundancy (`chapter_civil_time`, `open_chapter_telemetry`, `chapter_markers`) is real but orthogonal; noted in the proposal as a follow-up, not addressed here.
- `TeslaSource` progress reporting — its scan is directory/JSON reads, not worth instrumenting today; the plumbing is available to it for free if that changes.
- A `MultiProgress` or any concurrent-bars setup — scan and transfer bars are strictly sequential.

## Decisions

### D1: Progress reporter lives on `ScanContext`, not the `scan()` signature

`ScanContext` (`src/source/mod.rs`) gains a `progress: &'a Progress` field, alongside `ignore`, `tz`, and `imported_at` — the established place core hands cross-cutting, run-scoped concerns down into device `scan()` implementations. `TeslaSource::scan` already destructures the fields it wants out of `ctx`; it simply never reads `.progress`.

*Alternative — add a parameter to `ImportSource::scan`*: rejected. It forces every implementor (`GoproSource`, `TeslaSource`, `GenericSource` in `src/source/mod.rs`, plus the `StubSource` test double in `src/plan.rs`) to accept and thread a parameter most don't use. A `ScanContext` field only touches construction sites.

*Alternative — keep it fully internal to `gopro.rs`, constructing its own `ProgressBar` and TTY check*: rejected. It would duplicate the enable/hidden decision `run_scan`/`run_import` already make once, and risks drifting from the `--json` gating policy (D6 in `add-core-cli`'s design) that every other piece of output obeys.

### D2: `Progress` relocates to `src/progress.rs`; one more constructor, not a new type

`Progress` currently lives in `src/transfer.rs`, which imports from `src/source.rs` (`use crate::source::{Sidecar, Verdict}`). If `src/source/gopro.rs` needs `Progress` too, importing it from `transfer` would invert that dependency direction. Moving the type to a standalone `src/progress.rs` (no dependents in either direction) fixes this and gives progress a home that isn't tied to the transfer phase.

The existing `Progress` API (`set_length`, `inc`, `set_message`, `finish`, and the `#[cfg(test)] position()` used by transfer's tests) is already unit-agnostic — only the `ProgressStyle` template string (`{bytes}/{total_bytes}` vs. a count form like `{pos}/{len}`) differs between byte- and count-oriented use. So: keep one `Progress` struct, add `Progress::counted(enabled: bool)` next to the existing `Progress::new(enabled: bool)`, differing only in which template `set_style` applies. `hidden()` stays shared.

Two mechanical consequences of the move, easy to miss: `set_length`/`set_message`/`inc`/`finish`/`position` are currently private (module-private to `transfer.rs`) — they need `pub(crate)` visibility so `src/source/gopro.rs` (and anything cross-module asserting tick counts in tests) can call them. And `transfer::Progress::hidden()`/`Progress::new()` are referenced by path today in `src/lib.rs:201` and at every `Progress::hidden()` call site across `tests/integration.rs` (13 of them) — these update to the new `crate::progress::Progress` path as part of the move, rather than leaving a `pub use` re-export in `transfer.rs` (the codebase's convention is to change call sites, not grow compatibility shims).

*Alternative — a separate `ScanProgress` type*: rejected; it would duplicate the `Option<ProgressBar>`-wrapping, no-op-when-disabled shape for no semantic gain, and double the tests that pin "hidden mode never constructs a bar."

*Alternative — genericize over a unit type*: rejected as unneeded machinery for two template strings in a single-crate tool (ADR 0005's general bias against speculative abstraction applies here too).

### D3: Tick placement — every chapter ticks exactly once, weighted toward where the cost is

`derive_session_offset` (`src/source/gopro.rs:364`) takes an added `progress: &Progress` parameter (it's a free function, not a method — `ctx` belongs to `build_session`, not to `self`, so there is no method-on-`GoproSource` alternative that avoids passing something in). It ticks **unconditionally as the first statement of each loop iteration**, before the `telemetry[i].as_mut()` guard and before any of the loop's `continue`s (missing telemetry, a parse error, no fix, a fix payload without a usable sample) — every iteration the loop *executes* counts as one visited chapter, whether or not that chapter turns out to have telemetry or a usable fix. `set_message` on the same tick carries the session id and that chapter's file name, so the bar reads as active during exactly the part that's slow (telemetry itself is already open by this point — `build_session`'s earlier `open_chapter_telemetry` map — so "visits" here means "examines," not "opens").

`derive_session_offset`'s return type becomes `(Option<SessionTelemetry>, usize)`: the second element is the number of loop iterations executed — `i + 1` when it returns early on a fix at index `i`, or `chapters.len()` when the loop runs to exhaustion without one. This is exactly the tick count already emitted, by construction (one tick per iteration, one iteration per unit of the returned count) — there is no separate index to track and no room for the count to drift from the ticks.

Because the loop can return early, `build_session` ticks once more, in a single `inc(chapters.len() - visited)` call right after `derive_session_offset` returns, for the chapters it never reached. (Design intent only — whether this batched increment sits textually before or inside the subsequent marker-extraction loop is an implementation detail; either place is total-correct as long as it fires exactly once per session, using the returned `visited` count directly rather than a recomputed index.)

Net effect: the total always lands exactly on `discover()`'s chapter count; chapters actually examined during the GPS search advance the bar in near-real-time; chapters never reached (because a fix was already found, or the session has no telemetry at all) advance in one fast catch-up burst immediately after.

*Alternative — tick once per chapter in the cheap, always-complete loop (civil time or marker extraction) only*: rejected — this was the original plan and it's the "bar freezes, then jumps" failure mode described above.

*Alternative — tick per session instead of per chapter*: rejected — sessions vary widely in chapter count (and therefore in wall time), so a per-session bar would move in uneven, misleading jumps; per-chapter is the unit `discover()` already counts for free.

### D4: Progress construction moves upstream of `scan_profile`

`run_import` (`src/lib.rs`) currently constructs `transfer::Progress` *after* `scan_profile` returns (`lib.rs:188` builds the plan, `:201` builds the bar). For the scan phase to have progress, the enabled/hidden decision has to be made before `scan_profile` runs and threaded into the `ScanContext` it builds inside `plan::build_plan` (`src/plan.rs:94`). Both `run_scan` and `run_import` compute the same TTY/`--json` boolean today for their own bars; this change makes `scan_profile` (or its caller) compute it once and use it for the scan-phase `Progress`, then `run_import` separately constructs its transfer-phase `Progress` (a `Progress::new`, byte-oriented) afterward as it does today.

### D5: Scan and transfer bars are sequential, never concurrent

The scan-phase bar finishes and clears (`finish_and_clear`, the existing precedent in `transfer.rs`) before `scan_profile` returns its plan — by the time `print_plan` or the transfer bar appears, nothing is left on screen from scanning. `import`'s two bars (scan, then transfer) appear one after another; no `MultiProgress` is needed since they never overlap in time.

## Risks / Trade-offs

- **[Tick bookkeeping adds a small amount of state to `build_session`]** Tracking "which chapter index `derive_session_offset` stopped at" couples two functions that were previously independent. → Keep it explicit: `derive_session_offset` returns the count of chapters it visited alongside its existing `Option<SessionTelemetry>`, rather than reaching into shared mutable state.
- **[Progress ticks inside a hot parsing loop]** `Progress::inc`/`set_message` on a *hidden* `Progress` are no-ops (existing behavior), so non-interactive runs (the common case for `--json` or piped/CI use) pay no real cost. On interactive runs the redraw rate is bounded by indicatif's own internal throttling, same as the transfer bar today.
- **[Moving `Progress` changes its import path]** Every existing `transfer.rs` reference to `Progress` needs updating to the new module. → Mechanical; `cargo build` surfaces every call site.

## Migration Plan

Purely additive — no config or data migration. Existing profiles and both commands (`scan`, `import`) behave identically except for the new progress output when interactive.

## Open Questions

- Should very small scans (a handful of chapters) suppress the bar entirely to avoid a flash-then-clear that adds more noise than signal? Deferred — indicatif already renders quickly enough that this is likely a non-issue in practice; revisit if it reads as flickery once built.
