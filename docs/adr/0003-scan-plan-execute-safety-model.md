# 0003 — Scan → plan → execute safety model

- Status: accepted
- Date: 2026-07-09

## Context

The tool's whole point is destructive: it cleans source cards after import and discards unmarked commute footage. A bug or a missed HiLight press must not cost real footage. It also needs to be testable without real SD cards.

## Decision

Every import is split into three phases:

1. **Scan** (read-only): device modules discover media, read metadata, and produce an `ImportPlan` — every file's verdict (`Keep` / `Quarantine` / `Ignore`) with the reason.
2. **Plan review**: `scan` and `import --dry-run` print the plan without touching anything.
3. **Execute**: copy to destination → verify with a blake3 checksum → only then delete from the source.

Safety rules on top:

- Unmarked footage is **never deleted directly** — it moves to a quarantine folder, purged only by an explicit `cleanup` command (with `--older-than`).
- Source deletion happens only after checksum verification, and is configurable per profile (`delete_source`).
- Destructive steps prompt for confirmation unless `--yes` is passed.

## Consequences

- More code than a straight `mv`, and every import reads each file twice (copy + verify); accepted for footage safety.
- The plan/execute split makes integration tests natural: assert on the plan, then assert on the filesystem after execution.
- A missed marker costs a trip to the quarantine folder, not the footage.
