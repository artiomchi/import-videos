## 1. Dependency and config

- [x] 1.1 Add the `reflink-copy` crate to `Cargo.toml`; pin the version and confirm the strict `reflink()` function name/signature and its behavior when the target already exists (design D5 open question)
- [x] 1.2 Add a `reflink: bool` field to the profile config in `src/config.rs`, defaulting to `true` via serde (mirror how `copy_quarantine` defaults); ensure an omitted field loads as `true`

## 2. Transfer outcome and gates

- [x] 2.1 Add `TransferOutcome::Reflinked` to `src/transfer.rs` with a doc comment stating it is verified by construction (design D3)
- [x] 2.2 Include `Reflinked` in `in_place_at_destination` and `content_verified` so a reflinked file is deletion-eligible (design D3, D8)

## 3. Reflink fast path in the transfer engine

- [x] 3.1 Thread an effective `reflink: bool` through `execute` → `transfer_file` → `transfer_inner`, plumbed identically to `quick_match` (design D8)
- [x] 3.2 At the copy site in `transfer_inner` (after quick-match, collision resolution, and the identical-content skip), when `reflink` is enabled attempt a strict clone of the source into `<final>.part`; on success rename, stamp mtime, and return `Reflinked` (design D1, D5)
- [x] 3.3 On any clone error, remove any partial `.part` and fall through to the existing `copy_and_hash` + `verify_part` path unchanged; log the fallback reason at debug (design D2, D4)
- [x] 3.4 Remove a stale `<final>.part` before attempting the clone so the strict `reflink()` does not fail on a pre-existing target (design D5)
- [x] 3.5 In `execute_inner`, advance the progress bar by the file's full size on a `Reflinked` outcome, alongside the existing `SkippedIdentical` / `SkippedQuickMatch` arm (design D7)

## 4. CLI overrides

- [x] 4.1 Add `--reflink` / `--no-reflink` as a clap pair on `import` in `src/cli.rs`, matching the `--delete-source` / `--no-delete-source` shape (design D8, spec: Per-invocation profile overrides)
- [x] 4.2 Fold the override into the effective `reflink` value at profile resolution — the same place `--delete-source` and `--quick-match` are applied — so `transfer_inner` stays override-unaware

## 5. Reporting

- [x] 5.1 Render `Reflinked` in the three exhaustive matches in `src/report.rs`: a distinct `reflinked` tally counter, the verbose per-file line (e.g. `reflinked (instant): <name>`), and the JSON status string (`"reflinked"`)
- [x] 5.2 Add the reflinked count to the default summary line, counted distinctly from stream-copied transfers, and keep `Reflinked` out of the per-file lines shown by default (spec: Human-readable execution report)

## 6. Tests

- [x] 6.1 Deterministic fallback test: with reflink enabled but the destination on a non-CoW/other filesystem (or forced-error path), the file is transferred via stream-copy-and-read-back and reported as `Transferred`, source untouched (spec: Cross-device transfer falls back)
- [x] 6.2 `--no-reflink` / `reflink: false` test: no clone attempted, every file stream-copied (spec: Reflink disabled always stream-copies; Reflink override forces cloning off)
- [x] 6.3 CoW success-path test using runtime detection: attempt a clone in the tempdir and skip the test when unsupported; when supported, assert the outcome is `Reflinked`, the destination is byte-identical, no `.part` remains, mtime is stamped, and the source is independent of the destination (design D3, D6, Open Questions)
- [x] 6.4 Deletion-eligibility test: a reflinked group with `delete_source: true` and confirmation deletes the source (spec: Reflinked files are deletion candidates) — runtime-skipped like 6.3 if CoW is unavailable
- [x] 6.5 Config default test: omitted `reflink` loads as `true`; `reflink: false` loads as disabled (spec: Reflink defaults to enabled / can be disabled)
- [x] 6.6 Override resolution test: `--reflink` / `--no-reflink` flip the effective value against the opposite profile setting (spec: Reflink override scenarios)
- [x] 6.7 Reporting test: a run mixing reflinked and stream-copied files shows the reflinked count distinctly in the summary and lists neither per file by default (spec: Reflinked files are counted distinctly)

## 7. Docs and decision records

- [x] 7.1 Write a new ADR: reflink structural verification vs. empirical/heuristic verification, why it keeps source-deletion eligibility (unlike `--quick-match`, ADR 0009), why reflink over hard link (design D6), and the deliberate use of the `reflink-copy` crate rather than a hand-rolled ioctl (exception to ADR 0002); update `docs/adr/README.md` index
- [x] 7.2 Update `README.md`: config table (`reflink` field), flag reference (`--reflink` / `--no-reflink`), and a note that `delete_source: false` leaves source and library sharing extents until one is edited
- [x] 7.3 Add a learning note in `docs/learning/` if reflink/CoW or the crate's FFI surface proves instructive (ADR 0001)

## 8. Quality gates

- [x] 8.1 `cargo test`, `cargo clippy -- -D warnings`, and `cargo fmt --check` all pass
- [x] 8.2 Manually verify on the author's btrfs staging area: a same-filesystem import reflinks (near-instant, reported as reflinked) and a card import still stream-copies
