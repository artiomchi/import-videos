//! clap CLI surface: `scan` and `import`, plus the global flags every
//! subcommand shares.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "import-videos",
    version,
    about = "Import footage from camera storage into a date-organized library"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// Path to the config file (default: ~/.config/import-videos/config.yaml)
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,

    /// Increase log verbosity (-v info, -vv debug)
    #[arg(short = 'v', long = "verbose", action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    /// Emit machine-readable JSON on stdout instead of human-readable
    /// text; suppresses progress and informational lines (design D4)
    #[arg(long, global = true)]
    pub json: bool,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Discover media via a profile and print the import plan; touches nothing
    Scan {
        /// Profile name from the config file
        profile: String,

        /// Override the profile's configured source with this path
        #[arg(long)]
        source: Option<PathBuf>,

        #[command(flatten)]
        overrides: OverrideFlags,
    },

    /// Scan and execute the import plan for a profile
    Import {
        /// Profile name from the config file
        profile: String,

        /// Override the profile's configured source with this path
        #[arg(long)]
        source: Option<PathBuf>,

        /// Print the plan and exit without changing anything
        #[arg(long)]
        dry_run: bool,

        /// Force source deletion on for this run, even if the profile
        /// sets `delete_source: false`; still gated by the confirmation
        /// prompt unless `--yes` (design D3)
        #[arg(long, overrides_with = "no_delete_source")]
        delete_source: bool,

        /// Force source deletion off for this run, even if the profile
        /// sets `delete_source: true`
        #[arg(long, alias = "keep-source", overrides_with = "delete_source")]
        no_delete_source: bool,

        /// Assume "yes" at confirmation prompts
        #[arg(long)]
        yes: bool,

        /// Skip content hashing when the destination file's name, size,
        /// and mtime match within 0.1 s. Useful for regenerating
        /// `import.json` on already-imported footage. Files matched this
        /// way are never deletion candidates (ADR 0009).
        #[arg(long)]
        quick_match: bool,

        #[command(flatten)]
        overrides: OverrideFlags,
    },

    /// Purge a profile's quarantine directory
    Cleanup {
        /// Profile name from the config file
        profile: String,

        /// Only purge entries that have sat in quarantine longer than
        /// this (jiff friendly span format, e.g. "30d", "2w")
        #[arg(long)]
        older_than: Option<String>,

        /// Print the purge plan and exit without deleting anything
        #[arg(long)]
        dry_run: bool,

        /// Assume "yes" at the confirmation prompt
        #[arg(long)]
        yes: bool,
    },

    /// Dump a single file's device metadata (GoPro MP4 or Tesla event);
    /// needs no profile or config
    Inspect {
        /// A GoPro chapter MP4, a Tesla event folder, or an event.json
        path: PathBuf,
    },
}

/// Plan-shaping override flags shared by `scan` and `import` (design
/// D6) — everything that changes what the plan shows. `--delete-source`
/// only affects execution, so it lives on `Import` alone, outside this
/// struct.
#[derive(clap::Args, Debug, Default)]
pub struct OverrideFlags {
    /// Override the profile's quarantine directory for this run;
    /// implies `--copy-quarantine` (design D4). Combining this with
    /// `--no-copy-quarantine` is a usage error.
    #[arg(long, conflicts_with = "no_copy_quarantine")]
    pub quarantine: Option<PathBuf>,

    /// Force quarantine copying on for this run, even if the profile
    /// sets `copy_quarantine: false`
    #[arg(long, overrides_with = "no_copy_quarantine")]
    pub copy_quarantine: bool,

    /// Force quarantine copying off for this run, even if the profile
    /// sets `copy_quarantine: true`
    #[arg(long, overrides_with = "copy_quarantine")]
    pub no_copy_quarantine: bool,

    /// Force the GoPro marker requirement on for this run
    /// (GoPro profiles only)
    #[arg(long, overrides_with = "no_gopro_require_marker")]
    pub gopro_require_marker: bool,

