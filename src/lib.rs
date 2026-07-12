//! Core library: all import-videos logic lives here so it is testable
//! independent of the CLI binary (ADR 0005). `main.rs` is a one-liner
//! that calls `run()` and exits with its code.

pub mod cleanup;
pub mod cli;
pub mod config;
pub mod error;
pub mod inspect;
pub mod media;
pub mod plan;
pub mod progress;
pub mod report;
pub mod source;
pub mod transfer;

use std::io::IsTerminal;
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

/// Dispatches to each subcommand, loading the config only where it's
/// actually needed (design D5) — `inspect` operates on a bare path and
/// must work with no config file present at all.
fn run_inner(cli: Cli) -> Result<ExitCode> {
    let verbose = cli.verbose > 0;
    let json = cli.json;

    match cli.command {
        Command::Scan {
            profile,
            source,
            overrides,
        } => {
            let cfg = load_config(cli.config.as_deref())?;
            let overrides = overrides.to_overrides();
            run_scan(&cfg, &profile, source.as_deref(), &overrides, verbose, json)
        }
        Command::Import {
            profile,
            source,
            dry_run,
            delete_source,
            no_delete_source,
            yes,
            quick_match,
            reflink,
            no_reflink,
            overrides,
        } => {
            let cfg = load_config(cli.config.as_deref())?;
            let mut overrides = overrides.to_overrides();
            overrides.delete_source = cli::override_pair(delete_source, no_delete_source);
            overrides.reflink = cli::override_pair(reflink, no_reflink);
            run_import(
                &cfg,
                &profile,
                source.as_deref(),
                dry_run,
                &overrides,
                yes,
                quick_match,
                verbose,
                json,
            )
        }
        Command::Cleanup {
            profile,
            older_than,
            dry_run,
            yes,
        } => {
            let cfg = load_config(cli.config.as_deref())?;
            run_cleanup(&cfg, &profile, older_than.as_deref(), dry_run, yes, json)
        }
        Command::Inspect { path } => run_inspect(&path, json),
    }
}

fn load_config(config_path: Option<&Path>) -> Result<config::Config> {
    let path = match config_path {
        Some(path) => path.to_path_buf(),
        None => config::default_config_path()
            .ok_or_else(|| Error::Config("could not determine the default config path".into()))?,
    };
    config::load(&path)
}

fn get_profile<'a>(cfg: &'a config::Config, name: &str) -> Result<&'a config::Profile> {
    cfg.profiles
        .get(name)
        .ok_or_else(|| Error::Config(format!("unknown profile '{name}'")))
}

/// Resolves `profile_name` and applies its per-invocation `Overrides`
/// (design D5), producing the effective `Profile` that planning and
/// execution consume unaware overrides exist (task 2.4). Overrides are
/// applied once, here, so `scan`, `--dry-run`, and `import` always see
/// the same result (spec: "Overrides SHALL be applied when the profile
/// is resolved, before planning").
fn resolve_profile(
    cfg: &config::Config,
    profile_name: &str,
    overrides: &cli::Overrides,
) -> Result<config::Profile> {
    let profile = get_profile(cfg, profile_name)?;
    apply_overrides(profile, overrides, profile_name)
}

/// Clones `profile` and shadows each field with a `Some` override
/// (design D5). `--quarantine` forces `copy_quarantine` on even when
/// `--no-copy-quarantine` wasn't passed (design D4; the contradictory
/// combination is already rejected at parse time by clap's
/// `conflicts_with`). A marker override against a non-GoPro profile
/// fails with the same wording `config::load` uses, so scripts see one
/// consistent error regardless of whether the mistake is in the YAML or
/// on the command line.
fn apply_overrides(
    profile: &config::Profile,
    overrides: &cli::Overrides,
    profile_name: &str,
) -> Result<config::Profile> {
    let mut profile = profile.clone();

    if let Some(delete_source) = overrides.delete_source {
        profile.delete_source = delete_source;
    }

    if let Some(reflink) = overrides.reflink {
        profile.reflink = reflink;
    }

    if let Some(copy_quarantine) = overrides.copy_quarantine {
        profile.copy_quarantine = copy_quarantine;
    }
    if overrides.quarantine.is_some() {
        profile.copy_quarantine = true;
    }

    if let Some(path) = &overrides.quarantine {
        let path = if path.is_absolute() {
            path.clone()
        } else {
            profile.destination.join(path)
        };
        profile.quarantine = Some(path);
    }

    if let Some(require_marker) = overrides.gopro_require_marker {
        match &mut profile.kind {
            config::SourceKind::Gopro {
                require_marker: field,
            } => *field = require_marker,
            _ => {
                return Err(Error::Config(format!(
                    "profile '{profile_name}': require_marker is only valid for profiles of type gopro"
                )));
            }
        }
    }

    Ok(profile)
}

