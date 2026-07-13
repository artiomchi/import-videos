//! Human-readable rendering of an `ImportPlan` (scan / dry-run output)
//! and of an execution report. Kept separate from `println!` call
//! sites in `lib.rs` so the formatting is unit-testable.
//!
//! JSON output (design D4) lives here too, as dedicated view-model
//! types (`PlanJson`, `ResultsJson`, ...) rather than `Serialize` on
//! the domain types directly — the JSON shape is a public contract
//! that shouldn't drift with internal refactors.

use std::fmt::Write;
use std::path::Path;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde::Serialize;

use crate::plan::{ImportPlan, PlannedAction, ScanEntry, ScanSummary};
use crate::source::{MediaFile, Sidecar, Verdict};
use crate::transfer::{ExecuteReport, FileResult, GroupResult, TransferOutcome};

const RFC3339_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%:z";
const SHORT_TIME_FORMAT: &str = "%Y-%m-%d %H:%M";

/// The exact reason string both device modules (`gopro`, `tesla`) use
/// for their catch-all stray-files group (design D6) — the hook
/// `render_plan` uses to special-case that one group's listing instead
/// of the usual time/size line.
const UNRECOGNIZED_REASON: &str = "unrecognized file(s)";
const UNRECOGNIZED_DEFAULT_CAP: usize = 5;

/// Render-detail tier for plan/results/cleanup rendering (design
/// Decision 1): replaces a plain `verbose: bool` so the illegal state
/// "both summary and verbose" is unconstructable rather than merely
/// convention. Computed once in `lib.rs` from `cli.summary`/`cli.verbose`
/// and threaded down; `report.rs` never reads `-v`'s log-level effect,
/// only this.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Detail {
    /// Progress bars plus a closing summary/tally line only — no
    /// per-group or per-entry listing.
    Summary,
    Normal,
    Verbose,
}

/// Formats `n` with `word` pluralized the plain way (`"1 file"`,
/// `"3 files"`) — every count in the plan/results renderers goes
/// through this so wording stays consistent.
fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// Renders `ts` as an RFC 3339 string in `tz` (design D4: "Timestamps
/// SHALL be RFC 3339 strings rendered in the configured timezone").
fn format_ts(ts: Timestamp, tz: &TimeZone) -> String {
    let zoned = ts.to_zoned(tz.clone());
    jiff::fmt::strtime::format(RFC3339_FORMAT, &zoned).unwrap_or_else(|_| ts.to_string())
}

/// Per-verdict running totals (design D5's summary extension, task
/// 6.5): group count plus the file count and byte total across every
/// group of that verdict, so the closing summary line carries the same
/// weight information the entries themselves show.
#[derive(Default)]
struct VerdictTally {
    groups: usize,
    files: usize,
    bytes: u64,
}

impl VerdictTally {
    fn add(&mut self, files: &[MediaFile]) {
        self.add_counts(files.len(), files.iter().map(|f| f.size).sum());
    }

    /// Same tally, from a scan entry's already-computed counts rather
    /// than a live `&[MediaFile]` slice — `ScanEntry` carries only the
    /// count and byte total, not the files themselves (design D1).
    fn add_counts(&mut self, file_count: usize, bytes: u64) {
        self.groups += 1;
        self.files += file_count;
        self.bytes += bytes;
    }

    fn render(&self, label: &str) -> String {
        format!(
            "{} {label} ({}, {})",
            self.groups,
            plural(self.files, "file"),
            format_size(self.bytes)
        )
    }
}

#[derive(Default)]
struct VerdictTotals {
    kept: VerdictTally,
    quarantined: VerdictTally,
    ignored: VerdictTally,
}

impl VerdictTotals {
    fn record(&mut self, verdict: &Verdict, files: &[MediaFile]) {
        match verdict {
            Verdict::Keep => self.kept.add(files),
            Verdict::Quarantine => self.quarantined.add(files),
            Verdict::Ignore(_) => self.ignored.add(files),
        }
    }

    fn record_counts(&mut self, verdict: &Verdict, file_count: usize, bytes: u64) {
        match verdict {
            Verdict::Keep => self.kept.add_counts(file_count, bytes),
            Verdict::Quarantine => self.quarantined.add_counts(file_count, bytes),
            Verdict::Ignore(_) => self.ignored.add_counts(file_count, bytes),
        }
    }

    fn total_groups(&self) -> usize {
        self.kept.groups + self.quarantined.groups + self.ignored.groups
    }

    fn render(&self) -> String {
        format!(
            "{}, {}, {} ({} total)",
            self.kept.render("kept"),
            self.quarantined.render("quarantined"),
            self.ignored.render("ignored"),
            self.total_groups()
        )
    }
}

/// Renders every planned action, accounting for every group either
/// individually or in aggregate (design D5, D6). `Keep` and (with
/// `-v`) `Quarantine` entries show the group's recorded time, file
/// count, total size, and resolved path — no fixed per-verdict reason
/// text; only `Ignore` carries a reason, since it's the one verdict
/// where the reason varies per group. Quarantine entries collapse into
/// a single rollup line by default — a real card can quarantine
/// hundreds of unmarked sessions, and scrolling past all of them to
/// see what's actually being kept isn't useful. The unrecognized-files
/// group (present when a source has stray files) lists names instead,
/// capped at 5 by default. A summary line with per-verdict file/byte
/// totals always closes the output, so counts stay visible even when
/// entries are aggregated or capped.
pub fn render_plan(plan: &ImportPlan, detail: Detail, tz: &TimeZone) -> String {
    if plan.actions.is_empty() {
        return "No media found; nothing to import.\n".to_string();
    }

    let mut out = String::new();
    let mut totals = VerdictTotals::default();
    let mut quarantine_entries: Vec<&PlannedAction> = Vec::new();

    for action in &plan.actions {
        totals.record(&action.verdict, &action.group.files);

        if matches!(action.verdict, Verdict::Quarantine) {
            quarantine_entries.push(action);
            if detail != Detail::Verbose {
                continue;
            }
        }

        if detail != Detail::Summary {
            render_plan_entry(&mut out, action, detail, tz);
        }
    }

    if detail == Detail::Normal && !quarantine_entries.is_empty() {
        render_quarantine_rollup(&mut out, &quarantine_entries);
    }

    let _ = writeln!(out, "Summary: {}", totals.render());

    out
}

