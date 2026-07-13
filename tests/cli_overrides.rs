//! End-to-end coverage of
//! `openspec/changes/add-cli-overrides/specs/{cli-core,gopro-import}/spec.md`,
//! driven through the compiled binary against a fake HERO8 card (mirrors
//! `tests/gopro_import.rs`'s handcrafted MP4 fixtures — no binary
//! fixtures checked into the repo).

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

// --- Handcrafted MP4 byte fixtures (mirrors src/media/mp4.rs's test helpers) ---

fn make_box(fourcc: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(8 + payload.len());
    buf.extend_from_slice(&((8 + payload.len()) as u32).to_be_bytes());
    buf.extend_from_slice(fourcc);
    buf.extend_from_slice(payload);
    buf
}

fn make_container(fourcc: &[u8; 4], children: &[Vec<u8>]) -> Vec<u8> {
    make_box(fourcc, &children.concat())
}

fn hmmt_payload(offsets: &[u32]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(4 + offsets.len() * 4);
    payload.extend_from_slice(&(offsets.len() as u32).to_be_bytes());
    for offset in offsets {
        payload.extend_from_slice(&offset.to_be_bytes());
    }
    payload
}

fn mvhd_v0(creation_time: u32) -> Vec<u8> {
    let mut payload = vec![0u8; 4]; // version 0 + flags
    payload.extend_from_slice(&creation_time.to_be_bytes());
    make_box(b"mvhd", &payload)
}

fn chapter_bytes(creation_time: u32, marker_offsets_ms: &[u32]) -> Vec<u8> {
    let mut children = vec![mvhd_v0(creation_time)];
    if !marker_offsets_ms.is_empty() {
        let hmmt = make_box(b"HMMT", &hmmt_payload(marker_offsets_ms));
        children.push(make_container(b"udta", &[hmmt]));
    }
    make_container(b"moov", &children)
}

/// Unix seconds for 2026-07-09T00:00:00Z, converted to the MP4/
/// QuickTime 1904 epoch `mvhd` expects.
fn creation_time_2026_07_09() -> u32 {
    let unix_secs = jiff::civil::date(2026, 7, 9)
        .at(0, 0, 0, 0)
        .to_zoned(jiff::tz::TimeZone::UTC)
        .unwrap()
        .timestamp()
        .as_second();
    (unix_secs + 2_082_844_800) as u32
}

fn write_chapter(path: &Path, creation_time: u32, marker_offsets_ms: &[u32]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, chapter_bytes(creation_time, marker_offsets_ms)).unwrap();
}

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_import-videos"))
}

fn write_config(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn gopro_config(config_path: &Path, destination: &Path, extra: &str) {
    write_config(
        config_path,
        &format!(
            "timezone: UTC\nprofiles:\n  gopro:\n    type: gopro\n    source: auto\n    destination: {}\n    layout: \"{{date:%Y}}/{{date:%Y-%m-%d}}\"\n{extra}",
            destination.display()
        ),
    );
}

/// A card with one marked (session-0123, Keep) and one unmarked
/// (session-0124, Quarantine) session.
fn write_card(card: &Path) {
    let creation_time = creation_time_2026_07_09();
    write_chapter(
        &card.join("DCIM/100GOPRO/GX010123.MP4"),
        creation_time,
        &[5000],
    );
    write_chapter(&card.join("DCIM/100GOPRO/GX010124.MP4"), creation_time, &[]);
}

// --- 3.1: --delete-source forces deletion on against a safe profile; --yes gates it ---

#[test]
fn delete_source_forces_deletion_on_with_yes_skips_without() {
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card");
    write_card(&card);

    let dest = dir.path().join("dest");
    let config_path = dir.path().join("config.yaml");
    gopro_config(&config_path, &dest, "    delete_source: false\n");

    let status = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "import",
            "gopro",
            "--source",
            card.to_str().unwrap(),
            "--delete-source",
            "--yes",
        ])
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0));
    assert!(
        !card.join("DCIM/100GOPRO/GX010123.MP4").exists(),
        "--delete-source --yes must delete a verified source even though the profile is safe"
    );

    // Second card: same override, but no --yes and stdin is not a
    // terminal (Stdio::null()) — deletion must be skipped, not assumed.
    let card2 = dir.path().join("card2");
    write_card(&card2);
    let dest2 = dir.path().join("dest2");
    gopro_config(&config_path, &dest2, "    delete_source: false\n");

    let output = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "import",
            "gopro",
            "--source",
            card2.to_str().unwrap(),
            "--delete-source",
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert!(
        card2.join("DCIM/100GOPRO/GX010123.MP4").exists(),
        "without --yes on non-interactive stdin, forced deletion must be skipped"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("skipping source deletion") || stdout.contains("not deleted"),
        "must explain that deletion was skipped: {stdout}"
    );
}

