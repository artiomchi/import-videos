//! End-to-end coverage of the scenarios in
//! `openspec/changes/add-gopro-gps/specs/{gopro-import,gopro-telemetry}/spec.md`,
//! driven through the compiled binary against a fake HERO8 card built
//! in a tempdir (real directory layout, handcrafted MP4 + GPMF bytes —
//! no binary fixtures checked into the repo). Mirrors
//! `tests/gopro_import.rs`'s fixture style, extended with a synthetic
//! `gpmd` telemetry track.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use jiff::Timestamp;

// --- Handcrafted MP4 + GPMF byte fixtures ---

const MAC_EPOCH_OFFSET_SECS: i64 = 2_082_844_800;

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

fn hdlr(handler_type: &[u8; 4]) -> Vec<u8> {
    let mut payload = vec![0u8; 8];
    payload.extend_from_slice(handler_type);
    payload.extend_from_slice(&[0u8; 12]);
    make_box(b"hdlr", &payload)
}

fn stsd(format: &[u8; 4]) -> Vec<u8> {
    let mut entry = Vec::new();
    entry.extend_from_slice(&16u32.to_be_bytes());
    entry.extend_from_slice(format);
    entry.extend_from_slice(&[0u8; 8]);
    let mut payload = vec![0u8; 4];
    payload.extend_from_slice(&1u32.to_be_bytes());
    payload.extend_from_slice(&entry);
    make_box(b"stsd", &payload)
}

fn mdhd(timescale: u32) -> Vec<u8> {
    let mut payload = vec![0u8; 12];
    payload.extend_from_slice(&timescale.to_be_bytes());
    payload.extend_from_slice(&[0u8; 4]);
    make_box(b"mdhd", &payload)
}

fn stsz(sizes: &[u32]) -> Vec<u8> {
    let mut payload = vec![0u8; 4];
    payload.extend_from_slice(&0u32.to_be_bytes());
    payload.extend_from_slice(&(sizes.len() as u32).to_be_bytes());
    for size in sizes {
        payload.extend_from_slice(&size.to_be_bytes());
    }
    make_box(b"stsz", &payload)
}

fn stsc(entries: &[(u32, u32, u32)]) -> Vec<u8> {
    let mut payload = vec![0u8; 4];
    payload.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    for (first_chunk, samples_per_chunk, sdi) in entries {
        payload.extend_from_slice(&first_chunk.to_be_bytes());
        payload.extend_from_slice(&samples_per_chunk.to_be_bytes());
        payload.extend_from_slice(&sdi.to_be_bytes());
    }
    make_box(b"stsc", &payload)
}

fn stco(offsets: &[u32]) -> Vec<u8> {
    let mut payload = vec![0u8; 4];
    payload.extend_from_slice(&(offsets.len() as u32).to_be_bytes());
    for offset in offsets {
        payload.extend_from_slice(&offset.to_be_bytes());
    }
    make_box(b"stco", &payload)
}

fn stts(entries: &[(u32, u32)]) -> Vec<u8> {
    let mut payload = vec![0u8; 4];
    payload.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    for (count, delta) in entries {
        payload.extend_from_slice(&count.to_be_bytes());
        payload.extend_from_slice(&delta.to_be_bytes());
    }
    make_box(b"stts", &payload)
}

fn klv_item(key: &[u8; 4], type_char: u8, struct_size: u8, value: &[u8]) -> Vec<u8> {
    assert_eq!(value.len() % struct_size as usize, 0);
    let repeat = (value.len() / struct_size as usize) as u16;
    let mut buf = Vec::with_capacity(8 + value.len());
    buf.extend_from_slice(key);
    buf.push(type_char);
    buf.push(struct_size);
    buf.extend_from_slice(&repeat.to_be_bytes());
    buf.extend_from_slice(value);
    while buf.len() % 4 != 0 {
        buf.push(0);
    }
    buf
}

fn nested(key: &[u8; 4], children: &[Vec<u8>]) -> Vec<u8> {
    klv_item(key, 0, 1, &children.concat())
}

fn be_i32s(vals: &[i32]) -> Vec<u8> {
    vals.iter().flat_map(|v| v.to_be_bytes()).collect()
}

fn gpsu_string(ts: &str) -> String {
    let zoned = ts
        .parse::<Timestamp>()
        .unwrap()
        .to_zoned(jiff::tz::TimeZone::UTC);
    format!(
        "{:02}{:02}{:02}{:02}{:02}{:02}.{:03}",
        zoned.year() % 100,
        zoned.month(),
        zoned.day(),
        zoned.hour(),
        zoned.minute(),
        zoned.second(),
        zoned.subsec_nanosecond() / 1_000_000,
    )
}

fn mac_time(ts: &str) -> u32 {
    (ts.parse::<Timestamp>().unwrap().as_second() + MAC_EPOCH_OFFSET_SECS) as u32
}

