# 0009 — `--quick-match` trades content verification for speed

- Status: accepted
- Date: 2026-07-10
- Refines: [0003 — Scan → plan → execute safety model](0003-scan-plan-execute-safety-model.md)

## Context

ADR 0003 mandates that source deletion is gated on verified content transfer (copy → hash both sides → match). This is the right default but becomes painful when re-running `import` over footage that was already imported: re-hashing gigabytes of video just to regenerate an updated `import.json` sidecar is slow and unnecessary.

Destination files that this tool wrote already have their mtime stamped to `recorded_at` (design D8 from the gopro-telemetry change). That self-produced signal is strong: name + size + mtime within 0.1 s is an unlikely accidental match, and the original is still on the source if it turns out to be wrong.

## Decision

Add a `--quick-match` flag to `import`. When set, `transfer_inner` checks the canonical destination path before hashing: if it exists, its size equals the source's, and its mtime is within 0.1 s of `recorded_at`, return `SkippedQuickMatch` without hashing. Any miss falls through to the full verified path.

`SkippedQuickMatch` is deliberately **excluded from the source-deletion gate**. The content was not verified, so the safety invariant from ADR 0003 is preserved: only `content_verified` outcomes (`Transferred`, `SkippedIdentical`, `Suffixed`) unlock source deletion.

A `SkippedQuickMatch` file is still counted as "in place at the destination" for sidecar-writing purposes, so `import --quick-match --keep-source` becomes a cheap sidecar-regeneration recipe: the sidecar is rebuilt from source metadata (MP4 atoms, telemetry, `event.json`) and rewritten, while video files are matched without re-hashing.

## Consequences

- Re-running import over already-imported footage is fast with `--quick-match`.
- The ADR 0003 safety invariant holds: no source file is deleted unless its content was verified at the destination. `--quick-match` cannot weaken this — quick-matched files simply become ineligible for deletion.
- False-positive match (same name + size + mtime, different bytes) leaves a stale sidecar but the source is always preserved. Worst case is an incorrect sidecar, not data loss.
- Filesystems with coarse mtime precision (e.g. FAT's 2 s granularity) will see misses and fall through to full verification — correct, just not fast. This is documented expected degradation.
- `--quick-match` is opt-in and off by default; omitting the flag restores full verification semantics.
