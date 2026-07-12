# Design: improve-console-output

## Context

All human output flows through two seams: `Progress` (src/progress.rs), a thin `Option<ProgressBar>` wrapper whose enabled/hidden state is decided once per command, and `report.rs`, pure render functions kept out of `lib.rs` so formatting is unit-testable. Both seams are sound; the problems are what flows through them. Bar templates carry no operation label, `TeslaSource::scan` never touches `ctx.progress`, `render_results` ignores verbosity and prints no summary, and `init_tracing` writes to stdout — which violates the JSON-mode contract ("no other stdout output") and garbles a live bar.

This change is presentation-only. It builds on `single-pass-verified-transfer` (implement that first), which turns the transfer phases into copy → read-back verify.

## Goals / Non-Goals

**Goals:**
- Every visible bar names its operation and current file/phase.
- Both scan implementations report determinate progress.
- Default output after any command fits one glance; `-v` is the exhaustive view; the two never disagree on counts.
- Diagnostics on stderr, never corrupting stdout documents or bar rendering.

**Non-Goals:**
- No color/styling work, no new CLI flags (no `--quiet`; default *is* quiet), no localization.
- No JSON shape changes beyond the additive `files` array.
- No transfer/verification semantics — that is `single-pass-verified-transfer` (ADR 0012).

## Decisions

### D1: Phase labels via `{prefix}`, not message text

Both templates gain a leading `{prefix:.bold}`; constructors take the label (`Progress::counted(enabled, "Scanning")`, `Progress::new(enabled, "Importing")`). The prefix is set once at construction — call sites keep setting only the per-item message. Alternative rejected: baking the phase into every `set_message` call repeats the label at N call sites and couples device modules to presentation wording.

### D2: Per-file action messages set at the transfer loop, not inside I/O helpers

`execute_inner` sets `copying <name>`; the read-back step sets `verifying <name>`. `copy_and_hash`/`hash_file` stay message-unaware — they tick bytes only. The verify read-back is *not* added to the bar's byte length: it targets the fast destination disk, and a bar that counts source bytes finishes at 100% exactly when the slow medium is done. The `verifying` message explains the brief tail instead.

### D3: Tesla scan progress ticks per event unit

Total = Saved/Sentry event folders (+ RecentClips files when that category is enabled), known from cheap directory listings before any `event.json` parsing. One `inc(1)` per unit, message = folder (or file) name, `finish()` at the end — the exact shape of GoPro's chapter-level progress. No `ScanContext` changes needed; the plumbing already exists.

### D4: `import` prints the plan before executing it

Non-dry-run `import` renders the same plan output `scan` would (honoring `-v`) before the transfer bar starts, then the execution report after. This reuses the existing renderer rather than inventing a separate "Importing N files…" preamble, and it makes `scan` / `--dry-run` / `import` read identically up to the point of execution. JSON mode is unchanged: still exactly one document (the execution report) — the spec's one-document contract wins over symmetry there.

### D5: Plan lines carry time and size; fixed-string reasons disappear

Entry format: `[KEEP] <name>  <YYYY-MM-DD HH:MM>  <n> files, <size> -> <dest>`. The recorded time renders short-form in the configured timezone (full RFC 3339 stays in `-v`'s `recorded at:` line and in JSON). The `— reason` clause remains only where it varies (`Ignore`). Per-entry sidecar lines move to `-v` (filename only; the directory is the entry's path). Quarantine collapses to one default-mode rollup line — `Quarantine: <n> sessions (<size>) -> <root>  (-v to list)` — because its aggregate size is a disk-consumption decision input, while its individual names are not. Summary gains byte totals. Group size is `files.iter().map(|f| f.size).sum()`; `format_size` already exists.

### D6: Unrecognized files listed by name, capped at 5

Default shows the first 5 file paths (source-relative) then `… and <x> more (-v to list all)`; `-v` lists all; 5 or fewer renders identically in both modes. The count moves into the entry line itself. Rationale: unrecognized files are heterogeneous — the names *are* the information — unlike quarantined sessions where the count is. JSON: `PlanActionJson` gains `files: Vec<String>` for every action (uncapped, additive).

### D7: Results default to notable-only plus an always-present summary

`render_results(report, verbose)` gains the parameter. Notable (always shown): `Failed`, `Suffixed`, `SkippedQuarantineDisabled`, sidecar failures, and any group *not* deleted when deletion was in effect (named, with reason) — the surprising cases. Routine (`Transferred`, `SkippedIdentical`, `SkippedQuickMatch`) is counted only. Summary line mirrors the plan's: `Summary: <n> transferred, <n> skipped (already imported), <n> FAILED[, <n> groups deleted from source]`. Under `-v`, files render grouped per session with the destination hoisted to a group header — correlating with the plan output the user just read. Counters already exist in `results_to_json`; extract the tallying so both views share it.

### D8: Diagnostics: stderr writer + a process-global bar registry for suspension

`init_tracing` gains `.with_writer(std::io::stderr)` — that alone fixes the JSON contract violation. For bar coexistence, `progress.rs` keeps a `OnceLock<MultiProgress>`: bars register with it on construction, and `init_tracing` installs a `MakeWriter` that emits inside `MultiProgress::suspend`, so a log line temporarily clears the bar, prints, and redraws. Alternatives: the `tracing-indicatif` crate (rejected — a dependency for ~20 lines, and hand-rolling `MakeWriter` + `OnceLock` fits the learning charter, ADR 0001); accepting garbling (rejected — warnings fire mid-scan, exactly when a bar is up). The global is confined to `progress.rs`; nothing outside it knows the registry exists.

### D9: `-v` levels get content

Level mapping stays (WARN / INFO / DEBUG). Add `info!` phase milestones (source resolved, scan complete with group count, plan built, deletion decision) and `debug!` internals (per-chapter telemetry outcome, quick-match hit/miss with the compared values, collision resolution). User-facing reports stay on `println!` per AGENTS.md — tracing is diagnostics, not output.

## Risks / Trade-offs

- [Output-format churn breaks string-asserting tests] → `report.rs` unit tests are rewritten alongside the renderers; integration tests assert via `--json` where possible, and the JSON shape is unchanged except the additive `files` field.
- [Notable-only default hides a line someone greps for] → the summary always carries exact counts, `-v` restores every line, and `--json` was always complete; the failure mode is inconvenience, not information loss.
- [Global `MultiProgress` registry is hidden state] → confined to `progress.rs` behind the existing `Progress` facade; tests exercise it through the same public constructors.
- [Short-form times in the plan could be mistaken for UTC] → they render in the configured timezone like every other timestamp (timezone-rendering spec); `-v` and JSON keep the offset-bearing RFC 3339 form.
- [Sequencing: phase messages assume the single-pass transfer flow] → implement `single-pass-verified-transfer` first; its tasks land the `verifying` read-back this change labels.

## Open Questions

(none — presentation decisions were settled in review discussion; wording of individual lines may still drift during implementation without spec impact)
