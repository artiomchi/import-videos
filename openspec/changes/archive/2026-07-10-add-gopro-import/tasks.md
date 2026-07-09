## 1. Core plumbing (trait + sidecar support)

- [x] 1.1 Extend `ImportSource::scan` to `scan(&self, root: &Path, ignore: &GlobSet)` (design D2); update `GenericSource` and all call sites in `plan.rs`/`lib.rs`
- [x] 1.2 Add `Sidecar { filename, content: serde_json::Value }` and `MediaGroup.sidecar: Option<Sidecar>` in `src/source/mod.rs` (design D6)
- [x] 1.3 Render planned sidecars in `report.rs` plan output (spec: sidecar visible in plan before execution)
- [x] 1.4 Write the sidecar in `transfer::execute` after all group files transfer/verify; a sidecar write failure marks the group failed and blocks its source deletion
- [x] 1.5 Unit test in `transfer.rs`: sidecar written on success; sidecar failure → group failed, source retained

## 2. MP4 box walker (`src/media/mp4.rs`)

- [x] 2.1 Create `src/media/mod.rs` and `mp4.rs`; box-header walker over `Read + Seek` (u32 BE size + fourcc, 64-bit `size == 1` form, skip-by-seek), typed errors via `thiserror`
- [x] 2.2 Targeted descent: `moov/udta/HMMT` → `Vec<u32>` millisecond offsets (u32 BE count + offsets); missing box or zero count → empty, not an error
- [x] 2.3 `moov/mvhd` creation time (version 0 u32 / version 1 u64, 1904 epoch) → `jiff::Timestamp`
- [x] 2.4 Test fixture builder: handcrafted in-memory MP4 byte fixtures (helper assembling boxes) — no binary files in the repo
- [x] 2.5 Unit tests: markers parsed; no-HMMT and zero-count cases; mvhd v0/v1; 64-bit box size; truncated/garbage input fails with a typed error and no panic

## 3. GoPro source module (`src/source/gopro.rs`)

- [x] 3.1 Add `SourceKind::Gopro { require_marker: bool }` (default true) and its `build()` arm (design D1); serde round-trip test for the variant
- [x] 3.2 Reject `require_marker` on non-gopro profiles at config load with the profile named (exit 2) + test (spec: GoPro profile type)
- [x] 3.3 `detect()`: `DCIM/1*GOPRO/` containing a chapter-pattern file; tests for HERO8 layout, empty DCIM, TeslaCam root
- [x] 3.4 Chapter discovery and session grouping: `G[XH]ccnnnn.MP4` pattern, group by session across `1*GOPRO` dirs, chapters ordered by chapter number, `session` layout-context field
- [x] 3.5 Session timestamp from first chapter's mvhd creation time; mtime fallback with `tracing` warning (design D5)
- [x] 3.6 Marker extraction per chapter; unparseable chapter → zero markers + warning, scan continues (design D7)
- [x] 3.7 Verdicts: any marker → Keep whole session; none → Quarantine; `require_marker: false` → always Keep, markers still extracted (design D8)
- [x] 3.8 Ignore-glob filtering during discovery; unrecognized non-chapter files grouped under an `Ignore(reason)` verdict (design D3)
- [x] 3.9 Build `markers.json` sidecar content for Keep sessions: camera model, session, chapter files, `time_source: "camera"`, markers with file/offset_ms/camera-clock wall time

## 4. Integration tests (fake card in tempdir)

- [x] 4.1 End-to-end keep/quarantine: fake card with a marked and an unmarked session → marked lands under date layout with `markers.json`, unmarked lands in quarantine; source deleted only with `delete_source: true` + `--yes`
- [x] 4.2 Scan is read-only against the fake card; `import --dry-run` changes nothing
- [x] 4.3 Corrupt chapter file → warning, session quarantined, run exits 0 (spec: unparseable chapters degrade)
- [x] 4.4 Ignore globs (`*.LRV`/`*.THM`) and an unrecognized `.JPG`: plan contents and untouched files asserted

## 5. Docs & learning notes

- [x] 5.1 Write ADR 0002 (hand-rolled MP4/GPMF parsers vs. unmaintained crates) — recorded now that the first parser lands
- [x] 5.2 Learning note: binary parsing with std (`Read + Seek`, `from_be_bytes`, typed parse errors) → `docs/learning/`, indexed in its README
- [x] 5.3 README: GoPro profile example and a short "what gets kept" note; update AGENTS.md only if conventions changed

## 6. Quality gates

- [x] 6.1 `cargo test` green, `cargo clippy -- -D warnings` clean, `cargo fmt --check` clean
- [x] 6.2 Manual smoke test against a real HERO8 card (`scan --source <card>`): HiLight counts match GoPro Quik; record findings (multi-directory sessions, D3 open question) in the change
