## Why

The unified `import.json` `events[]` schema records *when* an event happened but not *which file it came from*, so when a kept group spans multiple files, the event list can't be traced back to a specific clip. For GoPro this loses real information — each HiLight marker belongs to one chapter of a multi-chapter session. The millisecond `offset_ms` is also unfriendly to read: a human scanning the sidecar can't tell that `734120` means ~12 minutes in. And once the schema grows these fields, users need a way to refresh `import.json` for footage they already imported **without** re-hashing gigabytes of video — the existing verified-transfer path re-reads and re-checksums every file.

## What Changes

- Add an optional per-event **source-file identifier** to `events[]`. For GoPro each `gopro:marker` event carries the base name of the chapter it came from; Tesla events, whose clips are time-synchronized around one trigger, omit it (no single source file applies).
- Add a **human-readable offset** to marker events alongside the existing `offset_ms`: a `min:sec.ms` string (e.g. `12:14.120`) so the sidecar is legible without arithmetic. `offset_ms` stays for machine consumers.
- Add a `--quick-match` flag to `import`. When set, a destination file whose **name, size, and mtime** match what this run would produce is accepted as already-imported and its full-content blake3 verification is skipped. Quick-matched files get a distinct outcome that is **excluded from the source-deletion gate** — trading verification for speed forfeits the right to delete the source (refines ADR 0003).
- As a consequence, `import <profile> --quick-match --keep-source` becomes a cheap way to **regenerate `import.json`** for an existing import: the sidecar is rebuilt from source metadata (small MP4 atoms, telemetry, `event.json`) and rewritten, while video files are matched cheaply rather than re-hashed.

## Capabilities

### New Capabilities
<!-- None. This extends existing capabilities. -->

### Modified Capabilities
- `unified-sidecar`: `events[]` entries gain an optional source-file field and marker entries gain a human-readable offset string; the `offset_ms`/human-offset relationship is specified.
- `gopro-import`: `gopro:marker` events record their originating chapter file and the human-readable offset.
- `cli-core`: new `--quick-match` flag on `import`; a quick-matched file is treated as already-imported for reporting but MUST NOT become a source-deletion candidate, keeping deletion gated on real content verification.

## Impact

- **Sidecar schema** (`src/source/sidecar.rs`): new optional event fields; example output in `README.md`.
- **GoPro source** (`src/source/gopro.rs`): thread each marker's chapter file into its event entry; format the human offset.
- **Transfer engine** (`src/transfer.rs`): new `SkippedQuickMatch` outcome excluded from `outcome_is_success`'s deletion gate; a stat-based fast path in `transfer_inner` before hashing; `--quick-match` wired through `src/cli.rs` and `src/lib.rs`.
- **Docs**: an ADR refining ADR 0003 for the verification/deletion trade-off; a learning note if the fast-path introduces a new Rust concept.
- **Tests**: quick-match hit/miss and the deletion-safety invariant (a quick-matched group is never deleted); sidecar assertions for the new event fields.
