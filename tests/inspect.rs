//! CLI-level coverage for `inspect` (cli-maintenance spec): config-free
//! operation, `--json` output, and the unsupported-input usage error,
//! driven through the compiled binary.

use std::fs;
use std::process::{Command, Stdio};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_import-videos"))
}

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

fn mvhd_v0(mac_creation_time: u32) -> Vec<u8> {
    let mut payload = vec![0u8; 4];
    payload.extend_from_slice(&mac_creation_time.to_be_bytes());
    make_box(b"mvhd", &payload)
}

fn hmmt(offsets: &[u32]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(4 + offsets.len() * 4);
    payload.extend_from_slice(&(offsets.len() as u32).to_be_bytes());
    for offset in offsets {
        payload.extend_from_slice(&offset.to_be_bytes());
    }
    make_box(b"HMMT", &payload)
}

const MAC_EPOCH_OFFSET_SECS: i64 = 2_082_844_800;

fn mac_time(ts: &str) -> u32 {
    (ts.parse::<jiff::Timestamp>().unwrap().as_second() + MAC_EPOCH_OFFSET_SECS) as u32
}

fn write_minimal_chapter(path: &std::path::Path) {
    let udta = make_container(b"udta", &[hmmt(&[5000])]);
    let moov = make_container(b"moov", &[mvhd_v0(mac_time("2026-07-09T07:41:03Z")), udta]);
    fs::write(path, moov).unwrap();
}

#[test]
fn inspect_mp4_works_with_no_config_file_present() {
    let dir = tempfile::tempdir().unwrap();
    let chapter = dir.path().join("GX010123.MP4");
    write_minimal_chapter(&chapter);

    // Deliberately point --config at a path that doesn't exist: inspect
    // must not even attempt to load it (design D5).
    let output = bin()
        .args([
            "--config",
            dir.path().join("nonexistent-config.yaml").to_str().unwrap(),
            "inspect",
            chapter.to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("HiLight markers: 1"));
}

#[test]
fn inspect_mp4_json_emits_one_document() {
    let dir = tempfile::tempdir().unwrap();
    let chapter = dir.path().join("GX010123.MP4");
    write_minimal_chapter(&chapter);

    let output = bin()
        .args(["--json", "inspect", chapter.to_str().unwrap()])
        .stdin(Stdio::null())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(value["markers"][0]["offset_ms"], 5000);
    assert!(value["creation_time"].is_string());
}

#[test]
fn inspect_tesla_event_folder() {
    let dir = tempfile::tempdir().unwrap();
    let event_dir = dir.path().join("2026-07-09_08-15-30");
    fs::create_dir_all(&event_dir).unwrap();
    fs::write(
        event_dir.join("event.json"),
        r#"{"timestamp":"2026-07-09T08:15:30","city":"London","reason":"user_interaction_honk"}"#,
    )
    .unwrap();
    fs::write(event_dir.join("2026-07-09_08-15-30-front.mp4"), b"front").unwrap();

    let output = bin()
        .args(["inspect", event_dir.to_str().unwrap()])
        .stdin(Stdio::null())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("user_interaction_honk"));
    assert!(stdout.contains("London"));
}

#[test]
fn unsupported_input_exits_2() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.txt");
    fs::write(&path, b"hello").unwrap();

    let output = bin()
        .args(["inspect", path.to_str().unwrap()])
        .stdin(Stdio::null())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("supported"));
}
