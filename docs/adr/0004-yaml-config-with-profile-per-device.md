# 0004 — YAML config with a profile per device

- Status: accepted
- Date: 2026-07-09

## Context

Behavior differs per device: destination, folder layout, ignore patterns, what counts as a keep-worthy event, whether to clean the source. The tool must support new device types without code changes to the config layer. Config needs comments (JSON lacks them) and nesting (TOML gets awkward for per-device profiles).

## Decision

A single YAML file at the XDG config path (`~/.config/import-videos/config.yaml`, `--config` to override), containing named **profiles**. Each profile has a `type` (selects the `ImportSource` implementation) plus common fields (source, destination, `layout` path template, ignore globs, `delete_source`) and device-specific fields (e.g. `require_marker` for GoPro, `events` for Tesla).

Parsing uses `serde` with `serde_yaml_ng` — the original `serde_yaml` is archived, and `_ng` is the maintained fork.

## Consequences

- Comments and multi-profile structure come for free; users can annotate why a rule exists.
- The `layout` template syntax (`{date:%Y}/...`) is our own mini-format and must be documented in the README and validated at config load, not at import time.
- Device-specific config is modeled as a tagged enum (`type` field), so an unknown type fails at load with a clear error.
