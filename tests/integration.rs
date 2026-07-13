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
use import_videos::progress::Progress;
use import_videos::report;
use import_videos::source::{ImportSource, MediaFile, MediaGroup, ScanContext, Verdict};
use import_videos::transfer::{self, TransferOutcome};
use import_videos::{ImportDriveOutcome, ScanDriveOutcome};

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

    fn scan(&self, _root: &Path, _ctx: &ScanContext) -> error::Result<Vec<(MediaGroup, Verdict)>> {
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
        copy_quarantine: true, // default: verified-copy behavior
        reflink: true,
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
        recorded_at: None,
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
//
// improve-scan-and-cleanup design D1: `scan` and `import --dry-run` no
// longer share `build_plan` — `scan` goes through `build_scan_summary`
// instead, so the read-only guarantee is exercised separately for each.

#[test]
fn dry_run_plan_building_performs_no_filesystem_changes() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("source/clip.mp4");
    write_file(&src, b"footage");
    let dest = dir.path().join("dest");

    let source_before = tree_snapshot(&dir.path().join("source"));

    let source_impl = TestSource {
        groups: vec![(group("session", vec![media_file(&src)]), Verdict::Keep)],
    };
    let prof = profile(&dest, None, false);
    let import_plan = plan::build_plan(
        &prof,
        &source_impl,
        &dir.path().join("source"),
        &jiff::tz::TimeZone::UTC,
        &Progress::hidden(),
    )
    .unwrap();

    assert_eq!(import_plan.actions.len(), 1);
    assert_eq!(tree_snapshot(&dir.path().join("source")), source_before);
    assert!(
        !dest.exists(),
        "building a plan must not create the destination"
    );
}

#[test]
fn scan_summary_building_performs_no_filesystem_changes() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("source/clip.mp4");
    write_file(&src, b"footage");
    let dest = dir.path().join("dest");

    let source_before = tree_snapshot(&dir.path().join("source"));

    let source_impl = TestSource {
        groups: vec![(group("session", vec![media_file(&src)]), Verdict::Keep)],
    };
    let prof = profile(&dest, None, false);
    let summary = plan::build_scan_summary(
        &prof,
        &source_impl,
        &dir.path().join("source"),
        &jiff::tz::TimeZone::UTC,
        &Progress::hidden(),
    )
    .unwrap();

    assert_eq!(summary.entries.len(), 1);
    assert_eq!(tree_snapshot(&dir.path().join("source")), source_before);
    assert!(
        !dest.exists(),
        "building a scan summary must never create the destination"
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
    let import_plan = plan::build_plan(
        &prof,
        &source_impl,
        Path::new("/ignored"),
        &jiff::tz::TimeZone::UTC,
        &Progress::hidden(),
    )
    .unwrap();

    let report = transfer::execute(
        &import_plan,
        dir.path(),
        false,
        false,
        false,
        false,
        &Progress::hidden(),
    )
    .unwrap();

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

    let report = transfer::execute(
        &import_plan,
        dir.path(),
        false,
        false,
        false,
        false,
        &Progress::hidden(),
    )
    .unwrap();

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
        plan::build_plan(
            &prof,
            &source_impl,
            Path::new("/ignored"),
            &jiff::tz::TimeZone::UTC,
            &Progress::hidden(),
        )
        .unwrap()
    };

    let first = transfer::execute(
        &make_plan(),
        dir.path(),
        false,
        false,
        false,
        false,
        &Progress::hidden(),
    )
    .unwrap();
    assert!(matches!(
        first.groups[0].files[0].outcome,
        TransferOutcome::Transferred
    ));

    let dest_snapshot = tree_snapshot(&dest);

    let second = transfer::execute(
        &make_plan(),
        dir.path(),
        false,
        false,
        false,
        false,
        &Progress::hidden(),
    )
    .unwrap();
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
    let import_plan = plan::build_plan(
        &prof,
        &source_impl,
        Path::new("/ignored"),
        &jiff::tz::TimeZone::UTC,
        &Progress::hidden(),
    )
    .unwrap();

    let report = transfer::execute(
        &import_plan,
        dir.path(),
        false,
        false,
        false,
        false,
        &Progress::hidden(),
    )
    .unwrap();

    assert!(matches!(
        report.groups[0].files[0].outcome,
        TransferOutcome::Suffixed(_)
    ));
    assert_eq!(fs::read(dest.join("clip.mp4")).unwrap(), b"old bytes");
    assert_eq!(fs::read(dest.join("clip-1.mp4")).unwrap(), b"new bytes");
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
    let import_plan = plan::build_plan(
        &prof,
        &source_impl,
        Path::new("/ignored"),
        &jiff::tz::TimeZone::UTC,
        &Progress::hidden(),
    )
    .unwrap();

    let report = transfer::execute(
        &import_plan,
        dir.path(),
        true,
        true,
        false,
        false,
        &Progress::hidden(),
    )
    .unwrap();

    assert!(report.groups[0].deleted_from_source);
    assert!(!src.exists());
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

// --- copy_quarantine: false leaves source untouched and creates no quarantine dir ---

