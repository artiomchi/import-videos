//! `inspect FILE`: a config-free, read-only metadata dump for a single
//! GoPro chapter MP4 or Tesla event folder (design D5) — debugging and
//! card triage without a profile. Reuses `media/mp4.rs` and
//! `media/gpmf.rs` read-only; no filesystem writes anywhere in this
//! module.

use std::fs::{self, File};
use std::path::{Path, PathBuf};

use jiff::{Span, Timestamp};

use crate::error::{Error, Result};
use crate::media::{gpmf, mp4};

/// What kind of input `inspect` was pointed at, resolved from the path
/// alone — no config, no profile (design D5).
pub enum InspectTarget {
    Mp4(PathBuf),
    /// Always the event *folder*, even when the user passed the
    /// `event.json` file itself.
    TeslaEvent(PathBuf),
}

/// Classifies `path` for `inspect`, or names the supported inputs when
/// it matches none of them (spec: "Unsupported input" is a usage
/// error).
pub fn classify(path: &Path) -> std::result::Result<InspectTarget, String> {
    if path.is_file()
        && path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("mp4"))
    {
        return Ok(InspectTarget::Mp4(path.to_path_buf()));
    }
    if path.is_dir() && path.join("event.json").is_file() {
        return Ok(InspectTarget::TeslaEvent(path.to_path_buf()));
    }
    if path.is_file() && path.file_name().and_then(|n| n.to_str()) == Some("event.json") {
        let dir = path.parent().unwrap_or(path).to_path_buf();
        return Ok(InspectTarget::TeslaEvent(dir));
    }
    Err(format!(
        "unsupported input '{}': expected a GoPro .mp4 file, a Tesla event folder (containing event.json), or an event.json file",
        path.display()
    ))
}

/// One HiLight marker's raw offset and, when the creation time parsed,
/// its derived wall-clock timestamp.
#[derive(Debug, Clone)]
pub struct MarkerDump {
    pub offset_ms: u32,
    pub timestamp: Option<Timestamp>,
}

/// GPS summary for an MP4's `gpmd` track (design D5): the first usable
/// fix, its offset from the camera clock, and how many samples the
/// track carries. `first_fix`/`clock_offset_s` are `None` when a
/// `gpmd` track exists but no sample cleared the fix-quality gate —
/// still a successful result, just an empty one.
#[derive(Debug, Clone)]
pub struct GpsSummary {
    pub sample_count: usize,
    pub first_fix: Option<(f64, f64)>,
    pub clock_offset_s: Option<f64>,
}

/// One GoPro MP4's metadata dump. Each section is independently
/// fallible (design D5 / spec: "Partial metadata still prints") —
/// a corrupt `gpmd` track must not hide HiLight markers that parsed
/// cleanly, and vice versa.
#[derive(Debug)]
pub struct Mp4Dump {
    pub path: PathBuf,
    pub creation_time: std::result::Result<Timestamp, String>,
    pub markers: std::result::Result<Vec<MarkerDump>, String>,
    /// `Ok(None)` means no `gpmd` track — not a failure.
    pub gps: std::result::Result<Option<GpsSummary>, String>,
}

impl Mp4Dump {
    /// Whether any section failed to parse — callers use this to
    /// decide the process exit code (spec: exit 1 on partial failure).
    pub fn has_errors(&self) -> bool {
        self.creation_time.is_err() || self.markers.is_err() || self.gps.is_err()
    }
}

/// Dumps `path`'s HiLight markers, creation time, and (when present) a
/// GPS summary. Only a failure to *open* the file is fatal; every
/// section beyond that degrades to its own `Err(String)` (design D5).
pub fn inspect_mp4(path: &Path) -> Result<Mp4Dump> {
    let mut file = File::open(path).map_err(|e| Error::io(path, e))?;

    let creation_time: std::result::Result<Timestamp, String> =
        mp4::read_creation_time(&mut file).map_err(|e| e.to_string());

    let markers: std::result::Result<Vec<MarkerDump>, String> = mp4::read_hilights(&mut file)
        .map(|offsets| {
            offsets
                .into_iter()
                .map(|offset_ms| MarkerDump {
                    offset_ms,
                    timestamp: creation_time
                        .as_ref()
                        .ok()
                        .map(|&ct| ct + Span::new().milliseconds(offset_ms as i64)),
                })
                .collect()
        })
        .map_err(|e| e.to_string());

    let gps = inspect_gps(&mut file, creation_time.as_ref().ok().copied());

    Ok(Mp4Dump {
        path: path.to_path_buf(),
        creation_time,
        markers,
        gps,
    })
}

