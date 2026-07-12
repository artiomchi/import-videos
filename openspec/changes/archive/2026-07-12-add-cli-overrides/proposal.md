## Why

Profiles hold deliberately safe defaults (ADR 0003, ADR 0004), but one-off intent — a conscious source deletion, a scratch quarantine location, a "what would this keep without the marker rule?" experiment — currently requires editing the config file and editing it back, leaving a window where the profile itself is dangerous. The only overrides today are `--source` (full replacement) and `--keep-source` (one-directional), an inconsistent surface that covers two of ten profile fields with two different philosophies.

## What Changes

- `import` (and `scan` where meaningful) gains paired boolean override flags aligned one-to-one with config field names, each collapsing to `Option<bool>` — unset means "use the profile", via clap `overrides_with` (last one wins):
  - `--delete-source` / `--no-delete-source` overrides `delete_source` in **both directions**. Forcing deletion on still requires the existing confirmation prompt unless `--yes`; quick-matched files remain non-candidates for deletion (ADR 0009).
  - `--copy-quarantine` / `--no-copy-quarantine` overrides `copy_quarantine` (ADR 0007).
  - `--gopro-require-marker` / `--no-gopro-require-marker` overrides the GoPro-specific `require_marker`; passing either against a non-GoPro profile is rejected at profile resolution with the same wording the config loader uses.
- `--quarantine PATH` overrides the profile's quarantine directory for the run (resolved by the same rules as the config field: relative paths resolve against the destination). Setting it **implies `copy_quarantine: true`**; combining it with `--no-copy-quarantine` is a contradiction and errors at parse time (`conflicts_with`).
- **BREAKING**: `--keep-source` is retired as a documented flag, replaced by `--no-delete-source`. A hidden clap alias (`--keep-source` → `--no-delete-source`) keeps existing muscle memory and scripts working. README recipes (e.g. sidecar regeneration, ADR 0009) update to the new spelling.
- New ADR: which config fields are CLI-overridable and why — the policy line between per-invocation intent (overridable, both directions) and profile identity (`type`, `events`/`reasons`, `layout`, `ignore`: not overridable; a different filter set is a different profile).

## Capabilities

### New Capabilities

None — this extends the existing CLI surface.

### Modified Capabilities

- `cli-core`: override-flag semantics (paired flags, `Option<bool>` precedence over profile values, last-one-wins), `--quarantine` override + `copy_quarantine` implication + conflict rule, `--keep-source` replaced by `--no-delete-source` with hidden alias, `--delete-source` able to force deletion on (prompt unchanged).
- `gopro-import`: `require_marker` becomes overridable per-invocation via `--gopro-require-marker`/`--no-gopro-require-marker`; rejection on non-GoPro profiles.

## Impact

- `src/cli.rs`: new paired flags on `Import` (subset on `Scan`), hidden `keep-source` alias, `conflicts_with` wiring.
- `src/lib.rs`: profile resolution applies overrides after `config::load` (shadow-if-`Some`); GoPro-only flag validation against the resolved profile's `SourceKind`.
- `src/plan.rs` / `src/transfer.rs`: consume the already-resolved effective profile; no logic change expected beyond plumbing.
- `docs/adr/`: new ADR (overridability policy, `--keep-source` rename noted).
- `README.md`: flags table, updated recipes.
- Tests: integration coverage for both directions of `delete_source`, the quarantine implication/conflict, and the wrong-device rejection — deletion-adjacent behavior requires integration tests per AGENTS.md.
