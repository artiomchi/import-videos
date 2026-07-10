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
