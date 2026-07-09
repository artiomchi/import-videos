# add-core-cli — Design

## Context

First code changeset. The architectural frame is already fixed by ADRs (scan → plan → execute per 0003, YAML profiles per 0004, single crate + `ImportSource` trait per 0005); this design settles the concrete Rust shapes inside that frame so device changesets can build on stable types. See `proposal.md` for scope and `docs/ROADMAP.md` for what comes after.

## Goals / Non-Goals

**Goals:**
- Compilable, tested foundation: CLI, config, core types, transfer engine, plan reporting.
- Types stable enough that `add-gopro-import` and `add-tesla-import` only *add* a module each.
- Transfer engine trustworthy enough to point at real footage from day one.

**Non-Goals:**
- Device modules, MP4/GPMF parsing (`src/media/` doesn't exist yet).
- Progress bars (`indicatif` lands with changeset 5), `cleanup`/`inspect` commands, `--json` output.
- Cross-filesystem rename fallbacks beyond plain copy+delete; parallel transfers.

## Decisions

### D1. Config: internally tagged enum for device-specific fields
`Profile` struct holds common fields (`source`, `destination`, `layout`, `ignore`, `quarantine`, `delete_source`); device-specific fields live in `#[serde(flatten)] kind: SourceKind`, an internally tagged enum on `type` (`#[serde(tag = "type")]`). Unknown `type` fails at load with a clear error.
*Alternative considered*: one struct per device type duplicating common fields — more boilerplate, and generic pipeline code would need a trait just to read `destination`. The flatten+tagged-enum combo has serde quirks → covered by round-trip tests (see Risks).

### D2. Layout templates: hand-rolled tokens, parsed at config load
`layout` strings like `{date:%Y}/{date:%Y-%m-%d}` parse into a token list (`Literal` / `Field { name, strftime: Option<String> }`) when the config loads, so a typo fails at startup, not mid-import (ADR 0004). Date fields format via jiff's strftime; non-date fields (`{event_type}`, `{time:...}`) resolve from a per-group key/value context supplied by the device module.
*Alternative*: a template crate (tinytemplate/handlebars) — no strftime support, heavier dependency for a three-token language.

### D3. Dispatch over sources: trait objects
The registry is `Vec<Box<dyn ImportSource>>`, built from the profiles present in config. Trait objects (vs. an enum of sources) keep device modules open for extension without touching core, and dyn dispatch cost is irrelevant at "files per card" scale. This is a designated learning topic (trait objects vs. generics note in `docs/learning/`).

### D4. Plan is data, execution consumes it
`scan` produces `ImportPlan { actions: Vec<PlannedAction> }`, where each `PlannedAction` pairs a `MediaGroup` with its `Verdict` and *fully resolved* destination/quarantine paths. Execution takes the plan and does nothing clever — every decision is visible in `scan`/`--dry-run` output verbatim. Re-running is idempotent: a destination file with matching blake3 counts as already-imported (skip; still eligible for source deletion).

### D5. Transfer: temp-file rename, hash both sides
Copy streams to `<dest>.part` while hashing the source bytes; after the copy, re-hash the written destination file, compare, then atomically rename to the final name. Source deletion happens only after the rename succeeds. A crash never leaves a half-written file under a final name.
Collision with *different* content at the destination: keep both — append `-1`, `-2`… and warn. Never overwrite footage.
*Alternative*: trust the copy and hash only once — cheaper, but misses write-side corruption, and verify-then-delete is the whole safety contract (ADR 0003).

### D6. `source: auto` probes mount roots
A locator walks candidate mount roots (`/run/media/<user>`, `/media`, `/mnt`; overridable via top-level `mount_roots` config) and offers each mounted volume to every profile's `detect()`. Explicit `source: <path>` skips probing. Core ships the locator; it finds nothing until device modules implement `detect()`.

### D7. Errors: thiserror enum in lib, anyhow in main
Library error type carries path context (`Io { path, source }`, `Config`, `VerifyMismatch { src, dest }`, `Template`), so messages name the file that failed. `main.rs` converts to `anyhow` for display and maps to exit codes: 0 success (including "nothing to import"), 1 failure, 2 config/usage error.

### D8. Confirmation prompts via std
Destructive steps prompt on stdin unless `--yes`; non-interactive stdin (not a tty) without `--yes` aborts rather than assumes. Plain `std::io` — no dialoguer dependency for one y/N prompt.

## Risks / Trade-offs

- [serde flatten + internally-tagged enum has known edge cases (e.g. with untyped numerics)] → round-trip serde tests for every profile type; config fixtures in unit tests.
- [Hashing source and destination reads every byte twice] → accepted for footage safety (ADR 0003); blake3 is far faster than SD-card I/O, which dominates anyway.
- [Layout template mini-language could grow ad hoc] → tokens are the contract; new field names must be documented in README when a device module introduces them.
- [Idempotency check hashes existing destination files on re-run] → only triggers on name collision, so the cost is proportional to re-imported files, not the library.
- [`auto` detection on oddly mounted cards may find nothing] → `--source PATH` always available as the explicit escape hatch.

## Open Questions

_None blocking — template field vocabulary beyond `{date}` is deliberately deferred to the device changesets that introduce the fields._
