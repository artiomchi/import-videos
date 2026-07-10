## Why

The import pipeline is complete (GoPro with GPS correction, Tesla events, unified
sidecars), but the tool still lacks its operational tail: quarantined GoPro footage
accumulates forever with no way to purge it except manual deletion, and there is no
way to inspect a single file's metadata (HiLights, GPS, event data) without running
a full scan against a card. This is the final roadmap changeset (#5), closing out
the CLI surface promised in the original plan.

## What Changes

- New `cleanup PROFILE` subcommand: purges the profile's quarantine directory,
  with `--older-than <duration>` (e.g. `30d`) to keep recent items, and the same
  plan/confirm/execute safety model as `import` (`--dry-run`, `--yes`) per ADR 0003.
- New `inspect FILE` subcommand: dumps one file's device metadata — HiLight
  markers, GPMF GPS summary and clock offset for GoPro MP4s; parsed `event.json`
  for Tesla event folders — for debugging and card triage without a profile.
- `--json` flag on `scan`, `import`, and `cleanup`: machine-readable report output
  (the plan and execution summary as JSON on stdout) instead of the human table.
- Progress bars (indicatif) during transfer, so multi-GB imports are no longer
  silent between plan and summary. Suppressed under `--json` and when stdout is
  not a TTY.
- README completed: full CLI reference, config reference, example workflows
  (the skeleton from `add-core-cli` promised this once the surface was final).
- `docs/learning/README.md` index tidy-up.

No breaking changes: existing subcommands, config schema, and sidecar formats are
untouched.

## Capabilities

### New Capabilities

- `cli-maintenance`: the maintenance/debugging surface — `cleanup` (quarantine
  purge with age filtering, confirmation, dry-run) and `inspect` (single-file
  metadata dump for GoPro MP4s and Tesla event folders).

### Modified Capabilities

- `cli-core`: report output gains a machine-readable mode — `--json` on plan-
  and summary-producing commands; progress reporting during transfer becomes a
  specified behavior (visible on TTY, absent in JSON/non-TTY output).

## Impact

- `src/cli.rs`: two new subcommands, `--json` flag; `src/main.rs` dispatch.
- `src/report.rs`: JSON serialization of plans/summaries alongside the existing
  human rendering; serde derives on report types.
- `src/transfer.rs`: progress hooks (indicatif) — `indicatif` moves from
  "planned dependency" to actually used.
- New module (likely `src/cleanup.rs` or within `transfer.rs`) for quarantine
  purge; reuses the plan/execute split and confirmation prompt from `cli-core`.
- `inspect` reuses `src/media/mp4.rs`, `src/media/gpmf.rs`, and Tesla
  `event.json` parsing read-only; no changes to those parsers expected.
- Duration parsing for `--older-than` (jiff spans or a small hand parser).
- Docs: `README.md` completion, `docs/learning/README.md` index. ADR only if a
  real decision surfaces during design (per roadmap).
