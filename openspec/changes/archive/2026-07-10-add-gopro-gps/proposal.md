## Why

GoPro clocks drift (and reset after battery pulls), so imported sessions currently carry camera-clock timestamps that can be minutes or even hours wrong — putting footage in the wrong dated folder and giving HiLight markers inaccurate wall times. The HERO8 embeds a GPMF telemetry track (`gpmd`) with GPS fixes and GPS-time samples in every recording; parsing it gives true UTC and per-marker location. This is roadmap changeset 3 (`gopro-telemetry`): milestone 2 shipped a working importer that degrades gracefully without GPS, and this change layers the telemetry on top.

## What Changes

- New `src/media/gpmf.rs`: hand-rolled GPMF KLV parser (per ADR 0002) extracting `GPS5` (lat/lon/alt/speed), `GPSU` (UTC timestamp), and `GPSF`/`GPSP` (fix quality) streams.
- Extend `src/media/mp4.rs`: locate the `gpmd` metadata track (handler type via `hdlr`) and extract its samples (`stco`/`co64` chunk offsets + `stsz` sizes).
- Clock-offset correction: `GPSU` vs. stream time yields the camera-clock offset; applied to session timestamps and HiLight marker wall times.
- Sidecar gains GPS fields: `"time_source": "gps"`, `clock_offset_s`, and per-marker `utc`/`lat`/`lon` (from the nearest `GPS5` sample to each marker).
- Destination `{date:...}` layout fields use the GPS-corrected session date.
- After copy + checksum verification, imported files get their filesystem mtime set to the GPS-corrected recording time — file content stays bit-exact (the blake3-verified-copy invariant of ADR 0003 holds; no in-file metadata rewriting).
- Graceful degradation: no `gpmd` track, no GPS fix, or unparseable telemetry → current behavior (camera clock, `"time_source": "camera"`), never a failed import.
- Learning note: iterators/lifetimes in stream parsing (as encountered).

## Capabilities

### New Capabilities
- `gopro-telemetry`: extracting GPMF telemetry from GoPro MP4s — locating the `gpmd` track, parsing GPMF KLV, deriving GPS fixes, UTC time, and the camera-clock offset, with fix-quality gating.

### Modified Capabilities
- `gopro-import`: session timestamps and `{date:...}` layout fields SHALL prefer GPS-corrected UTC over the `mvhd` camera clock when telemetry is available; the `markers.json` sidecar SHALL record `"time_source": "gps"`, the clock offset, and per-marker UTC + lat/lon; imported files' mtime SHALL be set to the corrected recording time after verification — all falling back to today's camera-clock behavior when telemetry is absent.

## Impact

- **Code**: new `src/media/gpmf.rs`; extended `src/media/mp4.rs` (track/sample walking on top of the existing box walker); `src/source/gopro.rs` wires telemetry into scan (timestamps, verdict-independent); `src/sidecar.rs` gains the GPS fields; `src/transfer.rs` sets destination mtime post-verification.
- **Specs**: new `gopro-telemetry` spec; delta to `gopro-import` (session timestamp and sidecar requirements).
- **Dependencies**: none new — parser is hand-rolled on `std::io` (ADR 0002); `jiff` already handles UTC math.
- **Compatibility**: sidecar JSON gains fields; existing fields keep their meaning (`time_source` now varies). No config changes. No CLI changes.
- **Docs**: learning note on iterators/lifetimes in stream parsing (`docs/learning/`).
