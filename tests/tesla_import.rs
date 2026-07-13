//! End-to-end coverage of the scenarios in
//! `openspec/changes/add-tesla-import/specs/tesla-import/spec.md`,
//! driven through the compiled binary against a fake TeslaCam drive
//! built in a tempdir. Tesla layouts are pure directories + JSON +
//! arbitrary bytes, so no binary fixtures are needed (unlike GoPro).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_import-videos"))
}

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn event_json(timestamp: &str, reason: &str) -> String {
    format!(
        r#"{{"timestamp":"{timestamp}","city":"London","est_lat":"51.5012","est_lon":"-0.1246","reason":"{reason}","camera":"0"}}"#
    )
}

fn tesla_config(config_path: &Path, destination: &Path, extra: &str) {
    write(
        config_path,
        &format!(
            "timezone: UTC\nprofiles:\n  tesla:\n    type: tesla\n    source: auto\n    destination: {}\n    layout: \"{{event_type}}/{{date:%Y-%m-%d}}/{{date:%H-%M-%S}}\"\n{extra}",
            destination.display()
        ),
    );
}

fn tree_snapshot(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if !root.exists() {
        return out;
    }
    for entry in fs::read_dir(root).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(tree_snapshot(&path));
        } else {
            out.push(path);
        }
    }
    out.sort();
    out
}

// --- 7.2: detection ---

#[test]
fn detects_teslacam_and_rejects_other_layouts() {
    let dir = tempfile::tempdir().unwrap();

    let teslacam_card = dir.path().join("teslacam_card");
    write(
        &teslacam_card.join("TeslaCam/SavedClips/2026-07-04_18-23-51/event.json"),
        &event_json("2026-07-04T18:23:51", "user_interaction_honk"),
    );

    let bare_card = dir.path().join("bare_card");
    fs::create_dir_all(bare_card.join("TeslaCam")).unwrap();

    let empty_card = dir.path().join("empty_card");
    fs::create_dir_all(&empty_card).unwrap();

    let gopro_card = dir.path().join("gopro_card");
    fs::create_dir_all(gopro_card.join("DCIM/100GOPRO")).unwrap();

    let dest = dir.path().join("dest");
    let config_path = dir.path().join("config.yaml");
    tesla_config(&config_path, &dest, "");

    for (card, should_find) in [
        (&teslacam_card, true),
        (&bare_card, false),
        (&empty_card, false),
        (&gopro_card, false),
    ] {
        let output = bin()
            .args([
                "--config",
                config_path.to_str().unwrap(),
                "scan",
                "tesla",
                "--source",
                card.to_str().unwrap(),
            ])
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(
            stdout.contains("KEEP"),
            should_find,
            "unexpected scan output for {card:?}: {stdout}"
        );
    }
}

// --- 7.3: end-to-end import, sidecar, deletion only after verify ---

#[test]
fn event_folder_imports_as_one_unit_with_sidecar() {
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card");
    let event_dir = card.join("TeslaCam/SavedClips/2026-07-04_18-23-51");

    write(
        &event_dir.join("event.json"),
        &event_json("2026-07-04T18:23:51", "user_interaction_honk"),
    );
    write(&event_dir.join("thumb.png"), "thumb-bytes");
    write(&event_dir.join("2026-07-04_18-18-32-front.mp4"), "front");
    write(&event_dir.join("2026-07-04_18-18-32-back.mp4"), "back");

    let dest = dir.path().join("dest");
    let config_path = dir.path().join("config.yaml");
    tesla_config(&config_path, &dest, "    delete_source: true\n");

    let status = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "import",
            "tesla",
            "--source",
            card.to_str().unwrap(),
            "--yes",
        ])
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0));

    let landing = dest.join("saved/2026-07-04/18-23-51");
    assert!(landing.join("event.json").exists());
    assert!(landing.join("thumb.png").exists());
    assert!(landing.join("2026-07-04_18-18-32-front.mp4").exists());
    assert!(landing.join("2026-07-04_18-18-32-back.mp4").exists());
    assert!(landing.join("import.json").exists());

    let sidecar: serde_json::Value =
        serde_json::from_slice(&fs::read(landing.join("import.json")).unwrap()).unwrap();
    assert_eq!(sidecar["camera"], "tesla");
    assert_eq!(sidecar["events"][0]["type"], "tesla:saved");
    assert_eq!(sidecar["events"][0]["reason"], "user_interaction_honk");
    assert_eq!(sidecar["time_source"], "event_json");

    assert!(
        !event_dir.exists() || fs::read_dir(&event_dir).unwrap().next().is_none(),
        "source event folder must be cleaned once verified-imported, with delete_source + --yes"
    );
}

