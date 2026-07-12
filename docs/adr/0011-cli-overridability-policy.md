# 0011 — Which profile settings are CLI-overridable, and why

- Status: accepted
- Date: 2026-07-12
- Relates to: [0003 — Scan → plan → execute safety model](0003-scan-plan-execute-safety-model.md), [0004 — YAML config with profile per device](0004-yaml-config-with-profile-per-device.md), [0007 — Quarantine copy is optional](0007-quarantine-copy-is-optional.md), [0009 — `--quick-match` trades verification for speed](0009-quick-match-trades-verification-for-speed.md)

## Context

Before this change, the only escape hatches from a profile's YAML were `--source` (full replacement of the source path) and `--keep-source` (a one-directional override: it could only make a `delete_source: true` profile safer, never the reverse). Everything else — quarantine copying, the quarantine directory, GoPro's marker requirement — required editing the config file for one run and editing it back, leaving a window where the profile itself carried unintended settings.

Not every profile field is a candidate for this treatment, though. Some fields express **per-invocation intent** — "delete the source this time," "skip the marker rule for this one card" — that is naturally decided at the command line and doesn't belong permanently in the profile. Others express **profile identity** — what a profile fundamentally *is*, as distinct from another profile of the same device type. Blurring that line would let a single flag turn one profile into a different one for the run, which undermines the point of naming profiles in the config at all.

## Decision

Per-invocation intent is CLI-overridable, in both directions, via paired flags named after the config field:

- `delete_source` — `--delete-source` / `--no-delete-source`
- `copy_quarantine` — `--copy-quarantine` / `--no-copy-quarantine`
- `quarantine` — `--quarantine PATH` (also forces `copy_quarantine` on for the run, design D4 of `add-cli-overrides`)
- `require_marker` (GoPro only) — `--gopro-require-marker` / `--no-gopro-require-marker`
- `reflink` — `--reflink` / `--no-reflink` (`add-reflink-transfer`; see [0013](0013-reflink-structural-verification.md))
- `source` (pre-existing) — `--source PATH`

Profile identity is not CLI-overridable: `type`, `destination`, `layout`, `ignore`, and Tesla's `events`/`reasons`. A profile with a different filter set, a different layout, or a different destination is a *different profile* — the config file, not a flag, is where that distinction belongs. Overriding these from the CLI would let one profile name mean something different from run to run, which is exactly the ambiguity naming profiles is meant to avoid.

**Both directions, not one.** ADR 0003 and 0007 justify why profiles default to safe (`delete_source: false`, `copy_quarantine: true`) or permissive (`require_marker: true`) values. But per-invocation intent doesn't only run against a permissive default — a conscious "delete these source files this one time" from a safe profile is exactly as legitimate as "don't delete this time" from a profile that requests it. One-directional overrides (like the old `--keep-source`) only cover half of that; paired flags cover both without weakening any safety mechanism downstream. Forcing `delete_source` on still goes through the same confirmation prompt (`--yes` or an interactive `[y/N]`) and the same verified-transfer gate as the profile-driven case — the override changes only the effective value going into that unchanged mechanism.

**`--keep-source` becomes `--no-delete-source`.** With `delete_source` now overridable in both directions, `--keep-source` reads oddly next to `--delete-source` — same field, different vocabulary. The documented flag is renamed to `--no-delete-source` for symmetry with its pair and with the config field name. `--keep-source` remains as an undocumented (hidden from `--help`) clap alias, so existing scripts and muscle memory keep working without a hard break.

## Consequences

- One flag vocabulary, one policy line: overridable fields are named identically to their config counterpart, prefixed `--no-` for the off direction; non-overridable fields have no CLI surface at all.
- `--keep-source` still works, silently, as an alias — no invocation breaks — but new documentation and the README teach `--no-delete-source` from the start.
- A future device's compile-time-fixed config field follows the same test before getting a flag: is this per-invocation intent, or does changing it make a different profile? The device-specific flags (`gopro-` prefixed today) are expected to grow one small set per device rather than converge on a generic `--set key=value` mechanism — the device set itself is compile-time-fixed (design D3 of `add-core-cli`), so a handful of typed flags per device is consistent with the rest of the architecture.