/// One GPMF payload: `DEVC { STRM { SCAL, GPSU, GPSF, GPSP, GPS5 } }`,
/// with a usable 2D+ fix and DOP <= 5.0.
fn gps_payload(gpsu: &str, gps5: [i32; 5]) -> Vec<u8> {
    let scal = klv_item(
        b"SCAL",
        b'l',
        4,
        &be_i32s(&[10_000_000, 10_000_000, 1000, 1000, 1000]),
    );
    let gpsu_klv = klv_item(b"GPSU", b'U', 16, gpsu_string(gpsu).as_bytes());
    let gpsf = klv_item(b"GPSF", b'L', 4, &3u32.to_be_bytes());
    let gpsp = klv_item(b"GPSP", b'S', 2, &150u16.to_be_bytes());
    let gps5_klv = klv_item(b"GPS5", b'l', 4, &be_i32s(&gps5));
    let strm = nested(b"STRM", &[scal, gpsu_klv, gpsf, gpsp, gps5_klv]);
    nested(b"DEVC", &[strm])
}

/// Bytes for a `gpmd` track box tree (`trak/mdia/{hdlr,mdhd,minf/stbl}`)
/// given already-built sample-table boxes, plus the payload bytes that
/// must be appended after `moov` at the sentinel-patched offsets.
struct GpmdTrak {
    trak: Vec<u8>,
    sentinel_payloads: Vec<(u32, Vec<u8>)>,
}

fn gpmd_trak(payloads: &[Vec<u8>]) -> GpmdTrak {
    let sentinels: Vec<u32> = (0..payloads.len() as u32)
        .map(|i| 0xA000_0000 + i)
        .collect();
    let sizes: Vec<u32> = payloads.iter().map(|p| p.len() as u32).collect();
    let stsc_entries: Vec<(u32, u32, u32)> =
        (0..payloads.len() as u32).map(|i| (i + 1, 1, 1)).collect();
    let stbl = make_container(
        b"stbl",
        &[
            stsd(b"gpmd"),
            stsz(&sizes),
            stsc(&stsc_entries),
            stco(&sentinels),
            stts(&[(payloads.len() as u32, 1000)]),
        ],
    );
    let minf = make_container(b"minf", &[stbl]);
    let mdia = make_container(b"mdia", &[hdlr(b"meta"), mdhd(1000), minf]);
    let trak = make_container(b"trak", &[mdia]);
    GpmdTrak {
        trak,
        sentinel_payloads: sentinels
            .into_iter()
            .zip(payloads.iter().cloned())
            .collect(),
    }
}

/// A `gpmd` track whose `stsz` declares more samples than `stsc`/`stco`
/// can place — malformed sample tables (spec: "Corrupt sample table
/// fails cleanly"), used to exercise graceful degradation.
fn corrupt_gpmd_trak() -> Vec<u8> {
    let stbl = make_container(
        b"stbl",
        &[
            stsd(b"gpmd"),
            stsz(&[10, 20, 30]),
            stsc(&[(1, 1, 1)]),
            stco(&[1000]),
            stts(&[(3, 1000)]),
        ],
    );
    let minf = make_container(b"minf", &[stbl]);
    let mdia = make_container(b"mdia", &[hdlr(b"meta"), mdhd(1000), minf]);
    make_container(b"trak", &[mdia])
}

/// Writes a synthetic HERO8 chapter file: `moov/mvhd`, optionally
/// `moov/udta/HMMT`, and optionally a `gpmd` track (well-formed or
/// deliberately corrupt).
enum Gpmd {
    None,
    Payloads(Vec<Vec<u8>>),
    Corrupt,
}

fn write_chapter(path: &Path, creation_time: &str, marker_offsets_ms: &[u32], gpmd: Gpmd) {
    let mut moov_children = vec![mvhd_v0(mac_time(creation_time))];
    if !marker_offsets_ms.is_empty() {
        moov_children.push(make_container(b"udta", &[hmmt(marker_offsets_ms)]));
    }

    let mut trailing_payloads = Vec::new();
    match gpmd {
        Gpmd::None => {}
        Gpmd::Corrupt => moov_children.push(corrupt_gpmd_trak()),
        Gpmd::Payloads(payloads) => {
            let built = gpmd_trak(&payloads);
            moov_children.push(built.trak);
            trailing_payloads = built.sentinel_payloads;
        }
    }

    let mut moov_bytes = make_container(b"moov", &moov_children);

    let mut cursor = moov_bytes.len() as u32;
    let mut patches = Vec::new();
    for (sentinel, payload) in &trailing_payloads {
        patches.push((*sentinel, cursor));
        cursor += payload.len() as u32;
    }
    for (sentinel, real_offset) in patches {
        let marker = sentinel.to_be_bytes();
        let pos = moov_bytes
            .windows(4)
            .position(|w| w == marker)
            .expect("stco sentinel not found");
        moov_bytes[pos..pos + 4].copy_from_slice(&real_offset.to_be_bytes());
    }
    for (_, payload) in &trailing_payloads {
        moov_bytes.extend_from_slice(payload);
    }

    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, moov_bytes).unwrap();
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

