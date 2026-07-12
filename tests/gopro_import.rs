//! End-to-end coverage of the scenarios in
//! `openspec/changes/add-gopro-import/specs/gopro-import/spec.md`,
//! driven through the compiled binary against a fake HERO8 card built
//! in a tempdir (real directory layout, handcrafted MP4 bytes — no
//! binary fixtures checked into the repo).

use std::fs;
use std::path::{Path, PathBuf};
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

/// A chapter file's full byte content: `moov` containing `mvhd` (so
/// the session gets a real camera-clock timestamp) and, if any
/// offsets are given, `udta/HMMT` with those HiLight markers.
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

// --- 4.1: end-to-end keep/quarantine, source deleted only with delete_source + --yes ---

#[test]
fn marked_session_kept_unmarked_session_quarantined_and_source_cleaned() {
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card");
    let creation_time = creation_time_2026_07_09();

    write_chapter(
        &card.join("DCIM/100GOPRO/GX010123.MP4"),
        creation_time,
        &[5000],
    );
    write_chapter(&card.join("DCIM/100GOPRO/GX010124.MP4"), creation_time, &[]);

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
            "--yes",
        ])
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0));

    let kept = dest.join("2026/2026-07-09/GX010123.MP4");
    assert!(
        kept.exists(),
        "marked session should land under the date layout"
    );
    assert!(
        dest.join("2026/2026-07-09/import.json").exists(),
        "kept session should get a import.json sidecar"
    );

    let quarantined = dest.join("_quarantine/session-0124/GX010124.MP4");
    assert!(
        quarantined.exists(),
        "unmarked session should land in quarantine, not be deleted"
    );
    assert!(
        !dest.join("_quarantine/session-0124/import.json").exists(),
        "quarantined sessions get no sidecar"
    );

    assert!(
        !card.join("DCIM/100GOPRO/GX010123.MP4").exists(),
        "source must be cleaned once verified-imported, with delete_source + --yes"
    );
    assert!(
        !card.join("DCIM/100GOPRO/GX010124.MP4").exists(),
        "quarantine is a verified copy too, so its source is also cleaned"
    );
}

// --- 4.2: scan / dry-run never touch the filesystem ---

#[test]
fn scan_and_dry_run_are_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card");
    let creation_time = creation_time_2026_07_09();
    write_chapter(
        &card.join("DCIM/100GOPRO/GX010123.MP4"),
        creation_time,
        &[5000],
    );

    let card_before = tree_snapshot(&card);
    let dest = dir.path().join("dest");
    let config_path = dir.path().join("config.yaml");
    gopro_config(&config_path, &dest, "    delete_source: true\n");

    let scan_output = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "scan",
            "gopro",
            "--source",
            card.to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(scan_output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&scan_output.stdout);
    assert!(stdout.contains("KEEP"));
    assert!(stdout.contains("session-0123"));
    assert!(!dest.exists(), "scan must not create the destination");
    assert_eq!(tree_snapshot(&card), card_before);

    let dry_run_status = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "import",
            "gopro",
            "--source",
            card.to_str().unwrap(),
            "--dry-run",
        ])
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert_eq!(dry_run_status.code(), Some(0));
    assert!(!dest.exists(), "dry-run must not create the destination");
    assert_eq!(tree_snapshot(&card), card_before);
}

// --- add-scan-progress: piped/--json output stays clean ---

