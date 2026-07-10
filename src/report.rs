//! Human-readable rendering of an `ImportPlan` (scan / dry-run output)
//! and of an execution report. Kept separate from `println!` call
//! sites in `lib.rs` so the formatting is unit-testable.
//!
//! JSON output (design D4) lives here too, as dedicated view-model
//! types (`PlanJson`, `ResultsJson`, ...) rather than `Serialize` on
//! the domain types directly — the JSON shape is a public contract
//! that shouldn't drift with internal refactors.

use std::fmt::Write;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde::Serialize;

use crate::plan::ImportPlan;
use crate::source::{Sidecar, Verdict};
use crate::transfer::{ExecuteReport, TransferOutcome};

const RFC3339_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%:z";

/// Renders `ts` as an RFC 3339 string in `tz` (design D4: "Timestamps
/// SHALL be RFC 3339 strings rendered in the configured timezone").
fn format_ts(ts: Timestamp, tz: &TimeZone) -> String {
    let zoned = ts.to_zoned(tz.clone());
    jiff::fmt::strtime::format(RFC3339_FORMAT, &zoned).unwrap_or_else(|_| ts.to_string())
}

/// Renders every planned action: verdict, reason, and resolved path.
/// `Keep`/`Quarantine` don't carry a per-group reason string (only
/// `Ignore` does — see `Verdict`), so they get a fixed label; richer
/// reasons are a device-module concern via the group's `context`.
///
/// Quarantine entries are omitted unless `verbose` — a real card can
/// quarantine hundreds of unmarked sessions, and scrolling past all of
/// them to see what's actually being kept isn't useful by default.
/// Marker details (per-marker timestamps) are likewise verbose-only.
/// A summary line always closes the output, so counts are visible
/// even when individual entries are suppressed.
pub fn render_plan(plan: &ImportPlan, verbose: bool, tz: &TimeZone) -> String {
    if plan.actions.is_empty() {
        return "No media found; nothing to import.\n".to_string();
    }

    let mut kept = 0usize;
    let mut quarantined = 0usize;
    let mut ignored = 0usize;

    let mut out = String::new();
    for action in &plan.actions {
        match &action.verdict {
            Verdict::Keep => kept += 1,
            Verdict::Quarantine => quarantined += 1,
            Verdict::Ignore(_) => ignored += 1,
        }

        if matches!(action.verdict, Verdict::Quarantine) && !verbose {
            continue;
        }

        let (verdict, reason, path) = match &action.verdict {
            Verdict::Keep => (
                "KEEP",
                "matches profile criteria".to_string(),
                action.destination.as_deref(),
            ),
            Verdict::Quarantine => (
                "QUARANTINE",
                "does not match profile criteria".to_string(),
                action.quarantine_path.as_deref(),
            ),
            Verdict::Ignore(reason) => ("IGNORE", reason.clone(), None),
        };
        let _ = write!(out, "[{verdict}] {} — {reason}", action.group.name);
        if let Some(path) = path {
            let _ = write!(out, " -> {}", path.display());
        } else if matches!(action.verdict, Verdict::Quarantine) {
            // quarantine_path is None only when copy_quarantine: false;
            // make this visible so the user knows the footage was
            // recognized but deliberately left on the source.
            let _ = write!(out, " (quarantine copy disabled, left on source)");
        }
        let _ = writeln!(out);

        if verbose {
            let zoned = action.group.timestamp.to_zoned(tz.clone());
            let rendered = jiff::fmt::strtime::format("%Y-%m-%dT%H:%M:%S%:z", &zoned)
                .unwrap_or_else(|_| action.group.timestamp.to_string());
            let _ = write!(out, "  recorded at: {rendered}");
            if let Some(source) = time_source(action.group.sidecar.as_ref()) {
                let _ = write!(out, " (source: {source})");
            }
            let _ = writeln!(out);
            for marker in &action.group.markers {
                let zoned_m = marker.timestamp.to_zoned(tz.clone());
                let rendered_m = jiff::fmt::strtime::format("%Y-%m-%dT%H:%M:%S%:z", &zoned_m)
                    .unwrap_or_else(|_| marker.timestamp.to_string());
                let _ = write!(out, "  marker at {rendered_m}");
                if let Some(label) = &marker.label {
                    let _ = write!(out, " ({label})");
                }
                let _ = writeln!(out);
            }
        }

        if let Some(sidecar) = &action.group.sidecar {
            let sidecar_path = path.map(|p| p.join(&sidecar.filename));
            match sidecar_path {
                Some(p) => {
                    let _ = writeln!(out, "  + sidecar: {}", p.display());
                }
                None => {
                    let _ = writeln!(out, "  + sidecar: {}", sidecar.filename);
                }
            }
        }
    }

    let _ = writeln!(
        out,
        "Summary: {kept} kept, {quarantined} quarantined, {ignored} ignored ({} total)",
        plan.actions.len()
    );

    out
}

