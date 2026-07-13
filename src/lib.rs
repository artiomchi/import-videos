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
use std::path::{Path, PathBuf};

use clap::Parser;

use cli::{Cli, Command};
use error::{Error, ExitCode, Result};
use source::ImportSource;

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
            quarantine,
            copy_quarantine,
            no_copy_quarantine,
            gopro_gps_lookup,
            no_gopro_gps_lookup,
            overrides,
        } => {
            let cfg = load_config(cli.config.as_deref())?;
            let mut overrides = overrides.to_overrides();
            overrides.delete_source = cli::override_pair(delete_source, no_delete_source);
            overrides.reflink = cli::override_pair(reflink, no_reflink);
            overrides.quarantine = quarantine;
            overrides.copy_quarantine = cli::override_pair(copy_quarantine, no_copy_quarantine);
            overrides.gps_lookup = cli::override_pair(gopro_gps_lookup, no_gopro_gps_lookup);
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
                ..
            } => *field = require_marker,
            _ => {
                return Err(Error::Config(format!(
                    "profile '{profile_name}': require_marker is only valid for profiles of type gopro"
                )));
            }
        }
    }

    if let Some(gps_lookup) = overrides.gps_lookup {
        match &mut profile.kind {
            config::SourceKind::Gopro {
                gps_lookup: field, ..
            } => *field = gps_lookup,
            _ => {
                return Err(Error::Config(format!(
                    "profile '{profile_name}': gps_lookup is only valid for profiles of type gopro"
                )));
            }
        }
    }

    Ok(profile)
}

/// A profile's source is either an explicit single path (CLI `--source`
/// override or a profile's own `source: <path>`) or zero-or-more
/// auto-detected drives (`source: auto`) — multi-drive-import design D1.
/// Explicit sourcing reuses `plan::resolve_source` untouched (task 1.3);
/// only the auto branch goes through the new `plan::resolve_sources`.
enum EffectiveSources {
    Explicit(PathBuf),
    Multi(Vec<plan::DetectedSource>),
}

/// Decides which of the two source-resolution paths a run takes,
/// without duplicating `resolve_source`'s explicit-path logic (design
/// D1). `resolve_source`'s explicit branch either resolves to `Some` or
/// returns `Err` — it only ever returns `Ok(None)` from its mount-roots
/// loop, which this function never reaches for the explicit case, so
/// the `expect` below cannot fail.
fn resolve_effective_sources(
    profile: &config::Profile,
    cli_source: Option<&Path>,
    source_impl: &dyn ImportSource,
    mount_roots: &[std::path::PathBuf],
) -> Result<EffectiveSources> {
    let explicit =
        cli_source.is_some() || matches!(profile.source, config::SourceLocation::Path(_));
    if explicit {
        let root = plan::resolve_source(profile, cli_source, source_impl, mount_roots)?
            .expect("explicit source resolution always yields Some or Err");
        Ok(EffectiveSources::Explicit(root))
    } else {
        Ok(EffectiveSources::Multi(plan::resolve_sources(
            source_impl,
            mount_roots,
        )))
    }
}

/// One detected drive's outcome after `run_scan_cycle` (multi-drive-import
/// design D3): distinct from a hard error, which is carried in the
/// `Result`'s `Err` side instead (so error-classification for exit codes
/// is never lost by converting it early — see `run_import_cycle`).
#[derive(Debug)]
pub enum ScanDriveOutcome {
    /// The drive was detected but its scan produced zero groups (spec:
    /// "A detected drive with nothing to import is reported distinctly").
    Empty,
    Found(plan::ScanSummary),
}

/// Mirrors `ScanDriveOutcome` for `import` (design D3). `Executed`'s
/// `any_failed` is the same per-file/sidecar failure check `run_import`
/// already performed for a single drive, now scoped to one drive in a
/// multi-drive batch.
#[derive(Debug)]
pub enum ImportDriveOutcome {
    Empty,
    /// `--dry-run`: plan built and printed, nothing executed.
    Planned(plan::ImportPlan),
    Executed {
        report: transfer::ExecuteReport,
        any_failed: bool,
    },
}

impl ImportDriveOutcome {
    /// Whether this drive's own outcome should count toward the run's
    /// aggregate failure (design D3): only an executed drive with at
    /// least one failed file/sidecar counts — `Empty` and `Planned`
    /// (dry-run) never do, since nothing was executed to fail.
    fn is_failure(&self) -> bool {
        matches!(
            self,
            ImportDriveOutcome::Executed {
                any_failed: true,
                ..
            }
        )
    }
}