#[test]
fn piped_scan_and_dry_run_import_produce_no_progress_bytes() {
    // Spec: "Piped or JSON output stays clean" — a `Command::output()`
    // capture is inherently non-interactive (stdout is a pipe, not a
    // TTY), so the scan-phase `Progress` must construct no bar and the
    // captured stdout must carry no escape/terminal-control bytes,
    // whether or not `--json` is also passed.
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card");
    let creation_time = creation_time_2026_07_09();
    write_chapter(
        &card.join("DCIM/100GOPRO/GX010123.MP4"),
        creation_time,
        &[5000],
    );

    let dest = dir.path().join("dest");
    let config_path = dir.path().join("config.yaml");
    gopro_config(&config_path, &dest, "");

    for extra_args in [vec![], vec!["--json".to_string()]] {
        let mut args = vec![
            "--config".to_string(),
            config_path.to_str().unwrap().to_string(),
            "scan".to_string(),
            "gopro".to_string(),
            "--source".to_string(),
            card.to_str().unwrap().to_string(),
        ];
        args.extend(extra_args.clone());
        let scan_output = bin().args(&args).stdin(Stdio::null()).output().unwrap();
        assert_eq!(scan_output.status.code(), Some(0));
        assert!(
            !scan_output.stdout.contains(&0x1b),
            "scan stdout must carry no escape/terminal-control bytes when piped: {:?}",
            String::from_utf8_lossy(&scan_output.stdout)
        );

        let mut import_args = vec![
            "--config".to_string(),
            config_path.to_str().unwrap().to_string(),
            "import".to_string(),
            "gopro".to_string(),
            "--source".to_string(),
            card.to_str().unwrap().to_string(),
            "--dry-run".to_string(),
        ];
        import_args.extend(extra_args);
        let dry_run_output = bin()
            .args(&import_args)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(dry_run_output.status.code(), Some(0));
        assert!(
            !dry_run_output.stdout.contains(&0x1b),
            "import --dry-run stdout must carry no escape/terminal-control bytes when piped: {:?}",
            String::from_utf8_lossy(&dry_run_output.stdout)
        );
    }
}

// --- 4.3: a corrupt chapter degrades to quarantine, not a failed run ---

#[test]
fn corrupt_chapter_quarantines_its_session_and_exits_0() {
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card");
    let corrupt_path = card.join("DCIM/100GOPRO/GX010125.MP4");
    fs::create_dir_all(corrupt_path.parent().unwrap()).unwrap();
    // Not a valid MP4 at all: truncated garbage.
    fs::write(&corrupt_path, [0xDE, 0xAD, 0xBE, 0xEF]).unwrap();

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
            "--yes",
        ])
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0));

    assert!(
        dest.join("_quarantine/session-0125/GX010125.MP4").exists(),
        "an unparseable chapter's session must still be preserved, in quarantine"
    );
}

// --- 4.4: ignore globs and unrecognized files are surfaced but untouched ---

#[test]
fn ignored_globs_and_unrecognized_files_are_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card");
    let creation_time = creation_time_2026_07_09();

    write_chapter(
        &card.join("DCIM/100GOPRO/GX010200.MP4"),
        creation_time,
        &[1000],
    );
    let lrv = card.join("DCIM/100GOPRO/GL010200.LRV");
    let thm = card.join("DCIM/100GOPRO/GX010200.THM");
    let unrecognized = card.join("DCIM/100GOPRO/GOPR0042.JPG");
    fs::write(&lrv, b"low-res proxy").unwrap();
    fs::write(&thm, b"thumbnail").unwrap();
    fs::write(&unrecognized, b"photo").unwrap();

    let dest = dir.path().join("dest");
    let config_path = dir.path().join("config.yaml");
    gopro_config(&config_path, &dest, "    ignore: [\"*.LRV\", \"*.THM\"]\n");

    let scan_output = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "scan",
            "gopro",
            "--source",
            card.to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(scan_output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&scan_output.stdout);
    assert!(stdout.contains("session-0200"));
    assert!(stdout.contains("IGNORE"));
    assert!(stdout.contains("unrecognized"));
    assert!(!stdout.contains("GL010200.LRV"));
    assert!(!stdout.contains("GX010200.THM"));

    let import_status = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "import",
            "gopro",
            "--source",
            card.to_str().unwrap(),
            "--yes",
        ])
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert_eq!(import_status.code(), Some(0));

    assert!(lrv.exists(), "glob-ignored files must never be touched");
    assert!(thm.exists(), "glob-ignored files must never be touched");
    assert!(
        unrecognized.exists(),
        "unrecognized files are surfaced but left in place"
    );
    assert!(
        dest.join("2026/2026-07-09/GX010200.MP4").exists(),
        "the recognized chapter still imports normally"
    );
}

// --- 5.3: copy_quarantine: false leaves unmarked session on card, no _quarantine dir ---

