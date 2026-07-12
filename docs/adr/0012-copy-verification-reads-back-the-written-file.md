# 0012 — Copy verification reads back the written file

- Status: accepted
- Date: 2026-07-12
- Refines: [0003 — Scan → plan → execute safety model](0003-scan-plan-execute-safety-model.md)
- Related: [0009 — `--quick-match` trades content verification for speed](0009-quick-match-trades-verification-for-speed.md)

## Context

ADR 0003 gates source deletion on verified content transfer, and the cli-core spec has always described that verification as: hash the source while streaming the copy, then hash the written temporary file, rename only on match. The implementation drifted: `transfer_inner` hashed the source up front (to resolve destination collisions before copying), hashed the source *again* while streaming the copy, and compared those two — the written file was never read back.

That scheme verifies the wrong thing. Two agreeing source reads prove the card read consistently; they prove nothing about what landed on disk, which is the only fact source deletion depends on. It also reads the source — typically a slow SD card over USB — twice per file, roughly doubling import wall time.

The up-front source hash existed for a real reason: collision resolution needs the hash *before* copying, so that an identical-content collision can be skipped without a copy. But collisions are the rare path; the common path paid for them on every file.

## Decision

Restructure `transfer_inner` around which fact is needed when:

- **No collision** (destination name unoccupied): single pass — read the source once, hashing while streaming to `<final>.part`; then re-read and hash the written `.part`; rename only when the read-back hash matches the source stream hash.
- **Collision** (destination name occupied): hash the source in a separate pass first, compare against the existing file(s) to decide `SkippedIdentical` (no copy at all) versus a suffixed name; an actual copy then follows the same single-pass + read-back scheme.

Verification now means: *the bytes persisted at the destination are the bytes read from the source*. The `content_verified` outcome set gating deletion (`Transferred`, `SkippedIdentical`, `Suffixed`) is unchanged, as is quick-match's exclusion from it (ADR 0009).

## Consequences

- The common path reads the source once instead of twice — on card-bound imports, close to halving transfer time — while verification becomes materially stronger, catching short writes, truncation, and write-path corruption that the old scheme could not see.
- The trade: a transient source read glitch during the copy is now copied faithfully and verifies "successfully", where two disagreeing source reads would previously have flagged it. Write-path corruption is judged the more realistic risk, and destination integrity is what deletion actually requires.
- The read-back may be partially served from the page cache, so it proves the write path, not the physical medium. For multi-gigabyte video files most of the read-back falls out of cache and does hit the disk; full medium verification (sync + cache drop) is deliberately out of scope.
- Verification-failure tests must inject corruption into the written file rather than into a second source read.
- Users who want to skip verification entirely already have `--quick-match` (ADR 0009), with its compensating rule that unverified files are never deletion candidates. This ADR does not add another speed/safety knob.
