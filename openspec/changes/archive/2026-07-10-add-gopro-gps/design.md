# add-gopro-gps — Design

## Context

Changeset 2 imports GoPro sessions using the `moov/mvhd` camera clock (`src/media/mp4.rs::read_creation_time`), which drifts and — on GoPros — is local camera time, not UTC. HERO8 recordings carry a GPMF telemetry track (`gpmd`) whose payloads include GPS fixes (`GPS5`), GPS-derived UTC (`GPSU`), and fix quality (`GPSF`/`GPSP`). This change parses that track to correct session/marker times, add per-marker coordinates to the sidecar, and stamp imported files' mtime with the corrected time.

Existing building blocks this design extends:

- `mp4.rs` — a linear box-path walker over `Read + Seek` (no materialized tree). It gains track/sample-table walking.
- `source/gopro.rs::build_session` — already collects `MarkerHit`s (file, offset_ms, wall time) and builds the sidecar; telemetry slots in here.
- `transfer.rs::execute/transfer_inner` — copy → verify → rename; mtime stamping lands after the rename.
- `MediaGroup.timestamp` already drives `{date:...}` layout fields, and `MediaGroup.geo` exists but is always `None` today — GPS wiring needs no layout/plan changes.

## Goals / Non-Goals

**Goals:**

- Parse GPMF KLV and the `gpmd` sample table well enough to extract `GPS5`/`GPSU`/`GPSF`/`GPSP` (with `SCAL`) — nothing more.
- One camera-clock offset per session, applied to the session timestamp and every marker wall time; recorded in the sidecar.
- Per-marker lat/lon from the nearest GPS sample.
- Destination file mtime = corrected per-chapter recording time, set only after checksum verification (content stays bit-exact — ADR 0003 invariant).
- Telemetry failure of any kind degrades to changeset-2 behavior; an import can never fail because of GPS.

**Non-Goals:**

- No in-file MP4 metadata rewriting (`mvhd`/`©xyz` patching) — deliberately rejected to preserve the verified-copy invariant; would be its own ADR/changeset.
- No other GPMF streams (accelerometer, gyro, face detection...).
- No models beyond HERO8, no `inspect` command output (changeset 5), no map/GPX exports.
- No per-GPS-sample track logging in the sidecar — only per-marker fixes and the session-level position.

## Decisions

### D1. GPMF parser: borrowing iterator over an in-memory payload