#[test]
fn disabled_quarantine_copy_leaves_source_and_creates_no_dir() {
    // Spec 5.1: with copy_quarantine: false, a Quarantine group is not
    // copied, no quarantine directory is created, the source file
    // remains byte-for-byte, and its outcome is SkippedQuarantineDisabled.
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("source/clip.mp4");
    write_file(&src, b"unmarked footage");
    let dest = dir.path().join("dest");
    let quarantine_dir = dir.path().join("quarantine");

    let source_impl = TestSource {
        groups: vec![(
            group("session", vec![media_file(&src)]),
            Verdict::Quarantine,
        )],
    };
    let prof = Profile {
        kind: SourceKind::Generic,
        source: SourceLocation::Auto,
        destination: dest.clone(),
        layout: LayoutTemplate::parse("{date:%Y}/{date:%Y-%m-%d}").unwrap(),
        ignore: empty_globset(),
        quarantine: Some(quarantine_dir.clone()),
        delete_source: false,
        copy_quarantine: false, // <-- the toggle under test
        reflink: true,
    };

    let import_plan = plan::build_plan(
        &prof,
        &source_impl,
        Path::new("/ignored"),
        &jiff::tz::TimeZone::UTC,
        &Progress::hidden(),
    )
    .unwrap();
    let report = transfer::execute(
        &import_plan,
        dir.path(),
        false,
        false,
        false,
        false,
        &Progress::hidden(),
    )
    .unwrap();

    // Source must be untouched.
    assert!(
        src.exists(),
        "source file must remain when copy_quarantine is false"
    );
    assert_eq!(
        fs::read(&src).unwrap(),
        b"unmarked footage",
        "source bytes must be unchanged"
    );

    // No quarantine directory must be created.
    assert!(
        !quarantine_dir.exists(),
        "no quarantine directory should be created when copy_quarantine is false"
    );

    // Outcome must be SkippedQuarantineDisabled.
    assert_eq!(
        report.groups[0].files[0].outcome,
        TransferOutcome::SkippedQuarantineDisabled,
    );
}

// --- copy_quarantine: false with delete_source: true never deletes the quarantine source ---

#[test]
fn disabled_quarantine_copy_source_not_deleted_even_with_delete_source() {
    // Spec 5.2: with copy_quarantine: false and delete_source: true +
    // --yes, quarantined source files are NOT deleted (no verified
    // transfer occurred), while an eligible Keep group IS cleaned.
    let dir = tempfile::tempdir().unwrap();
    let keep_src = dir.path().join("source/keep.mp4");
    let quarantine_src = dir.path().join("source/quarantine.mp4");
    write_file(&keep_src, b"good footage");
    write_file(&quarantine_src, b"unmarked footage");
    let dest = dir.path().join("dest");

    let source_impl = TestSource {
        groups: vec![
            (group("kept", vec![media_file(&keep_src)]), Verdict::Keep),
            (
                group("unmarked", vec![media_file(&quarantine_src)]),
                Verdict::Quarantine,
            ),
        ],
    };
    let prof = Profile {
        kind: SourceKind::Generic,
        source: SourceLocation::Auto,
        destination: dest.clone(),
        layout: LayoutTemplate::parse("{date:%Y}/{date:%Y-%m-%d}").unwrap(),
        ignore: empty_globset(),
        quarantine: None,
        delete_source: true,
        copy_quarantine: false,
        reflink: true,
    };

    let import_plan = plan::build_plan(
        &prof,
        &source_impl,
        Path::new("/ignored"),
        &jiff::tz::TimeZone::UTC,
        &Progress::hidden(),
    )
    .unwrap();
    // assume_yes = true to skip the interactive prompt.
    let report = transfer::execute(
        &import_plan,
        dir.path(),
        true,
        true,
        false,
        false,
        &Progress::hidden(),
    )
    .unwrap();

    // The Keep group's source is deleted after a verified transfer.
    assert!(
        !keep_src.exists(),
        "Keep group source should be deleted after verified import with delete_source"
    );
    let kept_group = report
        .groups
        .iter()
        .find(|g| g.group_name == "kept")
        .unwrap();
    assert!(kept_group.deleted_from_source);

    // The Quarantine group's source must NOT be deleted.
    assert!(
        quarantine_src.exists(),
        "quarantined source must NOT be deleted even with delete_source: true"
    );
    let quarantine_group = report
        .groups
        .iter()
        .find(|g| g.group_name == "unmarked")
        .unwrap();
    assert!(!quarantine_group.deleted_from_source);
}

// --- Diagnostics on stderr, never stdout (improve-console-output design D8, task 3.3) ---

