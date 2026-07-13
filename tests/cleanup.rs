//! CLI-level coverage for `cleanup` (openspec/changes/add-maintenance-commands,
//! cli-maintenance spec): exit codes, `--json` output, and the safety
//! refusal, driven through the compiled binary so `lib.rs`'s dispatch
//! (`--older-than` parsing, config loading, JSON printing) is exercised
//! end-to-end, not just the `cleanup` module's own unit tests.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_import-videos"))
}

fn write_config(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn generic_config(dir: &Path, destination: &Path) -> std::path::PathBuf {
    let config_path = dir.join("config.yaml");
    write_config(
        &config_path,
        &format!(
            "profiles:\n  cam:\n    type: generic\n    source: auto\n    destination: {}\n    layout: \"{{date}}\"\n",
            destination.display()
        ),
    );
    config_path
}

#[test]
fn dry_run_prints_plan_and_deletes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("dest");
    let config_path = generic_config(dir.path(), &dest);
    let group_dir = dest.join("_quarantine/group-a");
    fs::create_dir_all(&group_dir).unwrap();
    fs::write(group_dir.join("clip.mp4"), b"footage").unwrap();

    let output = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "cleanup",
            "cam",
            "--dry-run",
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    assert!(group_dir.exists(), "dry-run must not delete anything");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("group-a"));
}

#[test]
fn dry_run_json_emits_one_document() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("dest");
    let config_path = generic_config(dir.path(), &dest);
    let group_dir = dest.join("_quarantine/group-a");
    fs::create_dir_all(&group_dir).unwrap();
    fs::write(group_dir.join("clip.mp4"), b"footage").unwrap();

    let output = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "--json",
            "cleanup",
            "cam",
            "--dry-run",
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(value["entries"][0]["name"], "group-a");
    assert_eq!(value["summary"]["purge_count"], 1);
    assert!(group_dir.exists());
}

#[test]
fn empty_quarantine_reports_nothing_to_clean() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("dest");
    fs::create_dir_all(&dest).unwrap();
    let config_path = generic_config(dir.path(), &dest);

    let output = bin()
        .args(["--config", config_path.to_str().unwrap(), "cleanup", "cam"])
        .stdin(Stdio::null())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.to_lowercase().contains("nothing to clean"));
}

#[test]
fn yes_deletes_purge_candidates() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("dest");
    let config_path = generic_config(dir.path(), &dest);
    let group_dir = dest.join("_quarantine/group-a");
    fs::create_dir_all(&group_dir).unwrap();
    fs::write(group_dir.join("clip.mp4"), b"footage").unwrap();

    let status = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "cleanup",
            "cam",
            "--yes",
        ])
        .stdin(Stdio::null())
        .status()
        .unwrap();

    assert_eq!(status.code(), Some(0));
    assert!(
        !group_dir.exists(),
        "confirmed cleanup must delete the group"
    );
}

#[test]
fn non_interactive_without_yes_exits_nonzero_and_deletes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("dest");
    let config_path = generic_config(dir.path(), &dest);
    let group_dir = dest.join("_quarantine/group-a");
    fs::create_dir_all(&group_dir).unwrap();
    fs::write(group_dir.join("clip.mp4"), b"footage").unwrap();

    let status = bin()
        .args(["--config", config_path.to_str().unwrap(), "cleanup", "cam"])
        .stdin(Stdio::null())
        .status()
        .unwrap();

    assert_ne!(status.code(), Some(0));
    assert!(group_dir.exists(), "nothing must be deleted without --yes");
}

#[test]
fn invalid_older_than_span_exits_2() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("dest");
    let config_path = generic_config(dir.path(), &dest);
    let group_dir = dest.join("_quarantine/group-a");
    fs::create_dir_all(&group_dir).unwrap();
    fs::write(group_dir.join("clip.mp4"), b"footage").unwrap();

    let status = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "cleanup",
            "cam",
            "--older-than",
            "banana",
        ])
        .stdin(Stdio::null())
        .status()
        .unwrap();

    assert_eq!(status.code(), Some(2));
    assert!(group_dir.exists(), "a parse error must delete nothing");
}

// --- --summary (add-summary-flag, task 7.2) ---

#[test]
fn dry_run_summary_omits_per_entry_lines() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("dest");
    let config_path = generic_config(dir.path(), &dest);
    let group_dir = dest.join("_quarantine/group-a");
    fs::create_dir_all(&group_dir).unwrap();
    fs::write(group_dir.join("clip.mp4"), b"footage").unwrap();

    let output = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "cleanup",
            "cam",
            "--dry-run",
            "--summary",
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    assert!(group_dir.exists(), "dry-run must not delete anything");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("group-a"),
        "per-entry [PURGE]/[KEEP] lines must be omitted: {stdout}"
    );
    assert!(stdout.contains("Summary: 1 to purge"), "got: {stdout}");
}

#[test]
fn yes_summary_tallies_deletions_but_still_names_a_failure() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("dest");
    let config_path = generic_config(dir.path(), &dest);
    let quarantine = dest.join("_quarantine");
    let ok_group = quarantine.join("ok-group");
    let broken_group = quarantine.join("broken-group");
    fs::create_dir_all(&ok_group).unwrap();
    fs::create_dir_all(&broken_group).unwrap();
    fs::write(ok_group.join("clip.mp4"), b"footage").unwrap();
    fs::write(broken_group.join("clip.mp4"), b"footage").unwrap();

    // Strip write permission on broken-group so deleting its contents
    // fails, without preventing the directory listing itself from
    // succeeding (mirrors the permission-injection pattern used for
    // transfer failures in tests/integration.rs).
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&broken_group, fs::Permissions::from_mode(0o500)).unwrap();

    let output = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "cleanup",
            "cam",
            "--yes",
            "--summary",
        ])
        .stdin(Stdio::null())
        .output();

    // Restore permissions before any assertion can early-return, so the
    // tempdir can always be cleaned up.
    fs::set_permissions(&broken_group, fs::Permissions::from_mode(0o700)).unwrap();
    let output = output.unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(!ok_group.exists(), "the healthy entry must be deleted");
    assert!(
        broken_group.exists(),
        "the entry whose deletion failed must remain"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("deleted: "),
        "per-entry deleted lines must be omitted under --summary: {stdout}"
    );
    assert!(
        stdout.contains("Summary: 1 entry deleted"),
        "the closing tally must count the successful deletion: {stdout}"
    );
    assert!(
        stdout.contains("FAILED to delete") && stdout.contains("broken-group"),
        "the failed entry must still be individually named: {stdout}"
    );
}

#[test]
fn quarantine_root_equal_to_destination_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("dest");
    fs::create_dir_all(&dest).unwrap();
    let config_path = dir.path().join("config.yaml");
    write_config(
        &config_path,
        &format!(
            "profiles:\n  cam:\n    type: generic\n    source: auto\n    destination: {}\n    layout: \"{{date}}\"\n    quarantine: {}\n",
            dest.display(),
            dest.display()
        ),
    );

    let status = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "cleanup",
            "cam",
            "--yes",
        ])
        .stdin(Stdio::null())
        .status()
        .unwrap();

    assert_eq!(status.code(), Some(2));
}