/// GPS summary from the file's `gpmd` track, if any: the first sample
/// whose payload clears fix-quality gating supplies `first_fix` and
/// (when `creation_time` is known) the clock offset — same gate and
/// offset math as `source::gopro`'s session correction, but read-only
/// and reported rather than applied.
fn inspect_gps(
    file: &mut File,
    creation_time: Option<Timestamp>,
) -> std::result::Result<Option<GpsSummary>, String> {
    let Some(index) = mp4::read_gpmd_index(file).map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let sample_count = index.len();

    let mut first_fix = None;
    let mut clock_offset_s = None;
    for sample in &index {
        let bytes = mp4::read_gpmd_payload(file, sample).map_err(|e| e.to_string())?;
        let Some(payload) = gpmf::parse_gps_payload(&bytes).map_err(|e| e.to_string())? else {
            continue;
        };
        if payload.usable()
            && let Some(utc) = payload.utc
            && let Some(gps_sample) = payload.samples.first()
        {
            first_fix = Some((gps_sample.lat, gps_sample.lon));
            clock_offset_s = creation_time.map(|ct| {
                let reference =
                    ct + Span::new().milliseconds((sample.time_s * 1000.0).round() as i64);
                utc.duration_since(reference).as_secs_f64()
            });
            break;
        }
    }

    Ok(Some(GpsSummary {
        sample_count,
        first_fix,
        clock_offset_s,
    }))
}

/// One Tesla event folder's metadata dump: `event.json`'s fields,
/// tolerantly parsed field-by-field (matching `source::tesla`'s
/// approach — a missing/corrupt field degrades individually, not the
/// whole dump), plus the files present alongside it.
#[derive(Debug)]
pub struct TeslaEventDump {
    pub path: PathBuf,
    pub timestamp: Option<String>,
    pub reason: Option<String>,
    pub city: Option<String>,
    pub coordinates: Option<(f64, f64)>,
    pub clip_files: Vec<String>,
}