#[test]
fn warning_under_json_mode_stays_off_stdout_and_stdout_is_one_document() {
    // A garbage chapter file makes `chapter_civil_time` fail to read an
    // MVHD creation time and fall back to the file's mtime, firing
    // `tracing::warn!` mid-scan — exactly the case the JSON-mode
    // contract ("no other stdout output") must survive.
    let dir = tempfile::tempdir().unwrap();
    let source_dir = dir.path().join("card");
    write_file(
        &source_dir.join("DCIM/100GOPRO/GX010001.MP4"),
        b"not an mp4 at all",
    );
    let dest = dir.path().join("dest");
    let config_path = dir.path().join("config.yaml");
    write_config(
        &config_path,
        &format!(
            "profiles:\n  cam:\n    type: gopro\n    require_marker: false\n    source: auto\n    destination: {}\n    layout: \"{{date:%Y}}/{{date:%Y-%m-%d}}\"\n",
            dest.display()
        ),
    );

    let output = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "--json",
            "scan",
            "cam",
            "--source",
            source_dir.to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));

    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str::<serde_json::Value>(&stdout)
        .unwrap_or_else(|e| panic!("stdout must be exactly one JSON document: {e}\n{stdout}"));

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("could not read camera-clock creation time"),
        "warning must appear on stderr, got: {stderr}"
    );
}

// --- Quick-match tests (tasks 7.2-7.4) ---

/// Stamps a file's mtime to the given `jiff::Timestamp` (mirrors
/// `transfer_file`'s behaviour so the test can set up an
/// already-imported destination without re-running the full import).
fn stamp_mtime(path: &std::path::Path, recorded_at: jiff::Timestamp) {
    let file = std::fs::File::options().write(true).open(path).unwrap();
    file.set_modified(std::time::SystemTime::from(recorded_at))
        .unwrap();
}

fn media_file_with_ts(path: &Path, recorded_at: jiff::Timestamp) -> MediaFile {
    MediaFile {
        path: path.to_path_buf(),
        size: fs::metadata(path).unwrap().len(),
        recorded_at: Some(recorded_at),
    }
}