#[test]
fn copy_quarantine_false_leaves_unmarked_on_card_and_keeps_marked() {
    // Spec 5.3: end-to-end run with copy_quarantine: false.
    // The unmarked session must remain on the card and no _quarantine
    // folder is created, while the marked session imports normally.
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card");
    let creation_time = creation_time_2026_07_09();

    // Marked session (has a HiLight marker).
    write_chapter(
        &card.join("DCIM/100GOPRO/GX010123.MP4"),
        creation_time,
        &[5000],
    );
    // Unmarked session (no markers — will be quarantined).
    write_chapter(&card.join("DCIM/100GOPRO/GX010124.MP4"), creation_time, &[]);

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
            "--yes",
        ])
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0));

    // Marked session is imported.
    assert!(
        dest.join("2026/2026-07-09/GX010123.MP4").exists(),
        "marked session should be imported normally"
    );

    // No _quarantine directory must exist.
    assert!(
        !dest.join("_quarantine").exists(),
        "no _quarantine folder should be created when copy_quarantine is false"
    );

    // Unmarked session must still be on the card, untouched.
    assert!(
        card.join("DCIM/100GOPRO/GX010124.MP4").exists(),
        "unmarked session must remain on the card when copy_quarantine is false"
    );
}

// --- improve-console-output design D4, task 8.2: import states its plan before transferring ---

#[test]
fn non_dry_run_import_prints_the_plan_before_the_execution_report() {
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card");
    let creation_time = creation_time_2026_07_09();
    write_chapter(
        &card.join("DCIM/100GOPRO/GX010123.MP4"),
        creation_time,
        &[5000],
    );

    let dest = dir.path().join("dest");
    let config_path = dir.path().join("config.yaml");
    gopro_config(&config_path, &dest, "");

    let output = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "import",
            "gopro",
            "--source",
            card.to_str().unwrap(),
            "--yes",
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let plan_at = stdout
        .find("[KEEP] session-0123")
        .expect("plan rendering must appear on stdout before the transfer runs");
    let report_at = stdout
        .find("Summary: 1 transferred")
        .expect("execution report's summary line must appear on stdout");
    assert!(
        plan_at < report_at,
        "plan must print before the execution report: {stdout}"
    );
}

// --- improve-console-output design D9, task 9.3: -v unlocks INFO milestones ---

#[test]
fn verbose_flag_unlocks_info_milestones_a_default_run_does_not_emit() {
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card");
    let creation_time = creation_time_2026_07_09();
    write_chapter(
        &card.join("DCIM/100GOPRO/GX010123.MP4"),
        creation_time,
        &[5000],
    );

    let dest = dir.path().join("dest");
    let config_path = dir.path().join("config.yaml");
    gopro_config(&config_path, &dest, "");

    let default_output = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "scan",
            "gopro",
            "--source",
            card.to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(default_output.status.code(), Some(0));
    let default_stderr = String::from_utf8_lossy(&default_output.stderr);
    assert!(
        !default_stderr.contains("scan complete"),
        "a default run must not emit INFO milestones: {default_stderr}"
    );

    let verbose_output = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "-v",
            "scan",
            "gopro",
            "--source",
            card.to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(verbose_output.status.code(), Some(0));
    let verbose_stderr = String::from_utf8_lossy(&verbose_output.stderr);
    assert!(
        verbose_stderr.contains("source resolved"),
        "got: {verbose_stderr}"
    );
    assert!(
        verbose_stderr.contains("scan complete"),
        "got: {verbose_stderr}"
    );
    assert!(
        verbose_stderr.contains("plan built"),
        "got: {verbose_stderr}"
    );
}

#[test]
fn non_dry_run_import_json_still_emits_exactly_one_document() {
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card");
    let creation_time = creation_time_2026_07_09();
    write_chapter(
        &card.join("DCIM/100GOPRO/GX010123.MP4"),
        creation_time,
        &[5000],
    );

    let dest = dir.path().join("dest");
    let config_path = dir.path().join("config.yaml");
    gopro_config(&config_path, &dest, "");

    let output = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "--json",
            "import",
            "gopro",
            "--source",
            card.to_str().unwrap(),
            "--yes",
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout must be exactly one JSON document: {e}\n{stdout}"));
    assert!(
        value.get("groups").is_some(),
        "the one document must be the execution report, not the plan: {value}"
    );
}