// --- 7.2 (read-only) / 4.2: scan is read-only ---

#[test]
fn scan_is_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card");
    let event_dir = card.join("TeslaCam/SavedClips/2026-07-04_18-23-51");
    write(
        &event_dir.join("event.json"),
        &event_json("2026-07-04T18:23:51", "user_interaction_honk"),
    );
    write(&event_dir.join("2026-07-04_18-18-32-front.mp4"), "front");

    let card_before = tree_snapshot(&card);
    let dest = dir.path().join("dest");
    let config_path = dir.path().join("config.yaml");
    tesla_config(&config_path, &dest, "    delete_source: true\n");

    let output = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "scan",
            "tesla",
            "--source",
            card.to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert!(!dest.exists(), "scan must not create the destination");
    assert_eq!(tree_snapshot(&card), card_before);
}

// --- improve-scan-and-cleanup design D5/D6, task 7.5: deleted event never resurfaces ---

#[test]
fn deleted_saved_clips_event_does_not_resurface_on_a_later_scan_or_import() {
    // After import + delete removes a SavedClips event's directory
    // entirely (directory pruning, design D6), a later scan/import must
    // never report a phantom 0-file group for it — whether because
    // pruning removed the directory outright, or (were it to somehow
    // survive) because the zero-file-group filter (design D5) catches
    // it as a device-agnostic backstop.
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card");
    let event_dir = card.join("TeslaCam/SavedClips/2026-07-04_18-23-51");
    write(
        &event_dir.join("event.json"),
        &event_json("2026-07-04T18:23:51", "user_interaction_honk"),
    );
    write(&event_dir.join("2026-07-04_18-18-32-front.mp4"), "front");

    let dest = dir.path().join("dest");
    let config_path = dir.path().join("config.yaml");
    tesla_config(&config_path, &dest, "    delete_source: true\n");

    let status = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "import",
            "tesla",
            "--source",
            card.to_str().unwrap(),
            "--yes",
        ])
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0));
    assert!(
        !event_dir.exists(),
        "the emptied event folder must be pruned after verified deletion"
    );

    let scan_output = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "scan",
            "tesla",
            "--source",
            card.to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(scan_output.status.code(), Some(0));
    let scan_stdout = String::from_utf8_lossy(&scan_output.stdout);
    assert!(
        !scan_stdout.contains("2026-07-04_18-23-51"),
        "a deleted event must never resurface as a phantom group on scan: {scan_stdout}"
    );

    let import_output = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "import",
            "tesla",
            "--source",
            card.to_str().unwrap(),
            "--dry-run",
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(import_output.status.code(), Some(0));
    let import_stdout = String::from_utf8_lossy(&import_output.stdout);
    assert!(
        !import_stdout.contains("2026-07-04_18-23-51"),
        "a deleted event must never resurface as a phantom group on import: {import_stdout}"
    );
}

// --- 7.4: category and reason filtering yield visible Ignore, never quarantine ---

