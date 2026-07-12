# Proposal: single-pass-verified-transfer

## Why

The transfer engine violates its own spec: cli-core's "Verified transfer with atomic finalization" requires hashing the *written temporary file*, but `transfer_inner` hashes the source twice — once up front for collision resolution, once while streaming the copy — and never reads the destination back. Verification therefore proves only that the source read consistently twice: write-path corruption (short write, truncation, buffer corruption) passes undetected, yet that "verification" is what gates source deletion (ADR 0003). It also costs a full extra read of the slowest medium in the pipeline, the source card.

## What Changes

- **Single-pass copy**: in the common case (destination name unoccupied), the source is read exactly once, hashed while streaming to `<final>.part` — the separate `hash_file(src)` pre-pass is removed from this path.
- **Real verification**: the written `.part` file is re-read and hashed; the rename to the final name is gated on that hash matching the source stream hash. This implements what the spec already says.
- **Collision path keeps the pre-hash**: only when the destination name is occupied is the source hashed in a separate pass first, because knowing the hash before copying is what lets an identical-content collision skip the copy entirely (`SkippedIdentical`). An unresolved collision then proceeds through the same single-pass copy + read-back at the suffixed name.
- **Deletion gate unchanged**: the `content_verified` outcome set (`Transferred`, `SkippedIdentical`, `Suffixed`) and quick-match's exclusion from it (ADR 0009) are untouched.
- No CLI surface, config, or JSON output changes.

Trade-off made explicit (see ADR 0012): the old double-source-read caught a transient source read glitch; the new scheme catches write-path corruption instead. The latter is what source deletion actually depends on and what the spec intended.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `cli-core`: the "Verified transfer with atomic finalization" requirement is tightened — the source SHALL be read exactly once when no collision exists, and verification SHALL be a read-back hash of the written temporary file, so write-path corruption fails verification. The collision requirement's observable behavior is unchanged (hash comparison still decides skip vs suffix).

## Impact

- `src/transfer.rs`: `transfer_inner` restructured (hash-then-copy → copy-then-verify, collision branch reordered); `copy_and_hash` and `hash_file` both survive with new call sites.
- Tests: the verification-failure integration test must inject corruption into the written `.part` (or the write path) rather than relying on a second source read differing; a new test asserts the source is read once in the no-collision path.
- `docs/adr/0012` records the verification-semantics decision (refines ADR 0003, relates to ADR 0009).
- The `improve-console-output` change builds on this one: its transfer-phase progress messages become `copying` → `verifying`, and the pre-copy bar stall this change removes is one of its findings.
