//! End-to-end coverage of the spec-driven scenarios in
//! `openspec/changes/add-core-cli/specs/cli-core/spec.md`. Two levels:
//!
//! - A `TestSource` (`ImportSource` impl over real files in a tempdir)
//!   drives the planning/transfer pipeline directly for scenarios that
//!   need real `Keep`/`Quarantine` media — this changeset ships no
//!   real device modules, so there's no other way to get media into
//!   the pipeline.
//! - The compiled binary, invoked via `Command`, covers CLI-level
//!   concerns (config errors, exit codes, "no sources found") using
//!   the `generic` profile type, which never finds media on its own.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use import_videos::config::{LayoutTemplate, Profile, SourceKind, SourceLocation};
use import_videos::error;
use import_videos::plan::{self, ImportPlan};
use import_videos::source::{ImportSource, MediaFile, MediaGroup, Verdict};
use import_videos::transfer::{self, TransferOutcome};

fn ts(secs: i64) -> jiff::Timestamp {
    jiff::Timestamp::from_second(secs).unwrap()
}

/// Scans whatever groups it was built with, regardless of `root` —
/// the media those groups reference still lives under a real tempdir,
/// so transfer/hashing exercises real file I/O.
struct TestSource {
    groups: Vec<(MediaGroup, Verdict)>,
}

impl ImportSource for TestSource {
    fn detect(&self, _root: &Path) -> bool {
        true
    }

    fn scan(
        &self,
        _root: &Path,
        _ignore: &globset::GlobSet,
    ) -> error::Result<Vec<(MediaGroup, Verdict)>> {
        Ok(self.groups.clone())
    }
}

fn empty_globset() -> globset::GlobSet {
    globset::GlobSetBuilder::new().build().unwrap()
}

fn profile(destination: &Path, quarantine: Option<PathBuf>, delete_source: bool) -> Profile {
    Profile {
        kind: SourceKind::Generic,
        source: SourceLocation::Auto,
        destination: destination.to_path_buf(),
        layout: LayoutTemplate::parse("{date:%Y}/{date:%Y-%m-%d}").unwrap(),
        ignore: empty_globset(),
        quarantine,
        delete_source,
    }
}