/// A device's sidecar may note where a group's timestamp came from
/// (e.g. GoPro's `"time_source": "gps"`/`"camera"`) — read directly out
/// of the sidecar's JSON as an optional, soft convention rather than a
/// dedicated `MediaGroup` field, since it's device-specific and not
/// every device (or every group — quarantined groups have no sidecar
/// at all) will have one.
fn time_source(sidecar: Option<&Sidecar>) -> Option<&str> {
    sidecar?.content.get("time_source")?.as_str()
}

/// Renders the outcome of executing a plan: per-file transfer results,
/// which groups were cleaned off the source, and why deletion was
/// skipped, if it was.
pub fn render_results(report: &ExecuteReport) -> String {
    let mut out = String::new();
    for group in &report.groups {
        for file in &group.files {
            let line = match &file.outcome {
                TransferOutcome::Transferred => format!("transferred: {}", file.src.display()),
                TransferOutcome::SkippedIdentical => {
                    format!("skipped (already imported): {}", file.src.display())
                }
                TransferOutcome::SkippedQuickMatch => {
                    format!(
                        "skipped (quick-matched, not verified): {}",
                        file.src.display()
                    )
                }
                TransferOutcome::Suffixed(dest) => format!(
                    "stored as {} (destination name collision): {}",
                    dest.display(),
                    file.src.display()
                ),
                TransferOutcome::SkippedQuarantineDisabled => format!(
                    "left on source (quarantine copy disabled): {}",
                    file.src.display()
                ),
                TransferOutcome::Failed(message) => {
                    format!("FAILED: {} ({message})", file.src.display())
                }
            };
            let _ = writeln!(out, "{line}");
        }
        match &group.sidecar_outcome {
            Some(TransferOutcome::Transferred) => {
                let _ = writeln!(out, "  sidecar written: {}", group.group_name);
            }
            Some(TransferOutcome::Failed(message)) => {
                let _ = writeln!(out, "  SIDECAR FAILED: {} ({message})", group.group_name);
            }
            _ => {}
        }
        if group.deleted_from_source {
            let _ = writeln!(out, "  deleted from source: {}", group.group_name);
        }
    }
    if let Some(reason) = &report.deletion_skipped_reason {
        let _ = writeln!(out, "{reason}");
    }
    out
}

