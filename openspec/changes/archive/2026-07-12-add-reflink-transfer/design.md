## Context

The verified-transfer engine (`src/transfer.rs`) is the crate's one destructive
seam. Today every keeper and quarantine-copy file is stream-copied to `<final>.part`
while being hashed, the written `.part` is re-read and re-hashed, and only a matching
read-back finalizes the file with an atomic rename (ADR 0003, ADR 0012). This is
correct and safe on every filesystem, but on a copy-on-write filesystem where source
and destination share a mount, it does redundant work: the kernel can share the file's
extents in a single `FICLONE` ioctl instead of copying and re-reading every byte.

The primary workflow — importing from a mounted camera card — never benefits: a card
is a different filesystem, so a clone is impossible. But a staging workflow (dump a
card to a directory under `~`, then import into the library, both on btrfs here) hits
the same-filesystem case every time. See the proposal for the full motivation.

The engine already threads one opt-in speed knob, `--quick-match` (ADR 0009), as a
plain `bool` parameter from `execute` down into `transfer_inner`. Reflink follows that
exact plumbing shape, which keeps the new path override-unaware and unit-testable.

## Goals / Non-Goals

**Goals:**

- Attempt a copy-on-write clone for same-filesystem transfers; fall back to the
  existing verified stream-copy on any failure, with no filesystem probing.
- Preserve every safety invariant: `.part` discipline, atomic rename, and the rule
  that a source is deleted only when the destination is provably correct.
- Make reflink verification *structural* (a successful clone is identical by
  construction) so it keeps source-deletion eligibility, unlike `--quick-match`.
- Keep reflink default-on, disableable via config (`reflink: false`) and per run
  (`--reflink` / `--no-reflink`), per ADR 0011.

**Non-Goals:**

- A same-filesystem `rename`/move optimization (bypasses the deletion prompt — see
  proposal). Out of scope.
- Detecting or reporting *why* a clone was unavailable (cross-device vs. non-CoW fs).
  A failed attempt simply falls back; the distinction isn't surfaced.
- Reflink support on non-Unix targets. The `reflink-copy` crate degrades to a normal
  copy elsewhere; we don't special-case it.

## Decisions

### D1 — Fast path sits at the copy site, after destination resolution

The reflink attempt replaces the `copy_and_hash` + `verify_part` pair inside
`transfer_inner`, and only there. Everything upstream is unchanged: the quick-match
fast path, the destination-occupancy branch, the source pre-hash that resolves a
collision, and the identical-content skip all run first. By the time we reach the copy
site we already hold the resolved `final_path` (plain or `-N`-suffixed). So the change
is local:

```
transfer_inner (resolved final_path in hand):
  reflink enabled?
    ├─ yes → try reflink(src → final.part)
    │         ├─ Ok  → rename → stamp_mtime → Reflinked
    │         └─ Err → fall through ↓  (log at debug: reason)
    └─ no  ────────────────────────────↓
  copy_and_hash(src → final.part) → verify_part → rename → stamp_mtime → Transferred/Suffixed
```

*Alternative rejected:* branching earlier (e.g. choosing reflink vs copy before
collision resolution) would duplicate the occupancy/skip logic across two paths. Placing
the split at the single copy site keeps one resolution path and one finalization path.

### D2 — Always try, then fall back — no `st_dev` pre-check

We attempt the clone and treat any error as "not available here," falling through to
the stream-copy. We do **not** pre-compare source/destination device ids.

*Why:* the syscall is its own probe. It cleanly covers all three miss cases at once —
cross-device (`EXDEV`), same-device-but-non-CoW filesystem (`EOPNOTSUPP`), and any
other I/O error — with no platform-specific `MetadataExt::dev()` plumbing to maintain.
*Trade-off:* on the camera-card path (default `reflink: true`) we pay one immediately-
failing ioctl per file. `EXDEV` returns without touching data, so the cost is
negligible; if it ever mattered, an `st_dev` short-circuit is a pure optimization we
can add later without changing behavior. The `reflink: false` config already lets a
user who never stages skip the attempt entirely.

### D3 — Reflink verification is structural, not empirical (new ADR)

A successful `FICLONE` is all-or-nothing and shares the source's exact extents, so the
clone is byte-identical by construction. We therefore perform **no read-back hash** for
a reflinked file, and `Reflinked` returns `true` from both `in_place_at_destination`
and `content_verified` — meaning a reflinked group stays a source-deletion candidate.

This is the sharp distinction from `--quick-match`, whose size+mtime heuristic is
*unverified* and so is deliberately excluded from `content_verified` (ADR 0009). Both
skip hashing, but for opposite reasons: quick-match *guesses* and forfeits deletion;
reflink *guarantees* and keeps it. Because this claim sits right on the ADR 0003 safety
invariant, it gets its own ADR (reflink structural verification vs. empirical/heuristic).

*Alternative rejected:* hashing the reflinked `.part` anyway "to be safe." It would
re-read the same shared extents — tautological — and throw away the entire speed win.
The only thing it could catch is a bug in our own invocation, which the `.part`+rename
discipline and integration tests already guard.

### D4 — Use the crate's strict `reflink()`, never `reflink_or_copy()`

