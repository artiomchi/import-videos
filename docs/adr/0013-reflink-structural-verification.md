# 0013 — Reflink structural verification vs. empirical/heuristic verification

- Status: accepted
- Date: 2026-07-12
- Refines: [0003 — Scan → plan → execute safety model](0003-scan-plan-execute-safety-model.md), [0012 — Copy verification reads back the written file](0012-copy-verification-reads-back-the-written-file.md)
- Relates to: [0002 — Hand-rolled MP4 and GPMF parsers](0002-hand-rolled-mp4-and-gpmf-parsers.md), [0009 — `--quick-match` trades verification for speed](0009-quick-match-trades-verification-for-speed.md), [0011 — Which profile settings are CLI-overridable, and why](0011-cli-overridability-policy.md)

## Context

ADR 0012 established that verification means reading back the bytes actually persisted at the destination and hashing them — not trusting the write path. `add-reflink-transfer` adds a second fast path alongside the ordinary stream-copy: on a copy-on-write filesystem, the engine can clone a file (`FICLONE`) instead of streaming and re-hashing it. This raises the same question ADR 0009 already answered once for `--quick-match`: does skipping the hash also mean skipping source-deletion eligibility?

The two "skip the hash" paths are not the same kind of skip. `--quick-match` accepts a destination file on the strength of its name, size, and mtime — a *heuristic* that could, in principle, be fooled by a coincidentally matching file that was never actually copied from this source. Reflink is different: `FICLONE` is a kernel operation that is all-or-nothing and shares the source's exact extents. If it returns success, the destination *is* the source's data, block for block — there is no intermediate state where the call reports success but the bytes differ. The verification isn't skipped; it's satisfied by construction instead of by re-reading.

## Decision

`TransferOutcome::Reflinked` is treated as content-verified, exactly like `Transferred`, `SkippedIdentical`, and `Suffixed`: it is included in both `content_verified` and `in_place_at_destination` (`src/transfer.rs`), so a reflinked group is a source-deletion candidate under `delete_source: true`, and no read-back hash is performed for it.

This is the sharp distinction from `--quick-match`, which is deliberately excluded from `content_verified` (ADR 0009). Both skip hashing, but for opposite reasons:

| | Basis for skipping | Could it be wrong? | Deletion-eligible? |
|---|---|---|---|
| `--quick-match` | name + size + mtime heuristic | Yes — coincidental match | No |
| Reflink | kernel guarantees identical extents on success | No — success implies identity | Yes |

**Reflink over hard link.** A reflink clone is a new inode that shares extents copy-on-write; a hard link would share the *same* inode under two names. That distinction matters beyond deletion eligibility: `stamp_mtime` (gopro-telemetry design D8) sets the destination's mtime after every transfer, and with a hard link that write would also change the source card's mtime, silently corrupting the very timestamp the tool reads on a future run. A hard link would also make "delete source" degenerate into unlinking one of two names for the same data — the file would still exist, just under the destination's name, defeating the point of `delete_source: true` as a real cleanup step. Reflink's independent inode means none of the surrounding logic — mtime stamping, the confirm-then-delete flow — needs special-casing for this path.

**The `reflink-copy` crate, not a hand-rolled ioctl.** ADR 0002 established hand-rolling binary parsing as a deliberate learning choice for *read-only, non-destructive* code, where a parsing bug produces a wrong report, not data loss. The reflink call sits on the opposite side of that line: it is one syscall away from `src/transfer.rs`'s one destructive seam, and an `unsafe` `ioctl` invocation gone wrong here risks corrupting or losing footage. This is a deliberate exception to the hand-rolled ethos: `reflink-copy` is a thin, widely-used wrapper over the platform call, and only its strict `reflink()` is used — never `reflink_or_copy()`, whose fallback path would silently bypass this crate's own hashing, read-back verification, and progress accounting. Every fallback, on any error from the strict call, is routed back through the existing audited `copy_and_hash` + `verify_part` seam instead.

## Consequences

- A reflinked file participates fully in source deletion, sidecar-gating, and reporting exactly like a stream-copied-and-verified file — no caller needs to know which path produced it.
- The `content_verified`/`in_place_at_destination` gates in `src/transfer.rs` now encode two different *reasons* a file can be trusted (read-back hash, or structural guarantee) behind one boolean each; a future fast path must consciously decide which side of that line it falls on rather than defaulting to either.
- If `reflink-copy`'s strict `reflink()` were ever found to report success on a partial or corrupt clone, that would be a correctness bug in this ADR's premise, not a defense-in-depth gap this tool chose to skip — worth stating explicitly since no test can observe it locally (the fallback path is what CI actually exercises; see the reflink success-path tests' runtime CoW detection in `src/transfer.rs`).
- `reflink` joins the CLI-overridable, per-invocation-intent set from ADR 0011 (`--reflink` / `--no-reflink`), not profile identity — a run can always fall back to full verification without editing the config.
