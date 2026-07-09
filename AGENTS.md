# AGENTS.md

Guide for AI agents and contributors working on this repository.

## What this project is

`import-videos` is a Rust CLI that imports footage from camera storage (SD cards, USB drives) into a date-organized video library. It understands device-specific metadata to decide what's worth keeping:

- **GoPro HERO8** (cycling commutes): keep only sessions with HiLight markers (side-button presses); extract marker timestamps and GPS into a JSON sidecar; quarantine unmarked footage.
- **Tesla dashcam/sentry** (USB drive): import event-triggered clips (saved, sentry, honk) with their `event.json` metadata.

The design is generic: each device type implements the `ImportSource` trait, and behavior is driven by per-device profiles in a YAML config.

## This is a Rust learning project

The author is an experienced .NET developer learning Rust. This shapes how code should be written here:

- **Idiomatic Rust first.** Prefer the way a seasoned Rust developer would write it, even when a .NET-flavored approach would be shorter. The point is to learn the idioms.
- **Instructive doc comments, no noise.** When code uses a Rust concept that is non-obvious to someone coming from C# (ownership hand-offs, lifetimes, trait objects vs. generics, error design), a short doc comment explaining *why the code is shaped this way* is welcome. Comments that narrate what the next line does are not.
- **Prefer std and few, well-chosen crates.** Hand-rolling the MP4 box walker and GPMF parser is intentional (see ADR 0002) — binary parsing is prime learning material and the existing crates are unmaintained.
- **Capture learning notes.** When a change introduces a Rust concept new to this codebase, add or extend a note in `docs/learning/` tying the concept to the concrete code that uses it. See `docs/learning/README.md` for the format.

## Decisions live in ADRs

Significant decisions are recorded in `docs/adr/` (lightweight MADR format: Context / Decision / Consequences). **Check the ADRs before re-litigating a decision**, and add a new ADR when making a significant new one — don't rewrite history in an existing ADR, supersede it.

## Architecture in one paragraph

Single cargo package: `src/lib.rs` (all logic, testable) + a thin `src/main.rs`. Every import follows **scan → plan → execute**: a read-only scan produces an `ImportPlan` (what to keep, quarantine, or ignore — and why); execution copies files, verifies checksums (blake3), and only then deletes from the source. Unmarked footage is never deleted directly — it goes to a quarantine folder purged by an explicit `cleanup` command. Device support lives in `src/source/` behind the `ImportSource` trait; binary format parsing (MP4 boxes, GPMF telemetry) lives in `src/media/`.

## Development workflow

Development flows through **OpenSpec** (`/opsx:*` commands, spec-driven schema). One changeset at a time; each leaves the repo green. See `openspec/config.yaml` for project context given to OpenSpec artifacts.

Quality gates for every change:

```sh
cargo build
cargo test
cargo clippy -- -D warnings
cargo fmt --check
```

## Conventions

- Errors: `thiserror` for library error types, `anyhow` at the binary boundary.
- Logging: `tracing` (`-v`/`-vv` flags), not `println!` (user-facing report output excepted).
- Dates/times: `jiff`. Config/sidecars: `serde` + `serde_yaml_ng` / `serde_json`.
- Anything that deletes or overwrites user footage must be behind the plan/execute split and covered by an integration test.