`reflink-copy` exposes a strict `reflink()` that errors when cloning is unsupported,
and a `reflink_or_copy()` that silently falls back using the crate's *own* byte copy.
We use only the strict form. The crate's fallback copy would bypass our hashing,
read-back verification, and progress accounting — so every fallback must instead flow
through our audited `copy_and_hash` + `verify_part` seam. The crate is a thin,
well-tested wrapper over the platform ioctl; we adopt it rather than hand-rolling the
`unsafe` `FICLONE` call on a destructive path (a deliberate exception to the hand-rolled
ethos of ADR 0002, noted in the new ADR).

### D5 — Keep `.part` + rename discipline for reflink too

The clone targets `<final>.part`, then the existing atomic rename finalizes it. Even
though a reflink is effectively atomic (it fully succeeds or creates nothing), routing
it through `.part` means finalization, mtime stamping, cleanup-on-failure, and the
Suffixed-vs-Transferred distinction are all shared with the copy path. Practical note
for implementation: the strict `reflink()` expects the target not to exist, so a stale
`.part` from a previous aborted run must be removed before the attempt (the copy path
tolerates this today via `File::create`'s truncate); a clone failure removes the `.part`
exactly as a copy failure does.

### D6 — Independent inode is why reflink, not hard link

A reflink produces a new inode sharing extents copy-on-write, so stamping the
destination's mtime (`stamp_mtime`, gopro-telemetry D8) affects only the destination —
never the source. A hard link shares the inode and would rewrite the source card's
mtime, and would make "delete source" a mere unlink of one name. Reflink avoids both,
so the surrounding logic (mtime stamping, the confirm-then-delete flow) needs no
special-casing. This is recorded so the hard-link option isn't re-proposed later.

### D7 — Progress ticks the full size once for an instant clone

`copy_and_hash` ticks the progress bar as it streams; a reflink moves no bytes, so it
ticks nothing. `total_bytes` in `execute_inner` already counts reflinked files, so
`execute_inner` must advance the bar by the file's full size on a `Reflinked` outcome —
exactly as it already does for `SkippedIdentical` / `SkippedQuickMatch`. Concretely,
`Reflinked` joins that manual-`inc` match arm.

### D8 — Threading the `reflink` flag and the new outcome

- `reflink: bool` field on the profile config, default `true` (serde default), mirroring
  how `copy_quarantine` defaults. Resolved to an effective value at profile resolution
  where `--reflink` / `--no-reflink` are folded in — the same place `--delete-source`
  and `--quick-match` overrides are applied — so `transfer_inner` receives a plain
  `bool` and stays override-unaware.
- `--reflink` / `--no-reflink` added as a clap pair (ADR 0011), matching the existing
  `--delete-source` / `--no-delete-source` shape. `--reflink` forces on against a
  `reflink: false` profile; `--no-reflink` forces off.
- `execute` / `transfer_file` / `transfer_inner` gain a `reflink: bool` parameter,
  plumbed identically to `quick_match`.
- New `TransferOutcome::Reflinked` variant. It is added to: the `in_place_at_destination`
  and `content_verified` gate sets in `transfer.rs`; and the three exhaustive matches in
  `report.rs` — the tally (a new `reflinked` counter), the human per-file line
  (e.g. `reflinked (instant): <name>`), and the JSON status string (`"reflinked"`).

## Risks / Trade-offs

- **A well-tested crate now sits on the destructive path** → we constrain it to the
  strict `reflink()` only, keep `.part`+rename and the copy fallback, and cover both the
  clone and fallback outcomes with integration tests. No invariant depends on the crate
  doing more than "clone whole file or error."
- **CI likely can't exercise the reflink success path** (tmpfs/ext4 runners aren't CoW)
  → the fallback path is fully deterministic there and *is* the safety-critical one, so
  it's tested unconditionally; the success path is covered by a test that detects CoW
  support and skips otherwise (see Open Questions). The author's btrfs machine exercises
  it for real.
- **`delete_source: false` leaves source and library sharing extents** → this is the
  intended space-saving; a later edit to either triggers a transparent per-block CoW
  split. No correctness impact; worth a one-line note in the README.
- **Per-file failed ioctl on card imports** (D2) → negligible (`EXDEV` is immediate);
  `reflink: false` or a future `st_dev` short-circuit removes even that.

## Migration Plan

Purely additive and default-on. No config migration: absent `reflink` defaults to
`true`, so existing configs gain the fast path automatically where the filesystem
allows and fall back everywhere else. Rollback is per-run (`--no-reflink`) or persistent
(`reflink: false`); no on-disk format or library layout changes, so nothing to undo.

## Open Questions

- **Reflink success-path test strategy.** Detect CoW support at test time (attempt a
  clone in the tempdir and skip on `Err`) versus a `#[ignore]`d test run manually on
  btrfs. Leaning toward runtime detection so the test self-skips on non-CoW CI without
  silently never running. Resolve in tasks.
- **Exact `reflink-copy` API surface.** Confirm the strict function name/signature and
  its behavior when the target exists (expected: errors), to finalize the stale-`.part`
  handling in D5. Confirm during implementation against the pinned version.