#[derive(Debug, Serialize)]
pub struct MarkerJson {
    pub time: String,
    pub label: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PlanActionJson {
    pub group: String,
    pub verdict: String,
    pub reason: String,
    pub path: Option<String>,
    /// `true` for a `Quarantine` group whose profile has
    /// `copy_quarantine: false` — recognized but deliberately left on
    /// the source, so `path` is `None` for a different reason than an
    /// `Ignore` verdict.
    pub quarantine_copy_disabled: bool,
    pub recorded_at: String,
    pub markers: Vec<MarkerJson>,
    pub sidecar_path: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PlanSummaryJson {
    pub kept: usize,
    pub quarantined: usize,
    pub ignored: usize,
    pub total: usize,
}

#[derive(Debug, Serialize)]
pub struct PlanJson {
    pub actions: Vec<PlanActionJson>,
    pub summary: PlanSummaryJson,
}

/// Builds the JSON view of an `ImportPlan` (design D4). Unlike
/// `render_plan`'s human output, quarantined entries are always
/// included — there is no verbose/non-verbose distinction in JSON mode
/// (spec: "including quarantined entries, which the human output hides
/// by default").
pub fn plan_to_json(plan: &ImportPlan, tz: &TimeZone) -> PlanJson {
    let mut kept = 0usize;
    let mut quarantined = 0usize;
    let mut ignored = 0usize;

    let actions = plan
        .actions
        .iter()
        .map(|action| {
            match &action.verdict {
                Verdict::Keep => kept += 1,
                Verdict::Quarantine => quarantined += 1,
                Verdict::Ignore(_) => ignored += 1,
            }

            let (verdict, reason, path) = match &action.verdict {
                Verdict::Keep => (
                    "keep",
                    "matches profile criteria".to_string(),
                    action.destination.as_deref(),
                ),
                Verdict::Quarantine => (
                    "quarantine",
                    "does not match profile criteria".to_string(),
                    action.quarantine_path.as_deref(),
                ),
                Verdict::Ignore(reason) => ("ignore", reason.clone(), None),
            };
            let quarantine_copy_disabled =
                matches!(action.verdict, Verdict::Quarantine) && action.quarantine_path.is_none();

            let markers = action
                .group
                .markers
                .iter()
                .map(|m| MarkerJson {
                    time: format_ts(m.timestamp, tz),
                    label: m.label.clone(),
                })
                .collect();

            let sidecar_path = action.group.sidecar.as_ref().map(|sidecar| match path {
                Some(p) => p.join(&sidecar.filename).display().to_string(),
                None => sidecar.filename.clone(),
            });

            PlanActionJson {
                group: action.group.name.clone(),
                verdict: verdict.to_string(),
                reason,
                path: path.map(|p| p.display().to_string()),
                quarantine_copy_disabled,
                recorded_at: format_ts(action.group.timestamp, tz),
                markers,
                sidecar_path,
            }
        })
        .collect::<Vec<_>>();

    let total = plan.actions.len();
    PlanJson {
        actions,
        summary: PlanSummaryJson {
            kept,
            quarantined,
            ignored,
            total,
        },
    }
}

#[derive(Debug, Serialize)]
pub struct FileResultJson {
    pub src: String,
    pub outcome: String,
    pub dest: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct GroupResultJson {
    pub group: String,
    pub verdict: String,
    pub files: Vec<FileResultJson>,
    pub sidecar_outcome: Option<String>,
    pub sidecar_error: Option<String>,
    pub deleted_from_source: bool,
}

#[derive(Debug, Serialize)]
pub struct ResultsSummaryJson {
    pub transferred: usize,
    pub failed: usize,
    pub deleted_groups: usize,
}

#[derive(Debug, Serialize)]
pub struct ResultsJson {
    pub groups: Vec<GroupResultJson>,
    pub deletion_skipped_reason: Option<String>,
    pub summary: ResultsSummaryJson,
}

fn outcome_json(outcome: &TransferOutcome) -> (String, Option<String>, Option<String>) {
    match outcome {
        TransferOutcome::Transferred => ("transferred".to_string(), None, None),
        TransferOutcome::SkippedIdentical => ("skipped_identical".to_string(), None, None),
        TransferOutcome::SkippedQuickMatch => ("skipped_quick_match".to_string(), None, None),
        TransferOutcome::SkippedQuarantineDisabled => {
            ("skipped_quarantine_disabled".to_string(), None, None)
        }
        TransferOutcome::Suffixed(dest) => (
            "suffixed".to_string(),
            Some(dest.display().to_string()),
            None,
        ),
        TransferOutcome::Failed(message) => ("failed".to_string(), None, Some(message.clone())),
    }
}

/// Builds the JSON view of an `ExecuteReport` (design D4).
pub fn results_to_json(report: &ExecuteReport) -> ResultsJson {
    let mut transferred = 0usize;
    let mut failed = 0usize;
    let mut deleted_groups = 0usize;

    let groups = report
        .groups
        .iter()
        .map(|group| {
            if group.deleted_from_source {
                deleted_groups += 1;
            }
            let files = group
                .files
                .iter()
                .map(|f| {
                    let (outcome, dest, error) = outcome_json(&f.outcome);
                    match &f.outcome {
                        TransferOutcome::Transferred => transferred += 1,
                        TransferOutcome::Failed(_) => failed += 1,
                        _ => {}
                    }
                    FileResultJson {
                        src: f.src.display().to_string(),
                        outcome,
                        dest,
                        error,
                    }
                })
                .collect();

            let (sidecar_outcome, sidecar_error) = match &group.sidecar_outcome {
                Some(TransferOutcome::Transferred) => (Some("transferred".to_string()), None),
                Some(TransferOutcome::Failed(message)) => {
                    (Some("failed".to_string()), Some(message.clone()))
                }
                Some(_) | None => (None, None),
            };

            GroupResultJson {
                group: group.group_name.clone(),
                verdict: match group.verdict {
                    Verdict::Keep => "keep".to_string(),
                    Verdict::Quarantine => "quarantine".to_string(),
                    Verdict::Ignore(_) => "ignore".to_string(),
                },
                files,
                sidecar_outcome,
                sidecar_error,
                deleted_from_source: group.deleted_from_source,
            }
        })
        .collect();

    ResultsJson {
        groups,
        deletion_skipped_reason: report.deletion_skipped_reason.clone(),
        summary: ResultsSummaryJson {
            transferred,
            failed,
            deleted_groups,
        },
    }
}

// --- cleanup (cli-maintenance) ---

/// Renders a human-readable size in bytes, KiB, MiB, or GiB — whichever
/// keeps the number readable.
fn format_size(bytes: u64) -> String {
    const UNITS: [(&str, u64); 3] = [("GiB", 1 << 30), ("MiB", 1 << 20), ("KiB", 1 << 10)];
    for (unit, threshold) in UNITS {
        if bytes >= threshold {
            return format!("{:.1} {unit}", bytes as f64 / threshold as f64);
        }
    }
    format!("{bytes} B")
}

fn format_age_days(age_seconds: i64) -> String {
    let days = age_seconds as f64 / 86_400.0;
    format!("{days:.1}d")
}

/// Renders a purge plan: every entry with its age and size, marked
/// kept or purge-candidate, closed with a summary line (design D1,
/// task 3.5).
pub fn render_cleanup_plan(plan: &crate::cleanup::CleanupPlan) -> String {
    if plan.entries.is_empty() {
        return format!("Nothing to clean in {}\n", plan.quarantine_root.display());
    }

    let mut out = String::new();
    let _ = writeln!(out, "Quarantine: {}", plan.quarantine_root.display());
    let (mut purge_count, mut purge_size) = (0usize, 0u64);
    let (mut keep_count, mut keep_size) = (0usize, 0u64);

    for entry in &plan.entries {
        let label = if entry.purge { "PURGE" } else { "KEEP" };
        if entry.purge {
            purge_count += 1;
            purge_size += entry.size;
        } else {
            keep_count += 1;
            keep_size += entry.size;
        }
        let _ = writeln!(
            out,
            "[{label}] {} — {} old, {}",
            entry.name,
            format_age_days(entry.age_seconds),
            format_size(entry.size)
        );
    }

    let _ = writeln!(
        out,
        "Summary: {purge_count} to purge ({}), {keep_count} kept ({})",
        format_size(purge_size),
        format_size(keep_size)
    );
    out
}

/// Renders the outcome of executing a cleanup plan.
pub fn render_cleanup_report(report: &crate::cleanup::CleanupReport) -> String {
    let mut out = String::new();
    for result in &report.results {
        match &result.error {
            None => {
                let _ = writeln!(out, "deleted: {}", result.path.display());
            }
            Some(message) => {
                let _ = writeln!(out, "FAILED to delete {}: {message}", result.path.display());
            }
        }
    }
    if let Some(reason) = &report.aborted_reason {
        let _ = writeln!(out, "{reason}");
    }
    out
}

#[derive(Debug, Serialize)]
pub struct CleanupEntryJson {
    pub name: String,
    pub path: String,
    pub age_seconds: i64,
    pub size_bytes: u64,
    pub purge: bool,
}

#[derive(Debug, Serialize)]
pub struct CleanupSummaryJson {
    pub purge_count: usize,
    pub purge_size_bytes: u64,
    pub keep_count: usize,
    pub keep_size_bytes: u64,
}

#[derive(Debug, Serialize)]
pub struct CleanupPlanJson {
    pub quarantine_root: String,
    pub entries: Vec<CleanupEntryJson>,
    pub summary: CleanupSummaryJson,
}

pub fn cleanup_plan_to_json(plan: &crate::cleanup::CleanupPlan) -> CleanupPlanJson {
    let (mut purge_count, mut purge_size) = (0usize, 0u64);
    let (mut keep_count, mut keep_size) = (0usize, 0u64);

    let entries = plan
        .entries
        .iter()
        .map(|e| {
            if e.purge {
                purge_count += 1;
                purge_size += e.size;
            } else {
                keep_count += 1;
                keep_size += e.size;
            }
            CleanupEntryJson {
                name: e.name.clone(),
                path: e.path.display().to_string(),
                age_seconds: e.age_seconds,
                size_bytes: e.size,
                purge: e.purge,
            }
        })
        .collect();

    CleanupPlanJson {
        quarantine_root: plan.quarantine_root.display().to_string(),
        entries,
        summary: CleanupSummaryJson {
            purge_count,
            purge_size_bytes: purge_size,
            keep_count,
            keep_size_bytes: keep_size,
        },
    }
}

#[derive(Debug, Serialize)]
pub struct CleanupResultJson {
    pub name: String,
    pub path: String,
    pub deleted: bool,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CleanupReportJson {
    pub results: Vec<CleanupResultJson>,
    pub aborted_reason: Option<String>,
}

pub fn cleanup_report_to_json(report: &crate::cleanup::CleanupReport) -> CleanupReportJson {
    CleanupReportJson {
        results: report
            .results
            .iter()
            .map(|r| CleanupResultJson {
                name: r.name.clone(),
                path: r.path.display().to_string(),
                deleted: r.deleted,
                error: r.error.clone(),
            })
            .collect(),
        aborted_reason: report.aborted_reason.clone(),
    }
}

// --- inspect (cli-maintenance) ---

/// Renders an MP4 metadata dump: creation time, HiLight markers, and
/// GPS summary, each section printing what parsed and naming what
/// didn't (spec: "Partial metadata still prints").
pub fn render_mp4_dump(dump: &crate::inspect::Mp4Dump, tz: &TimeZone) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "File: {}", dump.path.display());

    match &dump.creation_time {
        Ok(ts) => {
            let _ = writeln!(out, "Creation time: {}", format_ts(*ts, tz));
        }
        Err(e) => {
            let _ = writeln!(out, "Creation time: FAILED ({e})");
        }
    }

    match &dump.markers {
        Ok(markers) => {
            let _ = writeln!(out, "HiLight markers: {}", markers.len());
            for marker in markers {
                match marker.timestamp {
                    Some(ts) => {
                        let _ = writeln!(out, "  {} ms -> {}", marker.offset_ms, format_ts(ts, tz));
                    }
                    None => {
                        let _ = writeln!(out, "  {} ms", marker.offset_ms);
                    }
                }
            }
        }
        Err(e) => {
            let _ = writeln!(out, "HiLight markers: FAILED ({e})");
        }
    }

    match &dump.gps {
        Ok(Some(gps)) => {
            let _ = writeln!(out, "GPS: {} sample(s)", gps.sample_count);
            match gps.first_fix {
                Some((lat, lon)) => {
                    let _ = writeln!(out, "  first usable fix: {lat}, {lon}");
                    match gps.clock_offset_s {
                        Some(offset) => {
                            let _ = writeln!(out, "  clock offset: {offset:.3}s");
                        }
                        None => {
                            let _ = writeln!(out, "  clock offset: unknown (no creation time)");
                        }
                    }
                }
                None => {
                    let _ = writeln!(out, "  no usable fix found");
                }
            }
        }
        Ok(None) => {
            let _ = writeln!(out, "GPS: no gpmd track");
        }
        Err(e) => {
            let _ = writeln!(out, "GPS: FAILED ({e})");
        }
    }

    out
}

/// Renders a Tesla event dump: parsed `event.json` fields plus the
/// clip files present in the folder.
pub fn render_tesla_dump(dump: &crate::inspect::TeslaEventDump) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "Event: {}", dump.path.display());
    let _ = writeln!(
        out,
        "Timestamp: {}",
        dump.timestamp.as_deref().unwrap_or("(unknown)")
    );
    let _ = writeln!(
        out,
        "Reason: {}",
        dump.reason.as_deref().unwrap_or("(unknown)")
    );
    let _ = writeln!(out, "City: {}", dump.city.as_deref().unwrap_or("(unknown)"));
    match dump.coordinates {
        Some((lat, lon)) => {
            let _ = writeln!(out, "Coordinates: {lat}, {lon}");
        }
        None => {
            let _ = writeln!(out, "Coordinates: (unknown)");
        }
    }
    let _ = writeln!(out, "Files:");
    for file in &dump.clip_files {
        let _ = writeln!(out, "  {file}");
    }
    out
}

