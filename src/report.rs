//! Human-readable rendering of an `ImportPlan` (scan / dry-run output)
//! and of an execution report. Kept separate from `println!` call
//! sites in `lib.rs` so the formatting is unit-testable.

use std::fmt::Write;

use crate::plan::ImportPlan;
use crate::source::Verdict;
use crate::transfer::{ExecuteReport, TransferOutcome};

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
pub fn render_plan(plan: &ImportPlan, verbose: bool) -> String {
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
        }
        let _ = writeln!(out);

        if verbose {
            for marker in &action.group.markers {
                let _ = write!(out, "  marker at {}", marker.timestamp);
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
                TransferOutcome::Suffixed(dest) => format!(
                    "stored as {} (destination name collision): {}",
                    dest.display(),
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

        let out = render_plan(&plan, false);

        assert!(out.contains("[KEEP] kept"));
        assert!(!out.contains("QUARANTINE"));
        assert!(!out.contains("marker at"));
        assert!(out.contains("Summary: 1 kept, 1 quarantined, 0 ignored (2 total)"));
    }

    #[test]
    fn verbose_shows_quarantine_and_marker_details() {
        let markers = vec![Marker {
            timestamp: ts(1_000),
            label: None,
        }];
        let plan = plan_with_one_keep_one_quarantine(markers);

        let out = render_plan(&plan, true);

        assert!(out.contains("[KEEP] kept"));
        assert!(out.contains("[QUARANTINE] unmarked"));
        assert!(out.contains("marker at 1970-01-01T00:16:40Z"));
        assert!(out.contains("Summary: 1 kept, 1 quarantined, 0 ignored (2 total)"));
    }
}