/// Resolves a source and builds a plan for the already-resolved
/// `profile` (overrides applied — task 2.4, plan/transfer/report stay
/// override-unaware), handling the two "nothing to do" cases (`scan`
/// and `import` share this exactly: spec requires `import` build "the
/// same plan `scan` would"). `scan_progress` reports the scan phase's
/// progress (add-scan-progress design D1, D4) — its enabled/hidden
/// state is decided once by the caller, never re-derived here.
/// `Ok(None)` means the caller should report "nothing found" and stop.
fn scan_profile(
    cfg: &config::Config,
    profile: &config::Profile,
    source_override: Option<&Path>,
    scan_progress: &progress::Progress,
) -> Result<Option<plan::ImportPlan>> {
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

    let import_plan = plan::build_plan(
        profile,
        source_impl.as_ref(),
        &root,
        &cfg.timezone,
        scan_progress,
    )?;
    if import_plan.actions.is_empty() {
        return Ok(None);
    }
    Ok(Some(import_plan))
}

/// Prints "no sources found" as a bare string, or — under `--json` — as
/// a JSON document (design D4: "not a bare human string"), so scripted
/// callers never have to special-case this outcome.
fn print_no_sources(profile_name: &str, json: bool) {
    if json {
        print_json(&serde_json::json!({
            "status": "no_sources",
            "profile": profile_name,
        }));
    } else {
        println!("no sources found for profile '{profile_name}'");
    }
}

fn print_json(value: &impl serde::Serialize) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).expect("view-model types always serialize")
    );
}

fn print_plan(plan: &plan::ImportPlan, verbose: bool, tz: &jiff::tz::TimeZone, json: bool) {
    if json {
        print_json(&report::plan_to_json(plan, tz));
    } else {
        print!("{}", report::render_plan(plan, verbose, tz));
    }
}

/// Progress bars are visible only on an interactive terminal with
/// JSON output off (design D6) — never interleaved with piped or
/// machine-readable output. Decided once per command and threaded
/// down, never re-derived (add-scan-progress design D4).
fn scan_progress_enabled(json: bool) -> bool {
    std::io::stdout().is_terminal() && !json
}

fn run_scan(
    cfg: &config::Config,
    profile_name: &str,
    source_override: Option<&Path>,
    overrides: &cli::Overrides,
    verbose: bool,
    json: bool,
) -> Result<ExitCode> {
    let profile = resolve_profile(cfg, profile_name, overrides)?;
    let scan_progress = progress::Progress::counted(scan_progress_enabled(json), "Scanning");
    match scan_profile(cfg, &profile, source_override, &scan_progress)? {
        None => print_no_sources(profile_name, json),
        Some(import_plan) => print_plan(&import_plan, verbose, &cfg.timezone, json),
    }
    Ok(ExitCode::Success)
}