    /// Force the GoPro marker requirement off for this run
    /// (GoPro profiles only)
    #[arg(long, overrides_with = "gopro_require_marker")]
    pub no_gopro_require_marker: bool,
}

impl OverrideFlags {
    /// Collapses the parsed flag pairs into a plain `Overrides`
    /// (design D5) — neutral data only. `--quarantine`'s implication
    /// of `copy_quarantine: true` and GoPro-only validation of the
    /// marker flags are business rules applied where the override is
    /// consumed (`lib.rs`), not here. `delete_source` is always `None`
    /// here since it isn't part of this shared flag set; `Import`
    /// fills it in separately.
    pub fn to_overrides(&self) -> Overrides {
        Overrides {
            delete_source: None,
            copy_quarantine: override_pair(self.copy_quarantine, self.no_copy_quarantine),
            gopro_require_marker: override_pair(
                self.gopro_require_marker,
                self.no_gopro_require_marker,
            ),
            quarantine: self.quarantine.clone(),
        }
    }
}

/// Per-invocation profile overrides collapsed from paired CLI flags,
/// consumed by profile resolution in `lib.rs`. `None` means "use the
/// profile" (design D5).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Overrides {
    pub delete_source: Option<bool>,
    pub copy_quarantine: Option<bool>,
    pub gopro_require_marker: Option<bool>,
    pub quarantine: Option<PathBuf>,
}

/// Collapses a paired override flag (`--foo`/`--no-foo`) to
/// `Option<bool>`: `None` when neither was passed, `Some` in the
/// direction of whichever flag clap recorded. Repeats of a pair are
/// resolved by clap's `overrides_with` before this ever runs (design
/// D1: last one wins), so `(true, true)` can't occur in practice.
pub fn override_pair(set: bool, unset: bool) -> Option<bool> {
    match (set, unset) {
        (true, false) => Some(true),
        (false, true) => Some(false),
        _ => None,
    }
}

/// Wires verbosity to a `tracing` filter. `-v`/`-vv` are the only
/// knobs (design D7 area); user-facing report output goes through
/// `println!`, not `tracing`, per AGENTS.md conventions.
pub fn init_tracing(verbosity: u8) {
    let level = match verbosity {
        0 => tracing::Level::WARN,
        1 => tracing::Level::INFO,
        _ => tracing::Level::DEBUG,
    };
    tracing_subscriber::fmt()
        .with_max_level(level)
        .with_target(false)
        .init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_pair_neither_flag_is_none() {
        assert_eq!(override_pair(false, false), None);
    }

    #[test]
    fn override_pair_each_direction() {
        assert_eq!(override_pair(true, false), Some(true));
        assert_eq!(override_pair(false, true), Some(false));
    }

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("import-videos").chain(args.iter().copied())).unwrap()
    }

    fn scan_overrides(cli: Cli) -> OverrideFlags {
        match cli.command {
            Command::Scan { overrides, .. } => overrides,
            _ => panic!("expected Scan"),
        }
    }

    #[test]
    fn last_flag_of_a_pair_wins() {
        let overrides = scan_overrides(parse(&[
            "scan",
            "cam",
            "--no-copy-quarantine",
            "--copy-quarantine",
        ]));
        assert_eq!(overrides.to_overrides().copy_quarantine, Some(true));

        let overrides = scan_overrides(parse(&[
            "scan",
            "cam",
            "--copy-quarantine",
            "--no-copy-quarantine",
        ]));
        assert_eq!(overrides.to_overrides().copy_quarantine, Some(false));
    }

    #[test]
    fn quarantine_conflicts_with_no_copy_quarantine() {
        let result = Cli::try_parse_from([
            "import-videos",
            "scan",
            "cam",
            "--quarantine",
            "/tmp/q",
            "--no-copy-quarantine",
        ]);
        assert!(result.is_err());
    }

    #[test]
    fn keep_source_is_a_hidden_alias_of_no_delete_source() {
        let cli = parse(&["import", "cam", "--keep-source"]);
        match cli.command {
            Command::Import {
                no_delete_source, ..
            } => assert!(no_delete_source),
            _ => panic!("expected Import"),
        }
    }
}
