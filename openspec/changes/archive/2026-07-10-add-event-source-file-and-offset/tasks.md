## 1. Sidecar schema: source file and human offset

- [x] 1.1 Add `file: Option<String>` to `EventEntry` in `src/source/sidecar.rs`; emit it into the event JSON only when `Some`.
- [x] 1.2 Add a `format_offset(ms: u32) -> String` helper rendering `M:SS.mmm` (whole minutes, no hour rollover, two-digit seconds, three-digit milliseconds); in the builder emit `offset` whenever `offset_ms` is `Some`, computed from the same value.
- [x] 1.3 Unit-test `format_offset` at boundaries: 0 (`0:00.000`), 5000 (`0:05.000`), 60000 (`1:00.000`), 734120 (`12:14.120`), and a value over 60 min (no hour component).

## 2. GoPro marker attribution

- [x] 2.1 Add `file: String` to `MarkerHit` in `src/source/gopro.rs`; capture the chapter base name from the loop's `path` when pushing each hit.
- [x] 2.2 In `build_sidecar`, set each `gopro:marker` `EventEntry.file` to the hit's chapter name (keep `offset_ms` as-is so `offset` is derived).
- [x] 2.3 Confirm Tesla event construction in `src/source/tesla.rs` passes `file: None` (no behavior change).

## 3. Transfer engine: predicate split and quick-match

- [x] 3.1 Add `TransferOutcome::SkippedQuickMatch` in `src/transfer.rs`.
- [x] 3.2 Replace `outcome_is_success` with two predicates: `in_place_at_destination` (adds `SkippedQuickMatch`) gating sidecar-writing and "handled" reporting, and `content_verified` (excludes `SkippedQuickMatch` and `SkippedQuarantineDisabled`) gating source deletion; rewire `all_files_ok` and the deletion `any_eligible`/`all_ok` checks accordingly.
- [x] 3.3 Thread a `quick_match: bool` through `execute` → `execute_inner` → `transfer_file` → `transfer_inner`, parallel to `keep_source`/`assume_yes`.
- [x] 3.4 In `transfer_inner`, before hashing: when `quick_match` and `recorded_at` is `Some`, stat `dest_dir.join(file_name)`; if it exists with matching size and mtime within 0.1 s of `recorded_at`, return `SkippedQuickMatch`; otherwise fall through unchanged.
- [x] 3.5 Add a `SystemTime`→`jiff::Timestamp` conversion for the destination mtime and the 0.1 s (100 ms) tolerance comparison.

## 4. CLI wiring

- [x] 4.1 Add `--quick-match` to the `import` subcommand in `src/cli.rs`.
- [x] 4.2 Pass the flag from `src/lib.rs`'s run path into `execute`.

## 5. Reporting

- [x] 5.1 Render `SkippedQuickMatch` in `src/report.rs` with a note distinct from the verified `already imported` skip (e.g. `quick-matched (not verified)`), and ensure such files are not shown as deletion candidates.

## 6. Docs and decisions

- [x] 6.1 Add ADR 0009 (accepted, refining ADR 0003): `--quick-match` trades content verification for a name+size+mtime heuristic and forfeits deletion eligibility; do not rewrite ADR 0003.
- [x] 6.2 Update `README.md`: `import.json` example showing `file`/`offset` on a marker event, and a `--quick-match` usage note including the sidecar-regeneration recipe.
- [x] 6.3 Add or extend a `docs/learning/` note if the `SystemTime`↔`jiff` conversion or the two-predicate safety split introduces a concept new to this codebase.

## 7. Tests

- [x] 7.1 GoPro import test: a multi-chapter session's markers each carry their own chapter `file`, plus `offset_ms` and the expected `offset` string.
- [x] 7.2 Quick-match hit test: `import --quick-match` over an already-imported source skips without hashing, reports the distinct outcome, and rewrites `import.json`.
- [x] 7.3 Quick-match miss test: a size or mtime difference falls through to verified transfer (and collision handling).
- [x] 7.4 Safety invariant test: a fully quick-matched group with `delete_source: true` and confirmation leaves every source file in place.

## 8. Quality gates

- [x] 8.1 Run `cargo test`, `cargo clippy -- -D warnings`, and `cargo fmt --check`; all green.
