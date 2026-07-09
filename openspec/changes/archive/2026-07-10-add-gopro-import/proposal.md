## Why

The core CLI (`add-core-cli`) runs end-to-end but knows no devices, so `scan`/`import` always report "no sources found". This change adds the first device module — GoPro HERO8 — making the tool actually useful: it imports commute sessions that have a HiLight marker and quarantines the rest, per the roadmap's changeset 2 (`docs/ROADMAP.md`).

## What Changes

- New `gopro` device type implementing the `ImportSource` trait (ADR 0005): card detection (`DCIM/1*GOPRO/`), media discovery, and keep/quarantine verdicts.
- Chapter grouping: HERO8 chapter files (`GX01nnnn.MP4`, `GX02nnnn.MP4`, … sharing the last four digits) group into one session; a session is the unit that is kept or quarantined.
- Hand-rolled minimal MP4 box walker (`src/media/mp4.rs`) to read `moov/udta/HMMT` HiLight markers (count + u32 big-endian millisecond offsets). **ADR 0002** (hand-rolled parsers vs. unmaintained crates) is written as part of this change, when the first parser lands.
- Verdict rule: a HiLight marker in *any* chapter keeps the *whole* session; a session with no markers anywhere is quarantined (quarantine-not-delete per ADR 0003).
- `markers.json` sidecar written next to each imported session: camera model, session id, chapter files, and marker offsets with camera-clock timestamps. GPS-corrected times and lat/lon are explicitly **out of scope** — they arrive with `add-gopro-gps` (changeset 3); until then sidecars carry `"time_source": "camera"`.
- Session date (from camera clock) feeds the profile's `layout` template, e.g. `{date:%Y}/{date:%Y-%m-%d}`.
- GoPro profile support in config: `type: gopro` becomes a known type; GoPro-specific profile field `require_marker` (default true).
- Learning note: binary parsing with `std` (`docs/learning/`), per the project's learning-project stance (ADR 0001).

## Capabilities

### New Capabilities
- `gopro-import`: GoPro HERO8 card detection, chapter-file session grouping, HMMT HiLight marker extraction, marker-driven keep/quarantine verdicts, `markers.json` sidecar, and session-date-based destination layout.

### Modified Capabilities

None. `cli-core` requires that adding a device type is only a new `ImportSource` implementation plus a profile `type`; this change is the first proof of that contract. Sidecar writes will appear as listed plan actions so `cli-core`'s "import executes exactly the scanned plan" requirement holds unchanged (design detail, settled in design.md).

## Impact

- **New code**: `src/source/gopro.rs` (detect, scan, grouping, verdicts), `src/media/mp4.rs` (box walker + HMMT parsing), sidecar writing (`src/sidecar.rs` or within the GoPro module — design decision).
- **Existing code**: device-type registry gains `gopro`; no changes to transfer, planning, or reporting logic expected.
- **Config**: `type: gopro` accepted; `require_marker` field parsed for GoPro profiles.
- **Dependencies**: none added — MP4 parsing is hand-rolled on `std::io` (ADR 0002).
- **Tests**: unit tests for the box walker against handcrafted `moov/udta/HMMT` byte fixtures; integration tests with fake card layouts in temp dirs asserting verdicts, layout, sidecar contents, and quarantine behavior (quarantine paths already covered by the plan/execute split's integration-test gate).
- **Docs**: ADR 0002, binary-parsing learning note, README gains a GoPro usage example.
