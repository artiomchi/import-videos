# add-core-cli — Proposal

## Why

The project currently has no code — only decisions (ADRs 0001, 0003–0005) and a roadmap (`docs/ROADMAP.md`). Every later changeset (GoPro import, GPS telemetry, Tesla import) needs the same foundation: a CLI with configuration, the scan → plan → execute pipeline, and a transfer engine that is safe to point at real footage. Building that foundation once, device-agnostic, is this change.

## What Changes

- New cargo package `import-videos`: `src/lib.rs` (all logic) + thin `src/main.rs` (ADR 0005).
- CLI skeleton (clap derive): `scan [PROFILE] [--source PATH]` and `import [PROFILE] [--source PATH] [--dry-run] [--keep-source] [--yes]`, plus `--config` and `-v/-vv` globals. Commands run end-to-end but report "no matching sources" until device modules exist.
- YAML configuration (ADR 0004): named profiles with `type`, `source` (`auto` or path), `destination`, `layout` path template, `ignore` globs, `quarantine`, `delete_source`. Validated at load, including layout template syntax.
- Core domain types and the `ImportSource` trait (ADR 0005): `MediaGroup`, `MediaFile`, `Marker`, `Verdict` (`Keep`/`Quarantine`/`Ignore`), `ImportPlan`.
- Transfer engine (ADR 0003): copy → blake3 verify → delete-source, quarantine moves, destination collision handling. Device-agnostic and fully covered by integration tests on temp dirs.
- Human-readable report rendering of an `ImportPlan` (scan / dry-run output) and of execution results.
- README skeleton (what/why/install/config example); learning notes for concepts introduced here (thiserror-vs-anyhow, lib/bin split).

No device support in this change — GoPro and Tesla are the next two changesets.

## Capabilities

### New Capabilities
- `cli-core`: the CLI surface (scan/import), YAML profile configuration, the scan → plan → execute pipeline contract (`ImportSource` trait and plan types), and the verified-transfer engine with quarantine semantics.

### Modified Capabilities
_None — this is the first capability._

## Impact

- **New code**: `Cargo.toml`, `src/main.rs`, `src/cli.rs`, `src/config.rs`, `src/transfer.rs`, `src/report.rs`, `src/source/mod.rs`, `tests/` (transfer + config integration tests).
- **New dependencies**: `clap`, `serde`, `serde_yaml_ng`, `serde_json`, `jiff`, `blake3`, `globset`, `thiserror`, `anyhow`, `tracing`, `tracing-subscriber`, `directories`; dev: `tempfile`. (`indicatif` deferred to the maintenance/polish changeset.)
- **Docs**: README skeleton, `docs/learning/` notes. No new ADRs — this change implements decisions already recorded.
- **Existing systems**: none affected; first code in the repo.
