## Context

Quarantine is currently a verified copy: `build_plan` resolves every `Quarantine` group to a quarantine path (`profile.quarantine` or `destination/_quarantine`), and `execute` transfers those files through the same copy → blake3-verify → rename path as `Keep` groups (ADR 0003). On a card that is mostly unmarked footage, this duplicates a large, mostly-throwaway volume onto the destination disk before the explicit `cleanup` command ever runs.

The proposal adds a per-profile opt-out (`copy_quarantine`, default `true`). The engine is device-agnostic (ADR 0005), so the decision lives entirely in `cli-core`'s config + plan + execute layers; GoPro/Tesla verdict logic is untouched. The change is small but sits on the footage-safety boundary AGENTS.md and ADR 0003 govern, which is why it gets a design note rather than going straight to tasks.

## Goals / Non-Goals

**Goals:**
- Let a profile skip copying `Quarantine` groups so their source files are never touched.
- Keep the disabled path safe by construction: an un-copied group can never be a source-deletion candidate.
- Keep the verdict visible — disabling the *copy* must not hide that footage was quarantined.
- Zero behavior change when the field is absent or `true`.

**Non-Goals:**
- No change to how verdicts are decided (GoPro `require_marker`, Tesla filtering are unaffected).
- No new CLI flag — this is a config-level, per-profile setting, not a per-run override.
- No change to the `cleanup` command or the quarantine-then-purge lifecycle for the enabled path.
- Not a global kill-switch for quarantine as a concept; the `Quarantine` verdict still exists and is reported.

## Decisions

### Decision: Represent "skip copy" as an absent quarantine path in the plan
When `copy_quarantine` is false, `build_plan` resolves `quarantine_path: None` for `Quarantine` groups (instead of `Some(base.join(name))`). Execution already branches on the target directory being present, so a `None` path naturally means "nothing to transfer."

- **Why:** The plan stays the single source of truth (ADR 0003) — the scanned plan already carries, verbatim, everything execution will do. Encoding the opt-out as "no resolved path" keeps `scan`/`--dry-run`/`import` consistent without a second flag threaded into the executor.
- **Alternative considered:** Pass `copy_quarantine` into `execute` and branch there. Rejected — it would let the plan and execution disagree (a plan showing a quarantine path that execution silently ignores), violating "import executes exactly the scanned plan."

### Decision: `copy_quarantine` is a common profile field, defaulting to `true`
It lives on `Profile`/`RawProfile` beside `quarantine` and `delete_source`, with `#[serde(default = ...)]` yielding `true`, and needs no cross-field validation (a plain boolean, unlike device-gated `require_marker`).

- **Why:** Quarantine copying is a property of the device-agnostic transfer engine, not of any one device; every device that can produce a `Quarantine` verdict (today, GoPro) should be able to opt out uniformly. Defaulting to `true` preserves ADR 0003's safety-first posture for anyone who doesn't set it.
- **Alternative considered:** A GoPro-only field. Rejected — it would bind an engine-level concern to one device and duplicate as new quarantining devices arrive.

### Decision: Report the disabled state instead of hiding the group
Report rendering shows the group as `QUARANTINE` with a "quarantine copy disabled" note where the resolved path would be; execution records a distinct per-file outcome (e.g. "left in source") rather than a transfer.

- **Why:** Silent omission would make it look like unmarked footage vanished. The user still needs to see what was recognized as quarantine so they can trust nothing was lost — and so the count in the summary line stays honest.
- **Alternative considered:** Downgrade to an `Ignore` verdict. Rejected — `Ignore` means "not media we act on"; this footage *is* recognized and deliberately left, which is a different meaning and would muddy device verdict semantics.

## Risks / Trade-offs

- **[Un-copied footage is left on the source, and with `delete_source` the card is not fully emptied]** → This is the intended trade-off, but it must be obvious. Mitigation: the plan/report explicitly show the disabled state, and the summary counts the group as quarantined; the source files simply remain.
- **[A future refactor could wire `delete_source` to quarantined groups directly]** → That would delete un-verified footage. Mitigation: safety is derived, not special-cased — deletion eligibility already requires a non-empty, all-verified file set, which a skipped group never has; an integration test asserts an un-copied quarantined source survives `delete_source: true`.
- **[ADR 0003 states "quarantine = verified copy," which this softens]** → Mitigation: record the nuance. ADR 0003's invariant is that footage is *never deleted without a verified copy*; leaving footage untouched on the source honors that (nothing is deleted). This is a refinement, not a reversal — see Open Questions on whether it warrants a superseding ADR note.

## Migration Plan

Additive and backward-compatible: existing configs omit `copy_quarantine` and keep today's verified-copy-to-quarantine behavior. No data migration, no config rewrite. Rollback is removing the field handling; existing configs remain valid either way.

## Open Questions

- Does this warrant a short ADR refining ADR 0003's "quarantine = verified copy" wording (to "quarantined footage is either verified-copied or left in place, never deleted without verification"), or is the design-note + spec delta sufficient? Leaning toward a brief ADR since it touches a recorded safety decision.
