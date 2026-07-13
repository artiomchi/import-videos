//! CLI-level coverage for `--summary` (openspec/changes/add-summary-flag,
//! task 7.2), driven through the compiled binary against fake GoPro card
//! fixtures — mirrors the fixture-building helpers in
//! `tests/gopro_import.rs` (kept file-local since integration test
//! binaries can't share code without a support crate).

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

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

fn creation_time_for_date(y: i16, m: i8, d: i8) -> u32 {
    let unix_secs = jiff::civil::date(y, m, d)
        .at(0, 0, 0, 0)
        .to_zoned(jiff::tz::TimeZone::UTC)
        .unwrap()
        .timestamp()
        .as_second();
    (unix_secs + 2_082_844_800) as u32
}

fn creation_time_2026_07_09() -> u32 {
    creation_time_for_date(2026, 7, 9)
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

// --- scan --summary: only the closing line (spec: "Summary mode prints only the closing line") ---

#[test]
fn scan_summary_prints_only_closing_line() {
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card");
    let creation_time = creation_time_2026_07_09();

    // Keep (marked).
    write_chapter(
        &card.join("DCIM/100GOPRO/GX010123.MP4"),
        creation_time,
        &[5000],
    );
    // Quarantine (unmarked).
    write_chapter(&card.join("DCIM/100GOPRO/GX010124.MP4"), creation_time, &[]);
    // Unrecognized stray file.
    fs::write(card.join("DCIM/100GOPRO/GOPR0042.JPG"), b"photo").unwrap();

    let dest = dir.path().join("dest");
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
            "--summary",
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));

    let stdout = String::from_utf8_lossy(&output.stdout);
    for marker in [
        "[KEEP]",
        "[QUARANTINE]",
        "[IGNORE]",
        "Quarantine:",
        "GOPR0042.JPG",
        "session-",
    ] {
        assert!(
            !stdout.contains(marker),
            "no per-group/per-verdict listing may appear under --summary, found {marker:?}: {stdout}"
        );
    }
    assert!(stdout.contains("1 kept"), "got: {stdout}");
    assert!(stdout.contains("1 quarantined"), "got: {stdout}");
    assert!(stdout.contains("1 ignored"), "got: {stdout}");
    assert_eq!(
        stdout.lines().filter(|l| l.starts_with("Summary:")).count(),
        1,
        "exactly one closing summary line: {stdout}"
    );
}

// --- import --summary: collapses collision/quarantine-disabled counts (spec scenario) ---

#[test]
fn import_summary_collapses_collision_and_disabled_quarantine_counts() {
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card");
    let creation_time = creation_time_2026_07_09();

    // Marked session: its file will collide with a pre-existing,
    // differently-content destination file, producing a Suffixed outcome.
    write_chapter(
        &card.join("DCIM/100GOPRO/GX010123.MP4"),
        creation_time,
        &[5000],
    );
    // Unmarked session: quarantined, but copy_quarantine: false leaves
    // its file on the source with a SkippedQuarantineDisabled outcome.
    write_chapter(&card.join("DCIM/100GOPRO/GX010124.MP4"), creation_time, &[]);

    let dest = dir.path().join("dest");
    let config_path = dir.path().join("config.yaml");
    gopro_config(&config_path, &dest, "    copy_quarantine: false\n");

    let collision_dest = dest.join("2026/2026-07-09/GX010123.MP4");
    fs::create_dir_all(collision_dest.parent().unwrap()).unwrap();
    fs::write(&collision_dest, b"pre-existing different bytes").unwrap();

    let output = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "import",
            "gopro",
            "--source",
            card.to_str().unwrap(),
            "--yes",
            "--summary",
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output);

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("destination name collision"),
        "the per-file collision line must be omitted under --summary: {stdout}"
    );
    assert!(
        !stdout.contains("left on source (quarantine copy disabled)"),
        "the per-file quarantine-disabled line must be omitted under --summary: {stdout}"
    );
    assert!(stdout.contains("1 renamed (collision)"), "got: {stdout}");
    assert!(
        stdout.contains("1 left on source (quarantine copying disabled)"),
        "got: {stdout}"
    );
}

// --- import --summary still names failures individually ---

