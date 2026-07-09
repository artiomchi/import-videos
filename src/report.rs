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
pub fn render_plan(plan: &ImportPlan) -> String {
    if plan.actions.is_empty() {
        return "No media found; nothing to import.\n".to_string();
    }

    let mut out = String::new();
    for action in &plan.actions {
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
    }
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
        if group.deleted_from_source {
            let _ = writeln!(out, "  deleted from source: {}", group.group_name);
        }
    }
    if let Some(reason) = &report.deletion_skipped_reason {
        let _ = writeln!(out, "{reason}");
    }
    out
}
