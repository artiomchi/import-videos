# 0005 — Single crate, trait-based extensibility

- Status: accepted
- Date: 2026-07-09

## Context

The app must be generic over device types (GoPro and Tesla today; other cameras later). Options ranged from a cargo workspace with a crate per device, to dynamic plugins, to a single crate with a trait.

## Decision

One cargo package: `src/lib.rs` holding all logic (testable as a library) with a thin `src/main.rs` binary. Device support is a module in `src/source/` implementing the `ImportSource` trait (`detect` a card, `scan` it into `MediaGroup`s with verdicts). Adding a device type = one new module + one new config profile `type`.

The core pipeline (transfer, sidecars, reporting) is device-agnostic and lives outside `src/source/`.

## Consequences

- Simple build, one binary to install; no plugin-loading machinery for a tool with one user.
- Trait objects vs. generics for dispatching over sources is a deliberate learning topic (note in `docs/learning/` when implemented).
- If device modules ever grow heavy dependencies of their own, splitting into a workspace is a mechanical refactor — a new ADR would supersede this one.