fn read_sidecar(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn mtime_of(path: &Path) -> Timestamp {
    Timestamp::try_from(fs::metadata(path).unwrap().modified().unwrap()).unwrap()
}

// --- 5.2: end-to-end GPS import ---

#[test]
fn gps_corrected_session_lands_in_gps_dated_folder_with_full_sidecar() {
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card");

    // Camera clock reads 2026-07-10T00:20 (drifted + local-time, not
    // UTC); GPS says the true instant is 2026-07-09T23:19:48Z — a
    // different calendar date, so the destination folder only matches
    // if GPS correction is actually applied.
    let payload = gps_payload(
        "2026-07-09T23:19:48Z",
        [515_012_340, -1_234_567, 100_000, 0, 0],
    );
    write_chapter(
        &card.join("DCIM/100GOPRO/GX010123.MP4"),
        "2026-07-10T00:20:00Z",
        &[500],
        Gpmd::Payloads(vec![payload]),
    );

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

    let dest_file = dest.join("2026/2026-07-09/GX010123.MP4");
    assert!(
        dest_file.exists(),
        "GPS-corrected date (07-09) should win over the camera-clock date (07-10)"
    );
    assert!(!dest.join("2026/2026-07-10").exists());

    let sidecar = read_sidecar(&dest.join("2026/2026-07-09/import.json"));
    assert_eq!(sidecar["time_source"], "gps");
    let offset = sidecar["gopro"]["clock_offset_s"].as_f64().unwrap();
    assert!((offset - (-3612.0)).abs() < 0.01, "offset was {offset}");

    let marker = &sidecar["events"][0];
    assert_eq!(marker["offset_ms"], 500);
    assert!(marker.get("time").is_some());
    assert!(marker.get("camera_time").is_none());
    assert!(marker.get("lat").is_some());
    assert!(marker.get("lon").is_some());

    let expected_mtime: Timestamp = "2026-07-09T23:19:48Z".parse().unwrap();
    assert_eq!(mtime_of(&dest_file), expected_mtime);
}

// --- 5.3: telemetry-absent and telemetry-corrupt degrade to changeset 2 ---

#[test]
fn card_without_gpmd_track_imports_with_camera_clock_sidecar() {
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card");
    write_chapter(
        &card.join("DCIM/100GOPRO/GX010200.MP4"),
        "2026-07-09T12:00:00Z",
        &[1000],
        Gpmd::None,
    );

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

    assert!(dest.join("2026/2026-07-09/GX010200.MP4").exists());
    let sidecar = read_sidecar(&dest.join("2026/2026-07-09/import.json"));
    assert_eq!(sidecar["time_source"], "camera");
    assert!(sidecar["gopro"].get("clock_offset_s").is_none());
    let marker = &sidecar["events"][0];
    assert!(marker.get("time").is_some());
    assert!(marker.get("camera_time").is_none());
}

#[test]
fn corrupt_telemetry_degrades_to_camera_clock_without_aborting() {
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card");
    write_chapter(
        &card.join("DCIM/100GOPRO/GX010201.MP4"),
        "2026-07-09T12:00:00Z",
        &[1000],
        Gpmd::Corrupt,
    );

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
    assert_eq!(
        status.code(),
        Some(0),
        "corrupt telemetry must not abort the run"
    );

    assert!(dest.join("2026/2026-07-09/GX010201.MP4").exists());
    let sidecar = read_sidecar(&dest.join("2026/2026-07-09/import.json"));
    assert_eq!(sidecar["time_source"], "camera");
}

#[test]
fn unmarked_session_without_telemetry_is_still_quarantined() {
    // Keep/Quarantine verdicts stay marker-driven regardless of
    // telemetry outcome (spec: "Verdict unaffected by telemetry").
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card");
    write_chapter(
        &card.join("DCIM/100GOPRO/GX010300.MP4"),
        "2026-07-09T12:00:00Z",
        &[],
        Gpmd::None,
    );

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
        dest.join("_quarantine/session-0300/GX010300.MP4").exists(),
        "unmarked session must still be quarantined, telemetry notwithstanding"
    );
}

// --- improve-scan-and-cleanup design D4, task 7.4: quarantine-bound sessions never pay telemetry cost ---

