//! Scan → plan: turns an `ImportSource`'s findings into a fully
//! resolved `ImportPlan` (design D4). Planning is pure data
//! transformation — no filesystem writes — so `scan` and
//! `import --dry-run` can share it verbatim.

use std::path::{Path, PathBuf};

use jiff::Timestamp;

use crate::config::{Profile, SourceLocation};
use crate::error::{Error, Result};
use crate::progress::Progress;
use crate::source::{ImportSource, MediaGroup, ScanContext, Verdict};

/// A `MediaGroup` paired with its verdict and fully resolved
/// destination (`Keep`) or quarantine (`Quarantine`) directory. Every
/// decision `import` will make is visible here, verbatim, before any
/// file moves (spec: "Import executes exactly the scanned plan").
///
/// A `Quarantine` action with `quarantine_path == None` means "report
/// the verdict, but leave the source untouched" — produced when the
/// profile sets `copy_quarantine: false`. Because no files are
/// transferred for such a group, it can never become a source-deletion
/// candidate.
#[derive(Debug, Clone)]
pub struct PlannedAction {
    pub group: MediaGroup,
    pub verdict: Verdict,
    pub destination: Option<PathBuf>,
    pub quarantine_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Default)]
pub struct ImportPlan {
    pub actions: Vec<PlannedAction>,
}

/// Resolves the effective source root for a profile: explicit
/// `--source` overrides the profile; the profile's own `source: <path>`
/// is used as-is; `source: auto` probes `mount_roots` and offers each
/// mounted volume to `source_impl.detect()` (design D6).
///
/// `Ok(None)` means "auto-detection found nothing" — not an error; the
/// caller reports "no sources found" and exits 0. An explicit path
/// (from either `--source` or the profile) that doesn't exist is an
/// error (spec: exits 1).
pub fn resolve_source(
    profile: &Profile,
    cli_source: Option<&Path>,
    source_impl: &dyn ImportSource,
    mount_roots: &[PathBuf],
) -> Result<Option<PathBuf>> {
    let explicit = cli_source
        .map(Path::to_path_buf)
        .or_else(|| match &profile.source {
            SourceLocation::Path(path) => Some(path.clone()),
            SourceLocation::Auto => None,
        });

    if let Some(path) = explicit {
        if !path.exists() {
            return Err(Error::io(
                &path,
                std::io::Error::new(std::io::ErrorKind::NotFound, "source path does not exist"),
            ));
        }
        tracing::info!(source = %path.display(), "source resolved");
        return Ok(Some(path));
    }

    for root in mount_roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let candidate = entry.path();
            if candidate.is_dir() && source_impl.detect(&candidate) {
                tracing::info!(source = %candidate.display(), "source resolved");
                return Ok(Some(candidate));
            }
        }
    }
    Ok(None)
}