#[derive(Debug, Serialize)]
pub struct MarkerDumpJson {
    pub offset_ms: u32,
    pub timestamp: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct GpsSummaryJson {
    pub sample_count: usize,
    pub first_fix: Option<(f64, f64)>,
    pub clock_offset_s: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct Mp4DumpJson {
    pub path: String,
    pub creation_time: Option<String>,
    pub creation_time_error: Option<String>,
    pub markers: Vec<MarkerDumpJson>,
    pub markers_error: Option<String>,
    pub gps: Option<GpsSummaryJson>,
    pub gps_error: Option<String>,
}

pub fn mp4_dump_to_json(dump: &crate::inspect::Mp4Dump, tz: &TimeZone) -> Mp4DumpJson {
    let (creation_time, creation_time_error) = match &dump.creation_time {
        Ok(ts) => (Some(format_ts(*ts, tz)), None),
        Err(e) => (None, Some(e.clone())),
    };
    let (markers, markers_error) = match &dump.markers {
        Ok(markers) => (
            markers
                .iter()
                .map(|m| MarkerDumpJson {
                    offset_ms: m.offset_ms,
                    timestamp: m.timestamp.map(|ts| format_ts(ts, tz)),
                })
                .collect(),
            None,
        ),
        Err(e) => (Vec::new(), Some(e.clone())),
    };
    let (gps, gps_error) = match &dump.gps {
        Ok(Some(g)) => (
            Some(GpsSummaryJson {
                sample_count: g.sample_count,
                first_fix: g.first_fix,
                clock_offset_s: g.clock_offset_s,
            }),
            None,
        ),
        Ok(None) => (None, None),
        Err(e) => (None, Some(e.clone())),
    };

    Mp4DumpJson {
        path: dump.path.display().to_string(),
        creation_time,
        creation_time_error,
        markers,
        markers_error,
        gps,
        gps_error,
    }
}

#[derive(Debug, Serialize)]
pub struct TeslaDumpJson {
    pub path: String,
    pub timestamp: Option<String>,
    pub reason: Option<String>,
    pub city: Option<String>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub clip_files: Vec<String>,
}

pub fn tesla_dump_to_json(dump: &crate::inspect::TeslaEventDump) -> TeslaDumpJson {
    TeslaDumpJson {
        path: dump.path.display().to_string(),
        timestamp: dump.timestamp.clone(),
        reason: dump.reason.clone(),
        city: dump.city.clone(),
        lat: dump.coordinates.map(|(lat, _)| lat),
        lon: dump.coordinates.map(|(_, lon)| lon),
        clip_files: dump.clip_files.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::PlannedAction;
    use crate::source::{Marker, MediaGroup};
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn ts(secs: i64) -> jiff::Timestamp {
        jiff::Timestamp::from_second(secs).unwrap()
    }

    fn group(name: &str, markers: Vec<Marker>) -> MediaGroup {
        MediaGroup {
            name: name.to_string(),
            files: vec![],
            timestamp: ts(0),
            markers,
            geo: None,
            context: HashMap::new(),
            sidecar: None,
        }
    }

    fn plan_with_one_keep_one_quarantine(markers: Vec<Marker>) -> ImportPlan {
        ImportPlan {
            actions: vec![
                PlannedAction {
                    group: group("kept", markers),
                    verdict: Verdict::Keep,
                    destination: Some(PathBuf::from("/dest/kept")),
                    quarantine_path: None,
                },
                PlannedAction {
                    group: group("unmarked", vec![]),
                    verdict: Verdict::Quarantine,
                    destination: None,
                    quarantine_path: Some(PathBuf::from("/quarantine/unmarked")),
                },
            ],
        }
    }

    #[test]
    fn non_verbose_hides_quarantine_and_markers_but_shows_summary() {
        let markers = vec![Marker {
            timestamp: ts(1_000),
            label: None,
        }];
        let plan = plan_with_one_keep_one_quarantine(markers);

        let out = render_plan(&plan, false, &jiff::tz::TimeZone::UTC);

        assert!(out.contains("[KEEP] kept"));
        assert!(!out.contains("QUARANTINE"));
        assert!(!out.contains("marker at"));
        assert!(!out.contains("recorded at:"));
        assert!(out.contains("Summary: 1 kept, 1 quarantined, 0 ignored (2 total)"));
    }

    #[test]
    fn verbose_shows_quarantine_and_marker_details() {
        let markers = vec![Marker {
            timestamp: ts(1_000),
            label: None,
        }];
        let plan = plan_with_one_keep_one_quarantine(markers);

        let out = render_plan(&plan, true, &jiff::tz::TimeZone::UTC);

        assert!(out.contains("[KEEP] kept"));
        assert!(out.contains("[QUARANTINE] unmarked"));
        assert!(out.contains("marker at 1970-01-01T00:16:40+00:00"));
        assert!(
            out.contains("recorded at: 1970-01-01T00:00:00+00:00"),
            "verbose mode should show the group's (GPS-corrected, when available) recorded time"
        );
        assert!(out.contains("Summary: 1 kept, 1 quarantined, 0 ignored (2 total)"));
    }

    #[test]
    fn recorded_at_has_no_source_annotation_without_a_sidecar() {
        let plan = plan_with_one_keep_one_quarantine(vec![]);
        let out = render_plan(&plan, true, &jiff::tz::TimeZone::UTC);
        assert!(!out.contains("(source:"));
    }

    #[test]
    fn recorded_at_shows_time_source_from_sidecar() {
        let mut kept = group("kept", vec![]);
        kept.sidecar = Some(Sidecar {
            filename: "import.json".to_string(),
            content: serde_json::json!({"time_source": "gps"}),
        });
        let plan = ImportPlan {
            actions: vec![PlannedAction {
                group: kept,
                verdict: Verdict::Keep,
                destination: Some(PathBuf::from("/dest/kept")),
                quarantine_path: None,
            }],
        };

        let out = render_plan(&plan, true, &jiff::tz::TimeZone::UTC);

        assert!(out.contains("recorded at: 1970-01-01T00:00:00+00:00 (source: gps)"));
    }

    #[test]
    fn verbose_quarantine_with_disabled_copy_shows_note_not_path() {
        // Task 4.3: a Quarantine group with quarantine_path == None
        // (copy_quarantine: false) must render the disabled note in
        // both verbose and non-verbose modes.
        let plan = ImportPlan {
            actions: vec![PlannedAction {
                group: group("unmarked", vec![]),
                verdict: Verdict::Quarantine,
                destination: None,
                quarantine_path: None, // copy_quarantine: false
            }],
        };

        let out_verbose = render_plan(&plan, true, &jiff::tz::TimeZone::UTC);
        assert!(
            out_verbose.contains("[QUARANTINE] unmarked"),
            "verbose must show the quarantine entry"
        );
        assert!(
            out_verbose.contains("quarantine copy disabled"),
            "verbose must show disabled note"
        );
        assert!(
            !out_verbose.contains("->"),
            "must not show a path when copy is disabled"
        );
        assert!(out_verbose.contains("Summary: 0 kept, 1 quarantined, 0 ignored (1 total)"));
    }

    #[test]
    fn results_render_left_on_source_outcome() {
        // Task 4.3: SkippedQuarantineDisabled renders a clear message.
        use crate::transfer::{ExecuteReport, FileResult, GroupResult};
        let report = ExecuteReport {
            groups: vec![GroupResult {
                group_name: "unmarked".to_string(),
                verdict: Verdict::Quarantine,
                files: vec![FileResult {
                    src: PathBuf::from("/card/clip.mp4"),
                    outcome: TransferOutcome::SkippedQuarantineDisabled,
                }],
                sidecar_outcome: None,
                deleted_from_source: false,
            }],
            deletion_skipped_reason: None,
        };

        let out = render_results(&report);
        assert!(out.contains("left on source (quarantine copy disabled): /card/clip.mp4"));
    }

    // --- JSON view-models (task 2.3, design D4) ---

    #[test]
    fn plan_json_includes_quarantine_entries_unlike_human_render() {
        let plan = plan_with_one_keep_one_quarantine(vec![]);
        let json = plan_to_json(&plan, &jiff::tz::TimeZone::UTC);

        assert_eq!(json.actions.len(), 2);
        assert_eq!(json.summary.kept, 1);
        assert_eq!(json.summary.quarantined, 1);
        assert_eq!(json.summary.ignored, 0);
        assert_eq!(json.summary.total, 2);

        let quarantine_action = json
            .actions
            .iter()
            .find(|a| a.verdict == "quarantine")
            .unwrap();
        assert_eq!(quarantine_action.group, "unmarked");
        assert_eq!(
            quarantine_action.path.as_deref(),
            Some("/quarantine/unmarked")
        );
        assert!(!quarantine_action.quarantine_copy_disabled);
    }

    #[test]
    fn plan_json_marks_disabled_quarantine_copy() {
        let plan = ImportPlan {
            actions: vec![PlannedAction {
                group: group("unmarked", vec![]),
                verdict: Verdict::Quarantine,
                destination: None,
                quarantine_path: None,
            }],
        };
        let json = plan_to_json(&plan, &jiff::tz::TimeZone::UTC);
        assert!(json.actions[0].quarantine_copy_disabled);
        assert_eq!(json.actions[0].path, None);
    }

    #[test]
    fn plan_json_recorded_at_is_rfc3339() {
        let plan = plan_with_one_keep_one_quarantine(vec![]);
        let json = plan_to_json(&plan, &jiff::tz::TimeZone::UTC);
        let kept = json.actions.iter().find(|a| a.verdict == "keep").unwrap();
        assert_eq!(kept.recorded_at, "1970-01-01T00:00:00+00:00");
    }

    #[test]
    fn plan_json_serializes_to_valid_json() {
        let plan = plan_with_one_keep_one_quarantine(vec![]);
        let json = plan_to_json(&plan, &jiff::tz::TimeZone::UTC);
        let value = serde_json::to_value(&json).unwrap();
        assert!(value["actions"].is_array());
        assert_eq!(value["summary"]["total"], 2);
    }

    #[test]
    fn results_json_reports_outcomes_and_summary() {
        use crate::transfer::{ExecuteReport, FileResult, GroupResult};
        let report = ExecuteReport {
            groups: vec![
                GroupResult {
                    group_name: "kept".to_string(),
                    verdict: Verdict::Keep,
                    files: vec![FileResult {
                        src: PathBuf::from("/card/clip.mp4"),
                        outcome: TransferOutcome::Transferred,
                    }],
                    sidecar_outcome: Some(TransferOutcome::Transferred),
                    deleted_from_source: true,
                },
                GroupResult {
                    group_name: "broken".to_string(),
                    verdict: Verdict::Keep,
                    files: vec![FileResult {
                        src: PathBuf::from("/card/bad.mp4"),
                        outcome: TransferOutcome::Failed("disk full".to_string()),
                    }],
                    sidecar_outcome: None,
                    deleted_from_source: false,
                },
            ],
            deletion_skipped_reason: None,
        };

        let json = results_to_json(&report);
        assert_eq!(json.summary.transferred, 1);
        assert_eq!(json.summary.failed, 1);
        assert_eq!(json.summary.deleted_groups, 1);
        assert_eq!(json.groups[0].files[0].outcome, "transferred");
        assert_eq!(json.groups[1].files[0].outcome, "failed");
        assert_eq!(json.groups[1].files[0].error.as_deref(), Some("disk full"));
    }

    #[test]
    fn results_json_serializes_to_valid_json() {
        use crate::transfer::ExecuteReport;
        let report = ExecuteReport {
            groups: vec![],
            deletion_skipped_reason: Some("declined".to_string()),
        };
        let json = results_to_json(&report);
        let value = serde_json::to_value(&json).unwrap();
        assert_eq!(value["deletion_skipped_reason"], "declined");
        assert_eq!(value["summary"]["transferred"], 0);
    }
}
