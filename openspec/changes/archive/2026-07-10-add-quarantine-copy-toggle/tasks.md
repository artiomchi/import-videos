## 1. Config

- [x] 1.1 Add `copy_quarantine: bool` to `Profile` and `RawProfile` in `src/config.rs`, with a `#[serde(default = ...)]` helper returning `true`; carry it through `validate_profile` into `Profile`.
- [x] 1.2 Unit test: a profile omitting `copy_quarantine` loads as `true`; a profile with `copy_quarantine: false` loads as `false`.
- [x] 1.3 Update the `RawProfile` serde round-trip tests to include the new field.

## 2. Planning

- [x] 2.1 In `build_plan` (`src/plan.rs`), resolve `quarantine_path: None` for `Quarantine` groups when `profile.copy_quarantine` is false; keep the existing resolution when true. Update the `PlannedAction` doc comment to explain that a `Quarantine` action with `None` path means "report, but leave the source untouched."
- [x] 2.2 Unit test: a `Quarantine` group under a `copy_quarantine: false` profile plans with `quarantine_path == None`; under a `true`/default profile it still resolves the quarantine path.

## 3. Execution

- [x] 3.1 Add a `TransferOutcome` variant for a quarantined file left in place (e.g. `SkippedQuarantineDisabled`); ensure it is NOT counted by `outcome_is_success` so it can never make a group a source-deletion candidate.
- [x] 3.2 In `execute` (`src/transfer.rs`), when a `Quarantine` action has no target directory, record the "left in source" outcome per file and transfer nothing; leave `Keep`/`Ignore` and the enabled-quarantine path unchanged.
- [x] 3.3 Verify the source-deletion gate already excludes these groups (empty/non-success files); add no new deletion logic.

## 4. Reporting

- [x] 4.1 In `render_plan` (`src/report.rs`), render a `Quarantine` group with no path as `QUARANTINE` with a "quarantine copy disabled" note instead of a `-> path`, keeping the summary counts correct.
- [x] 4.2 In `render_results`, render the new left-in-source outcome with a clear per-file message.
- [x] 4.3 Unit test the disabled-copy plan rendering (verbose) and results rendering.

## 5. Integration tests

- [x] 5.1 `tests/integration.rs`: with `copy_quarantine: false`, a `Quarantine` group is not copied, no quarantine directory is created, the source file remains byte-for-byte, and its outcome is the left-in-source variant.
- [x] 5.2 `tests/integration.rs`: same disabled profile with `delete_source: true` + confirmation — the quarantined source is NOT deleted while an eligible `Keep` group is cleaned.
- [x] 5.3 `tests/gopro_import.rs`: end-to-end run with `copy_quarantine: false` leaves the unmarked session on the card and creates no `_quarantine` folder, while the marked session imports normally.
- [x] 5.4 Update the `profile(...)` test helper in `tests/integration.rs` for the new field.

## 6. Docs and decisions

- [x] 6.1 Add `copy_quarantine` to the README common-fields table and the GoPro "What gets kept" note (still reported as QUARANTINE, left on source).
- [x] 6.2 Decide the ADR question from design (Open Questions): add a brief ADR refining ADR 0003's "quarantine = verified copy" wording if warranted; otherwise note the decision was deferred.

## 7. Quality gates

- [x] 7.1 Run `cargo build`, `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check` and resolve any findings.