// --- 3.2: --no-delete-source and its hidden --keep-source alias both prevent deletion ---

#[test]
fn no_delete_source_and_keep_source_alias_both_prevent_deletion() {
    for flag in ["--no-delete-source", "--keep-source"] {
        let dir = tempfile::tempdir().unwrap();
        let card = dir.path().join("card");
        write_card(&card);

        let dest = dir.path().join("dest");
        let config_path = dir.path().join("config.yaml");
        gopro_config(&config_path, &dest, "    delete_source: true\n");

        let status = bin()
            .args([
                "--config",
                config_path.to_str().unwrap(),
                "import",
                "gopro",
                "--source",
                card.to_str().unwrap(),
                flag,
                "--yes",
            ])
            .stdin(Stdio::null())
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(0));

        assert!(
            card.join("DCIM/100GOPRO/GX010123.MP4").exists(),
            "{flag} must prevent source deletion even though the profile requests it and confirms"
        );
        assert!(
            card.join("DCIM/100GOPRO/GX010124.MP4").exists(),
            "{flag} must prevent source deletion for quarantined sessions too"
        );
    }
}

// --- 3.3: --no-copy-quarantine / --copy-quarantine / --quarantine override copy_quarantine ---

#[test]
fn no_copy_quarantine_disables_the_copy_on_an_enabled_profile() {
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card");
    write_card(&card);

    let dest = dir.path().join("dest");
    let config_path = dir.path().join("config.yaml");
    gopro_config(&config_path, &dest, "");

    let status = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "import",
            "gopro",
            "--source",
            card.to_str().unwrap(),
            "--no-copy-quarantine",
            "--yes",
        ])
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0));

    assert!(
        !dest.join("_quarantine").exists(),
        "--no-copy-quarantine must prevent the quarantine directory from being created"
    );
    assert!(
        card.join("DCIM/100GOPRO/GX010124.MP4").exists(),
        "the unmarked session's source must stay on the card"
    );
}

#[test]
fn copy_quarantine_reenables_the_copy_on_a_disabled_profile() {
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card");
    write_card(&card);

    let dest = dir.path().join("dest");
    let config_path = dir.path().join("config.yaml");
    gopro_config(&config_path, &dest, "    copy_quarantine: false\n");

    let status = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "import",
            "gopro",
            "--source",
            card.to_str().unwrap(),
            "--copy-quarantine",
            "--yes",
        ])
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0));

    assert!(
        dest.join("_quarantine/session-0124/GX010124.MP4").exists(),
        "--copy-quarantine must re-enable the copy even though the profile disables it"
    );
}

#[test]
fn quarantine_path_redirects_and_forces_copying_on() {
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card");
    write_card(&card);

    let dest = dir.path().join("dest");
    let redirected = dir.path().join("elsewhere");
    let config_path = dir.path().join("config.yaml");
    // copy_quarantine: false in the profile — --quarantine must force it on.
    gopro_config(&config_path, &dest, "    copy_quarantine: false\n");

    let status = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "import",
            "gopro",
            "--source",
            card.to_str().unwrap(),
            "--quarantine",
            redirected.to_str().unwrap(),
            "--yes",
        ])
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0));

    assert!(
        redirected.join("session-0124/GX010124.MP4").exists(),
        "--quarantine must redirect the copy to the given path and force copying on"
    );
    assert!(
        !dest.join("_quarantine").exists(),
        "the profile's default quarantine directory must not be used"
    );
}

// --- 3.4: --quarantine + --no-copy-quarantine is a usage error; scan previews the override ---

#[test]
fn quarantine_and_no_copy_quarantine_is_a_usage_error_before_scanning() {
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card");
    write_card(&card);

    let dest = dir.path().join("dest");
    let config_path = dir.path().join("config.yaml");
    gopro_config(&config_path, &dest, "");

    let card_before = fs::read_dir(card.join("DCIM/100GOPRO")).unwrap().count();

    let output = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "import",
            "gopro",
            "--source",
            card.to_str().unwrap(),
            "--quarantine",
            "/tmp/q",
            "--no-copy-quarantine",
            "--yes",
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(
        !dest.exists(),
        "a contradictory override must fail before any scanning or filesystem changes"
    );
    assert_eq!(
        fs::read_dir(card.join("DCIM/100GOPRO")).unwrap().count(),
        card_before,
        "the card must be untouched"
    );
}