fn group_with_ts(name: &str, files: Vec<MediaFile>) -> MediaGroup {
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

// 7.2: quick-match hit — destination has same name, size, mtime → SkippedQuickMatch
#[test]
fn quick_match_hit_skips_hashing_and_reports_distinct_outcome() {
    let dir = tempfile::tempdir().unwrap();
    let recorded_at = ts(1_751_641_431); // arbitrary fixed instant

    let src = dir.path().join("source/clip.mp4");
    write_file(&src, b"footage bytes");
    let dest_dir = dir.path().join("dest/1970/1970-01-01");
    let dest_file = dest_dir.join("clip.mp4");
    // Pre-place a destination file with the same content and matching mtime.
    fs::create_dir_all(&dest_dir).unwrap();
    fs::write(&dest_file, b"footage bytes").unwrap();
    stamp_mtime(&dest_file, recorded_at);

    let source_impl = TestSource {
        groups: vec![(
            group_with_ts("a", vec![media_file_with_ts(&src, recorded_at)]),
            Verdict::Keep,
        )],
    };
    let prof = profile(&dir.path().join("dest"), None, false);
    let import_plan = plan::build_plan(
        &prof,
        &source_impl,
        Path::new("/ignored"),
        &jiff::tz::TimeZone::UTC,
        &Progress::hidden(),
    )
    .unwrap();

    let report = transfer::execute(
        &import_plan,
        dir.path(),
        false,
        false,
        true,
        false,
        &Progress::hidden(),
    )
    .unwrap();

    assert_eq!(
        report.groups[0].files[0].outcome,
        TransferOutcome::SkippedQuickMatch,
        "same-name + same-size + matching mtime must return SkippedQuickMatch"
    );
    // Source is untouched (no delete_source).
    assert!(src.exists());
}

// 7.3: quick-match miss (size differs) → falls through to verified transfer
#[test]
fn quick_match_miss_on_size_difference_falls_through_to_verified_transfer() {
    let dir = tempfile::tempdir().unwrap();
    let recorded_at = ts(1_751_641_431);

    let src = dir.path().join("source/clip.mp4");
    write_file(&src, b"footage bytes long");
    let dest_dir = dir.path().join("dest/1970/1970-01-01");
    let dest_file = dest_dir.join("clip.mp4");
    // Pre-place a destination file with *different* content (size mismatch).
    fs::create_dir_all(&dest_dir).unwrap();
    fs::write(&dest_file, b"short").unwrap();
    stamp_mtime(&dest_file, recorded_at);

    let source_impl = TestSource {
        groups: vec![(
            group_with_ts("a", vec![media_file_with_ts(&src, recorded_at)]),
            Verdict::Keep,
        )],
    };
    let prof = profile(&dir.path().join("dest"), None, false);
    let import_plan = plan::build_plan(
        &prof,
        &source_impl,
        Path::new("/ignored"),
        &jiff::tz::TimeZone::UTC,
        &Progress::hidden(),
    )
    .unwrap();

    let report = transfer::execute(
        &import_plan,
        dir.path(),
        false,
        false,
        true,
        false,
        &Progress::hidden(),
    )
    .unwrap();

    // Size mismatch → miss → falls through to full verified transfer; content
    // differs from destination, so it gets suffixed.
    assert!(
        matches!(
            report.groups[0].files[0].outcome,
            TransferOutcome::Suffixed(_) | TransferOutcome::Transferred
        ),
        "size mismatch must fall through, not return SkippedQuickMatch; got {:?}",
        report.groups[0].files[0].outcome
    );
}

// 7.4: safety invariant — quick-matched group with delete_source: true + --yes
//      leaves all source files in place (content was not verified).
#[test]
fn fully_quick_matched_group_is_never_deleted_even_with_delete_source_and_yes() {
    let dir = tempfile::tempdir().unwrap();
    let recorded_at = ts(1_751_641_431);

    let src = dir.path().join("source/clip.mp4");
    write_file(&src, b"footage");
    let dest_dir = dir.path().join("dest/1970/1970-01-01");
    let dest_file = dest_dir.join("clip.mp4");
    fs::create_dir_all(&dest_dir).unwrap();
    fs::write(&dest_file, b"footage").unwrap();
    stamp_mtime(&dest_file, recorded_at);

    let source_impl = TestSource {
        groups: vec![(
            group_with_ts("a", vec![media_file_with_ts(&src, recorded_at)]),
            Verdict::Keep,
        )],
    };
    let prof = profile(&dir.path().join("dest"), None, true); // delete_source: true
    let import_plan = plan::build_plan(
        &prof,
        &source_impl,
        Path::new("/ignored"),
        &jiff::tz::TimeZone::UTC,
        &Progress::hidden(),
    )
    .unwrap();

    // delete_source=true, assume_yes=true, quick_match=true
    let report = transfer::execute(
        &import_plan,
        dir.path(),
        true,
        true,
        true,
        false,
        &Progress::hidden(),
    )
    .unwrap();

    // The group must be SkippedQuickMatch (not content-verified).
    assert_eq!(
        report.groups[0].files[0].outcome,
        TransferOutcome::SkippedQuickMatch
    );
    // Source MUST still exist — quick-match forfeits deletion eligibility.
    assert!(
        src.exists(),
        "source must not be deleted when group was only quick-matched (ADR 0009)"
    );
    // deleted_from_source must be false.
    assert!(
        !report.groups[0].deleted_from_source,
        "deleted_from_source must be false for a quick-matched group"
    );
}

// --- Multi-drive: continue past a failed drive (multi-drive-import, tasks 3.5, 3.6) ---

/// Detects every directory (stands in for `source: auto` matching every
/// mounted volume) and, per root, either scans canned groups or returns
/// a hard error — lets a test drive `resolve_sources`/`*_drives` with
/// per-drive behavior, the same way `TestSource` stands in for real
/// device detection elsewhere in this file.
struct MultiDriveSource {
    behavior: HashMap<PathBuf, DriveBehavior>,
}

enum DriveBehavior {
    Groups(Vec<(MediaGroup, Verdict)>),
    HardError,
}

impl ImportSource for MultiDriveSource {
    fn detect(&self, _root: &Path) -> bool {
        true
    }

    fn scan(&self, root: &Path, _ctx: &ScanContext) -> error::Result<Vec<(MediaGroup, Verdict)>> {
        match self.behavior.get(root) {
            Some(DriveBehavior::Groups(groups)) => Ok(groups.clone()),
            Some(DriveBehavior::HardError) => Err(error::Error::io(
                root,
                std::io::Error::other("simulated malformed metadata"),
            )),
            None => Ok(vec![]),
        }
    }
}

fn group_at(name: &str, files: Vec<MediaFile>, timestamp: jiff::Timestamp) -> MediaGroup {
    MediaGroup {
        name: name.to_string(),
        files,
        timestamp,
        markers: vec![],
        geo: None,
        context: HashMap::new(),
        sidecar: None,
    }
}

#[test]
fn multi_drive_import_continues_past_a_hard_error_on_one_drive() {
    // Task 3.5: three fake drives, drive 2's scan step hard-errors —
    // drive 1 and drive 3 must still produce their normal output and
    // drive 3's files must actually transfer, with the run's aggregate
    // reflecting drive 2's failure.
    let dir = tempfile::tempdir().unwrap();
    let mount_root = dir.path().join("mount");
    let drive1 = mount_root.join("drive1");
    let drive2 = mount_root.join("drive2");
    let drive3 = mount_root.join("drive3");
    fs::create_dir_all(&drive1).unwrap();
    fs::create_dir_all(&drive2).unwrap();
    fs::create_dir_all(&drive3).unwrap();

    let file1 = drive1.join("a.mp4");
    let file3 = drive3.join("c.mp4");
    write_file(&file1, b"drive one footage");
    write_file(&file3, b"drive three footage");

    let dest = dir.path().join("dest");
    let prof = profile(&dest, None, false);

    let mut behavior = HashMap::new();
    behavior.insert(
        drive1.clone(),
        DriveBehavior::Groups(vec![(group("a", vec![media_file(&file1)]), Verdict::Keep)]),
    );
    behavior.insert(drive2.clone(), DriveBehavior::HardError);
    behavior.insert(
        drive3.clone(),
        DriveBehavior::Groups(vec![(group("c", vec![media_file(&file3)]), Verdict::Keep)]),
    );
    let source_impl = MultiDriveSource { behavior };

    let drives = plan::resolve_sources(&source_impl, std::slice::from_ref(&mount_root));
    assert_eq!(drives.len(), 3, "all three drives must be detected");

    let results = import_videos::import_drives(
        &prof,
        &source_impl,
        &drives,
        false, // dry_run
        true,  // assume_yes
        false, // quick_match
        &jiff::tz::TimeZone::UTC,
        report::Detail::Normal, // detail
        false,                  // json
    );

    assert_eq!(results.len(), 3);
    assert!(
        matches!(
            results[0].result,
            Ok(ImportDriveOutcome::Executed {
                any_failed: false,
                ..
            })
        ),
        "drive 1 must complete normally: {:?}",
        results[0].result
    );
    assert!(
        results[1].result.is_err(),
        "drive 2's hard error must be caught, not propagated"
    );
    assert!(
        matches!(
            results[2].result,
            Ok(ImportDriveOutcome::Executed {
                any_failed: false,
                ..
            })
        ),
        "drive 3 must still run after drive 2 failed: {:?}",
        results[2].result
    );

    assert_eq!(
        fs::read(dest.join("1970/1970-01-01/c.mp4")).unwrap(),
        b"drive three footage",
        "drive 3's file must have actually transferred despite drive 2's failure"
    );

    assert!(
        import_videos::any_import_drive_failed(&results),
        "the run's aggregate must reflect drive 2's failure (process exits 1)"
    );
}

#[test]
fn multi_drive_import_continues_past_a_transfer_failure_on_one_drive() {
    // Task 3.6: drive 2 has one file that fails verification (same
    // permission-based injection as
    // transfer_failure_keeps_source_and_does_not_block_other_groups)
    // while drives 1 and 3 succeed — all three drives' reports must
    // exist and the run's aggregate must reflect drive 2's failure.
    let dir = tempfile::tempdir().unwrap();
    let mount_root = dir.path().join("mount");
    let drive1 = mount_root.join("drive1");
    let drive2 = mount_root.join("drive2");
    let drive3 = mount_root.join("drive3");
    fs::create_dir_all(&drive1).unwrap();
    fs::create_dir_all(&drive2).unwrap();
    fs::create_dir_all(&drive3).unwrap();

    let file1 = drive1.join("a.mp4");
    let file2 = drive2.join("b.mp4");
    let file3 = drive3.join("c.mp4");
    write_file(&file1, b"drive one footage");
    write_file(&file2, b"drive two footage");
    write_file(&file3, b"drive three footage");

    let dest = dir.path().join("dest");
    let prof = profile(&dest, None, false);

    // Distinct dates so each drive resolves to its own destination
    // directory — only drive 2's must be made unwritable.
    let ts1 = ts(0);
    let ts2 = ts(200_000);
    let ts3 = ts(400_000);

    let dest2_relative = prof
        .layout
        .resolve(&HashMap::new(), ts2, &jiff::tz::TimeZone::UTC)
        .unwrap();
    let dest2_dir = dest.join(dest2_relative);
    fs::create_dir_all(&dest2_dir).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dest2_dir, fs::Permissions::from_mode(0o500)).unwrap();
    }

    let mut behavior = HashMap::new();
    behavior.insert(
        drive1.clone(),
        DriveBehavior::Groups(vec![(
            group_at("a", vec![media_file(&file1)], ts1),
            Verdict::Keep,
        )]),
    );
    behavior.insert(
        drive2.clone(),
        DriveBehavior::Groups(vec![(
            group_at("b", vec![media_file(&file2)], ts2),
            Verdict::Keep,
        )]),
    );
    behavior.insert(
        drive3.clone(),
        DriveBehavior::Groups(vec![(
            group_at("c", vec![media_file(&file3)], ts3),
            Verdict::Keep,
        )]),
    );
    let source_impl = MultiDriveSource { behavior };

    let drives = plan::resolve_sources(&source_impl, std::slice::from_ref(&mount_root));
    assert_eq!(drives.len(), 3);

    let results = import_videos::import_drives(
        &prof,
        &source_impl,
        &drives,
        false,
        true,
        false,
        &jiff::tz::TimeZone::UTC,
        report::Detail::Normal,
        false,
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dest2_dir, fs::Permissions::from_mode(0o700)).unwrap();
    }

    assert_eq!(results.len(), 3);
    assert!(
        matches!(
            results[0].result,
            Ok(ImportDriveOutcome::Executed {
                any_failed: false,
                ..
            })
        ),
        "drive 1 must complete normally: {:?}",
        results[0].result
    );
    assert!(
        matches!(
            results[2].result,
            Ok(ImportDriveOutcome::Executed {
                any_failed: false,
                ..
            })
        ),
        "drive 3 must complete normally: {:?}",
        results[2].result
    );

    match &results[1].result {
        Ok(ImportDriveOutcome::Executed { report, any_failed }) => {
            assert!(*any_failed, "drive 2 must record its transfer failure");
            assert!(matches!(
                report.groups[0].files[0].outcome,
                TransferOutcome::Failed(_)
            ));
        }
        other => {
            panic!("drive 2 should still execute (soft failure, not a hard error), got {other:?}")
        }
    }
    assert!(
        file2.exists(),
        "drive 2's source must remain after its failed transfer"
    );

    assert!(
        import_videos::any_import_drive_failed(&results),
        "the run's aggregate must reflect drive 2's transfer failure (process exits 1)"
    );
}

