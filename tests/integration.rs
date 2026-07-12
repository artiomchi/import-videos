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
use import_videos::source::{ImportSource, MediaFile, MediaGroup, ScanContext, Verdict};
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

    let report = transfer::execute(&import_plan, false, false, false, &Progress::hidden()).unwrap();

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

    let report = transfer::execute(&import_plan, false, false, false, &Progress::hidden()).unwrap();

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

    let first = transfer::execute(&make_plan(), false, false, false, &Progress::hidden()).unwrap();
    assert!(matches!(
        first.groups[0].files[0].outcome,
        TransferOutcome::Transferred
    ));

    let dest_snapshot = tree_snapshot(&dest);

    let second = transfer::execute(&make_plan(), false, false, false, &Progress::hidden()).unwrap();
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

    let report = transfer::execute(&import_plan, false, false, false, &Progress::hidden()).unwrap();

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

    let report = transfer::execute(&import_plan, true, true, false, &Progress::hidden()).unwrap();

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
    };

    let import_plan = plan::build_plan(
        &prof,
        &source_impl,
        Path::new("/ignored"),
        &jiff::tz::TimeZone::UTC,
        &Progress::hidden(),
    )
    .unwrap();
    let report = transfer::execute(&import_plan, false, false, false, &Progress::hidden()).unwrap();

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
    let report = transfer::execute(&import_plan, true, true, false, &Progress::hidden()).unwrap();

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

    let report = transfer::execute(&import_plan, false, false, true, &Progress::hidden()).unwrap();

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

    let report = transfer::execute(&import_plan, false, false, true, &Progress::hidden()).unwrap();

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
    let report = transfer::execute(&import_plan, true, true, true, &Progress::hidden()).unwrap();

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
