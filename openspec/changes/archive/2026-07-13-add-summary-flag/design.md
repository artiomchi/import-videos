## Context

`render_plan`/`render_scan_summary` (`src/report.rs`) print one line per `[KEEP]`/`[IGNORE]` group by default; only `Quarantine` already collapses to a rollup line (`render_quarantine_rollup`). `render_results` is already mostly summary-shaped by default — it lists only `Failed`, `Suffixed`, `SkippedQuarantineDisabled`, and "not deleted" outcomes individually, with everything else folded into `ResultsTally`'s closing `Summary:` line. `render_cleanup_plan` has no listing/rollup distinction at all today: it always prints every `[PURGE]`/`[KEEP]` entry. `render_cleanup_report` has no closing tally line at all — just `deleted: <path>` per entry.

`verbose` is a single `bool` threaded from `cli.verbose > 0` through every render call. It currently does two unrelated things depending on where it's read: gates `tracing`'s log level (`cli::init_tracing`, independent of rendering) and gates render-level per-entry detail (`report.rs`). This proposal needs to split those two effects apart so `--summary --verbose` can mean "collapsed rendering, but louder diagnostics."

## Goals / Non-Goals

**Goals:**
- A `--summary` flag, global like `--json`, that collapses per-entry/per-group listings on `scan`, `import`, and `cleanup` to progress bars + a closing tally, while keeping actionable exceptions (`FAILED`, `SIDECAR FAILED`, `FAILED to delete`, "not deleted from source") individually visible.
- `--summary` and `-v` combine: `-v`'s log-level effect keeps working; its render-detail effect is overridden off whenever `--summary` is set.
- No information silently disappears — every count `--summary` removes from the listing must still be recoverable from a tally line.

**Non-Goals:**
- No change to progress bar behavior, templates, or `--json` output (already the maximally compressed mode; `--summary` is a no-op there).
- Not introducing a fourth detail tier or per-flag knobs for individual sections — three tiers (`Summary` < `Normal` < `Verbose`) cover every call site in scope.
- Not touching `inspect` (outside the scan → plan → execute / cleanup surface).
- Not retrofitting a closing tally onto `render_cleanup_report`'s *default* mode — only `--summary` mode gains one (see Open Questions).

## Decisions

### 1. Replace the `verbose: bool` parameter with a `Detail` enum in `report.rs`

```rust
pub enum Detail { Summary, Normal, Verbose }
```

Computed once in `lib.rs`, next to today's `let verbose = cli.verbose > 0;`:

```rust
let detail = if cli.summary {
    report::Detail::Summary
} else if cli.verbose > 0 {
    report::Detail::Verbose
} else {
    report::Detail::Normal
};
```

**Alternatives considered**: two independent bools (`verbose`, `summary`) passed alongside each other. Rejected — it makes the illegal state `(verbose: true, summary: true)` representable at every call site and in every test, relying on a lib.rs-only invariant that nothing downstream can check. The enum makes that state unconstructable, which is worth the one-time mechanical churn of updating existing `render_plan(&plan, true, &tz)`-style call sites (compiler-checked, not runtime-checked). This is also the more idiomatic Rust shape, which matters for this project's learning-project framing (ADR 0001).