/// One drive's identity (design: "Each drive is identified by name and
/// path") paired with the `Result` of running its cycle — `Err` for a
/// hard error caught at this drive rather than propagated (spec: "A
/// hard error on one drive is caught, not propagated").
#[derive(Debug)]
pub struct DriveResult<T> {
    pub name: String,
    pub path: PathBuf,
    pub result: Result<T>,
}

/// The run's aggregate exit-code input (design D7): `Failure` if any
/// drive recorded a hard error or a failed transfer, `Success`
/// otherwise — the same rule `run_import` already applied to a single
/// drive's report, folded across every drive in the batch.
pub fn any_import_drive_failed(results: &[DriveResult<ImportDriveOutcome>]) -> bool {
    results.iter().any(|r| match &r.result {
        Err(_) => true,
        Ok(outcome) => outcome.is_failure(),
    })
}

/// Prints a drive's name and full path before anything else about it
/// (spec: "Each drive is identified by name and path before its
/// output") — the one line every per-drive human-mode branch (found,
/// empty, error) is printed after.
fn print_drive_header(name: &str, path: &Path) {
    println!("== {name} ({}) ==", path.display());
}

/// Same `any_failed` check `run_import` has always applied to a single
/// drive's `ExecuteReport` (design D3), factored out so the multi-drive
/// loop can apply it per drive without duplicating the closure.
fn report_any_failed(report: &transfer::ExecuteReport) -> bool {
    report.groups.iter().any(|g| {
        g.files
            .iter()
            .any(|f| matches!(f.outcome, transfer::TransferOutcome::Failed(_)))
            || matches!(
                g.sidecar_outcome,
                Some(transfer::TransferOutcome::Failed(_))
            )
    })
}

/// The scan phase for one already-resolved source root — shared by the
/// explicit-source call site and the auto multi-drive loop (design D1's
/// shared-logic mitigation). Prints nothing for an `Empty` result: the
/// two callers render "nothing found" differently (explicit: "no
/// sources found"; multi-drive: a per-drive "no media found" line), so
/// that decision stays with them. A hard error propagates via `?`
/// (never caught here) so the explicit-source caller keeps today's
/// exact exit-code classification (design D3's per-drive catching only
/// happens in the multi-drive loop, one layer up).
fn run_scan_cycle(
    profile: &config::Profile,
    source_impl: &dyn ImportSource,
    source_root: &Path,
    tz: &jiff::tz::TimeZone,
    verbose: bool,
    json: bool,
) -> Result<ScanDriveOutcome> {
    let scan_progress = progress::Progress::counted(scan_progress_enabled(json), "Scanning");
    let summary = plan::build_scan_summary(profile, source_impl, source_root, tz, &scan_progress)?;

    if summary.entries.is_empty() {
        return Ok(ScanDriveOutcome::Empty);
    }
    if !json {
        print_scan_summary(&summary, verbose, tz, json);
    }
    Ok(ScanDriveOutcome::Found(summary))
}

/// The scan → build plan → print plan → confirm → execute → print
/// report cycle for one already-resolved source root (design D1's
/// shared-logic mitigation, task 2.1): called once for explicit
/// sourcing (today's exact output, task 2.5) and once per drive in the
/// auto multi-drive loop. JSON printing is never done here — callers
/// print it themselves, immediately for explicit sourcing (single
/// document) or once after every drive has run for the multi-drive
/// loop (one combined document, design D4) — so this function only
/// ever prints in human mode. A hard error propagates via `?`, exactly
/// like `run_scan_cycle` above.
#[allow(clippy::too_many_arguments)]
fn run_import_cycle(
    profile: &config::Profile,
    source_impl: &dyn ImportSource,
    source_root: &Path,
    dry_run: bool,
    assume_yes: bool,
    quick_match: bool,
    tz: &jiff::tz::TimeZone,
    verbose: bool,
    json: bool,
) -> Result<ImportDriveOutcome> {
    let scan_progress = progress::Progress::counted(scan_progress_enabled(json), "Scanning");
    let import_plan = plan::build_plan(profile, source_impl, source_root, tz, &scan_progress)?;

    if import_plan.actions.is_empty() {
        return Ok(ImportDriveOutcome::Empty);
    }

    if dry_run {
        if !json {
            print_plan(&import_plan, verbose, tz, json);
        }
        return Ok(ImportDriveOutcome::Planned(import_plan));
    }

    // A non-dry-run, human-mode import states its intent before
    // transferring anything (improve-console-output design D4): the
    // same plan `scan` would print.
    if !json {
        print_plan(&import_plan, verbose, tz, json);
    }

    // A separate, byte-oriented Progress for the transfer phase (design
    // D6), built after scanning completes.
    let progress = progress::Progress::new(scan_progress_enabled(json), "Importing");

    let exec_report = transfer::execute(
        &import_plan,
        source_root,
        profile.delete_source,
        assume_yes,
        quick_match,
        profile.reflink,
        &progress,
    )?;

    if !json {
        print!("{}", report::render_results(&exec_report, verbose));
    }

    let any_failed = report_any_failed(&exec_report);
    Ok(ImportDriveOutcome::Executed {
        report: exec_report,
        any_failed,
    })
}