#[test]
fn scan_quarantine_override_is_a_usage_error() {
    // improve-scan-and-cleanup design D1/D7: scan never resolves or
    // shows a quarantine path, so --quarantine (and --copy-quarantine/
    // --no-copy-quarantine) moved off scan's flag set entirely — this
    // now fails to parse instead of being silently previewed.
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card");
    write_card(&card);

    let dest = dir.path().join("dest");
    let redirected = dir.path().join("preview-only");
    let config_path = dir.path().join("config.yaml");
    gopro_config(&config_path, &dest, "");

    let output = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "scan",
            "gopro",
            "--source",
            card.to_str().unwrap(),
            "--quarantine",
            redirected.to_str().unwrap(),
            "-v",
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(!dest.exists(), "scan must not create the destination");
    assert!(
        !redirected.exists(),
        "scan must not create the overridden quarantine directory"
    );
}

// --- 3.5: --gopro-require-marker / --no-gopro-require-marker; rejected on non-GoPro profiles ---

#[test]
fn no_gopro_require_marker_keeps_an_unmarked_session() {
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card");
    write_card(&card);

    let dest = dir.path().join("dest");
    let config_path = dir.path().join("config.yaml");
    gopro_config(&config_path, &dest, "");

    // Spec scenario is phrased as `scan --no-gopro-require-marker`: the
    // plan must show the unmarked session as Keep, and nothing on disk
    // changes.
    let scan_output = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "scan",
            "gopro",
            "--source",
            card.to_str().unwrap(),
            "--no-gopro-require-marker",
            "-v",
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(scan_output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&scan_output.stdout);
    assert!(
        stdout.contains("[KEEP] session-0124"),
        "scan --no-gopro-require-marker must show the unmarked session as Keep: {stdout}"
    );
    assert!(!dest.exists(), "scan must not create the destination");

    let status = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "import",
            "gopro",
            "--source",
            card.to_str().unwrap(),
            "--no-gopro-require-marker",
            "--yes",
        ])
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0));

    assert!(
        dest.join("2026/2026-07-09/GX010124.MP4").exists(),
        "--no-gopro-require-marker must keep the unmarked session instead of quarantining it"
    );
}

#[test]
fn gopro_require_marker_quarantines_an_unmarked_session_on_a_lenient_profile() {
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card");
    write_card(&card);

    let dest = dir.path().join("dest");
    let config_path = dir.path().join("config.yaml");
    gopro_config(&config_path, &dest, "    require_marker: false\n");

    let status = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "import",
            "gopro",
            "--source",
            card.to_str().unwrap(),
            "--gopro-require-marker",
            "--yes",
        ])
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0));

    assert!(
        dest.join("_quarantine/session-0124/GX010124.MP4").exists(),
        "--gopro-require-marker must quarantine the unmarked session even though the profile is lenient"
    );
}

#[test]
fn marker_flags_rejected_on_a_non_gopro_profile() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("dest");
    let config_path = dir.path().join("config.yaml");
    write_config(
        &config_path,
        &format!(
            "timezone: UTC\nprofiles:\n  tesla:\n    type: tesla\n    source: auto\n    destination: {}\n    layout: \"{{event_type}}\"\n",
            dest.display()
        ),
    );

    for flag in ["--gopro-require-marker", "--no-gopro-require-marker"] {
        let output = bin()
            .args([
                "--config",
                config_path.to_str().unwrap(),
                "import",
                "tesla",
                flag,
                "--yes",
            ])
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("require_marker is only valid for profiles of type gopro"),
            "{flag}: expected the config loader's wording, got: {stderr}"
        );
        assert!(!dest.exists(), "the run must fail before any scanning");
    }
}

// --- design D2: --keep-source is a hidden alias, not a documented flag ---

#[test]
fn keep_source_is_hidden_from_help_but_no_delete_source_is_documented() {
    let output = bin().args(["import", "--help"]).output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--no-delete-source"),
        "--no-delete-source must be documented in --help: {stdout}"
    );
    assert!(
        !stdout.contains("--keep-source"),
        "--keep-source must stay hidden from --help (design D2): {stdout}"
    );
}
