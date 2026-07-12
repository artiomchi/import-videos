//! Cleanup: purge a profile's quarantine directory (design D1). Mirrors
//! the plan/execute split from `plan.rs`/`transfer.rs`: a read-only
//! plan is built first, entries are marked for purge or retention, and
//! deletion happens only after the plan is reviewed and confirmed —
//! the ADR 0003 discipline applied to deletion instead of import.

use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use jiff::tz::TimeZone;
use jiff::{Span, Timestamp};

use crate::config::Profile;
use crate::error::{Error, Result};
use crate::transfer::{Confirmation, confirm};

/// One immediate child of the quarantine root: either a group directory
/// (one per quarantined session/event) or a stray loose file. Both are
/// treated uniformly — `age` and `size` come from the entry itself,
/// never from files nested inside it (design D2).
#[derive(Debug, Clone)]
pub struct CleanupEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    /// When this entry landed in quarantine — its own mtime, not the
    /// recording-stamped mtimes of any files inside it (design D2).
    pub landed_at: Timestamp,
    /// Whole seconds between `landed_at` and the plan's `now` — a
    /// display convenience so rendering doesn't need to redo the
    /// subtraction.
    pub age_seconds: i64,
    pub size: u64,
    /// Whether this entry will be deleted when the plan executes: every
    /// entry when `--older-than` is unset, otherwise only entries whose
    /// `landed_at` is older than the resolved cutoff.
    pub purge: bool,
}

#[derive(Debug, Clone, Default)]
pub struct CleanupPlan {
    pub quarantine_root: PathBuf,
    pub entries: Vec<CleanupEntry>,
}

impl CleanupPlan {
    pub fn purge_candidates(&self) -> impl Iterator<Item = &CleanupEntry> {
        self.entries.iter().filter(|e| e.purge)
    }

    pub fn kept(&self) -> impl Iterator<Item = &CleanupEntry> {
        self.entries.iter().filter(|e| !e.purge)
    }
}

#[derive(Debug, Clone)]
pub struct CleanupResult {
    pub name: String,
    pub path: PathBuf,
    pub deleted: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct CleanupReport {
    pub results: Vec<CleanupResult>,
    /// Set when execution didn't proceed because the confirmation
    /// prompt was declined (design D7) — distinct from the
    /// non-interactive-without-`--yes` case, which is a hard error
    /// (spec: "Non-interactive run without --yes").
    pub aborted_reason: Option<String>,
}

/// Resolves `profile`'s quarantine root and refuses to proceed if it
/// equals or contains the profile's destination — cleanup must never be
/// able to reach imported footage (spec: "Cleanup deletes only within
/// the quarantine directory").
pub fn resolve_and_check_quarantine_root(profile: &Profile) -> Result<PathBuf> {
    let root = profile.quarantine_root();
    if root == profile.destination || profile.destination.starts_with(&root) {
        return Err(Error::Config(format!(
            "quarantine directory '{}' equals or contains destination '{}'; refusing to run cleanup",
            root.display(),
            profile.destination.display()
        )));
    }
    Ok(root)
}

/// Builds a purge plan for `profile`'s quarantine directory (design
/// D1): the immediate children (group directories plus any stray loose
/// files), each with its own age and size, and a`purge` flag from the
/// `--older-than` filter. Read-only. An absent or empty quarantine
/// directory yields an empty plan (spec: "Empty quarantine").
pub fn build_plan(
    profile: &Profile,
    older_than: Option<Span>,
    tz: &TimeZone,
    now: Timestamp,
) -> Result<CleanupPlan> {
    let root = resolve_and_check_quarantine_root(profile)?;

    let cutoff = older_than
        .map(|span| {
            now.to_zoned(tz.clone())
                .checked_sub(span)
                .map(|z| z.timestamp())
                .map_err(|e| Error::Config(format!("--older-than: {e}")))
        })
        .transpose()?;

    let mut entries = Vec::new();
    let Ok(dir_entries) = fs::read_dir(&root) else {
        return Ok(CleanupPlan {
            quarantine_root: root,
            entries,
        });
    };

    for entry in dir_entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let mtime = metadata
            .modified()
            .map_err(|e| Error::io(&path, e))
            .and_then(|m| Timestamp::try_from(m).map_err(|e| Error::Config(e.to_string())))?;
        let is_dir = metadata.is_dir();
        let size = if is_dir {
            dir_size(&path)
        } else {
            metadata.len()
        };
        let purge = cutoff.is_none_or(|cutoff| mtime < cutoff);

        entries.push(CleanupEntry {
            name,
            path,
            is_dir,
            landed_at: mtime,
            age_seconds: now.as_second() - mtime.as_second(),
            size,
            purge,
        });
    }

    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(CleanupPlan {
        quarantine_root: root,
        entries,
    })
}

