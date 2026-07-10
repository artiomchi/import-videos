# Design — add-maintenance-commands

## Context

The pipeline (scan → plan → execute, ADR 0003) is complete for both devices; this
changeset adds the operational tail promised in the roadmap: `cleanup`, `inspect`,
`--json` output, and transfer progress. Relevant current state:

- `run_inner` (`src/lib.rs`) loads the config before dispatching any command.
  `inspect` must work on a bare file with no config present.
- Quarantine paths resolve in `plan.rs`: `profile.quarantine` or
  `{destination}/_quarantine`, one subdirectory per group name.
- Imported file mtimes are deliberately stamped to *recording* time (unified
  timestamps changeset / ADR 0008), not import time — this matters for how
  `cleanup --older-than` measures age.
- Report rendering (`report.rs`) is string-building over `ImportPlan` /
  `ExecuteReport`, unit-tested; `lib.rs` owns the `println!` call sites.
- The confirmation prompt (`confirm_deletion`) is private to `transfer.rs`.
- `indicatif` was planned from day one (roadmap dependency table) but never added.

## Goals / Non-Goals

**Goals:**
- `cleanup PROFILE [--older-than <span>] [--dry-run] [--yes]` — quarantine purge
  under the same plan/confirm/execute discipline as `import`.
- `inspect FILE` — one-file metadata dump (GoPro MP4 or Tesla event), zero config.
- `--json` on all subcommands: stable, machine-readable plan/report/inspect output.
- Byte-level progress during transfers, invisible to pipes and JSON consumers.
- README completed; learning-notes index tidied.