#[test]
fn quarantine_bound_session_with_usable_gps_fix_is_never_gps_corrected() {
    // A session with no HiLight markers under require_marker: true
    // (the profile default) is Quarantine-bound; even though its
    // chapter carries a usable GPS fix, that fix must never be applied
    // — the session's destination lands under the plain quarantine
    // path, not a GPS-corrected date, and it gets no sidecar (design
    // D4: verdict decided, and telemetry skipped, before the fix would
    // ever be read).
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card");

    // Camera clock reads 2026-07-10T00:20; the fixture's GPS fix
    // would move the session to 2026-07-09 if it were ever applied.
    let payload = gps_payload(
        "2026-07-09T23:19:48Z",
        [515_012_340, -1_234_567, 100_000, 0, 0],
    );
    write_chapter(
        &card.join("DCIM/100GOPRO/GX010400.MP4"),
        "2026-07-10T00:20:00Z",
        &[], // no markers -> Quarantine
        Gpmd::Payloads(vec![payload]),
    );

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
        dest.join("_quarantine/session-0400/GX010400.MP4").exists(),
        "unmarked session must land in quarantine, never a GPS-corrected date folder"
    );
    assert!(
        !dest.join("2026/2026-07-09").exists(),
        "the GPS fix must never be applied to a quarantine-bound session"
    );
    assert!(
        !dest.join("_quarantine/session-0400/import.json").exists(),
        "quarantined sessions get no sidecar, so there's nothing to show a gps time_source"
    );
}

#[test]
fn scan_never_opens_gpmd_track_even_for_a_keep_bound_session() {
    // design D1/D2: scan never performs GPS telemetry lookup, full stop
    // — not just for quarantine-bound sessions. A marked (Keep-bound)
    // session whose chapter carries a usable GPS fix that would move it
    // across a day boundary must still show camera-clock time in scan's
    // inventory, since scan structurally never consults telemetry.
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card");

    // Camera clock reads 2026-07-10T00:20; the fixture's GPS fix would
    // move the session to 2026-07-09 if telemetry were ever consulted.
    let payload = gps_payload(
        "2026-07-09T23:19:48Z",
        [515_012_340, -1_234_567, 100_000, 0, 0],
    );
    write_chapter(
        &card.join("DCIM/100GOPRO/GX010500.MP4"),
        "2026-07-10T00:20:00Z",
        &[500], // marker present -> Keep
        Gpmd::Payloads(vec![payload]),
    );

    let dest = dir.path().join("dest");
    let config_path = dir.path().join("config.yaml");
    gopro_config(&config_path, &dest, "");

    let scan_output = bin()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "scan",
            "gopro",
            "--source",
            card.to_str().unwrap(),
            "-v",
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(scan_output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&scan_output.stdout);
    assert!(
        stdout.contains("[KEEP] session-0500"),
        "marked session must scan as Keep: {stdout}"
    );
    assert!(
        stdout.contains("2026-07-10 00:20"),
        "scan must show the camera-clock time, never GPS-corrected, even for a Keep session: {stdout}"
    );
    assert!(
        !stdout.contains("2026-07-09"),
        "scan must never apply the GPS fix, even though it would otherwise move the session to a different day: {stdout}"
    );
}

// --- 7.1: multi-chapter session — each marker carries its chapter file + human offset ---

#[test]
fn multi_chapter_markers_carry_file_and_offset() {
    // Two chapters in one session; each has one marker at a different
    // offset. The sidecar's events[] should name the originating chapter
    // file and include both `offset_ms` and the rendered `offset` string.
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card");

    let ts = "2026-07-09T12:00:00Z";
    // Chapter 1: marker at 5000 ms (0:05.000)
    write_chapter(
        &card.join("DCIM/100GOPRO/GX010123.MP4"),
        ts,
        &[5000],
        Gpmd::None,
    );
    // Chapter 2: marker at 734120 ms (12:14.120)
    write_chapter(
        &card.join("DCIM/100GOPRO/GX020123.MP4"),
        ts,
        &[734120],
        Gpmd::None,
    );

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
        .stdin(std::process::Stdio::null())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0));

    let sidecar_path = dest.join("2026/2026-07-09/import.json");
    assert!(sidecar_path.exists(), "sidecar must be written");

    let sidecar = read_sidecar(&sidecar_path);
    let events = sidecar["events"]
        .as_array()
        .expect("events must be an array");
    assert_eq!(events.len(), 2, "two markers across two chapters");

    // First event: chapter 1 (GX010123.MP4), offset 5000 ms.
    let e0 = &events[0];
    assert_eq!(e0["offset_ms"], 5000);
    assert_eq!(e0["offset"], "0:05.000");
    assert_eq!(e0["file"], "GX010123.MP4");

    // Second event: chapter 2 (GX020123.MP4), offset 734120 ms.
    let e1 = &events[1];
    assert_eq!(e1["offset_ms"], 734120);
    assert_eq!(e1["offset"], "12:14.120");
    assert_eq!(e1["file"], "GX020123.MP4");
}
