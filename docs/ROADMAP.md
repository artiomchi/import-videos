<!-- Initial project plan, approved 2026-07-09. The execution roadmap below is the
source of truth for changeset sequencing. Device-pipeline details migrate into
OpenSpec capability specs as their changesets land; ADRs in docs/adr/ hold the decisions. -->

# import-videos — Rust CLI for importing camera footage

## Context

Artiom records cycling commutes on a GoPro HERO8 and has a Tesla writing dashcam/sentry footage to a USB drive. Manually triaging these cards is tedious: GoPro commutes without a HiLight marker (side-button press) are throwaway; Tesla clips only matter when an event triggered them. This project is a CLI that scans a mounted card, decides what to keep based on device-specific metadata, copies keepers into a date-organized library with JSON metadata sidecars, and safely cleans the card.

It is also deliberately a **Rust learning project**: Artiom is an experienced .NET developer learning Rust. Code should be idiomatic, well-structured, and documented where a concept is instructive (ownership patterns, trait objects, error design, binary parsing) — but not over-commented. This stance gets recorded in `AGENTS.md`.

Decisions already made with the user:
- **Rust** (learning project), **YAML** config, single CLI binary with subcommands
- Unmarked GoPro footage → **quarantine folder**, deleted only by an explicit `cleanup` command
- Source card files → **copy, verify checksum, then delete** (configurable per profile)

## Architecture

Single cargo package, `src/lib.rs` + thin `src/main.rs` (testable library, idiomatic layout). Extensibility via an `ImportSource` trait — each device type is one implementation; adding a drone/dashcam later means one new module + config entry.

```
src/
  main.rs           # entry: parse args, init tracing, call lib, map errors to exit codes
  cli.rs            # clap derive definitions
  config.rs         # YAML config: profiles, path templates, validation (serde)
  source/
    mod.rs          # ImportSource trait + shared types: MediaGroup, MediaFile, Marker, ImportPlan
    gopro.rs        # detect card, group chapter files, extract HiLights, keep/quarantine verdict
    tesla.rs        # walk SavedClips/SentryClips, parse event.json, filter by trigger reason
  media/
    mp4.rs          # minimal MP4 box walker (hand-rolled): udta/HMMT + locate gpmd track samples
    gpmf.rs         # GPMF KLV parser (hand-rolled): GPS5 (lat/lon/alt/speed), GPSU (UTC), GPSF/GPSP (fix quality)
  transfer.rs       # copy → blake3 verify → delete source; quarantine moves; collision handling
  sidecar.rs        # write metadata JSON next to imported footage
  report.rs         # scan/dry-run/summary rendering (with indicatif progress during transfer)
```

