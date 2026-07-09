# 0001 — Rust as a learning project

- Status: accepted
- Date: 2026-07-09

## Context

The author is an experienced .NET developer who wants to learn Rust on a real project. This tool is a good vehicle: it is I/O-heavy (file transfer, checksumming), involves binary format parsing (MP4 boxes, GoPro GPMF telemetry), and benefits from a strict compiler when the failure mode is deleting someone's footage. Python was ruled out by preference; .NET would teach nothing new.

## Decision

Build the CLI in Rust, and treat learning as an explicit project goal alongside shipping a working tool:

- Write idiomatic Rust, even where a .NET-flavored shortcut exists.
- Add instructive doc comments where a Rust concept is non-obvious to a C# developer; avoid narrating comments.
- Record recurring concepts as notes in `docs/learning/`, each tied to the code that uses it.

## Consequences

- Initial velocity is slower than it would be in C#; that is accepted.
- Code review (human or agent) should flag unidiomatic patterns, not just bugs.
- Dependency choices favor well-regarded teaching-quality crates (`clap`, `serde`, `jiff`, `thiserror`/`anyhow`) over kitchen-sink frameworks.