#[test]
fn multi_drive_scan_continues_past_a_hard_error_on_one_drive() {
    // Verification follow-up: scan_drives (the multi-drive `scan` path)
    // had no automated coverage — only import_drives did. Mirrors the
    // import-side hard-error test above, but through scan_drives.
    let dir = tempfile::tempdir().unwrap();
    let mount_root = dir.path().join("mount");
    let drive1 = mount_root.join("drive1");
    let drive2 = mount_root.join("drive2");
    let drive3 = mount_root.join("drive3");
    fs::create_dir_all(&drive1).unwrap();
    fs::create_dir_all(&drive2).unwrap();
    fs::create_dir_all(&drive3).unwrap();

    let file1 = drive1.join("a.mp4");
    let file3 = drive3.join("c.mp4");
    write_file(&file1, b"drive one footage");
    write_file(&file3, b"drive three footage");

    let dest = dir.path().join("dest");
    let prof = profile(&dest, None, false);

    let mut behavior = HashMap::new();
    behavior.insert(
        drive1.clone(),
        DriveBehavior::Groups(vec![(group("a", vec![media_file(&file1)]), Verdict::Keep)]),
    );
    behavior.insert(drive2.clone(), DriveBehavior::HardError);
    behavior.insert(
        drive3.clone(),
        DriveBehavior::Groups(vec![(group("c", vec![media_file(&file3)]), Verdict::Keep)]),
    );
    let source_impl = MultiDriveSource { behavior };

    let drives = plan::resolve_sources(&source_impl, std::slice::from_ref(&mount_root));
    assert_eq!(drives.len(), 3, "all three drives must be detected");

    let results = import_videos::scan_drives(
        &prof,
        &source_impl,
        &drives,
        &jiff::tz::TimeZone::UTC,
        report::Detail::Normal, // detail
        false,                  // json
    );

    assert_eq!(results.len(), 3);
    assert!(
        matches!(results[0].result, Ok(ScanDriveOutcome::Found(_))),
        "drive 1 must scan normally: {:?}",
        results[0].result
    );
    assert!(
        results[1].result.is_err(),
        "drive 2's hard scan error must be caught, not propagated"
    );
    assert!(
        matches!(results[2].result, Ok(ScanDriveOutcome::Found(_))),
        "drive 3 must still scan after drive 2 failed: {:?}",
        results[2].result
    );

    let any_error = results.iter().any(|r| r.result.is_err());
    assert!(
        any_error,
        "the run's aggregate must reflect drive 2's failure (scan process would exit 1)"
    );
}

