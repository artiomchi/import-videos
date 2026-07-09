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

        /// Never delete source files, even if the profile requests it
        #[arg(long)]
        keep_source: bool,

        /// Assume "yes" at confirmation prompts
        #[arg(long)]
        yes: bool,
    },
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