/// Renders one plan entry: `[VERDICT] name`, then either the
/// unrecognized-files listing or (for `Keep`/`Quarantine`) the time,
/// size, and resolved path — `Ignore`'s reason clause is the only
/// fixed-string exception, per design D5. Verbose-only detail (full
/// RFC 3339 time, markers, sidecar filename) follows on indented
/// lines.
fn render_plan_entry(out: &mut String, action: &PlannedAction, detail: Detail, tz: &TimeZone) {
    let label = match &action.verdict {
        Verdict::Keep => "KEEP",
        Verdict::Quarantine => "QUARANTINE",
        Verdict::Ignore(_) => "IGNORE",
    };
    let path = match &action.verdict {
        Verdict::Keep => action.destination.as_deref(),
        Verdict::Quarantine => action.quarantine_path.as_deref(),
        Verdict::Ignore(_) => None,
    };

    let _ = write!(out, "[{label}] {}", action.group.name);
    if let Verdict::Ignore(reason) = &action.verdict {
        let _ = write!(out, " — {reason}");
    }

    let is_unrecognized =
        matches!(&action.verdict, Verdict::Ignore(reason) if reason == UNRECOGNIZED_REASON);
    if is_unrecognized {
        render_unrecognized_files(out, &action.group.files, detail);
    } else if !matches!(action.verdict, Verdict::Ignore(_)) {
        let group_bytes: u64 = action.group.files.iter().map(|f| f.size).sum();
        let short_time = format_short_ts(action.group.timestamp, tz);
        let _ = write!(
            out,
            "  {short_time}  {}, {}",
            plural(action.group.files.len(), "file"),
            format_size(group_bytes)
        );
        if let Some(path) = path {
            let _ = write!(out, " -> {}", path.display());
        } else if matches!(action.verdict, Verdict::Quarantine) {
            // quarantine_path is None only when copy_quarantine: false;
            // make this visible so the user knows the footage was
            // recognized but deliberately left on the source.
            let _ = write!(out, " (quarantine copy disabled, left on source)");
        }
    }
    let _ = writeln!(out);

    if detail == Detail::Verbose {
        let _ = write!(
            out,
            "  recorded at: {}",
            format_ts(action.group.timestamp, tz)
        );
        if let Some(source) = time_source(action.group.sidecar.as_ref()) {
            let _ = write!(out, " (source: {source})");
        }
        let _ = writeln!(out);
        for marker in &action.group.markers {
            let _ = write!(out, "  marker at {}", format_ts(marker.timestamp, tz));
            if let Some(label) = &marker.label {
                let _ = write!(out, " ({label})");
            }
            let _ = writeln!(out);
        }
        if let Some(sidecar) = &action.group.sidecar {
            let _ = writeln!(out, "  + sidecar: {}", sidecar.filename);
        }
    }
}

fn format_short_ts(ts: Timestamp, tz: &TimeZone) -> String {
    let zoned = ts.to_zoned(tz.clone());
    jiff::fmt::strtime::format(SHORT_TIME_FORMAT, &zoned).unwrap_or_else(|_| ts.to_string())
}

/// Lists an unrecognized-files group's file names, sorted for
/// determinism, capped at `UNRECOGNIZED_DEFAULT_CAP` unless `verbose`
/// (design D6). The count lands in the entry line itself; a trailing
/// "… and N more" line appears only when the cap actually truncates —
/// at or under the cap, default and verbose output are identical.
fn render_unrecognized_files(out: &mut String, files: &[MediaFile], detail: Detail) {
    let names: Vec<String> = files.iter().map(|f| file_display_name(&f.path)).collect();
    render_file_name_list(out, names, detail);
}

/// Same rendering as `render_unrecognized_files`, from a scan entry's
/// already-collected file path strings (design D1) rather than a live
/// `&[MediaFile]` slice.
fn render_scan_unrecognized_files(out: &mut String, files: &[String], detail: Detail) {
    let names: Vec<String> = files
        .iter()
        .map(|f| file_display_name(Path::new(f)))
        .collect();
    render_file_name_list(out, names, detail);
}

/// Shared body of `render_unrecognized_files`/`render_scan_unrecognized_files`
/// (design D6, task 5.1): sorts, caps at `UNRECOGNIZED_DEFAULT_CAP`
/// unless `Detail::Verbose`, and appends a "… and N more (-v to list
/// all)" line only when the cap actually truncates. Never called under
/// `Detail::Summary` — the unrecognized-files listing is omitted
/// entirely in that mode (design Decision 2), same as every other
/// per-action listing.
fn render_file_name_list(out: &mut String, mut names: Vec<String>, detail: Detail) {
    names.sort();

    let _ = write!(out, " ({})", plural(names.len(), "file"));
    let _ = writeln!(out);

    let shown = if detail == Detail::Verbose {
        names.len()
    } else {
        names.len().min(UNRECOGNIZED_DEFAULT_CAP)
    };
    for name in &names[..shown] {
        let _ = writeln!(out, "  {name}");
    }
    let remaining = names.len() - shown;
    if remaining > 0 {
        let _ = writeln!(out, "  … and {remaining} more (-v to list all)");
    }
}

fn file_display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// One default-mode line standing in for every `Quarantine` entry
/// (design D5): count, aggregate size, and the shared quarantine root
/// — or a disabled note when `copy_quarantine: false` left every entry
/// with no resolved path. All `Quarantine` actions in one plan share a
/// profile-level `copy_quarantine` setting, so the first entry's
/// presence/absence of a path is representative of the whole group.
fn render_quarantine_rollup(out: &mut String, entries: &[&PlannedAction]) {
    let count = entries.len();
    let bytes: u64 = entries
        .iter()
        .flat_map(|a| &a.group.files)
        .map(|f| f.size)
        .sum();
    let root = entries
        .iter()
        .find_map(|a| a.quarantine_path.as_deref().and_then(Path::parent));

    let _ = write!(
        out,
        "Quarantine: {}, {}",
        plural(count, "group"),
        format_size(bytes)
    );
    match root {
        Some(root) => {
            let _ = write!(out, " -> {}", root.display());
        }
        None => {
            let _ = write!(out, " (quarantine copy disabled)");
        }
    }
    let _ = writeln!(out, "  (-v to list)");
}

/// Renders `scan`'s source-only inventory (design D1): the same
/// verdict-tally / unrecognized-files-cap conventions `render_plan`
/// uses, but with no per-entry destination or quarantine path —
/// `ScanEntry` is structurally incapable of carrying one.
pub fn render_scan_summary(summary: &ScanSummary, detail: Detail, tz: &TimeZone) -> String {
    if summary.entries.is_empty() {
        return "No media found; nothing to import.\n".to_string();
    }

    let mut out = String::new();
    let mut totals = VerdictTotals::default();
    let mut quarantine_entries: Vec<&ScanEntry> = Vec::new();

    for entry in &summary.entries {
        totals.record_counts(&entry.verdict, entry.file_count, entry.total_size);

        if matches!(entry.verdict, Verdict::Quarantine) {
            quarantine_entries.push(entry);
            if detail != Detail::Verbose {
                continue;
            }
        }

        if detail != Detail::Summary {
            render_scan_entry(&mut out, entry, detail, tz);
        }
    }

    if detail == Detail::Normal && !quarantine_entries.is_empty() {
        render_scan_quarantine_rollup(&mut out, &quarantine_entries);
    }

    let _ = writeln!(out, "Summary: {}", totals.render());

    out
}

