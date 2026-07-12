## 1. Verify seam (design D2, D3)

- [x] 1.1 Extract the read-back verify step in `src/transfer.rs`: hash the written `.part` via `hash_file`, compare against the expected source-stream hash, return `Error::VerifyMismatch` on difference
- [x] 1.2 Unit tests against the seam: matching content passes; a corrupted `.part` yields `VerifyMismatch`; an unreadable `.part` yields an I/O error

## 2. Restructure transfer_inner (design D1, D4)

- [x] 2.1 After the quick-match check, branch on whether `dest_dir.join(file_name)` exists; in the unoccupied branch drop `hash_file(src)` and `resolve_destination` entirely — `copy_and_hash`'s return is the source hash, final path is the plain name
- [x] 2.2 Occupied branch: keep `hash_file(src)` → `resolve_destination` unchanged (`None` → `SkippedIdentical`, `Some(path)` → suffixed copy through the same single-pass + verify flow)
- [x] 2.3 Wire the verify seam between copy and rename in both copying branches; a mismatch or read-back I/O error removes the `.part` and reports `Failed`, identical to today's copy-error cleanup
- [x] 2.4 Derive the `Suffixed` outcome from the occupied branch's name choice; keep `stamp_mtime` post-rename and unchanged
- [x] 2.5 Update the `transfer_file`/`transfer_inner` doc comments — the "re-hashes the written file" claim is finally true; note the non-atomic existence-check/rename window (TOCTOU) as a modeled-out concern

## 3. Behavior parity and new coverage

- [x] 3.1 Run the existing transfer/integration suites unmodified — idempotent re-run, same-name-different-content suffixing, quarantine transfer, quick-match, deletion gating must all pass without edits (design flags any needed edit to collision tests as a red flag)
- [x] 3.2 Confirm the old source-double-read mismatch coverage is fully replaced by the seam tests from 1.2; delete any test that only exercised the removed ordering
- [x] 3.3 Integration test for the happy path end state: transferred file at final name, no `.part` remnant, mtime stamped, group deletable — asserting the spec's "Successful verified copy" scenario against the new flow
- [x] 3.4 Document in the test module (brief comment) that the spec's "source is read exactly once" scenario is enforced structurally per design D3 — the unoccupied branch contains no source pre-hash call

## 4. Docs and quality gates

- [x] 4.1 Re-read ADR 0012 against the implemented code and fix any drift (it was written at proposal time)
- [x] 4.2 Quality gates: `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`
- [x] 4.3 `openspec validate single-pass-verified-transfer` passes; each delta-spec scenario maps to a test or the documented structural note from 3.4