#[test]
fn disabled_category_and_denied_reason_are_ignored_not_touched() {
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card");

    let saved_dir = card.join("TeslaCam/SavedClips/2026-07-04_18-23-51");
    write(
        &saved_dir.join("event.json"),
        &event_json("2026-07-04T18:23:51", "user_interaction_honk"),
    );

    let sentry_dir = card.join("TeslaCam/SentryClips/2026-07-04_19-00-00");
    write(
        &sentry_dir.join("event.json"),
        &event_json("2026-07-04T19:00:00", "sentry_aware_object_detection"),
    );

    let dest = dir.path().join("dest");
    let config_path = dir.path().join("config.yaml");
    tesla_config(
        &config_path,
        &dest,
        "    events: [saved, sentry]\n    reasons:\n      deny: [sentry_aware_object_detection]\n",
    );

    let output = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "scan",
            "tesla",
            "--source",
            card.to_str().unwrap(),
            "-v",
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("IGNORE"));
    assert!(stdout.contains("sentry_aware_object_detection"));
    assert!(!stdout.contains("QUARANTINE"), "Tesla never quarantines");

    let import_status = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "import",
            "tesla",
            "--source",
            card.to_str().unwrap(),
            "--yes",
        ])
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert_eq!(import_status.code(), Some(0));

    assert!(
        sentry_dir.join("event.json").exists(),
        "denied event must remain untouched on the card"
    );
    assert!(
        !dest.join("_quarantine").exists(),
        "Tesla import must never write a quarantine directory"
    );
    assert!(dest.join("saved/2026-07-04/18-23-51/event.json").exists());
}

// --- 7.5: degraded metadata ---

#[test]
fn corrupt_event_json_falls_back_to_folder_name_timestamp() {
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card");
    let event_dir = card.join("TeslaCam/SavedClips/2026-07-04_18-23-51");
    write(&event_dir.join("event.json"), "{not valid json at all");
    write(&event_dir.join("2026-07-04_18-18-32-front.mp4"), "front");

    let dest = dir.path().join("dest");
    let config_path = dir.path().join("config.yaml");
    tesla_config(&config_path, &dest, "");

    let status = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "import",
            "tesla",
            "--source",
            card.to_str().unwrap(),
            "--yes",
        ])
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0));

    let landing = dest.join("saved/2026-07-04/18-23-51");
    assert!(
        landing.join("2026-07-04_18-18-32-front.mp4").exists(),
        "corrupt event.json still keeps the event via the folder-name timestamp"
    );
    let sidecar: serde_json::Value =
        serde_json::from_slice(&fs::read(landing.join("import.json")).unwrap()).unwrap();
    assert_eq!(sidecar["time_source"], "folder_name");
}

#[test]
fn unparseable_folder_and_missing_timestamp_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card");
    let event_dir = card.join("TeslaCam/SavedClips/not-a-timestamp");
    write(&event_dir.join("event.json"), "{not valid json at all");
    write(&event_dir.join("clip.mp4"), "clip");

    let dest = dir.path().join("dest");
    let config_path = dir.path().join("config.yaml");
    tesla_config(&config_path, &dest, "    delete_source: true\n");

    let output = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "scan",
            "tesla",
            "--source",
            card.to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("unparseable event folder"));

    let status = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "import",
            "tesla",
            "--source",
            card.to_str().unwrap(),
            "--yes",
        ])
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0));
    assert!(
        event_dir.join("clip.mp4").exists(),
        "an event that can't be timestamped at all is left on the card"
    );
}

// --- 7.6: RecentClips opt-in ---

#[test]
fn recent_clips_skipped_by_default_then_clustered_when_enabled() {
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card");
    let recent = card.join("TeslaCam/RecentClips");
    for angle in ["front", "back"] {
        write(
            &recent.join(format!("2026-07-04_18-40-00-{angle}.mp4")),
            angle,
        );
        write(
            &recent.join(format!("2026-07-04_18-41-00-{angle}.mp4")),
            angle,
        );
    }

    let dest = dir.path().join("dest");
    let config_path = dir.path().join("config.yaml");
    tesla_config(&config_path, &dest, "");

    let default_output = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "scan",
            "tesla",
            "--source",
            card.to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(default_output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&default_output.stdout);
    assert!(
        stdout.contains("no media found") || !stdout.contains("KEEP"),
        "RecentClips must be skipped by default: {stdout}"
    );

    let enabled_config = dir.path().join("config_enabled.yaml");
    tesla_config(
        &enabled_config,
        &dest,
        "    events: [saved, sentry, recent]\n",
    );

    let status = bin()
        .args([
            "--config",
            enabled_config.to_str().unwrap(),
            "import",
            "tesla",
            "--source",
            card.to_str().unwrap(),
            "--yes",
        ])
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0));

    assert!(
        dest.join("recent/2026-07-04/18-40-00/2026-07-04_18-40-00-front.mp4")
            .exists()
    );
    assert!(
        dest.join("recent/2026-07-04/18-41-00/2026-07-04_18-41-00-back.mp4")
            .exists()
    );
}

