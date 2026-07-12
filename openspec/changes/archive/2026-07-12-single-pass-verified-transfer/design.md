# Design: single-pass-verified-transfer

## Context

`transfer_inner` (src/transfer.rs) currently runs: quick-match check → `hash_file(src)` → `resolve_destination` (needs the source hash to decide skip/suffix) → `copy_and_hash(src → .part)` → compare the two hashes → rename → `stamp_mtime`. Both compared hashes are reads of the *source*; the written `.part` is never re-read, so the comparison cannot see write-path corruption — despite the cli-core spec requiring a hash of the written temporary file, and despite that "verification" being what unlocks source deletion (ADR 0003). The source medium (SD card over USB) is read twice per transferred file.

The ordering exists because `resolve_destination` wants the source hash *before* any copy, so an identical-content collision can skip copying entirely. But name collisions are the rare path; the common path pays their cost on every file. ADR 0012 records the decision; this document covers the mechanics.

## Goals / Non-Goals

**Goals:**
- One source read per transferred file in the no-collision path.
- Verification that actually reads back the written `.part` before the rename.
- Byte-identical behavior for every outcome the caller can observe: same `TransferOutcome` variants, same collision/skip semantics, same deletion gating, same `.part` cleanup guarantees.

**Non-Goals:**
- No progress/message changes (that is `improve-console-output`; its D2 labels the phases this change creates).
- No change to quick-match (ADR 0009), sidecar writing, deletion confirmation, or the `content_verified` set.
- No physical-medium verification (`fsync` + cache-drop) — the read-back proves the write path, documented as such in ADR 0012.

## Decisions

### D1: Branch on destination-name occupancy, not hash-first

`transfer_inner` splits after the quick-match check on whether `dest_dir.join(file_name)` exists:

- **Unoccupied (common):** the final path is the plain name by construction — no `resolve_destination` call, no pre-hash. Stream `copy_and_hash(src → .part)`; its return value *is* the source hash. Verify, rename, stamp.
- **Occupied (rare):** `hash_file(src)` first, then the existing `resolve_destination` loop unchanged (`None` → `SkippedIdentical`; `Some(suffixed)` → proceed). The copy that follows uses the same single-pass + read-back as the other branch.

Alternative considered: always copy first and reconcile collisions afterwards (hash the existing file only on rename conflict). Rejected — it copies gigabytes before discovering the file was already imported, and `SkippedIdentical`'s "no copy at all" property is worth keeping.

A TOCTOU note for the doc comment, not a behavior change: the existence check and the rename are not atomic, same as today — the library destination is not concurrently mutated in this tool's model.

### D2: Verification is `hash_file(.part)` compared against the copy's stream hash

`copy_and_hash` already returns the hash of the bytes it streamed and is reused untouched; the read-back reuses `hash_file` on the `.part` path. Mismatch keeps the existing `Error::VerifyMismatch` and the existing cleanup contract: remove `.part`, leave the source, report `Failed`. A read-back I/O error gets the same cleanup as a copy error. The rename happens only after a successful match, so a `.part` file can never be promoted unverified.

### D3: Extract the verify step as a testable seam

The corruption path cannot be provoked end-to-end from a test (there is no hook between "copy finished" and "verify ran" to corrupt the file through public API). Extract `verify_part(part: &Path, expected: &blake3::Hash) -> Result<()>` (name indicative) so a unit test can write a `.part`, corrupt it, and assert the mismatch error — while `transfer_inner` remains the only production caller. Alternatives rejected: a FIFO source that can only be read once (not a regular file; blocks without a writer thread), `/proc/self/io` read accounting (cache-dependent, breaks under parallel tests).

Consequence for the spec's "source is read exactly once" scenario: it is enforced structurally — the unoccupied branch contains no `hash_file(src)` call — and covered behaviorally by the happy-path and parity tests rather than by counting syscalls.

### D4: Everything downstream is untouched

`Suffixed` detection becomes "the occupied branch chose a non-plain name" (today it compares paths after the fact — equivalent). `stamp_mtime` stays post-rename and metadata-only-failure. Progress byte accounting is unchanged: `total_bytes` still counts once per file, `copy_and_hash` still ticks copy bytes, and the skip-inc for `SkippedIdentical`/`SkippedQuickMatch` in `execute_inner` stays as is. The read-back read is deliberately unticked (fast destination disk; see improve-console-output D2).

## Risks / Trade-offs

- [A transient source read glitch during the copy is now copied and verifies cleanly, where two disagreeing source reads previously flagged it] → Accepted and recorded in ADR 0012: destination integrity is what deletion requires; the old scheme missed write corruption entirely while costing double source I/O.
- [Read-back may be served from page cache, weakening "verified on disk"] → Documented honestly (ADR 0012); multi-GiB files largely exceed cache; full medium verification is a non-goal.
- [Behavior drift in the collision path during restructure] → The `resolve_destination` loop is moved, not rewritten; existing collision/idempotency integration tests (re-run import, same-name-different-content) must pass unmodified — treat any needed edit to *those* tests as a red flag.
- [Existing tests that relied on the old double-read ordering] → Only the verify-mismatch coverage moves (to the `verify_part` seam); all outcome-level tests keep their assertions.

## Migration Plan

Internal restructure of one function plus one extracted helper; no config, CLI, on-disk format, or JSON changes. Rollback is reverting the commit. Sequence: land this change, then `improve-console-output` (its transfer-phase messages assume these phases exist).

## Open Questions

(none)