/// Renders one scan entry: `[VERDICT] name`, then either the
/// unrecognized-files listing or (for `Keep`/`Quarantine`) the
/// recorded time, file count, and total size — no path, ever
/// (design D1). `Ignore`'s reason clause is the only fixed-string
/// exception, matching `render_plan_entry`.
fn render_scan_entry(out: &mut String, entry: &ScanEntry, detail: Detail, tz: &TimeZone) {
    let label = match &entry.verdict {
        Verdict::Keep => "KEEP",
        Verdict::Quarantine => "QUARANTINE",
        Verdict::Ignore(_) => "IGNORE",
    };

    let _ = write!(out, "[{label}] {}", entry.name);
    if let Verdict::Ignore(reason) = &entry.verdict {
        let _ = write!(out, " — {reason}");
    }

    let is_unrecognized =
        matches!(&entry.verdict, Verdict::Ignore(reason) if reason == UNRECOGNIZED_REASON);
    if is_unrecognized {
        render_scan_unrecognized_files(out, &entry.files, detail);
    } else if !matches!(entry.verdict, Verdict::Ignore(_)) {
        let short_time = format_short_ts(entry.recorded_at, tz);
        let _ = write!(
            out,
            "  {short_time}  {}, {}",
            plural(entry.file_count, "file"),
            format_size(entry.total_size)
        );
    }
    let _ = writeln!(out);

    if detail == Detail::Verbose && !is_unrecognized {
        let _ = writeln!(out, "  recorded at: {}", format_ts(entry.recorded_at, tz));
    }
}

