# 0007 — Quarantine copy is optional

- Status: accepted
- Date: 2026-07-10
- Refines: [0003 — Scan → plan → execute safety model](0003-scan-plan-execute-safety-model.md)

## Context

ADR 0003 stated "unmarked footage is **never deleted directly** — it moves to a quarantine folder." This implied that quarantine always involves a copy. On a card where most sessions are unmarked, that copy duplicates a large, mostly-throwaway volume onto the destination disk before `cleanup` ever runs.

The `copy_quarantine` profile field (default `true`) lets a user opt out of the copy while keeping the safety invariant intact.

## Decision

ADR 0003's core invariant is refined, not reversed:

> **Quarantined footage is either verified-copied to a quarantine folder OR left untouched on the source. It is never deleted without a verified copy.**

When `copy_quarantine: false`, the plan resolves no quarantine path for `Quarantine` groups. Execution leaves their files on the source without touching them. Because no transfer occurred, these files are never eligible for source deletion — the "eligible only after verified transfer" gate already enforces this by construction, with no special-casing needed.

The `Quarantine` verdict is still produced and reported in `scan`/`--dry-run`/`import` output, with a "quarantine copy disabled" note in place of the resolved path. Footage is never silently omitted.

## Consequences

- A user with mostly-unmarked cards can avoid filling their destination disk with throwaway quarantine copies.
- The safety guarantee is unchanged in its essential form: no footage is deleted without a verified transfer.
- Unmarked footage left on the source accumulates there until the card is wiped or re-imported with `copy_quarantine: true`. This is the intended trade-off and is visible in `scan` output.
- Default behavior (`copy_quarantine: true` or field omitted) is the ADR 0003 verified-copy path — no change for existing configs.