/// Recursively sums file sizes under `dir`; unreadable subtrees
/// contribute nothing rather than failing the whole plan (this is a
/// display figure, not a safety-relevant value).
fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| {
            let path = entry.path();
            match entry.metadata() {
                Ok(meta) if meta.is_dir() => dir_size(&path),
                Ok(meta) => meta.len(),
                Err(_) => 0,
            }
        })
        .sum()
}

/// Executes a cleanup plan: prompts for confirmation (unless `--yes`),
/// then deletes every `purge`-marked entry. Non-interactive stdin
/// without `--yes` is a hard error (spec: "Non-interactive run without
/// --yes" — unlike `import`, where deletion is incidental, cleanup's
/// entire purpose is deletion, so silently skipping it would be
/// surprising).
pub fn execute(plan: &CleanupPlan, assume_yes: bool) -> Result<CleanupReport> {
    execute_inner(plan, assume_yes, std::io::stdin().is_terminal())
}

fn execute_inner(
    plan: &CleanupPlan,
    assume_yes: bool,
    stdin_is_terminal: bool,
) -> Result<CleanupReport> {
    let candidates: Vec<&CleanupEntry> = plan.purge_candidates().collect();
    if candidates.is_empty() {
        return Ok(CleanupReport::default());
    }

    match confirm(
        "Delete the quarantine entries listed above? [y/N]",
        assume_yes,
        stdin_is_terminal,
    )? {
        Confirmation::Confirmed => {
            let results = candidates
                .into_iter()
                .map(|entry| {
                    let outcome = if entry.is_dir {
                        fs::remove_dir_all(&entry.path)
                    } else {
                        fs::remove_file(&entry.path)
                    };
                    CleanupResult {
                        name: entry.name.clone(),
                        path: entry.path.clone(),
                        deleted: outcome.is_ok(),
                        error: outcome.err().map(|e| e.to_string()),
                    }
                })
                .collect();
            Ok(CleanupReport {
                results,
                aborted_reason: None,
            })
        }
        Confirmation::DeclinedInteractive => Ok(CleanupReport {
            results: Vec::new(),
            aborted_reason: Some(
                "deletion declined; quarantine entries were not deleted".to_string(),
            ),
        }),
        Confirmation::SkippedNonInteractive => Err(Error::Config(
            "stdin is not a terminal; pass --yes to confirm non-interactively".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{LayoutTemplate, SourceKind, SourceLocation};
    use globset::GlobSetBuilder;
    use std::time::{Duration, SystemTime};

    fn profile(dest: PathBuf, quarantine: Option<PathBuf>) -> Profile {
        Profile {
            kind: SourceKind::Generic,
            source: SourceLocation::Auto,
            destination: dest,
            layout: LayoutTemplate::parse("{date:%Y}/{date:%Y-%m-%d}").unwrap(),
            ignore: GlobSetBuilder::new().build().unwrap(),
            quarantine,
            delete_source: false,
            copy_quarantine: true,
            reflink: true,
        }
    }

    fn set_mtime(path: &Path, age_days: u64) {
        let mtime = SystemTime::now() - Duration::from_secs(age_days * 86_400);
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
        // For directories, set_modified via File::options().write(true)
        // fails on some platforms; use filetime-free approach: reopen
        // as a directory handle is not portable via std, so tests that
        // need directory mtimes set it via utimensat-equivalent below.
    }

    fn set_dir_mtime(path: &Path, age_days: u64) {
        let mtime = SystemTime::now() - Duration::from_secs(age_days * 86_400);
        // std has no portable directory mtime setter; open the dir via
        // File::open (read-only) and use set_modified, which works for
        // directories on Unix (the only platform this project targets
        // per AGENTS.md / directories crate usage elsewhere).
        std::fs::File::open(path)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
    }

    #[test]
    fn empty_quarantine_yields_empty_plan() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("dest");
        let prof = profile(dest, None);
        let plan = build_plan(&prof, None, &TimeZone::UTC, Timestamp::now()).unwrap();
        assert!(plan.entries.is_empty());
    }

    #[test]
    fn absent_quarantine_yields_empty_plan() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("dest");
        std::fs::create_dir_all(&dest).unwrap();
        let prof = profile(dest, None);
        let plan = build_plan(&prof, None, &TimeZone::UTC, Timestamp::now()).unwrap();
        assert!(plan.entries.is_empty());
    }

    #[test]
    fn without_older_than_every_entry_is_a_candidate() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("dest");
        let quarantine = dest.join("_quarantine");
        std::fs::create_dir_all(quarantine.join("group-a")).unwrap();
        std::fs::write(quarantine.join("group-a/clip.mp4"), b"x").unwrap();

        let prof = profile(dest, None);
        let plan = build_plan(&prof, None, &TimeZone::UTC, Timestamp::now()).unwrap();
        assert_eq!(plan.entries.len(), 1);
        assert!(plan.entries[0].purge);
    }

    #[test]
    fn older_than_retains_young_dir_with_old_file_mtimes_purges_old_dir() {
        // design D2's core scenario: a group directory that landed in
        // quarantine 5 days ago must be kept even though the file
        // inside it carries a much older recording-stamped mtime (200
        // days) — age comes from the *directory's own* mtime, never
        // from the files inside it.
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("dest");
        let quarantine = dest.join("_quarantine");
        let young = quarantine.join("young-group");
        let old = quarantine.join("old-group");
        std::fs::create_dir_all(&young).unwrap();
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(young.join("clip.mp4"), b"x").unwrap();
        std::fs::write(old.join("clip.mp4"), b"x").unwrap();
        set_mtime(&young.join("clip.mp4"), 200);
        set_dir_mtime(&young, 5);
        set_dir_mtime(&old, 45);

        let prof = profile(dest, None);
        let older_than: Span = "30d".parse().unwrap();
        let plan = build_plan(&prof, Some(older_than), &TimeZone::UTC, Timestamp::now()).unwrap();

        let young_entry = plan
            .entries
            .iter()
            .find(|e| e.name == "young-group")
            .unwrap();
        let old_entry = plan.entries.iter().find(|e| e.name == "old-group").unwrap();
        assert!(
            !young_entry.purge,
            "5-day-old dir must be kept despite a 200-day-old file inside it"
        );
        assert!(old_entry.purge, "45-day-old entry must be purged");
    }

    #[test]
    fn stray_file_uses_its_own_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("dest");
        let quarantine = dest.join("_quarantine");
        std::fs::create_dir_all(&quarantine).unwrap();
        let stray = quarantine.join("stray.mp4");
        std::fs::write(&stray, b"x").unwrap();
        set_mtime(&stray, 45);

        let prof = profile(dest, None);
        let older_than: Span = "30d".parse().unwrap();
        let plan = build_plan(&prof, Some(older_than), &TimeZone::UTC, Timestamp::now()).unwrap();
        assert_eq!(plan.entries.len(), 1);
        assert!(plan.entries[0].purge);
        assert!(!plan.entries[0].is_dir);
    }

    #[test]
    fn invalid_span_is_rejected_by_the_parser() {
        // Task 3.3: parsing happens at the CLI boundary (lib.rs); this
        // test pins that jiff's Span parser rejects garbage, which is
        // what lib.rs relies on to produce the exit-2 usage error.
        assert!("banana".parse::<Span>().is_err());
    }

    #[test]
    fn quarantine_root_equal_to_destination_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("dest");
        std::fs::create_dir_all(&dest).unwrap();
        let prof = profile(dest.clone(), Some(dest));
        let err = build_plan(&prof, None, &TimeZone::UTC, Timestamp::now()).unwrap_err();
        assert!(matches!(err, Error::Config(_)));
    }

    #[test]
    fn quarantine_root_containing_destination_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("dest/nested");
        std::fs::create_dir_all(&dest).unwrap();
        let quarantine_root = dir.path().join("dest");
        let prof = profile(dest, Some(quarantine_root));
        let err = build_plan(&prof, None, &TimeZone::UTC, Timestamp::now()).unwrap_err();
        assert!(matches!(err, Error::Config(_)));
    }

    #[test]
    fn default_quarantine_under_destination_is_allowed() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("dest");
        std::fs::create_dir_all(&dest).unwrap();
        let prof = profile(dest, None);
        let plan = build_plan(&prof, None, &TimeZone::UTC, Timestamp::now()).unwrap();
        assert!(plan.entries.is_empty());
    }

    #[test]
    fn dry_run_style_plan_deletes_nothing_by_itself() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("dest");
        let quarantine = dest.join("_quarantine");
        let group_dir = quarantine.join("group-a");
        std::fs::create_dir_all(&group_dir).unwrap();
        std::fs::write(group_dir.join("clip.mp4"), b"x").unwrap();

        let prof = profile(dest, None);
        let _plan = build_plan(&prof, None, &TimeZone::UTC, Timestamp::now()).unwrap();
        // build_plan is the entire "dry run" — no call to execute() was
        // made, so nothing should have been touched.
        assert!(group_dir.exists());
    }

    #[test]
    fn execute_deletes_only_purge_candidates() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("dest");
        let quarantine = dest.join("_quarantine");
        let young = quarantine.join("young-group");
        let old = quarantine.join("old-group");
        std::fs::create_dir_all(&young).unwrap();
        std::fs::create_dir_all(&old).unwrap();
        set_dir_mtime(&young, 5);
        set_dir_mtime(&old, 45);

        let prof = profile(dest.clone(), None);
        let older_than: Span = "30d".parse().unwrap();
        let plan = build_plan(&prof, Some(older_than), &TimeZone::UTC, Timestamp::now()).unwrap();

        let report = execute_inner(&plan, true, false).unwrap();
        assert_eq!(report.results.len(), 1);
        assert!(report.results[0].deleted);
        assert!(!old.exists(), "old entry must be deleted");
        assert!(young.exists(), "young entry must be retained");
    }

    #[test]
    fn destination_siblings_are_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("dest");
        let quarantine = dest.join("_quarantine");
        let kept_footage = dest.join("2026/2026-07-10");
        std::fs::create_dir_all(&kept_footage).unwrap();
        std::fs::write(kept_footage.join("clip.mp4"), b"real footage").unwrap();
        std::fs::create_dir_all(quarantine.join("group-a")).unwrap();

        let prof = profile(dest.clone(), None);
        let plan = build_plan(&prof, None, &TimeZone::UTC, Timestamp::now()).unwrap();
        let report = execute_inner(&plan, true, false).unwrap();

        assert_eq!(report.results.len(), 1);
        assert!(kept_footage.join("clip.mp4").exists());
    }

    #[test]
    fn non_interactive_without_yes_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("dest");
        let quarantine = dest.join("_quarantine");
        std::fs::create_dir_all(quarantine.join("group-a")).unwrap();

        let prof = profile(dest, None);
        let plan = build_plan(&prof, None, &TimeZone::UTC, Timestamp::now()).unwrap();
        let err = execute_inner(&plan, false, false).unwrap_err();
        assert!(matches!(err, Error::Config(_)));
        assert!(quarantine.join("group-a").exists(), "nothing deleted");
    }

    #[test]
    fn empty_plan_executes_as_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("dest");
        std::fs::create_dir_all(&dest).unwrap();
        let prof = profile(dest, None);
        let plan = build_plan(&prof, None, &TimeZone::UTC, Timestamp::now()).unwrap();
        // No candidates: must not even reach the confirmation gate,
        // so this must succeed with stdin_is_terminal = false and no --yes.
        let report = execute_inner(&plan, false, false).unwrap();
        assert!(report.results.is_empty());
    }
}