**Non-Goals:**
- No new import behavior, config schema changes, or sidecar format changes.
- No cleanup of *destination* footage — only the quarantine directory, ever.
- No `--json` stability guarantee across versions yet (document as "v0, may
  evolve"); no JSON schema files.
- No progress for scan/plan phases (they're metadata-speed, not byte-bound).

## Decisions

### D1. `cleanup` is its own plan/execute module (`src/cleanup.rs`)

A `CleanupPlan` (entries: path, group name, age, total size) is built read-only,
rendered like an import plan, and executed only after confirmation — the ADR 0003
split applied to deletion. Quarantine root resolution reuses the same rule as
`plan.rs` (extracted to one shared function on `Profile` so the two can't drift).
Entries are the immediate children of the quarantine root (one dir per group, per
the import layout); stray loose files are listed and removed too.

*Alternative considered:* folding purge into `transfer.rs`. Rejected — transfer is
about copy-verify-delete of planned imports; a standalone module keeps the
destructive surface small and separately testable.

### D2. `--older-than` age = time since the entry landed in quarantine

Age is measured from the **group directory's own mtime** (set when files were
copied in), *not* the files' mtimes — those are stamped to recording time, so a
commute recorded in March but quarantined yesterday would otherwise look
months old and be purged by `--older-than 30d` immediately. "Older than" must mean
"has sat in quarantine unreviewed for that long."

For stray loose files (not in a group dir) the file's own mtime is the only signal
available; documented as such. This is a footage-deletion policy decision →
**ADR 0010** lands with this changeset.

*Alternative considered:* newest file mtime inside the group. Rejected — wrong
semantics per above, and a directory scan per entry for no benefit.

### D3. `--older-than` parses via jiff's friendly span format

`"30d"`, `"2w"`, `"1mo"` already parse as `jiff::Span` (friendly format); the span
is subtracted from now in the configured timezone. No hand-rolled duration parser,
no new dependency. Calendar units (months) resolve correctly via jiff's zoned
arithmetic instead of a fake "30 days = 1 month".

### D4. `--json` is a global flag; JSON view-models live in `report.rs`

One global `--json` on `Cli` (like `--config`/`-v`) rather than per-subcommand
flags — every subcommand has a meaningful JSON answer, including `inspect`.

Serialization uses **dedicated `serde::Serialize` view-model structs in
`report.rs`** (e.g. `PlanJson`, `ResultsJson`, `CleanupJson`, `InspectJson`)
mapped from the domain types, rather than deriving `Serialize` on
`ImportPlan`/`ExecuteReport` directly. The JSON output is a public contract;
deriving on domain types would let internal refactors silently change it, and
domain types stay free of output-format concerns. Timestamps render as RFC 3339
strings in the configured timezone (matching the human output), paths as strings.

Behavior under `--json`: the JSON document is the *only* stdout output; progress
bars and informational lines are suppressed; errors still go to stderr; exit codes
unchanged. Confirmation prompts still apply — `--json` does not imply `--yes`
(non-interactive callers already need `--yes` per the existing spec).

*Alternative considered:* `--format human|json`. Rejected — YAGNI; a bool flag is
the whole requirement, and clap can migrate it later without breaking `--json`.

### D5. `inspect` needs no profile and no config

`inspect FILE` dispatches on the argument itself:
- `.mp4` file → MP4 path: HiLight offsets (`read_hilights`), creation time
  (`read_creation_time`), and if a `gpmd` track exists, a GPS summary (first fix,
  clock offset vs. creation time, sample count) via `read_gpmd_index` + GPMF parse.
- Directory containing `event.json`, or an `event.json` path → Tesla event dump
  (parsed fields + list of clip files present).
- Anything else → usage error (exit 2).

Because `run_inner` currently loads config unconditionally, config loading moves
into the command arms that need it; `inspect` renders timestamps in the system
timezone (`TimeZone::system()`), falling back to UTC. All parser use is read-only;
parse failures print what *was* readable plus the error, exit 1 — it's a debugging
tool, partial output is the point.

### D6. Progress: indicatif inside `transfer.rs`, behind a small wrapper

`transfer.rs` gets a thin `Progress` wrapper owning an `Option<ProgressBar>`;
`execute` takes it as a parameter. The CLI layer constructs it: real bar (bytes
style, per-file message) when stdout is a TTY and `--json` is absent, hidden
(`ProgressDrawTarget::hidden()`) otherwise. The copy loop in `transfer_file`
already reads in chunks for blake3 hashing — it ticks the bar per chunk; the
verify (re-hash) pass ticks a second phase on the same bar.

*Alternative considered:* a `ProgressSink` trait to keep indicatif out of the
library entirely. Rejected — indicatif is UI-agnostic enough (hidden target is a
no-op), the trait adds a layer with exactly one real implementation, and the
wrapper already gives tests a no-op path.

### D7. Confirmation prompt is shared, not duplicated

`confirm_deletion` generalizes to a `confirm(prompt, assume_yes, is_tty)` helper
(likely `transfer::confirm`, re-used by `cleanup`) keeping the existing spec
behavior: non-TTY without `--yes` fails rather than blocking; declined prompt
aborts execution cleanly.

## Risks / Trade-offs

- [Cleanup deletes footage by design] → Same mitigations as import deletion:
  plan/confirm/execute split, `--dry-run`, `--yes` required non-interactively,
  integration tests over tempfile quarantine layouts (per AGENTS.md: anything
  destructive gets an integration test). Cleanup only ever touches the resolved
  quarantine root — a sanity check refuses to run if the resolved root equals or
  contains the profile destination root.
- [Group-dir mtime is not a perfect "arrival" clock] → A re-import that adds a
  file to an existing quarantine group refreshes the dir mtime, resetting its age.
  Acceptable: it errs toward *keeping* footage longer, never purging sooner.
- [JSON contract can rot as features land] → View-models centralize the surface in
  `report.rs`; snapshot-style unit tests pin the shape; README documents fields.
- [indicatif enters the library crate] → Accepted consciously (D6): hidden draw
  target keeps tests and pipes clean; if a second UI ever appears, promote the
  wrapper to a trait then.
- [`Span` friendly parsing accepts more than we document] → Docs/help text show
  `30d`-style examples; anything jiff accepts is fine — no need to restrict.

## Open Questions

None blocking. Two small calls deferred to implementation, to be settled in specs
if they turn out to be spec-visible:
- Whether `cleanup` with an empty quarantine prints "nothing to clean" (exit 0)
  in JSON mode as an empty plan document — leaning yes (symmetric with scan).
- Whether `inspect --json` includes raw HiLight millisecond offsets alongside
  rendered timestamps — leaning yes (it's the debugging tool).