#[test]
fn multi_drive_empty_drive_is_reported_distinctly_and_does_not_affect_others() {
    // Verification follow-up: "A detected drive with nothing to import
    // is reported distinctly" had no automated coverage. Drive 2 is
    // detected (MultiDriveSource always detects) but scans to zero
    // groups; drives 1 and 3 have real media. Checked through both
    // scan_drives and import_drives, since the Empty outcome and the
    // "does not affect other drives / does not count as a failure"
    // guarantee apply to both commands identically (design D3, D6).
    let dir = tempfile::tempdir().unwrap();
    let mount_root = dir.path().join("mount");
    let drive1 = mount_root.join("drive1");
    let drive2 = mount_root.join("drive2");
    let drive3 = mount_root.join("drive3");
    fs::create_dir_all(&drive1).unwrap();
    fs::create_dir_all(&drive2).unwrap();
    fs::create_dir_all(&drive3).unwrap();

    let file1 = drive1.join("a.mp4");
    let file3 = drive3.join("c.mp4");
    write_file(&file1, b"drive one footage");
    write_file(&file3, b"drive three footage");

    let dest = dir.path().join("dest");
    let prof = profile(&dest, None, false);

    let mut behavior = HashMap::new();
    behavior.insert(
        drive1.clone(),
        DriveBehavior::Groups(vec![(group("a", vec![media_file(&file1)]), Verdict::Keep)]),
    );
    behavior.insert(drive2.clone(), DriveBehavior::Groups(vec![])); // detected, nothing to import
    behavior.insert(
        drive3.clone(),
        DriveBehavior::Groups(vec![(group("c", vec![media_file(&file3)]), Verdict::Keep)]),
    );
    let source_impl = MultiDriveSource { behavior };

    let drives = plan::resolve_sources(&source_impl, std::slice::from_ref(&mount_root));
    assert_eq!(
        drives.len(),
        3,
        "the empty drive is still detected, not dropped"
    );

    // --- scan ---
    let scan_results = import_videos::scan_drives(
        &prof,
        &source_impl,
        &drives,
        &jiff::tz::TimeZone::UTC,
        report::Detail::Normal,
        false,
    );
    assert!(matches!(
        scan_results[0].result,
        Ok(ScanDriveOutcome::Found(_))
    ));
    assert!(
        matches!(scan_results[1].result, Ok(ScanDriveOutcome::Empty)),
        "drive 2 must be reported as empty, not an error and not skipped: {:?}",
        scan_results[1].result
    );
    assert!(matches!(
        scan_results[2].result,
        Ok(ScanDriveOutcome::Found(_))
    ));
    assert!(
        !scan_results.iter().any(|r| r.result.is_err()),
        "an empty drive must never be recorded as a failure"
    );

    // --- import ---
    let import_results = import_videos::import_drives(
        &prof,
        &source_impl,
        &drives,
        false,
        true,
        false,
        &jiff::tz::TimeZone::UTC,
        report::Detail::Normal,
        false,
    );
    assert!(matches!(
        import_results[0].result,
        Ok(ImportDriveOutcome::Executed {
            any_failed: false,
            ..
        })
    ));
    assert!(
        matches!(import_results[1].result, Ok(ImportDriveOutcome::Empty)),
        "drive 2 must be reported as empty, not an error and not skipped: {:?}",
        import_results[1].result
    );
    assert!(matches!(
        import_results[2].result,
        Ok(ImportDriveOutcome::Executed {
            any_failed: false,
            ..
        })
    ));
    assert!(
        !import_videos::any_import_drive_failed(&import_results),
        "an empty drive must never count toward the run's failure aggregate"
    );

    // Drives 1 and 3 actually transferred, unaffected by drive 2 being empty.
    assert_eq!(
        fs::read(dest.join("1970/1970-01-01/a.mp4")).unwrap(),
        b"drive one footage"
    );
    assert_eq!(
        fs::read(dest.join("1970/1970-01-01/c.mp4")).unwrap(),
        b"drive three footage"
    );
}

