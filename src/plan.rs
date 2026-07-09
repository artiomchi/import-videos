//! Scan → plan: turns an `ImportSource`'s findings into a fully
//! resolved `ImportPlan` (design D4). Planning is pure data
//! transformation — no filesystem writes — so `scan` and
//! `import --dry-run` can share it verbatim.

use std::path::{Path, PathBuf};

use crate::config::{Profile, SourceLocation};
use crate::error::{Error, Result};
use crate::source::{ImportSource, MediaGroup, Verdict};

/// A `MediaGroup` paired with its verdict and fully resolved
/// destination (`Keep`) or quarantine (`Quarantine`) directory. Every
/// decision `import` will make is visible here, verbatim, before any
/// file moves (spec: "Import executes exactly the scanned plan").
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
        return Ok(Some(path));
    }

    for root in mount_roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let candidate = entry.path();
            if candidate.is_dir() && source_impl.detect(&candidate) {
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
) -> Result<ImportPlan> {
    let groups = source_impl.scan(source_root)?;
    let mut actions = Vec::with_capacity(groups.len());

    for (group, verdict) in groups {
        let (destination, quarantine_path) = match &verdict {
            Verdict::Keep => {
                let relative = profile.layout.resolve(&group.context, group.timestamp)?;
                (Some(profile.destination.join(relative)), None)
            }
            Verdict::Quarantine => {
                let base = profile
                    .quarantine
                    .clone()
                    .unwrap_or_else(|| profile.destination.join("_quarantine"));
                (None, Some(base.join(&group.name)))
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

    Ok(ImportPlan { actions })
}
