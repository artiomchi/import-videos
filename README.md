# import-videos

A CLI that imports footage from camera storage (SD cards, USB drives) into a
date-organized video library. It understands device-specific metadata to
decide what's worth keeping — for example, only pulling GoPro clips that
have a HiLight marker, or Tesla dashcam clips tied to a sentry/honk event —
and quarantines everything else instead of deleting it outright.

Every import is **scan → plan → execute**: a read-only scan produces a plan
(what will be kept, quarantined, or ignored, and why); nothing is copied,
moved, or deleted until you review that plan and run `import`.

This changeset ships the core CLI, configuration, and transfer engine —
no device support yet. `scan`/`import` run end-to-end but report "no
sources found" until a device module (GoPro, Tesla, ...) is added by a
later changeset.

## Install

```sh
cargo install --path .
```

## Configuration

Profiles live in a YAML file at `~/.config/import-videos/config.yaml`
(override with `--config`). Each profile selects a device `type` and
where its footage goes:

```yaml
# Optional: where `source: auto` looks for mounted cards.
# Defaults to /run/media/<user>, /media, /mnt.
mount_roots:
  - /run/media/alice
  - /media

profiles:
  commute:
    type: gopro               # selects the device implementation
    source: auto               # or an explicit path, e.g. /media/alice/GOPRO
    destination: ~/Videos/commute
    layout: "{date:%Y}/{date:%Y-%m-%d}"
    ignore:
      - "*.THM"
      - "*.LRV"
    quarantine: ~/Videos/commute/_quarantine
    delete_source: true
```

Common fields, available to every profile:

| Field           | Meaning                                                         |
| --------------- | ---------------------------------------------------------------- |
| `type`          | Selects the device implementation (e.g. `gopro`, `tesla`)        |
| `source`        | `auto` (probe mount roots) or an explicit path                   |
| `destination`   | Where kept footage lands                                         |
| `layout`        | Path template under `destination`, resolved per media group      |
| `ignore`        | Glob patterns for files to skip entirely                         |
| `quarantine`    | Where footage that doesn't meet the keep criteria goes           |
| `delete_source` | Delete source files after a verified transfer (per-run: `--keep-source` overrides) |

`layout` is a small template language: `{date:%Y}/{date:%Y-%m-%d}` resolves
`{date...}` against the media group's timestamp via
[jiff strftime](https://docs.rs/jiff) conversion specifiers; any other
`{field}` resolves from context the device module supplies (vocabulary is
defined per device). A malformed template is rejected when the config
loads, not partway through an import.

## Usage

Always scan before importing — it's read-only and shows exactly what
`import` would do:

```sh
import-videos scan commute
```

```sh
import-videos import commute
```

Useful flags:

- `--source PATH` — use this path instead of the profile's configured source
- `--dry-run` — print the plan and stop (same as `scan`, but via `import`)
- `--keep-source` — never delete source files, even if the profile requests it
- `--yes` — skip the confirmation prompt before deleting source files
- `-v` / `-vv` — increase log verbosity
- `--config PATH` — use a config file other than the default

Exit codes: `0` success (including "nothing to import"), `1` if any
planned action failed, `2` on a configuration or usage error.

## Development

```sh
cargo build
cargo test
cargo clippy -- -D warnings
cargo fmt --check
```

See `AGENTS.md` for project conventions and `docs/adr/` for the design
decisions behind the scan/plan/execute model, YAML profiles, and the
single-crate + trait-based extensibility approach.