`Detail` is `Copy`, defined in `report.rs` (it's a rendering concern, not a CLI-parsing one) and re-exported for `lib.rs` to construct.

### 2. `render_plan` / `render_scan_summary` under `Detail::Summary`: skip the per-action loop's output entirely

Every existing per-verdict clause the closing `Summary: {totals.render()}` line already carries (kept/quarantined/ignored group count, file count, byte total) is already sufficient — there's no need to invent a parallel set of per-verdict rollup lines (that would just duplicate the same numbers in a second format). So `Detail::Summary` means: still iterate `plan.actions` to accumulate `VerdictTotals`, but never call `render_plan_entry` or `render_quarantine_rollup`. Output is the "nothing found" special case (unaffected) or exactly one `Summary: ...` line.

**Trade-off accepted**: the destination/quarantine root path (`-> /library/...`) disappears in this mode, since it currently rides along on the per-entry/rollup lines this mode suppresses. Computing a synthetic "common ancestor of every Keep destination this run" was considered and rejected as unwarranted complexity for a deliberately terse mode — a user who wants the path drops `--summary`.

The "`-v` to list" / "`-v` to list all" hint text is dropped whenever `Detail::Summary`, since `-v` no longer unlocks a listing in that mode (avoids printing a hint that lies about what `-v` will do).

### 3. `render_results` under `Detail::Summary`: keep exceptions, collapse `Suffixed`/`SkippedQuarantineDisabled` into new tally fields

Unlike `render_plan`, the closing `Summary:` line does *not* already account for `Suffixed` (collision) or `SkippedQuarantineDisabled` counts — `ResultsTally` computes them (`report.rs:437-438`) but `summary_line` never reads them. Suppressing their per-file lines under `--summary` without surfacing the counts would silently drop information, which violates the goal above. So `summary_line` gains two new optional trailing clauses, appended only when `Detail::Summary` is active and the corresponding count is nonzero (e.g. `, 3 renamed (collision), 2 left on source`) — default/verbose output is untouched, so no existing pinned-string test changes behavior it didn't ask for.

`Failed`, `SIDECAR FAILED`, and "not deleted from source" lines stay unconditional in every `Detail` value — they're the one thing this flag is explicitly not meant to hide.

### 4. `render_cleanup_plan` under `Detail::Summary`: skip the per-entry loop, keep the existing header + `Summary:` line

`render_cleanup_plan` already closes with `Summary: {purge_count} to purge (...), {keep_count} kept (...)` — structurally identical to the `render_plan` case (Decision 2), so the same "just suppress the loop" treatment applies with no new tallying needed. This function currently takes no verbose/detail parameter at all (it always lists); it gains a `Detail` parameter of its own — plumbed the same way as `render_plan`'s.

### 5. `render_cleanup_report` under `Detail::Summary`: new tally line (net-new, since none exists today)

`render_cleanup_report` currently has no closing summary line — a clean cleanup run prints nothing but `deleted: <path>` per entry. Suppressing those lines under `Detail::Summary` with no replacement would mean a successful summarized cleanup prints *nothing at all* (aside from `FAILED to delete` lines, if any) — worse than confusing, it looks like the command did nothing. So this function gains a tally (deleted count + size, failed count) emitted only in `Detail::Summary`; `FAILED to delete` lines stay unconditional. Default mode's lack of a closing line is unchanged (see Open Questions).

### 6. `--summary` is an unconstrained global flag, not `conflicts_with`-gated against `--json`

`--json` branches in `lib.rs` never read `cli.summary`/`Detail` at all — they're independent code paths already. No clap-level conflict is needed; `--summary --json` simply has `--summary` silently ignored, consistent with `-v --json` today (verbosity flags don't affect JSON output either).

### 7. Defense in depth: `debug_assert!` is unnecessary now that illegal states are unrepresentable

Called out explicitly because Decision 1 was chosen partly *to avoid* needing this: there's no `(bool, bool)` pair to validate anymore, so no assertion is needed at each render function's entry.

## Risks / Trade-offs

- **Signature churn**: switching `verbose: bool` → `detail: Detail` touches every call site in `report.rs` and `lib.rs`, plus every existing test that currently passes a literal `true`/`false` for `verbose`. Mitigation: mechanical, compiler-driven — every call site that needs updating fails to compile until it is.
- **Asymmetric tallying**: `render_cleanup_report` gains a summary line only in `Detail::Summary`; default mode still ends with no closing tally, an existing inconsistency with `render_results` this change doesn't fully close. Accepted as out of scope (see Open Questions) rather than scope-creeping into fixing default-mode cleanup output.
- **No path in `Detail::Summary` plan output**: accepted per Decision 2's trade-off — a deliberately terse mode losing the one piece of information (destination path) that's genuinely per-entry rather than aggregable.

## Migration Plan

Purely additive: a new flag and a new enum variant space, no data/config migration. Rollback is "don't pass `--summary`" — default behavior (`Detail::Normal`/`Detail::Verbose`) is unchanged bit-for-bit by this change, so no version gate or feature flag beyond the CLI arg itself is needed.

## Open Questions

- Should `render_cleanup_report` also grow a closing tally line in default (non-summary) mode, fixing the pre-existing asymmetry with `render_results`, or is that a separate change? Leaning toward leaving it out of this change's scope.
- Exact wording for `render_results`'s two new summary clauses (`renamed (collision)` / `left on source`) — placeholder text above, to be finalized when writing specs.
