## Context

Profiles hold safe defaults; per-invocation intent currently has only two escape hatches — `--source` (full replacement) and `--keep-source` (one-directional). The proposal widens this to a consistent override surface: `delete_source`, `copy_quarantine`, `quarantine`, and GoPro's `require_marker`, each overridable per run without touching the YAML. Constraints: the scan → plan → execute split (ADR 0003) must stay intact — overrides shape the *effective profile* before planning, never mid-execution; deletion-adjacent behavior needs integration tests (AGENTS.md); config-load validation wording is the template for CLI-side validation errors.

## Goals / Non-Goals

**Goals:**
- Boolean profile fields overridable in **both directions** from the CLI, with unset meaning "use the profile".
- Flag vocabulary aligned one-to-one with config field names.
- One documented policy line (ADR) separating overridable per-invocation intent from non-overridable profile identity.

**Non-Goals:**
- No zero-config mode: `profile` stays a required positional; a config file is still required for `scan`/`import`/`cleanup`.
- No overrides for `type`, `layout`, `ignore`, `destination`, or Tesla `events`/`reasons` — a different filter set or layout is a different profile.
- No generic `--set key=value` mechanism; the device set is compile-time-fixed (design D3 of `add-core-cli`), so typed flags in `cli.rs` are consistent with the architecture and the current field count doesn't justify the indirection.

## Decisions

### D1. Paired `--flag` / `--no-flag`, collapsing to `Option<bool>`
Each boolean override is a pair of clap flags wired with `overrides_with` in both directions, derived into `Option<bool>`: `None` = use profile, `Some(true)` / `Some(false)` = force. Repeats are **last-one-wins** (standard clap pair semantics; lets a shell alias bake in a default that a later flag overrides), not a conflict error.
*Alternative considered*: value-form `--delete-source=off`. Rejected: optional-value flags (`num_args(0..=1)`) let `import --delete-source gopro` swallow the positional profile arg; `require_equals` avoids that but breaks the bare `--delete-source` spelling. Pairs are the ripgrep/cargo idiom and complete/document cleanly.

### D2. `--keep-source` becomes a hidden alias of `--no-delete-source`
The documented flag is `--no-delete-source` (aligned with `delete_source`); `--keep-source` remains as a clap `alias` (hidden from `--help`) so scripts and muscle memory keep working. README and the ADR 0009 sidecar-regeneration recipe update to the new spelling. **BREAKING** only at the documentation level.

### D3. `--delete-source` can force deletion on; prompt unchanged
The profile-is-conservative asymmetry inverts for a safe-by-default profile: deletion is the rare conscious act, and per-invocation is exactly where that intent belongs. Safety rails are untouched: deletion still happens only after blake3 verification, still prompts unless `--yes` (design D8 of `add-core-cli`), and quick-matched files remain non-candidates (ADR 0009).

### D4. `--quarantine PATH` implies `copy_quarantine: true`; contradiction is a parse error
Pointing quarantine at a path expresses intent to quarantine-copy, so the flag also forces `copy_quarantine` on — even against a profile's `copy_quarantine: false`. Combining `--quarantine` with `--no-copy-quarantine` is self-contradictory and fails at parse time via `conflicts_with` (clap usage error, exit 2). A relative `--quarantine` path resolves against the effective destination, the same rule the config field follows.

### D5. Overrides apply at profile resolution, producing an effective `Profile`
`cli.rs` collects the flags into a plain `Overrides` struct; `lib.rs` applies it right after `get_profile`, cloning the profile and shadowing each `Some` field. Everything downstream — `plan`, `transfer`, `report` — consumes the effective `Profile` unchanged and needs no knowledge that overrides exist. `require_marker` shadows into `SourceKind::Gopro { require_marker }`; passing either marker flag when the resolved profile is not GoPro fails with the config loader's wording (`require_marker is only valid for profiles of type gopro`) as `Error::Config`, exit 2.
*Alternative considered*: merging overrides into the raw YAML `Value` before deserialization to reuse `config::load` validation wholesale. Elegant, but it reaches into config internals for four fields' worth of benefit and makes flag→field mapping stringly; the typed shadow is more legible for this project.

### D6. Flag availability: plan-shaping flags on `scan` and `import`, execution flags on `import` only
`--quarantine`, `--copy-quarantine`/`--no-copy-quarantine`, and `--gopro-require-marker`/`--no-gopro-require-marker` change what the plan shows, so `scan` accepts them — it is the natural place to preview "what would this keep?". `--delete-source`/`--no-delete-source` only affects execution, so it lives on `import` alone.

### D7. ADR records the overridability policy
New ADR: per-invocation intent (`delete_source`, `copy_quarantine`, `quarantine`, `require_marker`, and the pre-existing `source`) is CLI-overridable in both directions where safe; profile identity (`type`, `destination`, `layout`, `ignore`, `events`/`reasons`) is not. It also records the `--keep-source` → `--no-delete-source` rename.

## Risks / Trade-offs

- [Device-specific flags accrete in `cli.rs` as devices grow] → accepted: the device set is compile-time-fixed by design; revisit a generic mechanism only if the flag count actually hurts. The `gopro-` name prefix keeps ownership legible.
- [Forcing `delete_source: true` from the CLI enlarges the blast radius of a typo] → the confirmation prompt (or explicit `--yes`) still gates every deletion; integration tests cover force-on, force-off, and prompt paths.
- [Hidden alias means `--keep-source` works but is undocumented] → intentional; the alias can be dropped in a later change once muscle memory fades.
- [`Option<bool>` pair pattern is boilerplate-ish in clap derive (two `bool` fields per knob)] → contained to `cli.rs`; a small helper converts each pair to `Option<bool>` in one place.

## Migration Plan

Single release; no data migration. `--keep-source` keeps working via the alias, so no invocation breaks. README flag table and recipes update in the same change.

## Open Questions

None — flag semantics, availability, conflict rules, and the policy line were settled during exploration.