/// Runs `scan`'s cycle once per detected drive (multi-drive-import
/// design D2), printing each drive's header first (human mode only —
/// JSON callers assemble their document from the returned results
/// instead) and catching a hard error at that drive rather than
/// propagating it (spec: "A hard error on one drive is caught, not
/// propagated").
pub fn scan_drives(
    profile: &config::Profile,
    source_impl: &dyn ImportSource,
    drives: &[plan::DetectedSource],
    tz: &jiff::tz::TimeZone,
    verbose: bool,
    json: bool,
) -> Vec<DriveResult<ScanDriveOutcome>> {
    drives
        .iter()
        .map(|drive| {
            if !json {
                print_drive_header(&drive.name, &drive.path);
            }
            let result = run_scan_cycle(profile, source_impl, &drive.path, tz, verbose, json);
            if !json {
                match &result {
                    Ok(ScanDriveOutcome::Empty) => {
                        print!(
                            "{}",
                            report::render_scan_summary(&plan::ScanSummary::default(), verbose, tz)
                        );
                    }
                    Ok(ScanDriveOutcome::Found(_)) => {}
                    Err(e) => println!("error: {e}"),
                }
            }
            DriveResult {
                name: drive.name.clone(),
                path: drive.path.clone(),
                result,
            }
        })
        .collect()
}