Core flow (same for every source): **scan → plan → execute**. `scan` produces an `ImportPlan` (what's kept, quarantined, skipped, and why); `--dry-run` just prints it; `import` executes it. This separation is what makes the tool safe and testable.

### `ImportSource` trait (shape)

```rust
trait ImportSource {
    /// Does this directory look like this device's card? (e.g. DCIM/**/GX*.MP4, TeslaCam/)
    fn detect(&self, root: &Path) -> bool;
    /// Discover media, read metadata, group related files, decide keep/quarantine.
    fn scan(&self, root: &Path, profile: &Profile) -> Result<Vec<MediaGroup>>;
}
```

`MediaGroup` = one logical event (a commute = all its chapter files; a Tesla event = its folder of camera angles + event.json), carrying markers, timestamps, optional geolocation, and a verdict (`Keep`/`Quarantine`/`Ignore(reason)`).

## GoPro pipeline (HERO8)

1. **Discovery & grouping**: files under `DCIM/1*GOPRO/`. HERO8 chapters: `GX01nnnn.MP4`, `GX02nnnn.MP4` share the last 4 digits → group into one commute session. Ignore `.LRV`/`.THM` (config `ignore` globs).
2. **HiLight markers**: parse `moov/udta/HMMT` box — a count + list of u32 big-endian millisecond offsets. A marker in *any* chapter keeps the *whole session*. No markers in session → quarantine verdict.
3. **GPS & time correction** (GPMF): locate the `gpmd` metadata track via the box walker (handler type in `hdlr`, sample offsets from `stco`/`co64` + `stsz`), parse GPMF KLV: `GPSU` (UTC timestamp) vs. stream time gives the camera-clock offset; apply it so marker timestamps and the commute date are true UTC even when the GoPro clock has drifted. `GPS5` at nearest sample gives lat/lon per marker. Degrade gracefully: no GPS fix → fall back to camera clock + note `"time_source": "camera"` in the sidecar.
4. **Sidecar** `markers.json` written next to the imported session:
   ```json
   { "camera": "gopro-hero8", "session": "0123",
     "files": ["GX010123.MP4", "GX020123.MP4"],
     "time_source": "gps", "clock_offset_s": -12.4,
     "markers": [ { "file": "GX010123.MP4", "offset_ms": 734120,
                    "utc": "2026-07-09T07:41:03Z", "lat": 51.5012, "lon": -0.1246 } ] }
   ```
5. **Destination layout** from config template using the (GPS-corrected) session date, e.g. `{destination}/2026/2026-07-09/`.

## Tesla pipeline

USB structure: `TeslaCam/SavedClips/<timestamp>/` and `TeslaCam/SentryClips/<timestamp>/`, each event folder containing `event.json` (`timestamp`, `city`, `est_lat`, `est_lon`, `reason` — e.g. `user_interaction_honk`, `sentry_aware_object_detection`), `thumb.png`, and per-minute, per-camera clips (`...-front.mp4`, `-back`, `-left_repeater`, `-right_repeater`).

1. Walk event folders; parse `event.json` for trigger reason and location.
2. Config filters which event types/reasons to import (`events: [saved, sentry]`, optional `reasons` allow/deny list — sentry generates a lot of noise).
3. Import the whole event folder (all angles + `event.json` + thumb) to `{destination}/{event_type}/{date}/{event_time}/`; sidecar is a normalized `event.json` superset (adds source path, import time, file checksums).
4. `RecentClips` (rolling buffer, no trigger) ignored by default, importable via config.

## Configuration

`~/.config/import-videos/config.yaml` (via `directories` crate), `--config` override.

```yaml
profiles:
  gopro:
    type: gopro
    source: auto                      # auto = probe mounted volumes with detect(); or a fixed path
    destination: ~/Videos/commutes
    layout: "{date:%Y}/{date:%Y-%m-%d}"
    ignore: ["*.LRV", "*.THM"]
    require_marker: true
    quarantine: "{destination}/_quarantine"
    delete_source: true               # only after checksum verification
  tesla:
    type: tesla
    source: auto
    destination: ~/Videos/tesla
    layout: "{event_type}/{date:%Y-%m-%d}/{time:%H-%M-%S}"
    events: [saved, sentry]
    delete_source: true
```

## CLI

```
import-videos scan    [PROFILE] [--source PATH]              # read-only preview (default when unsure)
import-videos import  [PROFILE] [--source PATH] [--dry-run] [--keep-source] [--yes]
import-videos cleanup [PROFILE] [--older-than 30d] [--yes]   # purge quarantine
import-videos inspect FILE                                    # debug: dump HiLights/GPS/metadata of one file
```

No profile argument → run every profile whose `detect()` matches a mounted source. Destructive steps (source deletion, quarantine purge) prompt unless `--yes`.

## Dependencies

| Crate | Why |
|---|---|
| `clap` (derive) | CLI |
| `serde`, `serde_json`, `serde_yaml_ng` | config + sidecars (`serde_yaml` is archived; `_ng` is the maintained fork) |
| `jiff` | dates/timezones (modern, great API — better learning than chrono) |
| `blake3` | fast copy verification |
| `globset` | ignore patterns |
| `indicatif` | progress bars |
| `thiserror` (lib) + `anyhow` (bin) | idiomatic Rust error design — instructive split |
| `tracing`, `tracing-subscriber` | logging (`-v` flags) |
| `directories` | XDG config path |

MP4 box walking and GPMF are **hand-rolled** (`std::io::Read` + `u32::from_be_bytes`): existing crates are unmaintained (`gpmf` 0.1.2, 2021), both formats are simple and well-documented, and binary parsing is prime Rust learning material.

## Documentation structure (learning-oriented)

```
AGENTS.md                 # agent/contributor guide — see below
docs/
  adr/                    # Architecture Decision Records (lightweight MADR: Context / Decision / Consequences)
    0001-rust-as-learning-project.md
    0002-hand-rolled-mp4-and-gpmf-parsers.md      # vs unmaintained crates; formats are simple KLV/box structures
    0003-scan-plan-execute-safety-model.md         # incl. quarantine-then-cleanup + verify-then-delete
    0004-yaml-config-with-profile-per-device.md
    0005-single-crate-trait-based-extensibility.md # ImportSource trait vs workspace/plugins
  learning/               # short Rust learning notes, written as concepts come up in the code
    README.md             # index; note format: concept → where it appears in this codebase → takeaway
    # e.g. errors-thiserror-vs-anyhow.md, trait-objects-vs-generics.md, ownership-in-the-transfer-pipeline.md
```

ADRs are written when the decision is made — 0001/0003/0004/0005 exist from day one (those calls are already made); 0002 lands with the changeset that writes the first parser. Learning notes accumulate organically per changeset — each ties a Rust concept to the concrete place it's used here, not textbook prose. New significant decisions (e.g. "parse gpmd track vs shell out to ffmpeg" if that ever flips) get a new ADR rather than editing history.

### AGENTS.md (to be written first)

Documents: project purpose; the "Rust learning project" stance (idiomatic code; doc comments where a Rust concept is instructive, no noise comments; prefer std + few well-chosen crates); pointers to `docs/adr/` ("check ADRs before re-litigating a decision; add one for any significant new decision") and `docs/learning/` ("when introducing a Rust concept new to this codebase, add/extend a learning note"); architecture summary (scan→plan→execute, `ImportSource` trait); dev commands (`cargo build`, `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt`); note that development flows through OpenSpec.

## Execution roadmap — OpenSpec changesets

Development flows through OpenSpec (spec-driven schema already configured). Five changesets, each independently proposable/archivable, each leaving the repo green (`cargo test`, `clippy -D warnings`, `fmt --check`). Capability specs that persist after archiving map roughly one-per-changeset.

**Pre-work (before the first changeset, plain commits — repo meta, not a capability):**
- Fill `openspec/config.yaml` `context:` — tech stack (Rust), conventions (clippy/fmt clean, ADRs in `docs/adr/`, learning notes in `docs/learning/`, thiserror-in-lib/anyhow-in-bin), domain summary (GoPro/Tesla import).
- `AGENTS.md` and the already-decided ADRs **0001** (Rust as learning project), **0003** (scan→plan→execute safety model), **0004** (YAML config), **0005** (single crate + trait extensibility). These decisions predate any changeset and future OpenSpec artifacts should inherit them.

| # | Changeset | Capability spec | Delivers | Docs written with it |
|---|---|---|---|---|
| 1 | `add-core-cli` | `cli-core` | cargo scaffold (lib+bin), clap skeleton (`scan`/`import` stubs), YAML config + profiles + path templates, `ImportSource` trait + `MediaGroup`/`ImportPlan` types, `transfer.rs` (copy → blake3 verify → delete, quarantine moves — device-agnostic, unit-testable standalone), report rendering | Learning notes: errors (thiserror/anyhow), modules & lib/bin split; README skeleton |
| 2 | `add-gopro-import` | `gopro-import` | DCIM discovery, chapter grouping, `mp4.rs` box walker → HMMT HiLight markers, keep/quarantine verdicts, `markers.json` sidecar (offsets, camera-clock times), end-to-end import. **Tool is useful from here.** | **ADR 0002** (hand-rolled parsers — recorded when the first parser lands); learning note: binary parsing with std |
| 3 | `add-gopro-gps` | `gopro-telemetry` | `gpmf.rs` KLV parser, `gpmd` track sample extraction, GPS5/GPSU parsing, clock-offset correction, lat/lon + true UTC in sidecars, GPS-derived dates in layout. Separate changeset: meaty, and the tool degrades gracefully without it | Learning note: iterators/lifetimes in stream parsing (as encountered) |
| 4 | `add-tesla-import` | `tesla-import` | `tesla.rs` — SavedClips/SentryClips walk, `event.json` parsing, event-type/reason filters, event-folder import with normalized sidecar | ADR only if a real decision surfaces (e.g. RecentClips handling) |
| 5 | `add-maintenance-commands` | `cli-maintenance` | `cleanup` (quarantine purge with `--older-than`), `inspect` (dump markers/GPS of a file), indicatif progress, `--json` report output, README completed | Learning notes index tidy-up |

**Docs timing principles:**
- **ADRs**: written when the decision is *made* — already-decided ones land in pre-work; ADR 0002 lands with changeset 2 where the parser code makes it concrete; later ADRs as decisions surface. Never batch-written after the fact.
- **Learning notes**: part of each changeset's tasks — when code introduces a Rust concept new to the codebase, add a short note tying the concept to the concrete code location. Not written speculatively.
- **README**: skeleton (what/why/install) in changeset 1, completed in changeset 5 when the CLI surface is final.
- **AGENTS.md**: pre-work; updated in any changeset that changes conventions.

**Per-changeset flow**: `/opsx:new` → proposal → specs/design → tasks → implement → `/opsx:verify` → `/opsx:archive`. Milestones 2–5 each start only after the previous is archived (no parallel changesets; each builds on the last).

## Verification

- **Unit tests**: `mp4.rs`/`gpmf.rs` against tiny synthetic fixtures built in tests (handcrafted `moov/udta/HMMT` bytes; synthetic GPMF KLV payloads) — no big binaries in the repo.
- **Integration tests**: fake card layouts in `tempfile` dirs (Tesla structure is pure files/JSON — fully synthesizable; GoPro uses minimal handcrafted MP4s). Assert plan verdicts, destination layout, sidecar contents, source deletion only after verify, quarantine behavior.
- **Real-hardware smoke test**: `inspect` + `scan --source <real card>` against actual GoPro/Tesla cards (env-gated test or manual step), comparing HiLight counts against what the GoPro Quik app shows.
- `cargo clippy -- -D warnings` and `cargo fmt --check` clean at every milestone.