#[test]
fn multi_drive_import_prompts_independently_per_drive_when_non_interactive() {
    // Verification follow-up: "Each drive in a multi-drive run prompts
    // independently" / "--yes skips every drive's prompt" had no
    // automated coverage — both earlier multi-drive tests use
    // delete_source: false, so neither ever reaches transfer::execute's
    // confirm() gate. Here delete_source: true and assume_yes: false;
    // the test process's stdin is not a terminal (confirmed via a
    // dedicated probe against this same harness), so each drive
    // independently hits the "non-interactive, no --yes" skip path
    // rather than being asked to confirm or assumed confirmed.
    let dir = tempfile::tempdir().unwrap();
    let mount_root = dir.path().join("mount");
    let drive1 = mount_root.join("drive1");
    let drive2 = mount_root.join("drive2");
    fs::create_dir_all(&drive1).unwrap();
    fs::create_dir_all(&drive2).unwrap();

    let file1 = drive1.join("a.mp4");
    let file2 = drive2.join("b.mp4");
    write_file(&file1, b"drive one footage");
    write_file(&file2, b"drive two footage");

    let dest = dir.path().join("dest");
    let prof = profile(&dest, None, true); // delete_source: true

    let ts1 = ts(0);
    let ts2 = ts(200_000);

    let mut behavior = HashMap::new();
    behavior.insert(
        drive1.clone(),
        DriveBehavior::Groups(vec![(
            group_at("a", vec![media_file(&file1)], ts1),
            Verdict::Keep,
        )]),
    );
    behavior.insert(
        drive2.clone(),
        DriveBehavior::Groups(vec![(
            group_at("b", vec![media_file(&file2)], ts2),
            Verdict::Keep,
        )]),
    );
    let source_impl = MultiDriveSource { behavior };

    let drives = plan::resolve_sources(&source_impl, std::slice::from_ref(&mount_root));
    assert_eq!(drives.len(), 2);

    // assume_yes: false — each drive's deletion gate depends solely on
    // its own confirm() call, never on any other drive's outcome.
    let results = import_videos::import_drives(
        &prof,
        &source_impl,
        &drives,
        false,
        false,
        false,
        &jiff::tz::TimeZone::UTC,
        report::Detail::Normal,
        false,
    );

    assert_eq!(results.len(), 2);
    for (i, r) in results.iter().enumerate() {
        match &r.result {
            Ok(ImportDriveOutcome::Executed { report, .. }) => {
                assert!(
                    report.deletion_skipped_reason.is_some(),
                    "drive {i}'s deletion must be independently skipped (non-interactive, no --yes): {:?}",
                    report.deletion_skipped_reason
                );
                assert!(
                    !report.groups[0].deleted_from_source,
                    "drive {i}'s source must not be deleted without confirmation"
                );
            }
            other => panic!(
                "drive {i} must execute (transfer succeeds; only deletion is gated): {other:?}"
            ),
        }
    }

    // Both files transferred (deletion is what's gated, not the copy)
    // and both sources remain, each drive's own skip independent of
    // the other's.
    let dest1_relative = prof
        .layout
        .resolve(&HashMap::new(), ts1, &jiff::tz::TimeZone::UTC)
        .unwrap();
    let dest2_relative = prof
        .layout
        .resolve(&HashMap::new(), ts2, &jiff::tz::TimeZone::UTC)
        .unwrap();
    assert_eq!(
        fs::read(dest.join(dest1_relative).join("a.mp4")).unwrap(),
        b"drive one footage"
    );
    assert_eq!(
        fs::read(dest.join(dest2_relative).join("b.mp4")).unwrap(),
        b"drive two footage"
    );
    assert!(file1.exists(), "drive 1's source must remain");
    assert!(file2.exists(), "drive 2's source must remain");
}

