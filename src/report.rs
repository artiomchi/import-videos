//! Human-readable rendering of an `ImportPlan` (scan / dry-run output)
//! and of an execution report. Kept separate from `println!` call
//! sites in `lib.rs` so the formatting is unit-testable.

use std::fmt::Write;

use jiff::tz::TimeZone;

use crate::plan::ImportPlan;
use crate::source::{Sidecar, Verdict};
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
}
