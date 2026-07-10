## Why

Quarantine today is a *verified copy* (ADR 0003): unmarked footage is copied into the quarantine folder so a missed marker costs a trip to quarantine, not the footage. But on a card where most sessions are unmarked, that doubles the footage onto the destination disk before `cleanup` ever runs — a large, mostly-throwaway copy the user never asked for. Some workflows would rather leave unwanted footage on the source untouched and never spend the destination space on it.

## What Changes

- New common profile field `copy_quarantine` (boolean, default `true`) — available to every device type, resolved and validated alongside the other common fields (`quarantine`, `delete_source`).
- With `copy_quarantine: false`, a `Quarantine` verdict is still produced and reported (in `scan`, `--dry-run`, and `import` output) with its resolved-path column replaced by a "copy disabled" note — but planning resolves **no** quarantine path for it, so `import` transfers nothing for that group and leaves its source files exactly where they are.
- Source-deletion safety is preserved by construction: a group whose files were never transferred is never an eligible deletion candidate, so `delete_source` can never delete an un-copied quarantined file (the plan/execute + verify-then-delete invariants of ADR 0003 continue to hold).
- Default behavior is unchanged: omitting `copy_quarantine`, or setting it `true`, keeps today's verified-copy-to-quarantine flow.

## Capabilities

### New Capabilities

None. This extends the existing device-agnostic core rather than introducing a new capability.

### Modified Capabilities

- `cli-core`: the profile configuration requirement SHALL accept the common `copy_quarantine` field (default `true`); the execute requirement SHALL, when a profile disables quarantine copying, resolve no quarantine path for `Quarantine` groups and leave their source files untouched (no transfer, and therefore never a source-deletion candidate), while the verdict itself remains `Quarantine` in scan/plan output.

## Impact

- **Code**:
  - `src/config.rs`: add `copy_quarantine` to `Profile`/`RawProfile` with a `true` default; no new validation error paths (a plain boolean like `delete_source`).
  - `src/plan.rs`: `build_plan` resolves `quarantine_path: None` for `Quarantine` groups when `copy_quarantine` is disabled.
  - `src/transfer.rs`: a `Quarantine` group with no target directory records a "left in source" per-file outcome instead of copying; the existing "eligible for deletion only after a verified transfer" gate already excludes it.
  - `src/report.rs`: render the disabled-copy note for such groups.
- **Specs**: delta to `cli-core` (profile-config and execute requirements). GoPro/Tesla verdict rules are unchanged — the `Quarantine` verdict is unaffected; only what execution does with it changes.
- **Dependencies**: none new.
- **Compatibility**: additive, backward-compatible — the field defaults to today's behavior. No CLI-flag changes.
- **Docs**: README common-fields table gains `copy_quarantine`; ADR only if design concludes this materially revises ADR 0003's "quarantine = verified copy" stance (candidate to note during design).