fn write_file(path: &Path, contents: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn media_file(path: &Path) -> MediaFile {
    MediaFile {
        path: path.to_path_buf(),
        size: fs::metadata(path).unwrap().len(),
    }
}

fn group(name: &str, files: Vec<MediaFile>) -> MediaGroup {
    MediaGroup {
        name: name.to_string(),
        files,
        timestamp: ts(0),
        markers: vec![],
        geo: None,
        context: HashMap::new(),
        sidecar: None,
    }
}

fn tree_snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut entries = Vec::new();
    if !root.exists() {
        return entries;
    }
    for entry in walk(root) {
        if entry.is_file() {
            entries.push((entry.clone(), fs::read(&entry).unwrap()));
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}

// --- Scan is read-only / dry-run performs no filesystem changes ---

#[test]
fn scan_and_dry_run_perform_no_filesystem_changes() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("source/clip.mp4");
    write_file(&src, b"footage");
    let dest = dir.path().join("dest");

    let source_before = tree_snapshot(&dir.path().join("source"));

    let source_impl = TestSource {
        groups: vec![(group("session", vec![media_file(&src)]), Verdict::Keep)],
    };
    let prof = profile(&dest, None, false);
    let import_plan = plan::build_plan(&prof, &source_impl, &dir.path().join("source")).unwrap();

    assert_eq!(import_plan.actions.len(), 1);
    assert_eq!(tree_snapshot(&dir.path().join("source")), source_before);
    assert!(
        !dest.exists(),
        "building a plan must not create the destination"
    );
}

// --- Execution follows the plan (Keep -> destination, Quarantine -> quarantine path) ---

#[test]
fn execution_follows_the_plan() {
    let dir = tempfile::tempdir().unwrap();
    let keep_src = dir.path().join("source/keep.mp4");
    let quarantine_src = dir.path().join("source/quarantine.mp4");
    write_file(&keep_src, b"good footage");
    write_file(&quarantine_src, b"unmarked footage");
    let dest = dir.path().join("dest");
    let quarantine = dir.path().join("quarantine");

    let source_impl = TestSource {
        groups: vec![
            (group("a", vec![media_file(&keep_src)]), Verdict::Keep),
            (
                group("b", vec![media_file(&quarantine_src)]),
                Verdict::Quarantine,
            ),
        ],
    };
    let prof = profile(&dest, Some(quarantine.clone()), false);
    let import_plan = plan::build_plan(&prof, &source_impl, Path::new("/ignored")).unwrap();

    let report = transfer::execute(&import_plan, false, false, false).unwrap();

    assert_eq!(
        fs::read(dest.join("1970/1970-01-01/keep.mp4")).unwrap(),
        b"good footage"
    );
    assert_eq!(
        fs::read(quarantine.join("b/quarantine.mp4")).unwrap(),
        b"unmarked footage"
    );
    // Nothing outside the plan's two actions moved.
    assert_eq!(report.groups.len(), 2);
    assert!(report.groups.iter().all(|g| {
        g.files
            .iter()
            .all(|f| matches!(f.outcome, TransferOutcome::Transferred))
    }));
}

// --- Verification failure preserves the source; other groups still succeed ---

#[test]
fn transfer_failure_keeps_source_and_does_not_block_other_groups() {
    let dir = tempfile::tempdir().unwrap();
    let ok_src = dir.path().join("source/ok.mp4");
    let broken_src = dir.path().join("source/broken.mp4");
    write_file(&ok_src, b"fine");
    write_file(&broken_src, b"also fine");

    let ok_dest = dir.path().join("dest_ok");
    let broken_dest = dir.path().join("dest_broken");
    fs::create_dir_all(&broken_dest).unwrap();
    // Simulate a write-time failure (the spec scenario is a hash
    // mismatch; making the destination directory unwritable is a
    // portable, deterministic way to hit the same failure path: copy
    // fails, the .part file never lands, and the source is untouched).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&broken_dest, fs::Permissions::from_mode(0o500)).unwrap();
    }

    // Built by hand (not via TestSource + build_plan) so each group
    // lands in its own directory — build_plan would put both Keep
    // groups under the same date-templated destination.
    let import_plan = ImportPlan {
        actions: vec![
            plan_action(
                &group("ok", vec![media_file(&ok_src)]),
                Verdict::Keep,
                Some(ok_dest.clone()),
                None,
            ),
            plan_action(
                &group("broken", vec![media_file(&broken_src)]),
                Verdict::Keep,
                Some(broken_dest.clone()),
                None,
            ),
        ],
    };

    let report = transfer::execute(&import_plan, false, false, false).unwrap();

    let ok_group = report.groups.iter().find(|g| g.group_name == "ok").unwrap();
    assert!(matches!(
        ok_group.files[0].outcome,
        TransferOutcome::Transferred
    ));

    let broken_group = report
        .groups
        .iter()
        .find(|g| g.group_name == "broken")
        .unwrap();
    assert!(matches!(
        broken_group.files[0].outcome,
        TransferOutcome::Failed(_)
    ));
    assert!(
        broken_src.exists(),
        "source must remain after a failed transfer"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&broken_dest, fs::Permissions::from_mode(0o700)).unwrap();
    }
}

fn plan_action(
    g: &MediaGroup,
    verdict: Verdict,
    destination: Option<PathBuf>,
    quarantine_path: Option<PathBuf>,
) -> plan::PlannedAction {
    plan::PlannedAction {
        group: g.clone(),
        verdict,
        destination,
        quarantine_path,
    }
}

// --- Re-running an import is idempotent ---

#[test]
fn rerunning_import_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("source/clip.mp4");
    write_file(&src, b"footage");
    let dest = dir.path().join("dest");

    let make_plan = || {
        let source_impl = TestSource {
            groups: vec![(group("a", vec![media_file(&src)]), Verdict::Keep)],
        };
        let prof = profile(&dest, None, false);
        plan::build_plan(&prof, &source_impl, Path::new("/ignored")).unwrap()
    };

    let first = transfer::execute(&make_plan(), false, false, false).unwrap();
    assert!(matches!(
        first.groups[0].files[0].outcome,
        TransferOutcome::Transferred
    ));

    let dest_snapshot = tree_snapshot(&dest);

    let second = transfer::execute(&make_plan(), false, false, false).unwrap();
    assert!(matches!(
        second.groups[0].files[0].outcome,
        TransferOutcome::SkippedIdentical
    ));
    assert_eq!(
        tree_snapshot(&dest),
        dest_snapshot,
        "destination must be unchanged"
    );
}

// --- Same name, different content gets suffixed ---

