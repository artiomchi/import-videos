# 0003 — Scan → plan → execute safety model

- Status: accepted
- Date: 2026-07-09

## Context

The tool's whole point is destructive: it cleans source cards after import and discards unmarked commute footage. A bug or a missed HiLight press must not cost real footage. It also needs to be testable without real SD cards.

## Decision

Every import is split into three phases:

1. **Scan** (read-only): device modules discover media and read metadata, producing every group's verdict (`Keep` / `Quarantine` / `Ignore`) with the reason.
2. **Plan review**: `import --dry-run` resolves and prints the full plan — every group's destination or quarantine path, and (for GoPro, unless disabled) its GPS-corrected timestamp — without touching anything. It is the sole exact preview of what a real `import` will do (improve-scan-and-cleanup design D1). The standalone `scan` command is a separate, lighter-weight source-only inventory: it shares phase 1's discovery but never resolves a destination or quarantine path and never runs GPS telemetry, so its output can diverge from what `import` actually resolves (e.g. across a GPS-corrected day boundary) — use `import --dry-run`, not `scan`, when you need the real answer.
3. **Execute**: copy to destination → verify with a blake3 checksum → only then delete from the source, pruning any source directory the deletion leaves empty (never the scanned source root itself).

Safety rules on top:

- Unmarked footage is **never deleted directly** — it moves to a quarantine folder, purged only by an explicit `cleanup` command (with `--older-than`).
- Source deletion happens only after checksum verification, and is configurable per profile (`delete_source`).
- Destructive steps prompt for confirmation unless `--yes` is passed.

## Consequences

- More code than a straight `mv`, and every import reads each file twice (copy + verify); accepted for footage safety.
- The plan/execute split makes integration tests natural: assert on the plan, then assert on the filesystem after execution.
- A missed marker costs a trip to the quarantine folder, not the footage.
- `scan` trading path/GPS accuracy for speed is a real capability loss for anyone who used it as a path preview — `import --dry-run` is the documented replacement (improve-scan-and-cleanup).
