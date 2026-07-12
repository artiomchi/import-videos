## Why

When the source and destination live on the same filesystem — a staging directory
under `~` rather than a mounted camera card — a full stream-copy duplicates every
byte and re-hashes it, even though the filesystem can share the data instantly.
On a copy-on-write filesystem (btrfs, XFS-with-reflinks, bcachefs) a reflink clones
the file in one syscall: near-instant, zero extra space until one side is written,
and byte-identical by construction. This is a real workflow for the author (library
and any staging area sit on btrfs), so the fast path will actually fire; for camera-
card imports it simply won't apply and the existing copy runs unchanged.

## What Changes

- The verified-transfer engine gains a **reflink fast path**: for each keeper (and
  quarantine-copy) file it first attempts a copy-on-write clone of the source into
  the `<final>.part` temporary, then finalizes with the existing atomic rename.
- **Always try, fall back cleanly**: on any reflink failure — cross-device (`EXDEV`),
  non-CoW filesystem (`EOPNOTSUPP`), or any other error — the engine falls through to
  the current single-pass stream-copy-and-read-back path (ADR 0003, ADR 0012). No
  filesystem probing; the attempt itself is the probe.
- A successful reflink is **verified by construction**, not by hashing: a `FICLONE`
  is all-or-nothing and shares the source's exact extents, so no read-back hash is
  performed and the file remains a source-deletion candidate — unlike `--quick-match`
  (ADR 0009), whose heuristic match forfeits deletion. This distinction gets a new ADR.
- New `TransferOutcome::Reflinked` variant so reports can distinguish an instant clone
  from a streamed copy. It counts as in-place and as content-verified.
- New profile config field `reflink` (boolean, default `true`) and matching per-run
  CLI overrides `--reflink` / `--no-reflink`, following the overridability policy of
  ADR 0011. When reflink is disabled, transfers always take the stream-copy path.
- New dependency: the `reflink-copy` crate (strict `reflink()` only; the crate's own
  copy fallback is deliberately not used, so every fallback flows through the audited
  stream-copy-and-verify seam).

Non-goal: a same-filesystem `rename`/move optimization. A move reaches the same end
state as reflink-then-delete for `delete_source: true`, but it collapses transfer and
deletion into one atomic act and bypasses the "delete source? [y/N]" confirmation that
the safety model relies on. Reflink is preferred precisely because it preserves that
flow. Move is left as a separate future idea.

## Capabilities

### New Capabilities

<!-- None: the transfer engine is part of cli-core, not a standalone capability. -->

### Modified Capabilities

- `cli-core`: the "Verified transfer with atomic finalization" requirement gains a
  reflink fast path with fallback; a new requirement covers reflink's structural
  verification and its source-deletion eligibility; the YAML-config requirement gains
  the `reflink` field; the CLI surface gains `--reflink` / `--no-reflink`.

## Impact

- **Code**: `src/transfer.rs` (fast-path seam in `transfer_inner`, new `Reflinked`
  outcome, gate functions `in_place_at_destination` / `content_verified`, progress
  ticking for an instant op); `src/report.rs` (tally, human line, JSON rendering of
  the new outcome); `src/config.rs` (`reflink` field, default `true`); `src/cli.rs`
  (`--reflink` / `--no-reflink`) and the profile-resolution that folds the override in.
- **Dependencies**: adds `reflink-copy`.
- **Docs**: new ADR (reflink structural verification vs. empirical/heuristic);
  README config table and flag reference.
- **Safety**: preserves the ADR 0003 invariant — no source is deleted unless the
  destination is provably correct; reflink makes "provably correct" free rather than
  weakening it. Confirmation-before-deletion is unchanged.