#[test]
fn different_content_at_destination_gets_suffixed() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("source/clip.mp4");
    write_file(&src, b"new bytes");
    let dest = dir.path().join("dest/1970/1970-01-01");
    write_file(&dest.join("clip.mp4"), b"old bytes");

    let source_impl = TestSource {
        groups: vec![(group("a", vec![media_file(&src)]), Verdict::Keep)],
    };
    let prof = profile(&dir.path().join("dest"), None, false);
    let import_plan = plan::build_plan(&prof, &source_impl, Path::new("/ignored")).unwrap();

    let report = transfer::execute(&import_plan, false, false, false).unwrap();

    assert!(matches!(
        report.groups[0].files[0].outcome,
        TransferOutcome::Suffixed(_)
    ));
    assert_eq!(fs::read(dest.join("clip.mp4")).unwrap(), b"old bytes");
    assert_eq!(fs::read(dest.join("clip-1.mp4")).unwrap(), b"new bytes");
}

// --- keep-source overrides delete_source for the run ---

#[test]
fn keep_source_flag_overrides_delete_source() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("source/clip.mp4");
    write_file(&src, b"footage");
    let dest = dir.path().join("dest");

    let source_impl = TestSource {
        groups: vec![(group("a", vec![media_file(&src)]), Verdict::Keep)],
    };
    let prof = profile(&dest, None, true); // delete_source: true
    let import_plan = plan::build_plan(&prof, &source_impl, Path::new("/ignored")).unwrap();

    // keep_source = true overrides the profile for this run.
    transfer::execute(&import_plan, prof.delete_source, true, true).unwrap();

    assert!(src.exists(), "--keep-source must prevent source deletion");
}

// --- Clean card after successful import (delete_source honored) ---

#[test]
fn delete_source_removes_file_after_confirmed_transfer() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("source/clip.mp4");
    write_file(&src, b"footage");
    let dest = dir.path().join("dest");

    let source_impl = TestSource {
        groups: vec![(group("a", vec![media_file(&src)]), Verdict::Keep)],
    };
    let prof = profile(&dest, None, true);
    let import_plan = plan::build_plan(&prof, &source_impl, Path::new("/ignored")).unwrap();

    let report = transfer::execute(&import_plan, true, false, true).unwrap();

    assert!(report.groups[0].deleted_from_source);
    assert!(!src.exists());
}

// --- Non-interactive run without --yes skips deletion ---

#[test]
fn non_interactive_without_yes_skips_deletion() {
    // cargo test processes have non-tty stdin (true under any CI or
    // headless runner, and here), so assume_yes=false with no `[y/N]`
    // input available takes the "stdin is not a terminal" branch.
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("source/clip.mp4");
    write_file(&src, b"footage");
    let dest = dir.path().join("dest");

    let source_impl = TestSource {
        groups: vec![(group("a", vec![media_file(&src)]), Verdict::Keep)],
    };
    let prof = profile(&dest, None, true);
    let import_plan = plan::build_plan(&prof, &source_impl, Path::new("/ignored")).unwrap();

    let report = transfer::execute(&import_plan, true, false, false).unwrap();

    assert!(src.exists(), "deletion must be skipped, not assumed");
    assert!(!report.groups[0].deleted_from_source);
    assert!(report.deletion_skipped_reason.is_some());
}

// --- CLI-level: exit codes and config errors (spec: "Process exit codes reflect outcome") ---

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_import-videos"))
}

fn write_config(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

#[test]
fn missing_config_file_exits_2() {
    let dir = tempfile::tempdir().unwrap();
    let status = bin()
        .args([
            "--config",
            dir.path().join("nope.yaml").to_str().unwrap(),
            "scan",
            "cam",
        ])
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(2));
}

#[test]
fn unknown_profile_exits_2() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.yaml");
    write_config(
        &config_path,
        &format!(
            "profiles:\n  cam:\n    type: generic\n    source: auto\n    destination: {}\n    layout: \"{{date}}\"\n",
            dir.path().join("dest").display()
        ),
    );

    let status = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "scan",
            "missing-profile",
        ])
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(2));
}

#[test]
fn nonexistent_explicit_source_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.yaml");
    write_config(
        &config_path,
        &format!(
            "profiles:\n  cam:\n    type: generic\n    source: auto\n    destination: {}\n    layout: \"{{date}}\"\n",
            dir.path().join("dest").display()
        ),
    );

    let status = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "scan",
            "cam",
            "--source",
            dir.path().join("does-not-exist").to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(1));
}

#[test]
fn no_sources_found_exits_0() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.yaml");
    let source_dir = dir.path().join("card");
    fs::create_dir_all(&source_dir).unwrap();
    write_config(
        &config_path,
        &format!(
            "profiles:\n  cam:\n    type: generic\n    source: auto\n    destination: {}\n    layout: \"{{date}}\"\n",
            dir.path().join("dest").display()
        ),
    );

    let output = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "scan",
            "cam",
            "--source",
            source_dir.to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stdout).contains("no sources found"));
}
