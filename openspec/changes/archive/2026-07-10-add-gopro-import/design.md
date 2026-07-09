## Context

`cli-core` is implemented and green: config, layout templates, plan building (`src/plan.rs`), verified transfer (`src/transfer.rs`), and the `ImportSource` trait (`src/source/mod.rs`) with a placeholder `GenericSource`. This change adds the first real device module (roadmap changeset 2), which makes it the first test of ADR 0005's claim that a new device is "one new module + config entry". GPS/GPMF work is deferred to `add-gopro-gps` (see proposal).

Relevant current shapes:

- `SourceKind` is an internally-tagged serde enum; its exhaustive `build()` match is the device registry.
- `MediaGroup` already carries `markers`, `timestamp`, and a `context` map for layout fields.
- `ImportSource::scan(&self, root)` receives only the source root — no profile data reaches the device module today.
- `transfer::execute` walks `PlannedAction`s and transfers `group.files` to the resolved target directory.

## Goals / Non-Goals

**Goals:**

- `type: gopro` profile scans a HERO8 card, groups chapter files into sessions, keeps sessions with HiLight markers, quarantines the rest.
- Hand-rolled minimal MP4 box walker sufficient for `moov/udta/HMMT` (markers) and `moov/mvhd` (camera-clock creation time) — ADR 0002 is written alongside it.
- `markers.json` sidecar next to each imported session, listed in the plan before it is written.
- Everything degrades safely on malformed input: a file the parser can't read never aborts a run and never causes data loss.

**Non-Goals:**

- GPS extraction, clock-drift correction, true-UTC marker times (`add-gopro-gps`).
- Any other GoPro model's quirks beyond HERO8; other chapter naming schemes are accepted where free (see D3) but not tested against real cards.
- `cleanup` / `inspect` commands (`add-maintenance-commands`).
- Video content inspection — verdicts come only from container metadata.

## Decisions

### D1: `SourceKind::Gopro` variant carries device-specific config

`require_marker: bool` (default `true`) lives on the new `Gopro` variant of `SourceKind`, not on `Profile`. Common fields stay device-agnostic; device knobs travel with the `type` tag, which is exactly what the internally-tagged enum is for (established in `add-core-cli`, ADR 0004). `build()` gains a `Gopro` arm constructing `GoproSource { require_marker }`.

*Alternative — flat optional fields on `Profile`*: rejected; every future device would pollute the common profile surface, and serde would accept `require_marker` on a Tesla profile silently.

### D2: Ignore globs are passed into `scan()`

`ImportSource::scan` becomes `scan(&self, root: &Path, ignore: &GlobSet)`. `ignore` is a *common* profile field, so core owns it and hands it to the device, which applies it during discovery (an ignored `.LRV` is never opened, let alone parsed). `GenericSource` and call sites update mechanically.

*Alternatives*: (a) core strips ignored files from groups post-scan — too late, the device has already paid the cost of parsing them, and grouping may have depended on them; (b) `build()` takes the whole `Profile` so the device grabs what it wants — gives devices access to fields core should mediate (destination, delete_source) and makes the contract fuzzier. Changing the trait signature is a core *code* change but not a `cli-core` *requirement* change; the spec-level contract (devices plug in via the trait; core stays device-agnostic) is untouched.

### D3: Discovery and session grouping (HERO8 naming)

- `detect(root)`: true iff `root/DCIM/` contains at least one directory matching `1*GOPRO` (e.g. `100GOPRO`) with at least one file matching the chapter pattern below. Cheap, read-only, specific enough not to claim Tesla drives or camera-less SD cards.
- Chapter pattern: `G[XH]ccnnnn.MP4` (case-insensitive extension) — `GX` = HEVC, `GH` = AVC; HERO8 produces both depending on encoding settings. `cc` = two-digit chapter number, `nnnn` = four-digit session number.
- Grouping: all chapter files sharing `nnnn` (across all `1*GOPRO` dirs) form one `MediaGroup` named after the session (e.g. `session-0123`), chapters sorted by `cc`. Layout `context` gets `session: "0123"`.
- Files under `DCIM/1*GOPRO/` that match neither the chapter pattern nor an ignore glob (e.g. `.JPG` photos) become a single `Ignore("unrecognized file(s)")` group so the scan report is honest about what it saw but the tool never touches them. Glob-ignored files are silently excluded — the user explicitly configured them away.

### D4: Minimal targeted MP4 box walker, not a general parser

`src/media/mp4.rs` (new `src/media/` module) walks boxes over any `Read + Seek`: read 8-byte header (u32 BE size + fourcc), handle the 64-bit `size == 1` form, descend only into the containers on the paths we need (`moov` → `udta`/`mvhd`), skip everything else by seeking. It extracts exactly two things:

1. **HiLights**: `moov/udta/HMMT` payload = u32 BE count, then that many u32 BE millisecond offsets.
2. **Camera clock**: `moov/mvhd` `creation_time` (u32 seconds since 1904-01-01 UTC, or u64 in version-1 boxes), converted via jiff.

No box tree is materialized and nothing outside these paths is interpreted — the `add-gopro-gps` changeset extends the same walker toward the `gpmd` track rather than replacing it. Rationale for hand-rolling vs. crates is **ADR 0002**, written as part of this change.

### D5: Session timestamp comes from `mvhd`, not filesystem mtime

The first chapter's `mvhd` creation time is the `MediaGroup.timestamp` (feeding `{date:...}` layout fields) and the base for marker wall-clock times (`creation_time + offset_ms`). It is in-band (survives file copies), and we are already inside `moov` for HMMT. FAT mtimes would also be camera clock but are one copy away from being lies. The sidecar records `"time_source": "camera"` — this is the uncorrected camera clock until `add-gopro-gps` lands. If `mvhd` is unreadable, fall back to the file's mtime and log a warning; timestamp quality is not worth failing an import over.

### D6: Sidecar is planned data, written by the transfer engine

`MediaGroup` gains `sidecar: Option<Sidecar>` where `Sidecar { filename: String, content: serde_json::Value }` (defined in `src/source/mod.rs`; a separate `src/sidecar.rs` is not warranted for one struct — revisit when Tesla adds its own). The GoPro module attaches `markers.json` content during `scan`; the plan renderer lists it; `transfer::execute` writes it into the group's target directory **after** all the group's files transferred successfully, and a sidecar write failure marks the group failed (so the source is not deleted — the sidecar is part of the import, not an optional extra).

This keeps `cli-core`'s "import executes exactly the scanned plan" requirement intact: the sidecar is visible in the plan before anything is written, and scan stays read-only (the *device* never writes; the transfer engine does, at execute time).

Sidecars are written for `Keep` groups only (per roadmap: "next to the imported session"). Sidecar content (camera model, session id, chapter file names, `time_source`, markers with `offset_ms` + camera-clock wall time) — exact shape pinned in the spec.

### D7: Parse failures degrade to "no markers", never to data loss

A chapter file whose `moov`/`HMMT` can't be parsed contributes zero markers and a `tracing` warning; the session then follows the normal verdict rule (typically → quarantine). Quarantine is non-destructive (verified copy before any source deletion, ADR 0003), so a corrupt file ends up preserved in quarantine for inspection rather than aborting the run or being skipped into oblivion. Parser errors are typed (`thiserror`) but consumed at the GoPro-module boundary, not propagated to abort a scan.

### D8: Verdict rule

Per session: any HiLight marker in any chapter → `Keep`; zero markers → `Quarantine`. `require_marker: false` short-circuits to `Keep` for every session (markers still extracted for the sidecar). There is no per-chapter verdict — the session is the atomic unit (a commute is one event; ADR 0003's quarantine-not-delete makes the coarse granularity safe).

## Risks / Trade-offs

- **[HMMT is undocumented]** The format (count + u32 offsets) is reverse-engineered community knowledge, not a published spec. → Unit tests use handcrafted fixtures encoding our understanding; the roadmap's real-hardware smoke test (compare HiLight counts against GoPro Quik) validates it before the tool is trusted with `delete_source: true`.
- **[Camera clock is wrong-ish by design]** Dates in the library layout can be off by the GoPro's clock drift until `add-gopro-gps` lands. → Accepted for this changeset; `time_source: "camera"` in the sidecar makes the provenance explicit, and the GPS changeset was split out deliberately (roadmap).
- **[serde flatten + internally-tagged enum with variant fields]** `require_marker` inside the `Gopro` variant rides on flatten behavior with known edge cases. → Same mitigation as `add-core-cli`: a round-trip test for the `Gopro` variant next to the existing `Generic` one.
- **[Trait signature churn]** D2 changes `scan()`'s signature; the next device (Tesla) may want more context and force another change. → Accepted: two call sites today, and a premature `ScanContext` struct is speculative generality. Revisit if `add-tesla-import` needs a third parameter.
- **[Session numbers recycle]** GoPro wraps `nnnn` after 9999 and resets on card format, so `session-0123` from two different days can collide in quarantine naming. → Quarantine paths join the group name under the quarantine root; collision handling already exists in transfer (suffixing). Destination paths include the date, so real imports don't collide.

## Migration Plan

None needed — purely additive. Existing configs keep working; a new `type: gopro` profile value becomes available. No data or config migration.

## Open Questions

- Should quarantined sessions *also* get a `markers.json` (stating "0 markers") to aid quarantine review? Deferred — cheap to add later; `cleanup` tooling in changeset 5 may want it.
- Do multi-`1*GOPRO`-directory cards (100GOPRO + 101GOPRO) ever split one session across directories? Assumed yes-is-possible (D3 groups across directories), to be confirmed against a real card during the smoke test.