#[allow(clippy::too_many_arguments)]
fn run_import(
    cfg: &config::Config,
    profile_name: &str,
    source_override: Option<&Path>,
    dry_run: bool,
    overrides: &cli::Overrides,
    assume_yes: bool,
    quick_match: bool,
    verbose: bool,
    json: bool,
) -> Result<ExitCode> {
    let profile = resolve_profile(cfg, profile_name, overrides)?;

    let scan_progress = progress::Progress::counted(scan_progress_enabled(json), "Scanning");
    let Some(import_plan) = scan_profile(cfg, &profile, source_override, &scan_progress)? else {
        print_no_sources(profile_name, json);
        return Ok(ExitCode::Success);
    };

    if dry_run {
        print_plan(&import_plan, verbose, &cfg.timezone, json);
        return Ok(ExitCode::Success);
    }

    // A non-dry-run, human-mode import states its intent before
    // transferring anything (improve-console-output design D4): the
    // same plan `scan` would print. JSON mode stays a single document
    // (the execution report) — the spec's one-document contract wins
    // over symmetry there.
    if !json {
        print_plan(&import_plan, verbose, &cfg.timezone, json);
    }

    // A separate, byte-oriented Progress for the transfer phase (design
    // D6), built after scanning completes — the scan bar has already
    // finished and cleared by this point (add-scan-progress design D5).
    let progress = progress::Progress::new(scan_progress_enabled(json), "Importing");

    let exec_report = transfer::execute(
        &import_plan,
        profile.delete_source,
        assume_yes,
        quick_match,
        profile.reflink,
        &progress,
    )?;

    if json {
        print_json(&report::results_to_json(&exec_report));
    } else {
        print!("{}", report::render_results(&exec_report, verbose));
    }

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

fn parse_older_than(raw: &str) -> Result<jiff::Span> {
    raw.parse::<jiff::Span>()
        .map_err(|e| Error::Config(format!("--older-than: invalid span '{raw}': {e}")))
}

fn print_cleanup_plan(plan: &cleanup::CleanupPlan, json: bool) {
    if json {
        print_json(&report::cleanup_plan_to_json(plan));
    } else {
        print!("{}", report::render_cleanup_plan(plan));
    }
}

fn run_cleanup(
    cfg: &config::Config,
    profile_name: &str,
    older_than: Option<&str>,
    dry_run: bool,
    assume_yes: bool,
    json: bool,
) -> Result<ExitCode> {
    let profile = get_profile(cfg, profile_name)?;
    let older_than = older_than.map(parse_older_than).transpose()?;

    let plan = cleanup::build_plan(profile, older_than, &cfg.timezone, jiff::Timestamp::now())?;

    if plan.entries.is_empty() || dry_run {
        print_cleanup_plan(&plan, json);
        return Ok(ExitCode::Success);
    }

    if !json {
        print!("{}", report::render_cleanup_plan(&plan));
    }

    let exec_report = cleanup::execute(&plan, assume_yes)?;

    if json {
        print_json(&report::cleanup_report_to_json(&exec_report));
    } else {
        print!("{}", report::render_cleanup_report(&exec_report));
    }

    let any_failed = exec_report.results.iter().any(|r| !r.deleted);
    Ok(if any_failed {
        ExitCode::Failure
    } else {
        ExitCode::Success
    })
}

/// `inspect` needs no profile and no config (design D5) — it dispatches
/// on the path alone, using the system timezone for rendering
/// (`TimeZone::system()` already falls back to UTC when it can't be
/// determined).
fn run_inspect(path: &Path, json: bool) -> Result<ExitCode> {
    let target = inspect::classify(path).map_err(Error::Config)?;
    let tz = jiff::tz::TimeZone::system();

    match target {
        inspect::InspectTarget::Mp4(mp4_path) => {
            let dump = inspect::inspect_mp4(&mp4_path)?;
            let has_errors = dump.has_errors();
            if json {
                print_json(&report::mp4_dump_to_json(&dump, &tz));
            } else {
                print!("{}", report::render_mp4_dump(&dump, &tz));
            }
            Ok(if has_errors {
                ExitCode::Failure
            } else {
                ExitCode::Success
            })
        }
        inspect::InspectTarget::TeslaEvent(dir) => {
            let dump = inspect::inspect_tesla_event(&dir)?;
            if json {
                print_json(&report::tesla_dump_to_json(&dump));
            } else {
                print!("{}", report::render_tesla_dump(&dump));
            }
            Ok(ExitCode::Success)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn profile(reflink: bool) -> config::Profile {
        config::Profile {
            kind: config::SourceKind::Generic,
            source: config::SourceLocation::Auto,
            destination: PathBuf::from("/dest"),
            layout: config::LayoutTemplate::parse("{date}").unwrap(),
            ignore: globset::GlobSetBuilder::new().build().unwrap(),
            quarantine: None,
            delete_source: false,
            copy_quarantine: true,
            reflink,
        }
    }

    // --- reflink override resolution (add-reflink-transfer, task 6.6) ---

    #[test]
    fn reflink_override_forces_cloning_off() {
        // Spec scenario: "Reflink override forces cloning off"
        let base = profile(true);
        let overrides = cli::Overrides {
            reflink: Some(false),
            ..Default::default()
        };
        let resolved = apply_overrides(&base, &overrides, "cam").unwrap();
        assert!(!resolved.reflink);
    }

    #[test]
    fn reflink_override_forces_cloning_on() {
        // Spec scenario: "Reflink override forces cloning on"
        let base = profile(false);
        let overrides = cli::Overrides {
            reflink: Some(true),
            ..Default::default()
        };
        let resolved = apply_overrides(&base, &overrides, "cam").unwrap();
        assert!(resolved.reflink);
    }

    #[test]
    fn unset_reflink_override_keeps_the_profile_value() {
        let base = profile(true);
        let resolved = apply_overrides(&base, &cli::Overrides::default(), "cam").unwrap();
        assert!(resolved.reflink);
    }
}
