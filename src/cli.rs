//! clap CLI surface: `scan` and `import`, plus the global flags every
//! subcommand shares.

use std::io::{self, Write};
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use tracing_subscriber::fmt::MakeWriter;

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

        /// Force copy-on-write cloning on for this run, even if the
        /// profile sets `reflink: false`
        #[arg(long, overrides_with = "no_reflink")]
        reflink: bool,

        /// Force copy-on-write cloning off for this run, even if the
        /// profile sets `reflink: true`; every file is stream-copied
        #[arg(long, overrides_with = "reflink")]
        no_reflink: bool,

        /// Override the profile's quarantine directory for this run;
        /// implies `--copy-quarantine` (design D4). Combining this with
        /// `--no-copy-quarantine` is a usage error. `import`-only: `scan`
        /// never resolves or shows a quarantine path (design D1/D7).
        #[arg(long, conflicts_with = "no_copy_quarantine")]
        quarantine: Option<PathBuf>,

        /// Force quarantine copying on for this run, even if the profile
        /// sets `copy_quarantine: false`
        #[arg(long, overrides_with = "no_copy_quarantine")]
        copy_quarantine: bool,

        /// Force quarantine copying off for this run, even if the
        /// profile sets `copy_quarantine: true`
        #[arg(long, overrides_with = "copy_quarantine")]
        no_copy_quarantine: bool,

        /// Force GoPro GPS telemetry lookup on for this run, even if the
        /// profile sets `gps_lookup: false` (GoPro profiles only)
        #[arg(long, overrides_with = "no_gopro_gps_lookup")]
        gopro_gps_lookup: bool,

        /// Force GoPro GPS telemetry lookup off for this run, even if
        /// the profile sets `gps_lookup: true` (GoPro profiles only)
        #[arg(long, overrides_with = "gopro_gps_lookup")]
        no_gopro_gps_lookup: bool,

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
/// D6): the one remaining pair whose effect `scan`'s inventory can
/// still show, since it changes verdict counts. Every other override
/// (`--quarantine`, `--copy-quarantine`/`--no-copy-quarantine`,
/// `--reflink`/`--no-reflink`, `--delete-source`/`--no-delete-source`,
/// `--gopro-gps-lookup`/`--no-gopro-gps-lookup`) lives on `Import`
/// alone, since `scan` never resolves or shows a destination/quarantine
/// path and never performs GPS lookup (design D1, D7).
#[derive(clap::Args, Debug, Default)]
pub struct OverrideFlags {
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
    /// Collapses the parsed flag pair into a plain `Overrides` (design
    /// D5) — neutral data only. Every other field is filled in
    /// separately by `Import`'s own flags in `run_inner`.
    pub fn to_overrides(&self) -> Overrides {
        Overrides {
            gopro_require_marker: override_pair(
                self.gopro_require_marker,
                self.no_gopro_require_marker,
            ),
            ..Default::default()
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
    pub reflink: Option<bool>,
    pub gps_lookup: Option<bool>,
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
///
/// Writes to stderr, never stdout (spec: "Diagnostic logging is
/// level-gated and never corrupts output") — stdout carries only the
/// plan/report/JSON document, and `--json` promises nothing else
/// appears there.
pub fn init_tracing(verbosity: u8) {
    let level = match verbosity {
        0 => tracing::Level::WARN,
        1 => tracing::Level::INFO,
        _ => tracing::Level::DEBUG,
    };
    tracing_subscriber::fmt()
        .with_max_level(level)
        .with_target(false)
        .with_writer(DiagnosticWriter)
        .init();
}

/// Writes diagnostic lines to stderr wrapped in `progress::suspend`
/// (design D8): while a progress bar is registered, this clears it,
/// prints the line, and lets the bar redraw underneath — the line
/// never lands mid-redraw. A no-op passthrough when no bar has ever
/// been registered, which is the common case outside an active scan
/// or transfer.
struct DiagnosticWriter;

impl Write for DiagnosticWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        crate::progress::suspend(|| io::stderr().write(buf))
    }

    fn flush(&mut self) -> io::Result<()> {
        io::stderr().flush()
    }
}

impl<'a> MakeWriter<'a> for DiagnosticWriter {
    type Writer = DiagnosticWriter;

    fn make_writer(&'a self) -> Self::Writer {
        DiagnosticWriter
    }
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
        // gopro_require_marker is the one pair still shared by scan and
        // import (design D7) — it's the flag exercised here.
        let overrides = scan_overrides(parse(&[
            "scan",
            "cam",
            "--no-gopro-require-marker",
            "--gopro-require-marker",
        ]));
        assert_eq!(overrides.to_overrides().gopro_require_marker, Some(true));

        let overrides = scan_overrides(parse(&[
            "scan",
            "cam",
            "--gopro-require-marker",
            "--no-gopro-require-marker",
        ]));
        assert_eq!(overrides.to_overrides().gopro_require_marker, Some(false));
    }

    #[test]
    fn import_only_last_flag_of_a_pair_wins() {
        // copy_quarantine and gps_lookup moved to Import-only (design
        // D7); last-one-wins still holds there.
        let cli = parse(&[
            "import",
            "cam",
            "--no-copy-quarantine",
            "--copy-quarantine",
            "--no-gopro-gps-lookup",
            "--gopro-gps-lookup",
        ]);
        match cli.command {
            Command::Import {
                copy_quarantine,
                no_copy_quarantine,
                gopro_gps_lookup,
                no_gopro_gps_lookup,
                ..
            } => {
                assert!(copy_quarantine && !no_copy_quarantine);
                assert!(gopro_gps_lookup && !no_gopro_gps_lookup);
            }
            _ => panic!("expected Import"),
        }
    }

    #[test]
    fn quarantine_conflicts_with_no_copy_quarantine() {
        let result = Cli::try_parse_from([
            "import-videos",
            "import",
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

    // --- scan-only usage errors (design D1/D7, task 3.5) ---
    //
    // scan never resolves or shows a destination/quarantine path and
    // never performs GPS lookup, so these Import-only flags are usage
    // errors on scan, exactly like `scan --reflink` already is.

    #[test]
    fn scan_quarantine_flag_fails_to_parse() {
        let result =
            Cli::try_parse_from(["import-videos", "scan", "cam", "--quarantine", "/tmp/q"]);
        assert!(result.is_err());
    }

    #[test]
    fn scan_copy_quarantine_flag_fails_to_parse() {
        let result = Cli::try_parse_from(["import-videos", "scan", "cam", "--copy-quarantine"]);
        assert!(result.is_err());
    }

    #[test]
    fn scan_no_copy_quarantine_flag_fails_to_parse() {
        let result = Cli::try_parse_from(["import-videos", "scan", "cam", "--no-copy-quarantine"]);
        assert!(result.is_err());
    }

    #[test]
    fn scan_gopro_gps_lookup_flag_fails_to_parse() {
        let result = Cli::try_parse_from(["import-videos", "scan", "cam", "--gopro-gps-lookup"]);
        assert!(result.is_err());
    }

    #[test]
    fn scan_no_gopro_gps_lookup_flag_fails_to_parse() {
        let result = Cli::try_parse_from(["import-videos", "scan", "cam", "--no-gopro-gps-lookup"]);
        assert!(result.is_err());
    }

    #[test]
    fn scan_reflink_flag_still_fails_to_parse() {
        // Pre-existing behavior (add-reflink-transfer); kept here as a
        // baseline alongside the new scan-only-usage-error tests above.
        let result = Cli::try_parse_from(["import-videos", "scan", "cam", "--reflink"]);
        assert!(result.is_err());
    }
}