// --- 7.7: mtime stamping in the system timezone ---

#[test]
fn clip_mtimes_match_their_own_stems() {
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card");
    let event_dir = card.join("TeslaCam/SavedClips/2026-07-04_18-23-51");
    write(
        &event_dir.join("event.json"),
        &event_json("2026-07-04T18:23:51", "user_interaction_honk"),
    );
    write(&event_dir.join("2026-07-04_18-18-32-front.mp4"), "front");

    let dest = dir.path().join("dest");
    let config_path = dir.path().join("config.yaml");
    tesla_config(&config_path, &dest, "");

    let status = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "import",
            "tesla",
            "--source",
            card.to_str().unwrap(),
            "--yes",
        ])
        .env("TZ", "UTC")
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0));

    let clip = dest.join("saved/2026-07-04/18-23-51/2026-07-04_18-18-32-front.mp4");
    let mtime = fs::metadata(&clip).unwrap().modified().unwrap();
    let expected: std::time::SystemTime = "2026-07-04T18:18:32Z"
        .parse::<jiff::Timestamp>()
        .unwrap()
        .into();
    assert_eq!(
        mtime, expected,
        "clip mtime should match its own filename stem, resolved in UTC"
    );
}

// --- --json output (add-maintenance-commands, task 2.4) ---

#[test]
fn import_json_yes_emits_one_document_with_per_file_outcomes() {
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card");
    let event_dir = card.join("TeslaCam/SavedClips/2026-07-04_18-23-51");
    write(
        &event_dir.join("event.json"),
        &event_json("2026-07-04T18:23:51", "user_interaction_honk"),
    );
    write(&event_dir.join("2026-07-04_18-18-32-front.mp4"), "front");

    let dest = dir.path().join("dest");
    let config_path = dir.path().join("config.yaml");
    tesla_config(&config_path, &dest, "    delete_source: true\n");

    let output = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "--json",
            "import",
            "tesla",
            "--source",
            card.to_str().unwrap(),
            "--yes",
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));

    // Exactly one JSON document on stdout: parsing the whole of stdout
    // as a single value fails if there's any extra text before/after it.
    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!("stdout was not a single JSON document: {e}\nstdout was:\n{stdout}")
    });

    assert_eq!(value["summary"]["transferred"], 2); // event.json + the clip
    let group = &value["groups"][0];
    assert_eq!(group["group"], "saved-2026-07-04_18-23-51");
    assert_eq!(group["deleted_from_source"], true);
    assert!(
        group["files"]
            .as_array()
            .unwrap()
            .iter()
            .all(|f| f["outcome"] == "transferred")
    );

    // Confirmation rules are unchanged under --json: --yes was required
    // and honored, matching non-JSON `import --yes` behavior.
    assert!(
        !event_dir.join("event.json").exists(),
        "source must be deleted after --yes"
    );
}

#[test]
fn scan_json_no_sources_is_a_json_document_not_a_bare_string() {
    let dir = tempfile::tempdir().unwrap();
    let source_dir = dir.path().join("empty-card");
    fs::create_dir_all(&source_dir).unwrap();
    let dest = dir.path().join("dest");
    let config_path = dir.path().join("config.yaml");
    tesla_config(&config_path, &dest, "");

    let output = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "--json",
            "scan",
            "tesla",
            "--source",
            source_dir.to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(value["status"], "no_sources");
    assert_eq!(value["profile"], "tesla");
}