/// One default-mode line standing in for every `Quarantine` scan entry
/// (design D1, mirrors `render_quarantine_rollup`): count and aggregate
/// size only — `scan` never resolves a quarantine path to show, so
/// there is no path-or-disabled-note branch here.
fn render_scan_quarantine_rollup(out: &mut String, entries: &[&ScanEntry]) {
    let count = entries.len();
    let bytes: u64 = entries.iter().map(|e| e.total_size).sum();
    let _ = writeln!(
        out,
        "Quarantine: {}, {}  (-v to list)",
        plural(count, "group"),
        format_size(bytes)
    );
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

/// Per-outcome running totals over an `ExecuteReport` (design D7, task
/// 7.1) — the single source of truth both `render_results` and
/// `results_to_json` tally from, so the human summary line and the
/// JSON summary can never disagree on counts. Sidecar outcomes are
/// deliberately excluded: they're never a source file, so they don't
/// contribute to file-outcome counts (mirrors the pre-existing
/// `results_to_json` behavior).
#[derive(Default)]
struct ResultsTally {
    transferred: usize,
    reflinked: usize,
    skipped_identical: usize,
    skipped_quick_match: usize,
    suffixed: usize,
    skipped_quarantine_disabled: usize,
    failed: usize,
    deleted_groups: usize,
}

impl ResultsTally {
    fn from_report(report: &ExecuteReport) -> Self {
        let mut tally = Self::default();
        for group in &report.groups {
            if group.deleted_from_source {
                tally.deleted_groups += 1;
            }
            for file in &group.files {
                match &file.outcome {
                    TransferOutcome::Transferred => tally.transferred += 1,
                    TransferOutcome::Reflinked => tally.reflinked += 1,
                    TransferOutcome::SkippedIdentical => tally.skipped_identical += 1,
                    TransferOutcome::SkippedQuickMatch => tally.skipped_quick_match += 1,
                    TransferOutcome::Suffixed(_) => tally.suffixed += 1,
                    TransferOutcome::SkippedQuarantineDisabled => {
                        tally.skipped_quarantine_disabled += 1
                    }
                    TransferOutcome::Failed(_) => tally.failed += 1,
                }
            }
        }
        tally
    }

    /// The always-present closing line (spec: "Human-readable
    /// execution report is summarized by default"). Quick-matched
    /// skips are counted distinctly from already-imported skips, since
    /// only the latter were content-verified; likewise reflinked files
    /// are counted distinctly from stream-copied transfers, since a
    /// clone is a near-instant kernel operation rather than a full read
    /// and write of the file's bytes (spec: "Reflinked files are
    /// counted distinctly"). The deleted-groups clause only appears
    /// when deletion was actually requested this run — showing "0
    /// deleted from source" on a run that never asked for deletion
    /// would read as a failure that never happened.
    fn summary_line(&self, delete_source_in_effect: bool, detail: Detail) -> String {
        let mut line = format!(
            "Summary: {} transferred, {} reflinked, {} skipped (already imported), {} quick-matched, {} FAILED",
            self.transferred,
            self.reflinked,
            self.skipped_identical,
            self.skipped_quick_match,
            self.failed
        );
        if delete_source_in_effect {
            let _ = write!(
                line,
                ", {} deleted from source",
                plural(self.deleted_groups, "group")
            );
        }
        // Only Detail::Summary folds these two per-file outcomes into
        // the summary line (design Decision 3) — default/verbose output
        // lists them individually instead, via render_group_notable, so
        // adding these clauses there would double-report the same files.
        if detail == Detail::Summary {
            if self.suffixed > 0 {
                let _ = write!(line, ", {} renamed (collision)", self.suffixed);
            }
            if self.skipped_quarantine_disabled > 0 {
                let _ = write!(
                    line,
                    ", {} left on source (quarantine copying disabled)",
                    self.skipped_quarantine_disabled
                );
            }
        }
        line
    }
}

/// Renders the outcome of executing a plan (design D7). Default output
/// shows only notable per-file outcomes (failed, suffixed, left on
/// source because quarantine copying is disabled) and — for any group
/// left undeleted while deletion was in effect — a line naming it with
/// the reason; routine outcomes (transferred, skipped-identical,
/// quick-matched) are counted but not listed. `-v` lists every file,
/// grouped per media group with the group's destination as a header. A
/// summary line always closes the output.
pub fn render_results(report: &ExecuteReport, detail: Detail) -> String {
    let tally = ResultsTally::from_report(report);
    let mut out = String::new();

    // A group-level "not deleted" line is only informative when
    // deletion ran per-group (no single global reason already explains
    // every group uniformly) — when the whole run declined or skipped
    // deletion up front, `deletion_skipped_reason` says so once, and
    // repeating that per group would be noise.
    let name_undeleted_groups = report.delete_source && report.deletion_skipped_reason.is_none();

    for group in &report.groups {
        if detail == Detail::Verbose {
            render_group_verbose(&mut out, group);
        } else {
            render_group_notable(&mut out, group, name_undeleted_groups, detail);
        }
    }

    if let Some(reason) = &report.deletion_skipped_reason {
        let _ = writeln!(out, "{reason}");
    }

    let _ = writeln!(out, "{}", tally.summary_line(report.delete_source, detail));
    out
}

fn file_outcome_line(file: &FileResult, include_name: bool) -> String {
    let name = if include_name {
        file.src.display().to_string()
    } else {
        file_display_name(&file.src)
    };
    match &file.outcome {
        TransferOutcome::Transferred => format!("transferred: {name}"),
        TransferOutcome::Reflinked => format!("reflinked (instant): {name}"),
        TransferOutcome::SkippedIdentical => format!("skipped (already imported): {name}"),
        TransferOutcome::SkippedQuickMatch => {
            format!("skipped (quick-matched, not verified): {name}")
        }
        TransferOutcome::Suffixed(dest) => format!(
            "stored as {} (destination name collision): {name}",
            dest.display()
        ),
        TransferOutcome::SkippedQuarantineDisabled => {
            format!("left on source (quarantine copy disabled): {name}")
        }
        TransferOutcome::Failed(message) => format!("FAILED: {name} ({message})"),
    }
}

/// Default-mode rendering for one group: only the outcomes worth
/// surfacing without `-v` (design D7) — routine per-file lines
/// (`Transferred`, `SkippedIdentical`, `SkippedQuickMatch`) are
/// omitted, counted only in the summary.
fn render_group_notable(
    out: &mut String,
    group: &GroupResult,
    name_undeleted_groups: bool,
    detail: Detail,
) {
    for file in &group.files {
        let notable = match file.outcome {
            TransferOutcome::Failed(_) => true,
            TransferOutcome::Suffixed(_) | TransferOutcome::SkippedQuarantineDisabled => {
                detail != Detail::Summary
            }
            _ => false,
        };
        if notable {
            let _ = writeln!(out, "{}", file_outcome_line(file, true));
        }
    }
    if let Some(TransferOutcome::Failed(message)) = &group.sidecar_outcome {
        let _ = writeln!(out, "SIDECAR FAILED: {} ({message})", group.group_name);
    }
    if name_undeleted_groups
        && !group.deleted_from_source
        && let Some(reason) = undeleted_reason(group)
    {
        let _ = writeln!(
            out,
            "{}: not deleted from source ({reason})",
            group.group_name
        );
    }
}

/// `-v` rendering for one group: a header naming its destination, then
/// every file's outcome indented beneath it (design D7) — correlating
/// with the plan output the same run's scan/dry-run would have shown.
fn render_group_verbose(out: &mut String, group: &GroupResult) {
    let _ = write!(out, "{}", group.group_name);
    if let Some(dest) = &group.destination {
        let _ = write!(out, " -> {}", dest.display());
    }
    let _ = writeln!(out);

    for file in &group.files {
        let _ = writeln!(out, "  {}", file_outcome_line(file, false));
    }
    match &group.sidecar_outcome {
        Some(TransferOutcome::Transferred) => {
            let _ = writeln!(out, "  sidecar written");
        }
        Some(TransferOutcome::Failed(message)) => {
            let _ = writeln!(out, "  SIDECAR FAILED: {message}");
        }
        _ => {}
    }
    if group.deleted_from_source {
        let _ = writeln!(out, "  deleted from source");
    }
}

/// Explains, in one short clause, why a group wasn't cleaned off the
/// source despite deletion being in effect (design D7, spec: "states
/// why its group was not deleted from the source") — derived from the
/// same eligibility rules `execute_inner` applies (`content_verified`,
/// `sidecar_ok`), so it can never claim a reason execution didn't
/// actually enforce.
fn undeleted_reason(group: &GroupResult) -> Option<String> {
    // An empty `files` list means this group was never a deletion
    // candidate to begin with (an `Ignore` verdict never touches the
    // filesystem) — nothing surprising to explain, so stay silent
    // rather than flagging every ignored group whenever deletion is
    // in effect elsewhere in the same run.
    if group.files.is_empty() {
        return None;
    }
    if let Some(file) = group
        .files
        .iter()
        .find(|f| matches!(f.outcome, TransferOutcome::Failed(_)))
    {
        return Some(format!(
            "{} failed to transfer",
            file_display_name(&file.src)
        ));
    }
    if group
        .files
        .iter()
        .any(|f| matches!(f.outcome, TransferOutcome::SkippedQuickMatch))
    {
        return Some("quick-matched files were not content-verified".to_string());
    }
    if group
        .files
        .iter()
        .any(|f| matches!(f.outcome, TransferOutcome::SkippedQuarantineDisabled))
    {
        return Some("quarantine copying is disabled".to_string());
    }
    if matches!(group.sidecar_outcome, Some(TransferOutcome::Failed(_))) {
        return Some("its sidecar failed to write".to_string());
    }
    None
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
    /// Every file in the group, uncapped (design D6/task 6.6) — unlike
    /// the human renderer, JSON never truncates an unrecognized-files
    /// listing or omits quarantined groups' contents.
    pub files: Vec<String>,
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

            let files = action
                .group
                .files
                .iter()
                .map(|f| f.path.display().to_string())
                .collect();

            PlanActionJson {
                group: action.group.name.clone(),
                verdict: verdict.to_string(),
                reason,
                path: path.map(|p| p.display().to_string()),
                quarantine_copy_disabled,
                recorded_at: format_ts(action.group.timestamp, tz),
                markers,
                files,
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
pub struct ScanEntryJson {
    pub group: String,
    pub verdict: String,
    pub reason: String,
    pub file_count: usize,
    pub total_size_bytes: u64,
    pub recorded_at: String,
    /// Every file in the group, uncapped, naming the same field the
    /// human render truncates for the unrecognized-files group (spec:
    /// "scan's inventory entries SHALL do the same for their file
    /// listings"). No `path` field anywhere — `scan` never resolves one
    /// (design D1).
    pub files: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ScanSummaryJson {
    pub entries: Vec<ScanEntryJson>,
    pub summary: PlanSummaryJson,
}

/// Builds the JSON view of a `ScanSummary` (design D1, D4). Distinct
/// from `PlanJson`: no entry carries a `path` field, since `scan` never
/// resolves a destination or quarantine path. Reuses `PlanSummaryJson`
/// for the closing tally — the shape (kept/quarantined/ignored/total)
/// is identical, so a separate type would only duplicate it.
pub fn scan_summary_to_json(summary: &ScanSummary, tz: &TimeZone) -> ScanSummaryJson {
    let mut kept = 0usize;
    let mut quarantined = 0usize;
    let mut ignored = 0usize;

    let entries = summary
        .entries
        .iter()
        .map(|entry| {
            match &entry.verdict {
                Verdict::Keep => kept += 1,
                Verdict::Quarantine => quarantined += 1,
                Verdict::Ignore(_) => ignored += 1,
            }
            let (verdict, reason) = match &entry.verdict {
                Verdict::Keep => ("keep", "matches profile criteria".to_string()),
                Verdict::Quarantine => {
                    ("quarantine", "does not match profile criteria".to_string())
                }
                Verdict::Ignore(reason) => ("ignore", reason.clone()),
            };
            ScanEntryJson {
                group: entry.name.clone(),
                verdict: verdict.to_string(),
                reason,
                file_count: entry.file_count,
                total_size_bytes: entry.total_size,
                recorded_at: format_ts(entry.recorded_at, tz),
                files: entry.files.clone(),
            }
        })
        .collect::<Vec<_>>();

    let total = summary.entries.len();
    ScanSummaryJson {
        entries,
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
    /// Counted distinctly from `transferred` (spec: "Reflinked files
    /// are counted distinctly") — a clone shares the source's extents
    /// rather than streaming and re-hashing its bytes.
    pub reflinked: usize,
    /// Additive (task 7.1): previously folded into an unlabeled
    /// per-file `outcome` string only; broken out here so the human
    /// summary's "skipped (already imported)" / "quick-matched" counts
    /// can be verified equal to the JSON report for the same run
    /// (spec: "The summary counts SHALL equal those in the JSON
    /// report").
    pub skipped_identical: usize,
    pub skipped_quick_match: usize,
    pub suffixed: usize,
    pub skipped_quarantine_disabled: usize,
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
        TransferOutcome::Reflinked => ("reflinked".to_string(), None, None),
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

/// Builds the JSON view of an `ExecuteReport` (design D4). The summary
/// is tallied from the same `ResultsTally` the human renderer uses
/// (task 7.1), so the two can never disagree on counts.
pub fn results_to_json(report: &ExecuteReport) -> ResultsJson {
    let tally = ResultsTally::from_report(report);

    let groups = report
        .groups
        .iter()
        .map(|group| {
            let files = group
                .files
                .iter()
                .map(|f| {
                    let (outcome, dest, error) = outcome_json(&f.outcome);
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
            transferred: tally.transferred,
            reflinked: tally.reflinked,
            skipped_identical: tally.skipped_identical,
            skipped_quick_match: tally.skipped_quick_match,
            suffixed: tally.suffixed,
            skipped_quarantine_disabled: tally.skipped_quarantine_disabled,
            failed: tally.failed,
            deleted_groups: tally.deleted_groups,
        },
    }
}

// --- multi-drive JSON (multi-drive-import design D4, task 4.1-4.3) ---
//
// Explicit sourcing keeps printing `ScanSummaryJson`/`PlanJson`/
// `ResultsJson` directly at the top level, byte-for-byte as before this
// capability (task 2.5, 4.4) — these wrapper types are only ever used
// for `source: auto`, where more than zero drives can exist in the same
// invocation. `error`/`summary`/`plan`/`results` are omitted from the
// serialized JSON entirely (not merely `null`) when absent, via
// `skip_serializing_if`, matching the spec's "an error-status drive's
// entry has no summary/plan/results key".

#[derive(Debug, Serialize)]
pub struct ScanDriveJson {
    pub name: String,
    pub path: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<ScanSummaryJson>,
}

#[derive(Debug, Serialize)]
pub struct MultiScanJson {
    pub drives: Vec<ScanDriveJson>,
}

/// Builds one drive's JSON entry for `scan --json` against a
/// `source: auto` profile (spec: "Multi-drive JSON output enumerates
/// every drive"). `result` is the `Result<ScanDriveOutcome>` `scan_drives`
/// recorded for this drive — `Err` becomes `status: "error"` with the
/// error's `Display` text and no `summary` key.
pub fn scan_drive_json(
    name: &str,
    path: &Path,
    result: &crate::error::Result<crate::ScanDriveOutcome>,
    tz: &TimeZone,
) -> ScanDriveJson {
    let (status, error, summary) = match result {
        Ok(crate::ScanDriveOutcome::Empty) => ("empty".to_string(), None, None),
        Ok(crate::ScanDriveOutcome::Found(summary)) => (
            "completed".to_string(),
            None,
            Some(scan_summary_to_json(summary, tz)),
        ),
        Err(e) => ("error".to_string(), Some(e.to_string()), None),
    };
    ScanDriveJson {
        name: name.to_string(),
        path: path.display().to_string(),
        status,
        error,
        summary,
    }
}

#[derive(Debug, Serialize)]
pub struct ImportDriveJson {
    pub name: String,
    pub path: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<PlanJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub results: Option<ResultsJson>,
}

#[derive(Debug, Serialize)]
pub struct MultiImportJson {
    pub drives: Vec<ImportDriveJson>,
    pub any_failed: bool,
}

/// Builds one drive's JSON entry for `import --json` (`--dry-run` or a
/// real run) against a `source: auto` profile. `plan`/`results` mirror
/// whichever payload a single-drive JSON response would carry for that
/// mode — never both for the same drive.
pub fn import_drive_json(
    name: &str,
    path: &Path,
    result: &crate::error::Result<crate::ImportDriveOutcome>,
    tz: &TimeZone,
) -> ImportDriveJson {
    let (status, error, plan, results) = match result {
        Ok(crate::ImportDriveOutcome::Empty) => ("empty".to_string(), None, None, None),
        Ok(crate::ImportDriveOutcome::Planned(plan)) => (
            "completed".to_string(),
            None,
            Some(plan_to_json(plan, tz)),
            None,
        ),
        Ok(crate::ImportDriveOutcome::Executed { report, any_failed }) => (
            if *any_failed {
                "completed_with_failures".to_string()
            } else {
                "completed".to_string()
            },
            None,
            None,
            Some(results_to_json(report)),
        ),
        Err(e) => ("error".to_string(), Some(e.to_string()), None, None),
    };
    ImportDriveJson {
        name: name.to_string(),
        path: path.display().to_string(),
        status,
        error,
        plan,
        results,
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
pub fn render_cleanup_plan(plan: &crate::cleanup::CleanupPlan, detail: Detail) -> String {
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
        if detail != Detail::Summary {
            let _ = writeln!(
                out,
                "[{label}] {} — {} old, {}",
                entry.name,
                format_age_days(entry.age_seconds),
                format_size(entry.size)
            );
        }
    }

    let _ = writeln!(
        out,
        "Summary: {purge_count} to purge ({}), {keep_count} kept ({})",
        format_size(purge_size),
        format_size(keep_size)
    );
    out
}

/// Renders the outcome of executing a cleanup plan. Default mode lists
/// every deleted entry individually with no closing tally (an existing
/// asymmetry with `render_results` left out of scope — see design's Open
/// Questions). `Detail::Summary` replaces the routine `deleted: <path>`
/// lines with a closing tally (count and total size); `FAILED to
/// delete` lines stay individually listed in every `Detail` value, since
/// they're the actionable exception this flag is not meant to hide.
pub fn render_cleanup_report(report: &crate::cleanup::CleanupReport, detail: Detail) -> String {
    let mut out = String::new();
    let (mut deleted_count, mut deleted_size) = (0usize, 0u64);

    for result in &report.results {
        match &result.error {
            None => {
                deleted_count += 1;
                deleted_size += result.size;
                if detail != Detail::Summary {
                    let _ = writeln!(out, "deleted: {}", result.path.display());
                }
            }
            Some(message) => {
                let _ = writeln!(out, "FAILED to delete {}: {message}", result.path.display());
            }
        }
    }

    if detail == Detail::Summary && deleted_count > 0 {
        let noun = if deleted_count == 1 {
            "entry"
        } else {
            "entries"
        };
        let _ = writeln!(
            out,
            "Summary: {deleted_count} {noun} deleted ({})",
            format_size(deleted_size)
        );
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

        let out = render_plan(&plan, Detail::Normal, &jiff::tz::TimeZone::UTC);

        assert!(out.contains("[KEEP] kept"));
        assert!(!out.contains("QUARANTINE"));
        assert!(!out.contains("marker at"));
        assert!(!out.contains("recorded at:"));
        assert!(
            out.contains("Quarantine: 1 group, 0 B -> /quarantine  (-v to list)"),
            "quarantine collapses to one rollup line naming the shared root: {out}"
        );
        assert!(out.contains(
            "Summary: 1 kept (0 files, 0 B), 1 quarantined (0 files, 0 B), 0 ignored (0 files, 0 B) (2 total)"
        ));
    }

    #[test]
    fn non_verbose_quarantine_rollup_shows_disabled_note_without_a_path() {
        let plan = ImportPlan {
            actions: vec![PlannedAction {
                group: group("unmarked", vec![]),
                verdict: Verdict::Quarantine,
                destination: None,
                quarantine_path: None, // copy_quarantine: false
            }],
        };

        let out = render_plan(&plan, Detail::Normal, &jiff::tz::TimeZone::UTC);

        assert!(
            !out.contains("[QUARANTINE]"),
            "individual entry suppressed by default"
        );
        assert!(out.contains("Quarantine: 1 group, 0 B (quarantine copy disabled)  (-v to list)"));
    }

    #[test]
    fn verbose_shows_quarantine_and_marker_details() {
        let markers = vec![Marker {
            timestamp: ts(1_000),
            label: None,
        }];
        let plan = plan_with_one_keep_one_quarantine(markers);

        let out = render_plan(&plan, Detail::Verbose, &jiff::tz::TimeZone::UTC);

        assert!(out.contains("[KEEP] kept"));
        assert!(out.contains("[QUARANTINE] unmarked"));
        assert!(out.contains("marker at 1970-01-01T00:16:40+00:00"));
        assert!(
            out.contains("recorded at: 1970-01-01T00:00:00+00:00"),
            "verbose mode should show the group's (GPS-corrected, when available) recorded time"
        );
        assert!(
            !out.contains("Quarantine:"),
            "verbose mode lists quarantine entries individually, no rollup line"
        );
        assert!(out.contains(
            "Summary: 1 kept (0 files, 0 B), 1 quarantined (0 files, 0 B), 0 ignored (0 files, 0 B) (2 total)"
        ));
    }

    #[test]
    fn recorded_at_has_no_source_annotation_without_a_sidecar() {
        let plan = plan_with_one_keep_one_quarantine(vec![]);
        let out = render_plan(&plan, Detail::Verbose, &jiff::tz::TimeZone::UTC);
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

        let out = render_plan(&plan, Detail::Verbose, &jiff::tz::TimeZone::UTC);

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

        let out_verbose = render_plan(&plan, Detail::Verbose, &jiff::tz::TimeZone::UTC);
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
        assert!(out_verbose.contains(
            "Summary: 0 kept (0 files, 0 B), 1 quarantined (0 files, 0 B), 0 ignored (0 files, 0 B) (1 total)"
        ));
    }

    // --- results renderer (design D7, task 7.6) ---

    fn file_result(src: &str, outcome: TransferOutcome) -> FileResult {
        FileResult {
            src: PathBuf::from(src),
            outcome,
        }
    }

    fn group_result(
        name: &str,
        files: Vec<FileResult>,
        deleted_from_source: bool,
        destination: Option<&str>,
    ) -> GroupResult {
        GroupResult {
            group_name: name.to_string(),
            verdict: Verdict::Keep,
            files,
            sidecar_outcome: None,
            deleted_from_source,
            destination: destination.map(PathBuf::from),
        }
    }

    #[test]
    fn results_render_left_on_source_outcome() {
        // Task 4.3: SkippedQuarantineDisabled renders a clear message.
        let report = ExecuteReport {
            groups: vec![group_result(
                "unmarked",
                vec![file_result(
                    "/card/clip.mp4",
                    TransferOutcome::SkippedQuarantineDisabled,
                )],
                false,
                None,
            )],
            deletion_skipped_reason: None,
            delete_source: false,
        };

        let out = render_results(&report, Detail::Normal);
        assert!(out.contains("left on source (quarantine copy disabled): /card/clip.mp4"));
    }

    #[test]
    fn clean_run_renders_only_the_summary_line() {
        let report = ExecuteReport {
            groups: vec![
                group_result(
                    "a",
                    vec![file_result("/card/a.mp4", TransferOutcome::Transferred)],
                    true,
                    Some("/dest/a"),
                ),
                group_result(
                    "b",
                    vec![file_result(
                        "/card/b.mp4",
                        TransferOutcome::SkippedIdentical,
                    )],
                    true,
                    Some("/dest/b"),
                ),
            ],
            deletion_skipped_reason: None,
            delete_source: true,
        };

        let out = render_results(&report, Detail::Normal);
        assert_eq!(
            out.trim_end(),
            "Summary: 1 transferred, 0 reflinked, 1 skipped (already imported), 0 quick-matched, 0 FAILED, 2 groups deleted from source"
        );
    }

    #[test]
    fn failure_is_visible_without_verbosity_and_names_why_its_group_stayed() {
        let report = ExecuteReport {
            groups: vec![
                group_result(
                    "ok",
                    vec![file_result("/card/ok.mp4", TransferOutcome::Transferred)],
                    true,
                    Some("/dest/ok"),
                ),
                group_result(
                    "broken",
                    vec![file_result(
                        "/card/broken.mp4",
                        TransferOutcome::Failed("hash mismatch".to_string()),
                    )],
                    false,
                    Some("/dest/broken"),
                ),
            ],
            deletion_skipped_reason: None,
            delete_source: true,
        };

        let out = render_results(&report, Detail::Normal);
        assert!(out.contains("FAILED: /card/broken.mp4 (hash mismatch)"));
        assert!(
            out.contains("broken: not deleted from source (broken.mp4 failed to transfer)"),
            "got: {out}"
        );
        assert!(
            !out.contains("ok: not deleted"),
            "a successfully deleted group must not get a not-deleted line"
        );
        assert!(out.contains("1 transferred"));
        assert!(out.contains("1 FAILED"));
        assert!(out.contains("1 group deleted from source"));
    }

    #[test]
    fn no_undeleted_line_when_deletion_was_never_requested() {
        let report = ExecuteReport {
            groups: vec![group_result(
                "a",
                vec![file_result(
                    "/card/a.mp4",
                    TransferOutcome::Failed("disk full".to_string()),
                )],
                false,
                Some("/dest/a"),
            )],
            deletion_skipped_reason: None,
            delete_source: false,
        };

        let out = render_results(&report, Detail::Normal);
        assert!(out.contains("FAILED: /card/a.mp4"));
        assert!(
            !out.contains("not deleted from source"),
            "delete_source was never requested, so no group should claim it wasn't deleted"
        );
        assert!(!out.contains("deleted from source"));
    }

    #[test]
    fn verbose_groups_files_under_a_destination_header() {
        let report = ExecuteReport {
            groups: vec![group_result(
                "session-a",
                vec![
                    file_result("/card/clip1.mp4", TransferOutcome::Transferred),
                    file_result("/card/clip2.mp4", TransferOutcome::SkippedIdentical),
                ],
                true,
                Some("/dest/2026/2026-07-09"),
            )],
            deletion_skipped_reason: None,
            delete_source: true,
        };

        let out = render_results(&report, Detail::Verbose);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "session-a -> /dest/2026/2026-07-09");
        assert_eq!(lines[1], "  transferred: clip1.mp4");
        assert_eq!(lines[2], "  skipped (already imported): clip2.mp4");
        assert_eq!(lines[3], "  deleted from source");
        assert!(out.contains("Summary: 1 transferred, 0 reflinked, 1 skipped"));
    }

    // --- reflinked outcome rendering (add-reflink-transfer, task 6.7) ---

    #[test]
    fn reflinked_files_counted_distinctly_and_not_listed_by_default() {
        // Spec scenario: "Reflinked files are counted distinctly in the
        // summary" — a run mixing reflinked and stream-copied files
        // shows the reflinked count separately from transferred, with
        // neither listed per file by default.
        let report = ExecuteReport {
            groups: vec![group_result(
                "a",
                vec![
                    file_result("/card/a1.mp4", TransferOutcome::Transferred),
                    file_result("/card/a2.mp4", TransferOutcome::Reflinked),
                ],
                false,
                Some("/dest/a"),
            )],
            deletion_skipped_reason: None,
            delete_source: false,
        };

        let out = render_results(&report, Detail::Normal);
        assert_eq!(
            out.trim_end(),
            "Summary: 1 transferred, 1 reflinked, 0 skipped (already imported), 0 quick-matched, 0 FAILED"
        );

        let json = results_to_json(&report);
        assert_eq!(json.summary.transferred, 1);
        assert_eq!(json.summary.reflinked, 1);
    }

    #[test]
    fn reflinked_file_renders_instant_line_with_verbose() {
        let report = ExecuteReport {
            groups: vec![group_result(
                "a",
                vec![file_result("/card/a1.mp4", TransferOutcome::Reflinked)],
                true,
                Some("/dest/a"),
            )],
            deletion_skipped_reason: None,
            delete_source: true,
        };

        let out = render_results(&report, Detail::Verbose);
        assert!(out.contains("reflinked (instant): a1.mp4"));
    }

    #[test]
    fn reflinked_outcome_json_status_is_reflinked() {
        let report = ExecuteReport {
            groups: vec![group_result(
                "a",
                vec![file_result("/card/a1.mp4", TransferOutcome::Reflinked)],
                true,
                Some("/dest/a"),
            )],
            deletion_skipped_reason: None,
            delete_source: true,
        };

        let json = results_to_json(&report);
        assert_eq!(json.groups[0].files[0].outcome, "reflinked");
    }

    #[test]
    fn human_summary_counts_equal_json_summary_counts() {
        let report = ExecuteReport {
            groups: vec![
                group_result(
                    "a",
                    vec![
                        file_result("/card/a1.mp4", TransferOutcome::Transferred),
                        file_result("/card/a2.mp4", TransferOutcome::SkippedIdentical),
                    ],
                    true,
                    Some("/dest/a"),
                ),
                group_result(
                    "b",
                    vec![
                        file_result("/card/b1.mp4", TransferOutcome::SkippedQuickMatch),
                        file_result(
                            "/card/b2.mp4",
                            TransferOutcome::Suffixed(PathBuf::from("/dest/b/b2-1.mp4")),
                        ),
                        file_result("/card/b3.mp4", TransferOutcome::Failed("nope".to_string())),
                    ],
                    false,
                    Some("/dest/b"),
                ),
            ],
            deletion_skipped_reason: None,
            delete_source: true,
        };

        let human = render_results(&report, Detail::Normal);
        let json = results_to_json(&report);

        assert!(human.contains(&format!("{} transferred", json.summary.transferred)));
        assert!(human.contains(&format!(
            "{} skipped (already imported)",
            json.summary.skipped_identical
        )));
        assert!(human.contains(&format!(
            "{} quick-matched",
            json.summary.skipped_quick_match
        )));
        assert!(human.contains(&format!("{} FAILED", json.summary.failed)));
        assert!(human.contains(&format!(
            "{} deleted from source",
            plural(json.summary.deleted_groups, "group")
        )));
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

    // --- entry format: time and size instead of boilerplate (design D5, task 6.1/6.7) ---

    fn media_file(path: &str, size: u64) -> MediaFile {
        MediaFile {
            path: PathBuf::from(path),
            size,
            recorded_at: None,
        }
    }

    #[test]
    fn keep_entry_shows_short_time_file_count_and_size_no_boilerplate_reason() {
        let mut kept = group("kept", vec![]);
        kept.timestamp = "2026-07-09T07:41:03Z".parse().unwrap();
        kept.files = vec![
            media_file("/card/a.mp4", 1024),
            media_file("/card/b.mp4", 1024),
        ];
        let plan = ImportPlan {
            actions: vec![PlannedAction {
                group: kept,
                verdict: Verdict::Keep,
                destination: Some(PathBuf::from("/dest/kept")),
                quarantine_path: None,
            }],
        };

        let out = render_plan(&plan, Detail::Normal, &jiff::tz::TimeZone::UTC);

        assert!(
            out.contains("[KEEP] kept  2026-07-09 07:41  2 files, 2.0 KiB -> /dest/kept"),
            "got: {out}"
        );
        assert!(
            !out.contains("matches profile criteria"),
            "fixed boilerplate reason text must be gone"
        );
    }

    // --- unrecognized files: capped by default, uncapped with -v (design D6, task 6.3/6.7) ---

    fn unrecognized_plan(n: usize) -> ImportPlan {
        let files: Vec<MediaFile> = (0..n)
            .map(|i| media_file(&format!("/card/stray{i:02}.dat"), 10))
            .collect();
        let group = MediaGroup {
            name: "unrecognized".to_string(),
            files,
            timestamp: ts(0),
            markers: vec![],
            geo: None,
            context: HashMap::new(),
            sidecar: None,
        };
        ImportPlan {
            actions: vec![PlannedAction {
                group,
                verdict: Verdict::Ignore("unrecognized file(s)".to_string()),
                destination: None,
                quarantine_path: None,
            }],
        }
    }

    #[test]
    fn unrecognized_files_capped_at_five_by_default() {
        let plan = unrecognized_plan(8);
        let out = render_plan(&plan, Detail::Normal, &jiff::tz::TimeZone::UTC);

        assert!(out.contains("[IGNORE] unrecognized — unrecognized file(s) (8 files)"));
        for i in 0..5 {
            assert!(out.contains(&format!("stray{i:02}.dat")), "{out}");
        }
        for i in 5..8 {
            assert!(!out.contains(&format!("stray{i:02}.dat")), "{out}");
        }
        assert!(out.contains("… and 3 more (-v to list all)"));
    }

    #[test]
    fn unrecognized_files_all_listed_with_verbose() {
        let plan = unrecognized_plan(8);
        let out = render_plan(&plan, Detail::Verbose, &jiff::tz::TimeZone::UTC);

        for i in 0..8 {
            assert!(out.contains(&format!("stray{i:02}.dat")), "{out}");
        }
        assert!(!out.contains("more (-v to list all)"));
    }

    #[test]
    fn unrecognized_files_at_cap_render_identically_default_and_verbose() {
        let plan = unrecognized_plan(5);
        let default_out = render_plan(&plan, Detail::Normal, &jiff::tz::TimeZone::UTC);
        let verbose_out = render_plan(&plan, Detail::Verbose, &jiff::tz::TimeZone::UTC);

        assert!(!default_out.contains("more (-v to list all)"));
        for i in 0..5 {
            assert!(default_out.contains(&format!("stray{i:02}.dat")));
        }
        // The verbose output additionally carries the `recorded at:` line;
        // strip it before comparing the file listing itself.
        let verbose_files: String = verbose_out
            .lines()
            .filter(|l| !l.trim_start().starts_with("recorded at:"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(default_out.trim_end(), verbose_files.trim_end());
    }

    // --- plan JSON files array: uncapped even for a truncated human listing (task 6.6) ---

    #[test]
    fn plan_json_files_array_is_never_truncated() {
        let plan = unrecognized_plan(8);
        let json = plan_to_json(&plan, &jiff::tz::TimeZone::UTC);

        assert_eq!(json.actions.len(), 1);
        assert_eq!(
            json.actions[0].files.len(),
            8,
            "JSON must name every file even though the human render caps at 5"
        );
    }

    #[test]
    fn results_json_reports_outcomes_and_summary() {
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
                    destination: Some(PathBuf::from("/dest/kept")),
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
                    destination: Some(PathBuf::from("/dest/broken")),
                },
            ],
            deletion_skipped_reason: None,
            delete_source: true,
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
        let report = ExecuteReport {
            groups: vec![],
            deletion_skipped_reason: Some("declined".to_string()),
            delete_source: true,
        };
        let json = results_to_json(&report);
        let value = serde_json::to_value(&json).unwrap();
        assert_eq!(value["deletion_skipped_reason"], "declined");
        assert_eq!(value["summary"]["transferred"], 0);
    }

    // --- scan summary rendering and JSON (design D1, task 5.4) ---

    fn scan_entry(name: &str, verdict: Verdict, file_count: usize, total_size: u64) -> ScanEntry {
        ScanEntry {
            name: name.to_string(),
            verdict,
            file_count,
            total_size,
            recorded_at: "2026-07-09T07:41:03Z".parse().unwrap(),
            files: vec![],
        }
    }

    #[test]
    fn scan_summary_shows_time_file_count_and_size_no_destination_path() {
        let summary = ScanSummary {
            entries: vec![scan_entry("session-0123", Verdict::Keep, 2, 2048)],
        };

        let out = render_scan_summary(&summary, Detail::Normal, &jiff::tz::TimeZone::UTC);

        assert!(
            out.contains("[KEEP] session-0123  2026-07-09 07:41  2 files, 2.0 KiB"),
            "got: {out}"
        );
        assert!(
            !out.contains("->"),
            "scan must never show a destination path: {out}"
        );
    }

    #[test]
    fn scan_quarantine_rollup_shows_count_and_size_no_quarantine_path() {
        let summary = ScanSummary {
            entries: vec![
                scan_entry("session-a", Verdict::Quarantine, 1, 1024),
                scan_entry("session-b", Verdict::Quarantine, 1, 1024),
            ],
        };

        let out = render_scan_summary(&summary, Detail::Normal, &jiff::tz::TimeZone::UTC);

        assert!(
            !out.contains("[QUARANTINE]"),
            "individual entries stay collapsed by default: {out}"
        );
        assert!(
            out.contains("Quarantine: 2 groups, 2.0 KiB  (-v to list)"),
            "got: {out}"
        );
        assert!(
            !out.contains("->"),
            "scan must never show a quarantine path: {out}"
        );
    }

    #[test]
    fn scan_unrecognized_files_capped_at_five_by_default() {
        let files: Vec<String> = (0..8).map(|i| format!("/card/stray{i:02}.dat")).collect();
        let summary = ScanSummary {
            entries: vec![ScanEntry {
                name: "unrecognized".to_string(),
                verdict: Verdict::Ignore("unrecognized file(s)".to_string()),
                file_count: 8,
                total_size: 80,
                recorded_at: ts(0),
                files,
            }],
        };

        let default_out = render_scan_summary(&summary, Detail::Normal, &jiff::tz::TimeZone::UTC);
        assert!(default_out.contains("[IGNORE] unrecognized — unrecognized file(s) (8 files)"));
        for i in 0..5 {
            assert!(
                default_out.contains(&format!("stray{i:02}.dat")),
                "{default_out}"
            );
        }
        for i in 5..8 {
            assert!(
                !default_out.contains(&format!("stray{i:02}.dat")),
                "{default_out}"
            );
        }
        assert!(default_out.contains("… and 3 more (-v to list all)"));

        let verbose_out = render_scan_summary(&summary, Detail::Verbose, &jiff::tz::TimeZone::UTC);
        for i in 0..8 {
            assert!(
                verbose_out.contains(&format!("stray{i:02}.dat")),
                "{verbose_out}"
            );
        }
        assert!(!verbose_out.contains("more (-v to list all)"));
    }

    #[test]
    fn scan_summary_json_has_no_path_field_anywhere() {
        let summary = ScanSummary {
            entries: vec![
                scan_entry("session-a", Verdict::Keep, 1, 1024),
                scan_entry("session-b", Verdict::Quarantine, 1, 1024),
            ],
        };

        let json = scan_summary_to_json(&summary, &jiff::tz::TimeZone::UTC);
        let value = serde_json::to_value(&json).unwrap();
        let dumped = serde_json::to_string(&value).unwrap();
        assert!(
            !dumped.contains("\"path\""),
            "scan JSON must never carry a path field: {dumped}"
        );
        assert_eq!(json.summary.kept, 1);
        assert_eq!(json.summary.quarantined, 1);
        assert_eq!(json.summary.total, 2);
    }

    #[test]
    fn scan_summary_json_files_array_is_never_truncated() {
        let files: Vec<String> = (0..8).map(|i| format!("/card/stray{i:02}.dat")).collect();
        let summary = ScanSummary {
            entries: vec![ScanEntry {
                name: "unrecognized".to_string(),
                verdict: Verdict::Ignore("unrecognized file(s)".to_string()),
                file_count: 8,
                total_size: 80,
                recorded_at: ts(0),
                files,
            }],
        };

        let json = scan_summary_to_json(&summary, &jiff::tz::TimeZone::UTC);
        assert_eq!(
            json.entries[0].files.len(),
            8,
            "JSON must name every file even though the human render caps at 5"
        );
    }
}
