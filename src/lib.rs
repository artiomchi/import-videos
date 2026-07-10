//! Core library: all import-videos logic lives here so it is testable
//! independent of the CLI binary (ADR 0005). `main.rs` is a one-liner
//! that calls `run()` and exits with its code.

pub mod cli;
pub mod config;
pub mod error;
pub mod media;
pub mod plan;
pub mod report;
pub mod source;
pub mod transfer;

use std::path::Path;

use clap::Parser;

use cli::{Cli, Command};
use error::{Error, ExitCode, Result};

/// Parses arguments, runs the requested command, and returns the
/// process exit code (design D7: 0 success, 1 failure, 2 usage/config
/// error). Errors are printed here rather than propagated further —
/// this is the one place `main.rs` calls into.
pub fn run() -> i32 {
    let cli = Cli::parse();
    cli::init_tracing(cli.verbose);

    match run_inner(cli) {
        Ok(code) => code as i32,
        Err(e) => {
            eprintln!("error: {e}");
            e.exit_code() as i32
        }
    }
}

fn run_inner(cli: Cli) -> Result<ExitCode> {
    let config_path = match cli.config {
        Some(path) => path,
        None => config::default_config_path()
            .ok_or_else(|| Error::Config("could not determine the default config path".into()))?,
    };
    let cfg = config::load(&config_path)?;
    let verbose = cli.verbose > 0;

    match cli.command {
        Command::Scan { profile, source } => run_scan(&cfg, &profile, source.as_deref(), verbose),
        Command::Import {
            profile,
            source,
            dry_run,
            keep_source,
            yes,
            quick_match,
        } => run_import(
            &cfg,
            &profile,
            source.as_deref(),
            dry_run,
            keep_source,
            yes,
            quick_match,
            verbose,
        ),
    }
}

fn get_profile<'a>(cfg: &'a config::Config, name: &str) -> Result<&'a config::Profile> {
    cfg.profiles
        .get(name)
        .ok_or_else(|| Error::Config(format!("unknown profile '{name}'")))
}

/// Resolves a source and builds a plan for `profile_name`, handling the
/// two "nothing to do" cases (`scan` and `import` share this exactly:
/// spec requires `import` build "the same plan `scan` would").
/// `Ok(None)` means the caller should report "nothing found" and stop.
fn scan_profile(
    cfg: &config::Config,
    profile_name: &str,
    source_override: Option<&Path>,
) -> Result<Option<plan::ImportPlan>> {
    let profile = get_profile(cfg, profile_name)?;
    let source_impl = profile.kind.build();

    let Some(root) = plan::resolve_source(
        profile,
        source_override,
        source_impl.as_ref(),
        &cfg.mount_roots,
    )?
    else {
        return Ok(None);
    };

    let import_plan = plan::build_plan(profile, source_impl.as_ref(), &root, &cfg.timezone)?;
    if import_plan.actions.is_empty() {
        return Ok(None);
    }
    Ok(Some(import_plan))
}

fn run_scan(
    cfg: &config::Config,
    profile_name: &str,
    source_override: Option<&Path>,
    verbose: bool,
) -> Result<ExitCode> {
    match scan_profile(cfg, profile_name, source_override)? {
        None => {
            println!("no sources found for profile '{profile_name}'");
        }
        Some(import_plan) => print!(
            "{}",
            report::render_plan(&import_plan, verbose, &cfg.timezone)
        ),
    }
    Ok(ExitCode::Success)
}

#[allow(clippy::too_many_arguments)]
fn run_import(
    cfg: &config::Config,
    profile_name: &str,
    source_override: Option<&Path>,
    dry_run: bool,
    keep_source_flag: bool,
    assume_yes: bool,
    quick_match: bool,
    verbose: bool,
) -> Result<ExitCode> {
    let profile = get_profile(cfg, profile_name)?;

    let Some(import_plan) = scan_profile(cfg, profile_name, source_override)? else {
        println!("no sources found for profile '{profile_name}'");
        return Ok(ExitCode::Success);
    };

    if dry_run {
        print!(
            "{}",
            report::render_plan(&import_plan, verbose, &cfg.timezone)
        );
        return Ok(ExitCode::Success);
    }

    let exec_report = transfer::execute(
        &import_plan,
        profile.delete_source,
        keep_source_flag,
        assume_yes,
        quick_match,
    )?;
    print!("{}", report::render_results(&exec_report));

    let any_failed = exec_report.groups.iter().any(|g| {
        g.files
            .iter()
            .any(|f| matches!(f.outcome, transfer::TransferOutcome::Failed(_)))
            || matches!(
                g.sidecar_outcome,
                Some(transfer::TransferOutcome::Failed(_))
            )
    });

    Ok(if any_failed {
        ExitCode::Failure
    } else {
        ExitCode::Success
    })
}