`src/media/gpmf.rs` parses one payload (`&[u8]`) at a time. A `KlvIter<'a>` yields `Klv<'a>` items — fourcc key, type char, struct size, repeat count, and a **borrowed** value slice — advancing by the 4-byte-aligned length. Nesting (type `\0`) is handled by constructing a child `KlvIter` over the value slice; no tree is allocated. Typed accessors (`as_i32s()`, `as_u32()`, `as_utc()` for `GPSU`'s `yymmddhhmmss.sss` string) decode on demand.

Rationale: payloads are tiny (a few KB, one per recorded second), so slice-borrowing beats streaming `Read` here — and a lifetime-carrying iterator over borrowed bytes is exactly the learning-note material this changeset owes (iterators/lifetimes in stream parsing). Alternative (owned `Vec<Klv>` tree) rejected: allocation noise, weaker learning value.

### D2. `gpmd` track location and sample index (mp4.rs)

Extend `mp4.rs` with `read_gpmd_index`: scan `moov` for `trak` boxes; a track qualifies when `mdia/hdlr` handler type is `meta` **and** `mdia/minf/stbl/stsd`'s first entry format is `gpmd` (the stsd check is what actually distinguishes it from other meta tracks). From that track's `stbl`, parse:

- `stsz` → per-sample sizes,
- `stsc` + `stco`/`co64` → per-sample absolute file offsets (parsed properly, not assuming one sample per chunk),
- `stts` + `mdhd` timescale → per-sample stream-time start/duration in seconds.

Result: a `Vec<GpmdSample { offset, size, time_s, duration_s }>` — an index only. Payload bytes are fetched lazily (`read_exact` at offset) for just the samples a caller needs: the clock-offset search reads from the front until it finds a good fix; marker lookup reads one payload per marker. A missing/`gpmd`-less file yields a clean "no telemetry" result, mirroring `read_hilights`' treatment of absent boxes.

### D3. Fix-quality gating

A payload is *usable* iff `GPSF ≥ 2` (2D lock) and `GPSP ≤ 500` (DOP ≤ 5.0, GoPro's own recommended threshold). Unusable payloads are skipped, not errors — the camera logs zeros/garbage coordinates before lock, and trusting them would put markers in the wrong place and, worse, wrong dates in the layout.

### D4. One clock offset per session, from the first good fix

For each chapter in order, scan its payload index for the first usable payload carrying `GPSU`. Then:

```
clock_offset = GPSU_utc − (chapter_mvhd_time + payload.time_s)
```

The first chapter that yields an offset wins; it is applied session-wide: corrected session timestamp = `mvhd + offset` of the first chapter, corrected marker wall time = existing camera wall time + offset. The sidecar records it as `clock_offset_s` (fractional seconds).

Rationale: the camera clock is self-consistent within a session, so one offset suffices; computing per-chapter offsets would just smear GPS noise across the session. Note the offset absorbs *both* drift and the fact that GoPro writes local time into `mvhd` — `GPSU` is true UTC, so the correction fixes the timezone misinterpretation for free. Alternative (average GPSU over many payloads) rejected as precision theater: we need folder-date and marker accuracy, ~1 s is plenty.

### D5. Marker → GPS sample mapping

A marker at `offset_ms` in a chapter maps to the payload whose `[time_s, time_s + duration_s)` covers it. Within that payload, `GPS5` holds N samples (typically ~18 at 18 Hz) assumed uniformly spaced across the payload's duration; pick index `round((offset_ms/1000 − time_s) / duration_s × (N−1))`, clamped. Lat/lon come from `GPS5[0..2]` divided by the stream's `SCAL` divisors. If the covering payload is unusable (D3), search the nearest usable payload within ±2 payloads; beyond that the marker gets no coordinates (fields omitted), while still getting the corrected UTC from D4.

### D6. Degradation is per-session and silent-but-logged

Telemetry runs after marker extraction in `build_session`, wrapped so that **any** failure (no `gpmd` track, malformed KLV, no usable fix in any chapter) produces `None` + a `tracing::warn!`, and the session proceeds exactly as in changeset 2: camera-clock timestamp, `"time_source": "camera"` sidecar, no `geo`. Telemetry never influences the Keep/Quarantine verdict — verdicts are purely marker-driven, so a GPS regression can never quarantine or drop footage.

### D7. Sidecar shape

With telemetry (`"time_source": "gps"`): top level gains `"clock_offset_s"`; each marker entry carries `"file"`, `"offset_ms"`, `"utc"` (corrected), and `"lat"`/`"lon"` when D5 found a fix. Without telemetry the sidecar is byte-for-byte today's camera shape (`"time_source": "camera"`, markers with `"camera_time"`). `MediaGroup.geo` is set to the session's first good fix so future reporting can use it.

### D8. mtime stamping lives in the transfer engine, driven by a new `MediaFile.recorded_at`

`MediaFile` gains `recorded_at: Option<Timestamp>`. The GoPro source fills it per chapter with the corrected (or camera-fallback) chapter time. In `transfer_inner`, after the verified `.part` → final rename succeeds, `File::set_times` (std) sets the destination's mtime from `recorded_at`; a failure to set mtime is a `tracing::warn!`, never a transfer failure (the verified copy is intact — mtime is best-effort metadata). `SkippedIdentical` files are left untouched; `Suffixed` and quarantined files are stamped like any other.

Rationale: transfer stays device-agnostic (it reads a field, not GoPro logic), and the stamp happens strictly after verification so the ADR 0003 bit-exact/verify story is unchanged. Alternative (stamp in the source module post-execute) rejected: the source module must not touch destination files.

### D9. Layout dates need no new code

`MediaGroup.timestamp` already drives `{date:...}`; correcting it in `build_session` corrects the destination folder automatically. Dates remain UTC-based (as today) — a 23:30 UTC commute filed under the UTC date is accepted behavior for now (see Risks).

## Risks / Trade-offs

- **Real-world GPMF variance** (firmware quirks, unexpected stream layout) → parser treats anything unexpected as "no telemetry" (D6), never an abort; verified against a real HERO8 card via `scan --source` before archiving, comparing dates/coordinates against GoPro Quik.
- **~1 s marker/GPS alignment error** at payload boundaries (D5's uniform-spacing assumption) → accepted; markers locate a commute moment, not a survey point.
- **UTC vs local layout dates** (D9): a late-evening UK ride can land in the "wrong" folder by local reckoning → accepted for now; a `{date:local:...}` layout field would be a future, config-level change — flag it in the README rather than solving it here.
- **`stsc`/`stts` parsing bugs corrupt the index, not footage** — all telemetry reads are read-only over the source; the destructive path (transfer/delete) is untouched except for post-verify mtime, which cannot alter content.
- **Filesystems/mounts that reject `set_times`** → warn-and-continue (D8); the sidecar still records the corrected times, so nothing is lost.

## Open Questions

None blocking. Local-time layout dates (D9) deliberately deferred.