/// Builds an `ImportPlan` by scanning `source_root` and resolving each
/// group's destination or quarantine path against the profile's layout
/// template. Fails (naming the missing field) if a `Keep` group's
/// context doesn't satisfy the layout template (spec: "Unknown field at
/// resolution time").
pub fn build_plan(
    profile: &Profile,
    source_impl: &dyn ImportSource,
    source_root: &Path,
    timezone: &jiff::tz::TimeZone,
    progress: &Progress,
) -> Result<ImportPlan> {
    let ctx = ScanContext {
        ignore: &profile.ignore,
        tz: timezone,
        imported_at: Timestamp::now(),
        progress,
    };
    let groups = source_impl.scan(source_root, &ctx)?;
    tracing::info!(groups = groups.len(), "scan complete");
    let mut actions = Vec::with_capacity(groups.len());

    let (mut kept, mut quarantined, mut ignored) = (0usize, 0usize, 0usize);
    for (group, verdict) in groups {
        match &verdict {
            Verdict::Keep => kept += 1,
            Verdict::Quarantine => quarantined += 1,
            Verdict::Ignore(_) => ignored += 1,
        }
        let (destination, quarantine_path) = match &verdict {
            Verdict::Keep => {
                let relative = profile
                    .layout
                    .resolve(&group.context, group.timestamp, timezone)?;
                (Some(profile.destination.join(relative)), None)
            }
            Verdict::Quarantine => {
                if profile.copy_quarantine {
                    (None, Some(profile.quarantine_root().join(&group.name)))
                } else {
                    // copy_quarantine: false — leave source in place;
                    // no path to transfer to.
                    (None, None)
                }
            }
            Verdict::Ignore(_) => (None, None),
        };
        actions.push(PlannedAction {
            group,
            verdict,
            destination,
            quarantine_path,
        });
    }
    tracing::info!(kept, quarantined, ignored, "plan built");

    Ok(ImportPlan { actions })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{LayoutTemplate, SourceKind};
    use crate::error;
    use crate::source::{MediaGroup, ScanContext};
    use globset::GlobSetBuilder;
    use std::collections::HashMap;

    fn empty_globset() -> globset::GlobSet {
        GlobSetBuilder::new().build().unwrap()
    }

    fn profile_with_copy_quarantine(dest: PathBuf, copy_quarantine: bool) -> Profile {
        Profile {
            kind: SourceKind::Generic,
            source: SourceLocation::Auto,
            destination: dest,
            layout: LayoutTemplate::parse("{date:%Y}/{date:%Y-%m-%d}").unwrap(),
            ignore: empty_globset(),
            quarantine: None,
            delete_source: false,
            copy_quarantine,
            reflink: true,
        }
    }

    struct StubSource {
        groups: Vec<(MediaGroup, Verdict)>,
    }

    impl ImportSource for StubSource {
        fn detect(&self, _root: &Path) -> bool {
            true
        }
        fn scan(
            &self,
            _root: &Path,
            _ctx: &ScanContext,
        ) -> error::Result<Vec<(MediaGroup, Verdict)>> {
            Ok(self.groups.clone())
        }
    }

    fn quarantine_group() -> MediaGroup {
        MediaGroup {
            name: "unmarked".to_string(),
            files: vec![],
            timestamp: jiff::Timestamp::from_second(0).unwrap(),
            markers: vec![],
            geo: None,
            context: HashMap::new(),
            sidecar: None,
        }
    }

    #[test]
    fn quarantine_group_resolves_path_when_copy_quarantine_enabled() {
        let dest = PathBuf::from("/dest");
        let prof = profile_with_copy_quarantine(dest.clone(), true);
        let source = StubSource {
            groups: vec![(quarantine_group(), Verdict::Quarantine)],
        };
        let plan = build_plan(
            &prof,
            &source,
            Path::new("/src"),
            &jiff::tz::TimeZone::UTC,
            &Progress::hidden(),
        )
        .unwrap();
        let action = &plan.actions[0];
        assert!(
            action.quarantine_path.is_some(),
            "copy_quarantine: true should resolve a quarantine path"
        );
        assert_eq!(
            action.quarantine_path,
            Some(dest.join("_quarantine").join("unmarked"))
        );
    }

    #[test]
    fn quarantine_group_has_no_path_when_copy_quarantine_disabled() {
        let prof = profile_with_copy_quarantine(PathBuf::from("/dest"), false);
        let source = StubSource {
            groups: vec![(quarantine_group(), Verdict::Quarantine)],
        };
        let plan = build_plan(
            &prof,
            &source,
            Path::new("/src"),
            &jiff::tz::TimeZone::UTC,
            &Progress::hidden(),
        )
        .unwrap();
        let action = &plan.actions[0];
        assert_eq!(
            action.quarantine_path, None,
            "copy_quarantine: false must resolve quarantine_path to None"
        );
        assert_eq!(
            action.verdict,
            Verdict::Quarantine,
            "verdict must still be Quarantine"
        );
    }
}
