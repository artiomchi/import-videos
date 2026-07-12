# Tasks — add-cli-overrides

## 1. CLI surface

- [x] 1.1 Add paired override flags to `cli.rs`: `--copy-quarantine`/`--no-copy-quarantine` and `--gopro-require-marker`/`--no-gopro-require-marker` on `Scan` and `Import`; `--delete-source`/`--no-delete-source` on `Import` only; `--quarantine PATH` on both. Wire each pair with `overrides_with` in both directions (last-one-wins, design D1)
- [x] 1.2 Replace `--keep-source` with a hidden clap alias of `--no-delete-source` (design D2); add `conflicts_with` between `--quarantine` and `--no-copy-quarantine` (design D4)
- [x] 1.3 Add an `Overrides` struct + a helper collapsing each flag pair to `Option<bool>`; unit-test the pair semantics (neither → `None`, each direction, last-one-wins)

## 2. Profile resolution

- [x] 2.1 Apply `Overrides` in `lib.rs` right after `get_profile`: clone the profile, shadow `delete_source`, `copy_quarantine`, and `quarantine` when `Some` (design D5); resolve a relative `--quarantine` against the effective destination (same rule as config)
- [x] 2.2 Make `--quarantine` force effective `copy_quarantine: true` (design D4)
- [x] 2.3 Shadow `require_marker` into `SourceKind::Gopro`; reject either marker flag on a non-GoPro profile with the config loader's wording, `Error::Config`, exit 2 (design D5)
- [x] 2.4 Thread the effective profile through `run_scan`/`run_import` so `plan`, `transfer`, and `report` stay override-unaware; drop the now-redundant `keep_source_flag` plumbing

## 3. Tests

- [x] 3.1 Integration: `--delete-source --yes` on a `delete_source: false` profile deletes verified sources; without `--yes` (non-tty stdin) deletion is skipped with the explanatory message
- [x] 3.2 Integration: `--no-delete-source` and alias `--keep-source` both prevent deletion on a `delete_source: true` profile
- [x] 3.3 Integration: `--no-copy-quarantine` leaves an unmarked group's sources in place on a copy-enabled profile; `--copy-quarantine` re-enables on a `copy_quarantine: false` profile; `--quarantine /tmp/q` redirects the copy and forces copying on against `copy_quarantine: false`
- [x] 3.4 Integration: `--quarantine` + `--no-copy-quarantine` fails as a usage error (exit 2) before scanning; `scan --quarantine` shows the overridden path read-only
- [x] 3.5 Integration: `--no-gopro-require-marker` keeps an unmarked session; `--gopro-require-marker` quarantines it on a `require_marker: false` profile; either marker flag on a Tesla profile exits 2 with the config wording
- [x] 3.6 `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check` all clean

## 4. Docs

- [x] 4.1 Write the overridability-policy ADR: per-invocation intent vs profile identity, both-directions rule, `--keep-source` → `--no-delete-source` rename (design D7)
- [x] 4.2 Update README: flags table for `scan`/`import`, sidecar-regeneration recipe to `--quick-match --no-delete-source`, note the hidden alias