/// Dumps `dir`'s `event.json` plus the other files present in the
/// folder. Only a missing/unreadable `event.json` is fatal — this is
/// the one piece `inspect` cannot proceed without.
pub fn inspect_tesla_event(dir: &Path) -> Result<TeslaEventDump> {
    let event_json_path = dir.join("event.json");
    let text = fs::read_to_string(&event_json_path).map_err(|e| Error::io(&event_json_path, e))?;
    let value: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);

    let timestamp = value
        .get("timestamp")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let reason = value
        .get("reason")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let city = value
        .get("city")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let lat = value
        .get("est_lat")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<f64>().ok());
    let lon = value
        .get("est_lon")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<f64>().ok());
    let coordinates = lat.zip(lon);

    let mut clip_files: Vec<String> = fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.is_file() && p.file_name().and_then(|n| n.to_str()) != Some("event.json")
                })
                .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                .collect()
        })
        .unwrap_or_default();
    clip_files.sort();

    Ok(TeslaEventDump {
        path: dir.to_path_buf(),
        timestamp,
        reason,
        city,
        coordinates,
        clip_files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- classify ---

    #[test]
    fn classifies_mp4_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("GX010123.MP4");
        fs::write(&path, b"").unwrap();
        assert!(matches!(classify(&path), Ok(InspectTarget::Mp4(_))));
    }

    #[test]
    fn classifies_tesla_event_folder() {
        let dir = tempfile::tempdir().unwrap();
        let event_dir = dir.path().join("2026-07-09_08-15-30");
        fs::create_dir_all(&event_dir).unwrap();
        fs::write(event_dir.join("event.json"), "{}").unwrap();
        assert!(matches!(
            classify(&event_dir),
            Ok(InspectTarget::TeslaEvent(_))
        ));
    }

    #[test]
    fn classifies_bare_event_json_by_its_parent_folder() {
        let dir = tempfile::tempdir().unwrap();
        let event_dir = dir.path().join("2026-07-09_08-15-30");
        fs::create_dir_all(&event_dir).unwrap();
        let event_json = event_dir.join("event.json");
        fs::write(&event_json, "{}").unwrap();
        let Ok(InspectTarget::TeslaEvent(resolved)) = classify(&event_json) else {
            panic!("expected TeslaEvent");
        };
        assert_eq!(resolved, event_dir);
    }

    #[test]
    fn unsupported_input_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.txt");
        fs::write(&path, b"hello").unwrap();
        assert!(classify(&path).is_err());
    }

    // --- MP4 dump fixtures (mirrors media::mp4's test builders) ---

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

    fn hmmt(offsets: &[u32]) -> Vec<u8> {
        let mut payload = Vec::with_capacity(4 + offsets.len() * 4);
        payload.extend_from_slice(&(offsets.len() as u32).to_be_bytes());
        for offset in offsets {
            payload.extend_from_slice(&offset.to_be_bytes());
        }
        make_box(b"HMMT", &payload)
    }

    const MAC_EPOCH_OFFSET_SECS: i64 = 2_082_844_800;

    fn mvhd_v0(creation_time: u32) -> Vec<u8> {
        let mut payload = vec![0u8; 4];
        payload.extend_from_slice(&creation_time.to_be_bytes());
        make_box(b"mvhd", &payload)
    }

    fn mac_time(ts: &str) -> u32 {
        (ts.parse::<Timestamp>().unwrap().as_second() + MAC_EPOCH_OFFSET_SECS) as u32
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

    fn gps_payload(gpsu: &str, lat: i32, lon: i32) -> Vec<u8> {
        let scal = klv_item(
            b"SCAL",
            b'l',
            4,
            &be_i32s(&[10_000_000, 10_000_000, 1000, 1000, 1000]),
        );
        let gpsu_klv = klv_item(b"GPSU", b'U', 16, gpsu_string(gpsu).as_bytes());
        let gpsf = klv_item(b"GPSF", b'L', 4, &3u32.to_be_bytes());
        let gpsp = klv_item(b"GPSP", b'S', 2, &150u16.to_be_bytes());
        let gps5 = klv_item(b"GPS5", b'l', 4, &be_i32s(&[lat, lon, 100_000, 0, 0]));
        let strm = nested(b"STRM", &[scal, gpsu_klv, gpsf, gpsp, gps5]);
        nested(b"DEVC", &[strm])
    }

    /// Writes a synthetic HERO8 chapter: `moov/mvhd`, optionally
    /// `moov/udta/HMMT`, and — when `gpmd_payload` is given — a `gpmd`
    /// track with one sample.
    fn write_chapter(
        path: &Path,
        creation_time: &str,
        markers: &[u32],
        gpmd_payload: Option<Vec<u8>>,
    ) {
        let mut moov_children = vec![mvhd_v0(mac_time(creation_time))];
        if !markers.is_empty() {
            moov_children.push(make_container(b"udta", &[hmmt(markers)]));
        }

        let mut trailer = None;
        if let Some(payload) = &gpmd_payload {
            const SENTINEL: u32 = 0xAB19_2F03;
            let stbl = make_container(
                b"stbl",
                &[
                    stsd(b"gpmd"),
                    stsz(&[payload.len() as u32]),
                    stsc(&[(1, 1, 1)]),
                    stco(&[SENTINEL]),
                    stts(&[(1, 1000)]),
                ],
            );
            let minf = make_container(b"minf", &[stbl]);
            let mdia = make_container(b"mdia", &[hdlr(b"meta"), mdhd(1000), minf]);
            let trak = make_container(b"trak", &[mdia]);
            moov_children.push(trak);
            trailer = Some((SENTINEL, payload.clone()));
        }

        let mut moov_bytes = make_container(b"moov", &moov_children);

        if let Some((sentinel, payload)) = trailer {
            let real_offset = moov_bytes.len() as u32;
            let marker = sentinel.to_be_bytes();
            let pos = moov_bytes
                .windows(4)
                .position(|w| w == marker)
                .expect("stco sentinel not found");
            moov_bytes[pos..pos + 4].copy_from_slice(&real_offset.to_be_bytes());
            moov_bytes.extend_from_slice(&payload);
        }

        fs::write(path, moov_bytes).unwrap();
    }

    #[test]
    fn mp4_dump_reports_hilights_and_gps() {
        let dir = tempfile::tempdir().unwrap();
        let chapter = dir.path().join("GX010123.MP4");
        let payload = gps_payload("2026-07-09T07:41:00Z", 515_012_340, -1_234_567);
        write_chapter(&chapter, "2026-07-09T07:41:03Z", &[5000], Some(payload));

        let dump = inspect_mp4(&chapter).unwrap();
        assert!(!dump.has_errors());
        assert_eq!(
            dump.creation_time.unwrap().to_string(),
            "2026-07-09T07:41:03Z"
        );
        let markers = dump.markers.unwrap();
        assert_eq!(markers.len(), 1);
        assert_eq!(markers[0].offset_ms, 5000);
        assert!(markers[0].timestamp.is_some());

        let gps = dump.gps.unwrap().unwrap();
        assert_eq!(gps.sample_count, 1);
        let (lat, lon) = gps.first_fix.unwrap();
        assert!((lat - 51.5012340).abs() < 1e-9);
        assert!((lon - (-0.1234567)).abs() < 1e-9);
        assert!(gps.clock_offset_s.is_some());
    }

    #[test]
    fn mp4_dump_without_gpmd_track_has_no_gps_section() {
        let dir = tempfile::tempdir().unwrap();
        let chapter = dir.path().join("GX010124.MP4");
        write_chapter(&chapter, "2026-07-09T07:41:03Z", &[], None);

        let dump = inspect_mp4(&chapter).unwrap();
        assert!(!dump.has_errors());
        assert!(dump.gps.unwrap().is_none());
        assert!(dump.markers.unwrap().is_empty());
    }

    #[test]
    fn corrupt_gpmd_track_reports_partial_output() {
        // stsz declares more samples than stsc/stco can place —
        // read_gpmd_index errors, but HMMT and mvhd still parse fine.
        let dir = tempfile::tempdir().unwrap();
        let chapter = dir.path().join("GX010125.MP4");

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
        let trak = make_container(b"trak", &[mdia]);
        let moov = make_container(
            b"moov",
            &[
                mvhd_v0(mac_time("2026-07-09T07:41:03Z")),
                hmmt(&[1000]),
                trak,
            ],
        );
        fs::write(&chapter, moov).unwrap();

        let dump = inspect_mp4(&chapter).unwrap();
        assert!(dump.has_errors(), "corrupt gpmd must surface as an error");
        assert!(dump.creation_time.is_ok(), "creation time still parses");
        assert!(dump.markers.is_ok(), "HiLights still parse");
        assert!(dump.gps.is_err(), "GPS section reports the parse failure");
    }

    // --- Tesla event dump ---

    #[test]
    fn tesla_dump_parses_event_json_and_lists_clips() {
        let dir = tempfile::tempdir().unwrap();
        let event_dir = dir.path().join("2026-07-09_08-15-30");
        fs::create_dir_all(&event_dir).unwrap();
        fs::write(
            event_dir.join("event.json"),
            r#"{"timestamp":"2026-07-09T08:15:30","city":"London","est_lat":"51.5012","est_lon":"-0.1246","reason":"user_interaction_honk"}"#,
        )
        .unwrap();
        fs::write(event_dir.join("2026-07-09_08-15-30-front.mp4"), b"front").unwrap();
        fs::write(event_dir.join("thumb.png"), b"thumb").unwrap();

        let dump = inspect_tesla_event(&event_dir).unwrap();
        assert_eq!(dump.timestamp.as_deref(), Some("2026-07-09T08:15:30"));
        assert_eq!(dump.reason.as_deref(), Some("user_interaction_honk"));
        assert_eq!(dump.city.as_deref(), Some("London"));
        let (lat, lon) = dump.coordinates.unwrap();
        assert!((lat - 51.5012).abs() < 1e-9);
        assert!((lon - (-0.1246)).abs() < 1e-9);
        assert_eq!(
            dump.clip_files,
            vec![
                "2026-07-09_08-15-30-front.mp4".to_string(),
                "thumb.png".to_string()
            ]
        );
    }

    #[test]
    fn tesla_dump_tolerates_corrupt_event_json() {
        let dir = tempfile::tempdir().unwrap();
        let event_dir = dir.path().join("2026-07-09_08-15-30");
        fs::create_dir_all(&event_dir).unwrap();
        fs::write(event_dir.join("event.json"), "{not valid json").unwrap();

        let dump = inspect_tesla_event(&event_dir).unwrap();
        assert!(dump.timestamp.is_none());
        assert!(dump.reason.is_none());
    }
}
