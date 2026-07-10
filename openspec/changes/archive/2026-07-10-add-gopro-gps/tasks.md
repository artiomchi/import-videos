# add-gopro-gps — Tasks

## 1. GPMF KLV parser (`src/media/gpmf.rs`, design D1)

- [x] 1.1 Create `src/media/gpmf.rs` with `GpmfError` and the `KlvIter<'a>`/`Klv<'a>` borrowing iterator: key, type, struct size, repeat count, borrowed value slice, 4-byte alignment; child iteration for nested (type `0x00`) containers; malformed lengths return errors, unknown keys are skipped
- [x] 1.2 Typed value accessors: `i32`/`u32`/`u16` arrays for `GPS5`/`GPSF`/`GPSP`, `SCAL` divisors, and `GPSU`'s `yymmddhhmmss.sss` string → `jiff::Timestamp` (UTC)
- [x] 1.3 `parse_gps_payload(&[u8]) -> Result<Option<GpsPayload>>` extracting scaled GPS5 samples, GPSU, GPSF, GPSP from the GPS stream; `usable()` gate per D3 (`GPSF ≥ 2 && GPSP ≤ 500`)
- [x] 1.4 Unit tests against synthetic KLV bytes: SCAL scaling, GPSU parsing, nested containers, unknown-stream skipping, truncated/garbage payloads erroring without panic (spec: GPMF KLV parsing, Fix-quality gating)

## 2. gpmd track sample index (`src/media/mp4.rs`, design D2)

- [x] 2.1 Extend the box walker to iterate sibling boxes (multiple `trak`s), locate the track with `mdia/hdlr` type `meta` and `stsd` entry `gpmd`; return a clean `None` when absent
- [x] 2.2 Parse `stsz`, `stsc` + `stco`/`co64`, `stts` + `mdhd` timescale into `Vec<GpmdSample { offset, size, time_s, duration_s }>`, honoring sample-to-chunk mapping; malformed tables error without panic
- [x] 2.3 On-demand payload reader (`read_exact` at sample offset), read-only over the source
- [x] 2.4 Unit tests with synthetic `moov` fixtures: track selection among video/audio/meta tracks, index offsets/times from multi-chunk layouts, corrupt-table errors (spec: GPMF track discovery, Telemetry sample index)

## 3. Session telemetry in the GoPro source (`src/source/gopro.rs`, design D3–D7)

- [x] 3.1 Telemetry lookup type (e.g. `ChapterTelemetry`) combining the sample index and payload parsing; derive the session clock offset from the first usable `GPSU` payload across chapters in chapter order (D4)
- [x] 3.2 Wire into `build_session`: corrected session timestamp and marker wall times when an offset exists; any telemetry failure → `tracing::warn!` and unchanged changeset-2 behavior; verdicts untouched (D6)
- [x] 3.3 Marker → GPS mapping (D5): covering payload by stream time, nearest uniform-spaced GPS5 sample, ±2-payload fallback search, coordinates omitted when nothing usable
- [x] 3.4 Sidecar shape (D7): `"time_source": "gps"` with `clock_offset_s` and per-marker `utc` (+ `lat`/`lon` when present); camera fallback byte-identical to today; set `MediaGroup.geo` and per-chapter `MediaFile.recorded_at`
- [x] 3.5 Unit tests: offset math (drift + local-time absorption), offset from a later chapter, marker mapping edge cases, GPS vs camera sidecar shape (spec: Session clock offset, Marker coordinates, sidecar requirement)

## 4. mtime stamping in transfer (`src/transfer.rs`, design D8)

- [x] 4.1 Add `recorded_at: Option<Timestamp>` to `MediaFile` (update all constructors); in `transfer_inner`, after the verified rename, set destination mtime via `File::set_times` — warn-and-continue on failure, no stamping for `SkippedIdentical`
- [x] 4.2 Unit tests: mtime matches `recorded_at` after transfer (destination and quarantine), content hash unchanged, skipped-identical file untouched, `None` leaves mtime alone (spec: Imported files carry the recorded time as mtime)

## 5. Integration tests (fake card in tempdir)

- [x] 5.1 Test-fixture builder producing a minimal MP4 with `mvhd`, `HMMT`, and a synthetic `gpmd` track (hand-assembled boxes + KLV payloads — no binaries in the repo)
- [x] 5.2 End-to-end GPS import: drifted camera clock lands in the GPS-corrected dated folder, sidecar carries `clock_offset_s` + marker `utc`/`lat`/`lon`, destination files' mtime is corrected (spec: Session timestamp prefers GPS-corrected time, GPS sidecar scenario)
- [x] 5.3 End-to-end degradation: card without `gpmd` (and one with corrupt telemetry) imports exactly as changeset 2 — camera-clock folder, `"time_source": "camera"` sidecar, Keep/Quarantine verdicts unchanged (spec: Telemetry failures degrade to camera clock)

## 6. Documentation

- [x] 6.1 Learning note `docs/learning/iterators-lifetimes-in-stream-parsing.md`: `KlvIter<'a>` as the concrete anchor — borrowed slices vs owned trees, lifetime elision in iterator impls, contrast with C#'s `IEnumerable`/`Span<T>`; add to the learning notes index
- [x] 6.2 Update `README` GoPro section: GPS correction behavior, sidecar fields, the UTC-date layout trade-off (design D9 risk)

## 7. Verification

- [x] 7.1 Quality gates green: `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`
- [x] 7.2 Real-card smoke test: `scan --source` against the actual HERO8 card — GPS-corrected dates and marker coordinates sanity-checked against GoPro Quik; note results in the change before archiving (**needs a physical HERO8 card — not runnable in this environment**)