// --- Multi-drive JSON shape (multi-drive-import design D4, task 4.5) ---

#[test]
fn scan_drive_json_lists_every_drive_with_correct_shape() {
    let tz = jiff::tz::TimeZone::UTC;
    let summary = plan::ScanSummary {
        entries: vec![plan::ScanEntry {
            name: "session".to_string(),
            verdict: Verdict::Keep,
            file_count: 1,
            total_size: 10,
            recorded_at: ts(0),
            files: vec!["/drive/session/clip.mp4".to_string()],
        }],
    };
    let ok_result: error::Result<ScanDriveOutcome> = Ok(ScanDriveOutcome::Found(summary));
    let err_result: error::Result<ScanDriveOutcome> = Err(error::Error::Config("boom".to_string()));

    let doc = report::MultiScanJson {
        drives: vec![
            report::scan_drive_json("DRIVE1", Path::new("/mnt/DRIVE1"), &ok_result, &tz),
            report::scan_drive_json("DRIVE2", Path::new("/mnt/DRIVE2"), &err_result, &tz),
        ],
    };
    let value = serde_json::to_value(&doc).unwrap();
    let drives = value["drives"].as_array().unwrap();
    assert_eq!(
        drives.len(),
        2,
        "drives array must have one entry per drive"
    );

    assert_eq!(drives[0]["name"], "DRIVE1");
    assert_eq!(drives[0]["path"], "/mnt/DRIVE1");
    assert_eq!(drives[0]["status"], "completed");
    assert!(drives[0]["summary"].is_object());
    assert!(
        drives[0].get("error").is_none(),
        "a completed drive's entry must carry no error key"
    );

    assert_eq!(drives[1]["name"], "DRIVE2");
    assert_eq!(drives[1]["status"], "error");
    assert!(drives[1]["error"].as_str().unwrap().contains("boom"));
    assert!(
        drives[1].get("summary").is_none(),
        "an error-status drive's entry must carry no summary key"
    );
}

#[test]
fn import_multi_drive_any_failed_is_true_only_when_a_drive_failed() {
    let ok_report = transfer::ExecuteReport {
        groups: vec![],
        deletion_skipped_reason: None,
        delete_source: false,
    };

    let all_succeeded = vec![
        import_videos::DriveResult {
            name: "d1".to_string(),
            path: PathBuf::from("/mnt/d1"),
            result: Ok(ImportDriveOutcome::Executed {
                report: ok_report.clone(),
                any_failed: false,
            }),
        },
        import_videos::DriveResult {
            name: "d2".to_string(),
            path: PathBuf::from("/mnt/d2"),
            result: Ok(ImportDriveOutcome::Executed {
                report: ok_report.clone(),
                any_failed: false,
            }),
        },
    ];
    assert!(
        !import_videos::any_import_drive_failed(&all_succeeded),
        "any_failed must be false when every drive succeeded"
    );

    let one_failed = vec![
        import_videos::DriveResult {
            name: "d1".to_string(),
            path: PathBuf::from("/mnt/d1"),
            result: Ok(ImportDriveOutcome::Executed {
                report: ok_report.clone(),
                any_failed: false,
            }),
        },
        import_videos::DriveResult {
            name: "d2".to_string(),
            path: PathBuf::from("/mnt/d2"),
            result: Err(error::Error::Config("boom".to_string())),
        },
    ];
    assert!(
        import_videos::any_import_drive_failed(&one_failed),
        "any_failed must be true when any drive failed"
    );
}

// --- Explicit-source JSON stays byte-for-byte unchanged (task 4.4) ---

#[test]
fn explicit_source_scan_json_has_no_drives_key_and_keeps_flat_shape() {
    let dir = tempfile::tempdir().unwrap();
    let source_dir = dir.path().join("card");
    write_file(
        &source_dir.join("DCIM/100GOPRO/GX010001.MP4"),
        b"not an mp4 at all",
    );
    let dest = dir.path().join("dest");
    let config_path = dir.path().join("config.yaml");
    write_config(
        &config_path,
        &format!(
            "profiles:\n  cam:\n    type: gopro\n    require_marker: false\n    source: auto\n    destination: {}\n    layout: \"{{date:%Y}}/{{date:%Y-%m-%d}}\"\n",
            dest.display()
        ),
    );

    let output = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "--json",
            "scan",
            "cam",
            "--source",
            source_dir.to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert!(
        value.get("drives").is_none(),
        "explicit-source JSON must not carry a drives key"
    );
    assert!(
        value.get("entries").is_some(),
        "explicit-source scan JSON keeps its flat entries/summary shape"
    );
    assert!(value.get("summary").is_some());
}
