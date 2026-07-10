## 1. Plumbing

- [x] 1.1 Add global `--json` flag to `Cli` (`src/cli.rs`), alongside `--config`/`-v`
- [x] 1.2 Move config loading from `run_inner` into the command arms that need it, so `inspect` runs config-free (design D5)
- [x] 1.3 Extract quarantine-root resolution (`profile.quarantine` or `{destination}/_quarantine`) from `plan.rs` into a shared method on `Profile`, used by both planning and cleanup (design D1)
- [x] 1.4 Generalize `confirm_deletion` into a shared `confirm(prompt, assume_yes, is_tty)` helper reusable by cleanup (design D7), keeping existing non-TTY/decline semantics and tests

## 2. JSON report output (cli-core delta)

- [x] 2.1 Add serialize view-models in `report.rs` (`PlanJson`, `ResultsJson`) mapped from `ImportPlan`/`ExecuteReport`: verdicts, group names, resolved paths as strings, RFC 3339 timestamps in the configured timezone, summary counts; quarantined entries always included (design D4)
- [x] 2.2 Wire `scan`/`import --dry-run`/`import` in `lib.rs` to emit exactly one JSON document on stdout under `--json`, suppressing informational lines; "no sources found" becomes a JSON document with exit 0
- [x] 2.3 Unit tests pinning the JSON shape (snapshot-style asserts on serialized output) and covering the no-sources document
- [x] 2.4 Integration test: `import --json --yes` end-to-end over a tempfile card — stdout parses as a single JSON document with per-file outcomes; confirmation rules unchanged under `--json`

## 3. Cleanup command (cli-maintenance)

- [x] 3.1 Add `Cleanup { profile, older_than, dry_run, yes }` to the CLI and dispatch in `lib.rs`
- [x] 3.2 Implement `src/cleanup.rs` plan phase: enumerate immediate quarantine entries (group dirs + stray files) with name, age, and total size — read-only; empty/absent quarantine reports "nothing to clean", exit 0
- [x] 3.3 Parse `--older-than` as a jiff friendly `Span` (usage error exit 2 on garbage) and filter entries by quarantine age: group-dir mtime, not recording-stamped file mtimes; stray files use their own mtime (design D2/D3)
- [x] 3.4 Safety check: refuse (exit 2, nothing deleted) when the resolved quarantine root equals or contains the destination root
- [x] 3.5 Execute phase: confirmation via the shared helper, then delete planned entries; human rendering (plan, kept-vs-purged, summary) plus `CleanupJson` view-model for `--json`
- [x] 3.6 Integration tests over tempfile quarantine layouts (destructive path — required by AGENTS.md): dry-run deletes nothing; `--older-than` retains a young-dir/old-file-mtimes group and purges an old one; non-interactive without `--yes` fails; declined prompt aborts; destination siblings untouched; safety refusal case

## 4. Inspect command (cli-maintenance)

- [x] 4.1 Add `Inspect { path }` to the CLI; dispatch on the argument — `.mp4` file, directory containing `event.json`, or `event.json` path; anything else exits 2 with supported-inputs message (design D5)
- [x] 4.2 MP4 dump: HiLight count + per-marker raw ms offsets and derived timestamps, creation time, GPS summary (first usable fix, clock offset vs creation time, sample count) when a gpmd track exists; read-only reuse of `media/mp4.rs` + `media/gpmf.rs`
- [x] 4.3 Partial-failure behavior: print sections that parsed, report the failing section's error, exit 1
- [x] 4.4 Tesla dump: parsed `event.json` fields (timestamp, reason, city, coordinates) + clip files present in the folder
- [x] 4.5 Timestamps via `TimeZone::system()` with UTC fallback; verify `inspect` works with no config file present
- [x] 4.6 `InspectJson` view-model for `--json` (raw offsets alongside rendered timestamps)
- [x] 4.7 Tests: synthetic MP4 fixture (HMMT + gpmd, reusing existing test builders) and a synthetic Tesla event folder; corrupt-gpmd partial-output case; unsupported-input exit 2

## 5. Transfer progress

- [x] 5.1 Add `indicatif` to Cargo.toml
- [x] 5.2 Add a `Progress` wrapper in `transfer.rs` owning `Option<ProgressBar>`; `execute` takes it as a parameter; CLI constructs a real bytes-style bar only when stdout is a TTY and `--json` is off, hidden target otherwise (design D6)
- [x] 5.3 Tick the bar from the existing chunked copy loop in `transfer_file` (copy phase) and the verify re-hash pass
- [x] 5.4 Test that piped/JSON output contains no progress or terminal-control bytes (run execute with the hidden/no-op progress path)

## 6. Documentation

- [x] 6.1 Write ADR 0010: cleanup age is quarantine arrival time (group-dir mtime), not recording time — consequence of the ADR 0008 mtime stamping
- [x] 6.2 Complete README: full CLI reference (scan/import/cleanup/inspect, global flags), config reference, example workflows, `--json` field documentation with "v0, may evolve" note
- [x] 6.3 Learning note if a new Rust concept lands (candidate: serde view-model structs vs deriving on domain types — serialization as a public contract); tidy `docs/learning/README.md` index either way

## 7. Quality gates

- [x] 7.1 `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check` all clean
- [x] 7.2 Manual smoke test: `inspect` against a real GoPro MP4 and Tesla event folder; `cleanup --dry-run` against the real quarantine directory
