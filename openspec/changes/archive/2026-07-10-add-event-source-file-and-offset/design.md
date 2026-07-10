## Context

The unified `import.json` (from `unify-timestamps-and-sidecars`) already emits `events[]` with `offset_ms` for GoPro markers, built during **scan** in `src/source/gopro.rs` and serialized in `src/source/sidecar.rs`. Marker attribution is already available at the point of construction: `build_session` loops over chapters with the chapter `path` in hand when it pushes each `MarkerHit`, so the owning file is known but currently discarded. The human-readable offset is a pure rendering of the `offset_ms` we already carry.

The `--quick-match` half touches the one destructive part of the crate, `src/transfer.rs`. Today `transfer_inner` unconditionally hashes the source, then `resolve_destination` hashes any existing destination to decide `SkippedIdentical` vs a suffixed write. A single predicate, `outcome_is_success`, decides both whether a group's sidecar is written (`all_files_ok`) and whether the group is a source-deletion candidate. Quick-match must keep the former true while forcing the latter false — otherwise a fast, unverified skip could green-light deleting the original (contradicting ADR 0003's verified-transfer safety model).

Destination files this tool wrote already carry a stamped mtime equal to their `recorded_at` (`stamp_mtime`), so quick-match has a strong, self-produced signal to match against — and existing imports are matchable without re-hashing.

## Goals / Non-Goals

**Goals:**
- Attribute each GoPro marker event to the chapter `file` it was pressed in.
- Render each marker's position as a human `offset` string alongside `offset_ms`.
- Add `--quick-match` so re-running `import` can skip content hashing on name+size+mtime agreement, with mtime compared within 0.1 s to absorb filesystem truncation.
- Preserve the ADR 0003 safety invariant: a source file is deleted only when its content was actually verified at the destination.
- Make `import --quick-match --keep-source` a cheap way to regenerate `import.json` (rebuilt from source metadata) for an existing import.

**Non-Goals:**
- No dedicated `regen-sidecars` subcommand — `--quick-match` covers the need.
- No `file` field for Tesla events (their clips are synchronized around one trigger; no single file owns the event).
- No change to how markers, telemetry, or timestamps are computed.
- No hour component in `offset` (whole minutes only).

## Decisions

### D1: Split the transfer outcome predicate in two
Replace the single `outcome_is_success` with two predicates so sidecar-writing and deletion-eligibility can diverge:
- **in-place-at-destination** (`Transferred | SkippedIdentical | Suffixed | SkippedQuickMatch`) — gates whether the group's sidecar is written and whether the group counts as "handled" for reporting.
- **content-verified** (`Transferred | SkippedIdentical | Suffixed`) — the deletion gate; excludes `SkippedQuickMatch` and, as today, `SkippedQuarantineDisabled`.

*Rationale:* the invariant "delete only what was verified" becomes a property of one small, testable predicate. *Alternative considered:* a boolean field on `FileResult` carried alongside the outcome — rejected as redundant with the outcome enum, which is already the source of truth.

### D2: `SkippedQuickMatch` as a distinct outcome
Add `TransferOutcome::SkippedQuickMatch`, mirroring the treatment of `SkippedQuarantineDisabled`: reported distinctly, excluded from the deletion gate. Reusing `SkippedIdentical` was rejected because that variant means "content-verified identical" and *is* deletion-eligible — overloading it would silently make quick-matched files deletable.

### D3: Quick-match check placement and criteria
In `transfer_inner`, before hashing, when quick-match is enabled and `recorded_at` is `Some`: stat the canonical destination path (`dest_dir.join(file_name)`); if it exists, its size equals the source's, and its mtime equals `recorded_at` within 0.1 s, return `SkippedQuickMatch`. Any miss falls through unchanged, including collision suffixing. Quick-match only considers the canonical name — it never inspects suffixed variants — keeping "already imported here" unambiguous. Files with no `recorded_at` can't be matched and always fall through to hashing.

*mtime comparison:* read `fs::metadata(dest).modified()` as `SystemTime`, convert to `jiff::Timestamp`, compare `(dest_mtime - recorded_at).abs() <= 100ms`. The 0.1 s tolerance covers filesystems that truncate sub-second precision (e.g. 1 s granularity).

### D4: Thread `quick_match` as a plain bool
Carry the flag `cli.rs → lib.rs run → execute → transfer_file → transfer_inner`, parallel to `keep_source`/`assume_yes`. Quick-match applies uniformly to Keep and Quarantine transfers since both go through `transfer_file` and both stamp mtime — no special-casing per verdict.

### D5: `file` on `EventEntry`, `offset` derived in the builder
Add `MarkerHit.file: String` (captured from the chapter `path` in the existing loop) and `EventEntry.file: Option<String>`. The builder emits `offset` whenever `offset_ms` is `Some`, computed by a small `format_offset(ms: u32) -> String` helper — so `offset` and `offset_ms` cannot drift, and no separate `EventEntry` field is needed. Tesla passes `file: None`, leaving its behavior untouched.

*`offset` format:* `M:SS.mmm` — whole minutes with no leading zero and no hour rollover, two-digit seconds, three-digit milliseconds (`734120 → "12:14.120"`, `5000 → "0:05.000"`).

### D6: Record the safety trade-off in an ADR
Add ADR 0009 refining ADR 0003 (as ADR 0007 did for `copy_quarantine`): `--quick-match` deliberately substitutes a name+size+mtime heuristic for content verification and, in exchange, forfeits deletion eligibility. Do not rewrite ADR 0003.

## Risks / Trade-offs

- **False-positive match (same name+size+mtime, different bytes)** → The destination keeps an unverified copy, but the source is *never* deleted for quick-matched files (D1), so the original always survives as the source of truth. Worst case is a stale sidecar, not data loss.
- **Filesystem mtime coarseness beyond 0.1 s** (e.g. FAT's 2 s granularity) → Such files simply miss and fall through to a full verified transfer — correct, just not fast. Documented as expected degradation rather than worked around.
- **Predicate split touches the deletion path** → Mitigation: an integration test asserting a fully quick-matched group with `delete_source: true` + confirmation leaves every source file in place, alongside the existing verified-delete test.
- **`offset` formatting drift** → Mitigation: derive `offset` solely from `offset_ms` in one helper; unit-test boundary values (0, sub-minute, exactly 60 s, >60 min).

## Migration Plan

- Additive schema: `file` and `offset` are new optional fields; existing consumers ignore them.
- Backfill for already-imported footage: re-insert the source and run `import <profile> --quick-match --keep-source`; sidecars are recomputed from source metadata and rewritten while video files are matched cheaply. Requires the source still present and the destination resolving to the same directory (unchanged `timezone`/layout).
- Rollback: `--quick-match` is opt-in and off by default; omitting the flag restores full verification. The schema additions are inert if unused.

## Open Questions

- Report wording for the new outcome — e.g. `quick-matched (not verified)` — to be finalized when updating `src/report.rs`; it must read as clearly distinct from the verified `already imported` skip.