#[test]
fn import_summary_still_names_a_failed_file() {
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card");

    // Two marked sessions on different days so each lands in its own
    // date folder — lets one destination folder be made unwritable
    // without affecting the other session's transfer.
    write_chapter(
        &card.join("DCIM/100GOPRO/GX010123.MP4"),
        creation_time_for_date(2026, 7, 9),
        &[5000],
    );
    write_chapter(
        &card.join("DCIM/100GOPRO/GX010124.MP4"),
        creation_time_for_date(2026, 7, 10),
        &[5000],
    );

    let dest = dir.path().join("dest");
    let config_path = dir.path().join("config.yaml");
    gopro_config(&config_path, &dest, "");

    let broken_dir = dest.join("2026/2026-07-10");
    fs::create_dir_all(&broken_dir).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&broken_dir, fs::Permissions::from_mode(0o500)).unwrap();
    }

    let output = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "import",
            "gopro",
            "--source",
            card.to_str().unwrap(),
            "--yes",
            "--summary",
        ])
        .stdin(Stdio::null())
        .output();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&broken_dir, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let output = output.unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("FAILED: ") && stdout.contains("GX010124.MP4"),
        "a failed transfer must still be individually named under --summary: {stdout}"
    );
    assert!(stdout.contains("1 FAILED"), "got: {stdout}");
}

// --- import --summary -v: same stdout as --summary alone, plus stderr diagnostics ---

#[test]
fn import_summary_verbose_matches_summary_alone_on_stdout() {
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card");
    write_chapter(
        &card.join("DCIM/100GOPRO/GX010123.MP4"),
        creation_time_2026_07_09(),
        &[5000],
    );

    let config_path = dir.path().join("config.yaml");
    let dest_a = dir.path().join("dest_a");
    gopro_config(&config_path, &dest_a, "");
    let summary_output = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "import",
            "gopro",
            "--source",
            card.to_str().unwrap(),
            "--yes",
            "--summary",
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(summary_output.status.code(), Some(0));

    // Re-run against a fresh destination and source copy so the second
    // invocation sees the same starting state as the first.
    let card2 = dir.path().join("card2");
    write_chapter(
        &card2.join("DCIM/100GOPRO/GX010123.MP4"),
        creation_time_2026_07_09(),
        &[5000],
    );
    let dest_b = dir.path().join("dest_b");
    let config_path_b = dir.path().join("config_b.yaml");
    gopro_config(&config_path_b, &dest_b, "");
    let summary_verbose_output = bin()
        .args([
            "--config",
            config_path_b.to_str().unwrap(),
            "import",
            "gopro",
            "--source",
            card2.to_str().unwrap(),
            "--yes",
            "--summary",
            "-v",
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(summary_verbose_output.status.code(), Some(0));

    let stdout_a = String::from_utf8_lossy(&summary_output.stdout);
    let stdout_b = String::from_utf8_lossy(&summary_verbose_output.stdout);
    assert_eq!(
        stdout_a, stdout_b,
        "--summary -v's stdout must be identical to --summary alone"
    );

    let stderr_b = String::from_utf8_lossy(&summary_verbose_output.stderr);
    assert!(
        stderr_b.contains("scan complete"),
        "-v's diagnostic log lines must still appear on stderr: {stderr_b}"
    );
}

// --- --summary is a no-op under --json ---

#[test]
fn summary_is_a_noop_under_json() {
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card");
    write_chapter(
        &card.join("DCIM/100GOPRO/GX010123.MP4"),
        creation_time_2026_07_09(),
        &[5000],
    );

    let dest = dir.path().join("dest");
    let config_path = dir.path().join("config.yaml");
    gopro_config(&config_path, &dest, "");

    let json_output = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "--json",
            "scan",
            "gopro",
            "--source",
            card.to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(json_output.status.code(), Some(0));

    let json_summary_output = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "--json",
            "--summary",
            "scan",
            "gopro",
            "--source",
            card.to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(json_summary_output.status.code(), Some(0));

    assert_eq!(
        String::from_utf8_lossy(&json_output.stdout),
        String::from_utf8_lossy(&json_summary_output.stdout),
        "--summary must not change --json output"
    );
}
