# add-core-cli — Tasks

## 1. Scaffold

- [x] 1.1 `cargo init` (lib + bin: `src/lib.rs`, thin `src/main.rs`), edition 2024; add dependencies from proposal Impact (clap derive, serde, serde_yaml_ng, serde_json, jiff, blake3, globset, thiserror, anyhow, tracing, tracing-subscriber, directories; dev: tempfile); extend `.gitignore` for `target/`
- [x] 1.2 Library error type in `src/lib.rs` (or `src/error.rs`): thiserror enum with path-carrying variants (`Io { path, source }`, `Config`, `Template`, `VerifyMismatch`) per design D7; `main.rs` maps errors to exit codes 0/1/2
- [x] 1.3 CLI skeleton in `src/cli.rs`: `scan`/`import` subcommands with `--source`, `--dry-run`, `--keep-source`, `--yes`, global `--config` and `-v/-vv`; tracing-subscriber init keyed off verbosity

## 2. Core types and trait

- [x] 2.1 `src/source/mod.rs`: `MediaFile`, `Marker`, `MediaGroup` (files, timestamps, optional geo, template context map), `Verdict` (`Keep`/`Quarantine`/`Ignore(reason)`)
- [x] 2.2 `ImportSource` trait (`detect`, `scan`) + registry mapping profile `type` → implementation via `Box<dyn ImportSource>` (design D3); core has zero device-specific logic (spec: ImportSource trait requirement)

## 3. Configuration

- [x] 3.1 `src/config.rs`: `Config` (profiles map, `mount_roots`), `Profile` common fields + `#[serde(flatten)] SourceKind` internally tagged on `type` (design D1); tilde expansion for paths
- [x] 3.2 Load + validate: named errors for unreadable file/invalid YAML/unknown type/invalid glob (spec: config requirement); unit tests incl. serde round-trip per profile kind (design risk)
- [x] 3.3 Layout template parser: `{field}` / `{field:%strftime}` → token list at load, resolution against group context + jiff date at plan time (design D2); load-time rejection tests for malformed templates, resolution-time error for unknown fields

## 4. Planning

- [x] 4.1 Source resolution: explicit path > `--source` override > `auto` mount-root probing with `detect()` (design D6); missing explicit path fails per spec
- [x] 4.2 `ImportPlan` / `PlannedAction` with fully resolved destination and quarantine paths (design D4); plan building from `Vec<MediaGroup>` + profile
- [x] 4.3 `src/report.rs`: human-readable rendering of a plan (verdict, reason, resolved path per group) and of execution results (transferred / skipped-identical / suffixed / failed / deleted-from-source)

## 5. Transfer engine

- [x] 5.1 `src/transfer.rs`: stream copy to `<final>.part` hashing source, re-hash written file, rename on match; remove `.part` and keep source on any failure (design D5, spec: verified transfer)
- [x] 5.2 Collision handling: identical hash → skip as already-imported; different → numeric suffix, never overwrite (spec: collisions)
- [x] 5.3 Quarantine moves and source deletion strictly after verification/already-imported confirmation; honor `delete_source` and `--keep-source` (spec: source deletion)
- [x] 5.4 Confirmation prompt before source deletion: stdin y/N, `--yes` bypass, non-tty without `--yes` skips deletion with message (design D8, spec: destructive steps)

## 6. Wiring and integration tests

- [x] 6.1 Wire `scan` and `import` end-to-end in `src/lib.rs` with a stub-free path: no sources found → message + exit 0; `--dry-run` prints plan only
- [x] 6.2 `tests/` integration suite with a test-only `ImportSource` impl over tempdirs covering the spec scenarios: scan read-only, dry-run no-op, plan==execution, verify-failure keeps source, idempotent re-run, different-content suffixing, keep-source override, non-tty deletion skip, exit codes
- [x] 6.3 README skeleton: what/why, install (`cargo install --path .`), config example from spec, scan-before-import workflow

## 7. Docs and quality gates

- [x] 7.1 Learning notes (`docs/learning/` + index): errors-thiserror-vs-anyhow; lib/bin split; trait-objects-vs-generics (design D3) — each tied to the code that uses it
- [x] 7.2 Quality gates green: `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`