/// Runs `import`'s cycle once per detected drive, sequentially —
/// completing one drive's cycle (report printed) before the next
/// starts (spec: "Import processes drives sequentially with
/// independent confirmation") — with the same per-drive header and
/// error-catching as `scan_drives`.
#[allow(clippy::too_many_arguments)]
pub fn import_drives(
    profile: &config::Profile,
    source_impl: &dyn ImportSource,
    drives: &[plan::DetectedSource],
    dry_run: bool,
    assume_yes: bool,
    quick_match: bool,
    tz: &jiff::tz::TimeZone,
    verbose: bool,
    json: bool,
) -> Vec<DriveResult<ImportDriveOutcome>> {
    drives
        .iter()
        .map(|drive| {
            if !json {
                print_drive_header(&drive.name, &drive.path);
            }
            let result = run_import_cycle(
                profile,
                source_impl,
                &drive.path,
                dry_run,
                assume_yes,
                quick_match,
                tz,
                verbose,
                json,
            );
            if !json {
                match &result {
                    Ok(ImportDriveOutcome::Empty) => {
                        print!(
                            "{}",
                            report::render_plan(&plan::ImportPlan::default(), verbose, tz)
                        );
                    }
                    Ok(_) => {}
                    Err(e) => println!("error: {e}"),
                }
            }
            DriveResult {
                name: drive.name.clone(),
                path: drive.path.clone(),
                result,
            }
        })
        .collect()
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

fn print_scan_summary(
    summary: &plan::ScanSummary,
    verbose: bool,
    tz: &jiff::tz::TimeZone,
    json: bool,
) {
    if json {
        print_json(&report::scan_summary_to_json(summary, tz));
    } else {
        print!("{}", report::render_scan_summary(summary, verbose, tz));
    }
}

/// Progress bars are visible only on an interactive terminal with
/// JSON output off (design D6) — never interleaved with piped or
/// machine-readable output. Decided once per command and threaded
/// down, never re-derived (add-scan-progress design D4).
fn scan_progress_enabled(json: bool) -> bool {
    std::io::stdout().is_terminal() && !json
}

/// Dispatches on `EffectiveSources` (multi-drive-import design D1):
/// explicit sourcing runs `run_scan_cycle` once with today's exact
/// output; `source: auto` runs it once per detected drive via
/// `scan_drives`, printing each drive's header first and assembling one
/// combined JSON document afterward (design D4) rather than one per
/// drive. Zero detected drives reports "no sources found" exactly like
/// a single-source run (spec: "Zero matching drives").
fn run_scan(
    cfg: &config::Config,
    profile_name: &str,
    source_override: Option<&Path>,
    overrides: &cli::Overrides,
    verbose: bool,
    json: bool,
) -> Result<ExitCode> {
    let profile = resolve_profile(cfg, profile_name, overrides)?;
    let source_impl = profile.kind.build();

    match resolve_effective_sources(
        &profile,
        source_override,
        source_impl.as_ref(),
        &cfg.mount_roots,
    )? {
        EffectiveSources::Explicit(root) => {
            match run_scan_cycle(
                &profile,
                source_impl.as_ref(),
                &root,
                &cfg.timezone,
                verbose,
                json,
            )? {
                ScanDriveOutcome::Empty => print_no_sources(profile_name, json),
                ScanDriveOutcome::Found(summary) => {
                    if json {
                        print_json(&report::scan_summary_to_json(&summary, &cfg.timezone));
                    }
                }
            }
            Ok(ExitCode::Success)
        }
        EffectiveSources::Multi(drives) => {
            if drives.is_empty() {
                print_no_sources(profile_name, json);
                return Ok(ExitCode::Success);
            }
            let results = scan_drives(
                &profile,
                source_impl.as_ref(),
                &drives,
                &cfg.timezone,
                verbose,
                json,
            );
            let any_error = results.iter().any(|r| r.result.is_err());
            if json {
                let json_drives = results
                    .iter()
                    .map(|r| report::scan_drive_json(&r.name, &r.path, &r.result, &cfg.timezone))
                    .collect();
                print_json(&report::MultiScanJson {
                    drives: json_drives,
                });
            }
            Ok(if any_error {
                ExitCode::Failure
            } else {
                ExitCode::Success
            })
        }
    }
}

/// Mirrors `run_scan`'s dispatch for `import` (multi-drive-import
/// design D1-D3): explicit sourcing runs `run_import_cycle` once,
/// unchanged from before this change (task 2.5); `source: auto` runs it
/// once per detected drive sequentially via `import_drives`, aggregating
/// each drive's failure into the run's overall exit code (design D7).
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
    let source_impl = profile.kind.build();

    match resolve_effective_sources(
        &profile,
        source_override,
        source_impl.as_ref(),
        &cfg.mount_roots,
    )? {
        EffectiveSources::Explicit(root) => {
            match run_import_cycle(
                &profile,
                source_impl.as_ref(),
                &root,
                dry_run,
                assume_yes,
                quick_match,
                &cfg.timezone,
                verbose,
                json,
            )? {
                ImportDriveOutcome::Empty => {
                    print_no_sources(profile_name, json);
                    Ok(ExitCode::Success)
                }
                ImportDriveOutcome::Planned(plan) => {
                    if json {
                        print_json(&report::plan_to_json(&plan, &cfg.timezone));
                    }
                    Ok(ExitCode::Success)
                }
                ImportDriveOutcome::Executed { report, any_failed } => {
                    if json {
                        print_json(&report::results_to_json(&report));
                    }
                    Ok(if any_failed {
                        ExitCode::Failure
                    } else {
                        ExitCode::Success
                    })
                }
            }
        }
        EffectiveSources::Multi(drives) => {
            if drives.is_empty() {
                print_no_sources(profile_name, json);
                return Ok(ExitCode::Success);
            }
            let results = import_drives(
                &profile,
                source_impl.as_ref(),
                &drives,
                dry_run,
                assume_yes,
                quick_match,
                &cfg.timezone,
                verbose,
                json,
            );
            let any_failed = any_import_drive_failed(&results);
            if json {
                let json_drives = results
                    .iter()
                    .map(|r| report::import_drive_json(&r.name, &r.path, &r.result, &cfg.timezone))
                    .collect();
                print_json(&report::MultiImportJson {
                    drives: json_drives,
                    any_failed,
                });
            }
            Ok(if any_failed {
                ExitCode::Failure
            } else {
                ExitCode::Success
            })
        }
    }
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
