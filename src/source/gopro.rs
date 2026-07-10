//! GoPro HERO8 device module (roadmap changeset 2): card detection,
//! chapter-file session grouping, HiLight-marker-driven keep/quarantine
//! verdicts, and the `import.json` sidecar. See
//! `openspec/changes/add-gopro-import/design.md` for the decisions
//! (D1-D8) this module implements.

use std::collections::HashMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

use globset::GlobSet;
use jiff::civil::DateTime as CivilDateTime;
use jiff::tz::TimeZone;
use jiff::{Span, Timestamp};
use serde_json::json;

use crate::error::Result;
use crate::media::{gpmf, mp4};
use crate::source::sidecar::{self, EventEntry, SidecarEnvelope};
use crate::source::{ImportSource, Marker, MediaFile, MediaGroup, ScanContext, Verdict};

/// This changeset only claims HERO8 (design Non-Goals); the sidecar
/// records it verbatim so a future multi-model changeset has a field
/// to branch on.
const CAMERA_MODEL: &str = "gopro-hero8";

pub struct GoproSource {
    pub require_marker: bool,
}

impl ImportSource for GoproSource {
    fn detect(&self, root: &Path) -> bool {
        for dir in gopro_dirs(&root.join("DCIM")) {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            let has_chapter = entries.flatten().any(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| parse_chapter_name(name).is_some())
            });
            if has_chapter {
                return true;
            }
        }
        false
    }

    fn scan(&self, root: &Path, ctx: &ScanContext) -> Result<Vec<(MediaGroup, Verdict)>> {
        let (sessions, unrecognized) = discover(&root.join("DCIM"), ctx.ignore);

        let mut groups = Vec::with_capacity(sessions.len() + 1);
        let mut session_ids: Vec<&String> = sessions.keys().collect();
        session_ids.sort();

        for session_id in session_ids {
            let mut chapters = sessions[session_id].clone();
            chapters.sort_by_key(|(chapter, _)| *chapter);
            groups.push(self.build_session(session_id, &chapters, ctx));
        }

        if !unrecognized.is_empty() {
            groups.push((
                MediaGroup {
                    name: "unrecognized".to_string(),
                    files: unrecognized
                        .into_iter()
                        .map(|path| MediaFile {
                            size: fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
                            path,
                            recorded_at: None,
                        })
                        .collect(),
                    timestamp: Timestamp::UNIX_EPOCH,
                    markers: Vec::new(),
                    geo: None,
                    context: HashMap::new(),
                    sidecar: None,
                },
                Verdict::Ignore("unrecognized file(s)".to_string()),
            ));
        }

        Ok(groups)
    }
}

impl GoproSource {
    /// Builds one session's `MediaGroup` and verdict from its
    /// chapter-ordered file list (design D3, D5-D8), correcting for
    /// GPS telemetry when it's available (design D3-D7).
    fn build_session(
        &self,
        session_id: &str,
        chapters: &[(u32, PathBuf)],
        ctx: &ScanContext,
    ) -> (MediaGroup, Verdict) {
        let chapter_civil_times: Vec<CivilDateTime> = chapters
            .iter()
            .map(|(_, path)| chapter_civil_time(path))
            .collect();

        let mut telemetry: Vec<Option<ChapterTelemetry>> = chapters
            .iter()
            .map(|(_, path)| open_chapter_telemetry(path))
            .collect();

        // Convert camera-clock civil times to instants. For GPS-corrected
        // sessions the GPS path overrides these later; for camera-clock
        // sessions these ARE the instants — interpreted in ctx.tz so the
        // civil reading maps to the right instant (design D3).
        let chapter_times: Vec<Timestamp> = chapter_civil_times
            .iter()
            .map(|&dt| civil_to_instant(dt, ctx.tz))
            .collect();

        let session_offset = derive_session_offset(chapters, &chapter_times, &mut telemetry);

        let mut all_markers: Vec<MarkerHit> = Vec::new();
        for (i, (_, path)) in chapters.iter().enumerate() {
            for offset_ms in chapter_markers(path) {
                let camera_wall_time =
                    chapter_times[i] + Span::new().milliseconds(offset_ms as i64);
                let (wall_time, coords) = match &session_offset {
                    Some(t) => {
                        let corrected = camera_wall_time + span_from_secs(t.offset_s);
                        let coords = telemetry[i]
                            .as_mut()
                            .and_then(|tel| marker_coordinates(tel, offset_ms as f64 / 1000.0));
                        (corrected, coords)
                    }
                    None => (camera_wall_time, None),
                };
                all_markers.push(MarkerHit {
                    offset_ms,
                    wall_time,
                    coords,
                    file: file_name(path),
                });
            }
        }

        let verdict = if !self.require_marker || !all_markers.is_empty() {
            Verdict::Keep
        } else {
            Verdict::Quarantine
        };

        // Session timestamp: GPS-corrected when telemetry provided an
        // offset (already a real instant); otherwise the first chapter's
        // camera-clock civil time interpreted in ctx.tz (design D3 /
        // unify-timestamps D3).
        let session_timestamp = match &session_offset {
            Some(t) => chapter_times[0] + span_from_secs(t.offset_s),
            None => chapter_times[0], // already interpreted in ctx.tz above
        };

        let files: Vec<MediaFile> = chapters
            .iter()
            .zip(&chapter_times)
            .map(|((_, path), &camera_time)| {
                let recorded_at = match &session_offset {
                    Some(t) => camera_time + span_from_secs(t.offset_s),
                    None => camera_time,
                };
                MediaFile {
                    size: fs::metadata(path).map(|m| m.len()).unwrap_or(0),
                    path: path.clone(),
                    recorded_at: Some(recorded_at),
                }
            })
            .collect();

        let markers: Vec<Marker> = all_markers
            .iter()
            .map(|hit| Marker {
                timestamp: hit.wall_time,
                label: None,
            })
            .collect();

        let mut context = HashMap::new();
        context.insert("session".to_string(), session_id.to_string());

        let sidecar = (verdict == Verdict::Keep).then(|| {
            build_sidecar(
                session_id,
                chapters,
                &all_markers,
                &session_offset,
                session_timestamp,
                ctx,
            )
        });

        (
            MediaGroup {
                name: format!("session-{session_id}"),
                files,
                timestamp: session_timestamp,
                markers,
                geo: session_offset.as_ref().map(|t| t.geo),
                context,
                sidecar,
            },
            verdict,
        )
    }
}

/// A HiLight marker with enough detail to render into the sidecar
/// (design D6) — richer than the core `Marker` type, which only needs
/// a wall-clock timestamp.
struct MarkerHit {
    offset_ms: u32,
    /// Corrected (GPS) or camera-clock wall time, matching whichever
    /// `session_offset` produced it.
    wall_time: Timestamp,
    /// Nearest usable GPS fix (design D5); `None` when telemetry is
    /// unavailable or no usable fix was found near this marker.
    coords: Option<(f64, f64)>,
    /// Base name of the chapter file this marker was pressed in
    /// (design D5). Captured at construction so `build_sidecar` can
    /// attribute each event to its originating clip.
    file: String,
}

fn build_sidecar(
    session_id: &str,
    chapters: &[(u32, PathBuf)],
    markers: &[MarkerHit],
    session_offset: &Option<SessionTelemetry>,
    session_timestamp: Timestamp,
    ctx: &ScanContext,
) -> super::Sidecar {
    let files: Vec<String> = chapters.iter().map(|(_, path)| file_name(path)).collect();

    let events_json: Vec<EventEntry> = markers
        .iter()
        .map(|hit| EventEntry {
            event_type: "gopro:marker".to_string(),
            time: hit.wall_time,
            lat: hit.coords.map(|(lat, _)| lat),
            lon: hit.coords.map(|(_, lon)| lon),
            reason: None,
            offset_ms: Some(hit.offset_ms),
            file: Some(hit.file.clone()),
        })
        .collect();

    let mut gopro_block = json!({ "session": session_id });
    if let Some(t) = session_offset {
        gopro_block["clock_offset_s"] = json!(t.offset_s);
    }

    let tz_name = ctx.tz.iana_name().unwrap_or("").to_string();
    let source_dir = chapters
        .first()
        .and_then(|(_, p)| p.parent())
        .map(|d| d.display().to_string())
        .unwrap_or_default();

    sidecar::build(
        ctx.tz,
        SidecarEnvelope {
            camera: CAMERA_MODEL,
            source: source_dir,
            imported_at: ctx.imported_at,
            timezone_name: tz_name,
            recorded_at: session_timestamp,
            time_source: if session_offset.is_some() {
                "gps"
            } else {
                "camera"
            },
            files,
        },
        events_json,
        Some(("gopro", gopro_block)),
    )
}

/// The session-wide GPS correction (gopro-telemetry design D4/D7): a
/// single camera-clock offset plus the fix it was derived from, used
/// as the session's location.
struct SessionTelemetry {
    offset_s: f64,
    geo: (f64, f64),
}

fn span_from_secs(secs: f64) -> Span {
    Span::new().milliseconds((secs * 1000.0).round() as i64)
}

/// One chapter's `gpmd` sample index plus the open file handle needed
/// to fetch payloads on demand (gopro-telemetry design D2/D3).
struct ChapterTelemetry {
    file: File,
    index: Vec<mp4::GpmdSample>,
}

impl ChapterTelemetry {
    /// `Ok(None)` covers both "file can't be opened as MP4" in a way
    /// that's really "missing gpmd" and, more precisely, the clean
    /// track-not-found case from `mp4::read_gpmd_index`.
    fn open(path: &Path) -> std::result::Result<Option<Self>, String> {
        let mut file = File::open(path).map_err(|e| e.to_string())?;
        let index = mp4::read_gpmd_index(&mut file).map_err(|e| e.to_string())?;
        Ok(index.map(|index| ChapterTelemetry { file, index }))
    }

    fn payload(
        &mut self,
        sample: &mp4::GpmdSample,
    ) -> std::result::Result<Option<gpmf::GpsPayload>, String> {
        let bytes = mp4::read_gpmd_payload(&mut self.file, sample).map_err(|e| e.to_string())?;
        gpmf::parse_gps_payload(&bytes).map_err(|e| e.to_string())
    }

    /// The first usable payload carrying `GPSU`, in index order
    /// (gopro-telemetry design D4), together with the sample it came
    /// from (its stream time anchors the offset calculation).
    fn first_good_fix(
        &mut self,
    ) -> std::result::Result<Option<(mp4::GpmdSample, gpmf::GpsPayload)>, String> {
        for i in 0..self.index.len() {
            let sample = self.index[i];
            if let Some(payload) = self.payload(&sample)?
                && payload.usable()
                && payload.utc.is_some()
            {
                return Ok(Some((sample, payload)));
            }
        }
        Ok(None)
    }
}

/// Opens a chapter's telemetry, warning (and treating it as
/// untelemetered) on any I/O or parse failure — a file with simply no
/// `gpmd` track is `Ok(None)` and never warns (gopro-telemetry design
/// D6, spec: "File without telemetry yields no-telemetry").
fn open_chapter_telemetry(path: &Path) -> Option<ChapterTelemetry> {
    match ChapterTelemetry::open(path) {
        Ok(telemetry) => telemetry,
        Err(error) => {
            tracing::warn!(
                file = %path.display(),
                %error,
                "could not read GPMF telemetry; treating chapter as untelemetered"
            );
            None
        }
    }
}

/// Scans chapters in order for the first usable `GPSU` fix (design
/// D4): the first chapter to yield one wins, and its offset is applied
/// session-wide. A parse failure on one chapter's telemetry is logged
/// and treated the same as that chapter having none, so a later
/// chapter can still supply the offset (spec: "Offset from a later
/// chapter").
fn derive_session_offset(
    chapters: &[(u32, PathBuf)],
    chapter_times: &[Timestamp],
    telemetry: &mut [Option<ChapterTelemetry>],
) -> Option<SessionTelemetry> {
    for (i, (_, path)) in chapters.iter().enumerate() {
        let Some(chapter_telemetry) = telemetry[i].as_mut() else {
            continue;
        };
        let fix = match chapter_telemetry.first_good_fix() {
            Ok(fix) => fix,
            Err(error) => {
                tracing::warn!(
                    file = %path.display(),
                    %error,
                    "GPMF telemetry parse failed; skipping this chapter's fix"
                );
                None
            }
        };
        let Some((sample, payload)) = fix else {
            continue;
        };
        let (Some(utc), Some(gps_sample)) = (payload.utc, payload.samples.first()) else {
            continue;
        };

        let reference = chapter_times[i] + span_from_secs(sample.time_s);
        let offset_s = utc.duration_since(reference).as_secs_f64();
        return Some(SessionTelemetry {
            offset_s,
            geo: (gps_sample.lat, gps_sample.lon),
        });
    }
    None
}

/// Finds coordinates for a marker at `offset_s` (its chapter's stream
/// time) per gopro-telemetry design D5: the payload covering that
/// time, or — if unusable — the nearest usable payload within ±2
/// payloads, preferring the closer candidates first.
fn marker_coordinates(telemetry: &mut ChapterTelemetry, offset_s: f64) -> Option<(f64, f64)> {
    let index = telemetry.index.clone();
    let covering = covering_sample_index(&index, offset_s)?;

    for delta in [0i64, 1, -1, 2, -2] {
        let j = covering as i64 + delta;
        if j < 0 || j as usize >= index.len() {
            continue;
        }
        let sample = index[j as usize];
        let Ok(Some(payload)) = telemetry.payload(&sample) else {
            continue;
        };
        if !payload.usable() {
            continue;
        }
        if let Some(coords) = nearest_sample_coords(&payload, &sample, offset_s) {
            return Some(coords);
        }
    }
    None
}

/// The index of the sample whose `[time_s, time_s + duration_s)`
/// interval covers `offset_s`.
fn covering_sample_index(index: &[mp4::GpmdSample], offset_s: f64) -> Option<usize> {
    index
        .iter()
        .position(|s| offset_s >= s.time_s && offset_s < s.time_s + s.duration_s)
}

/// Within one payload, the `GPS5` sample nearest `offset_s` assuming
/// uniform spacing across the payload's duration (design D5).
fn nearest_sample_coords(
    payload: &gpmf::GpsPayload,
    sample: &mp4::GpmdSample,
    offset_s: f64,
) -> Option<(f64, f64)> {
    let n = payload.samples.len();
    if n == 0 {
        return None;
    }
    let frac = if sample.duration_s > 0.0 {
        (offset_s - sample.time_s) / sample.duration_s
    } else {
        0.0
    };
    let idx = (frac * (n as f64 - 1.0)).round().clamp(0.0, (n - 1) as f64) as usize;
    let gps_sample = payload.samples[idx];
    Some((gps_sample.lat, gps_sample.lon))
}

/// Chapter files sharing a session number, each tagged with its
/// chapter number for ordering (design D3).
type SessionChapters = HashMap<String, Vec<(u32, PathBuf)>>;

/// Discovers chapter files across every `1*GOPRO` directory under
/// `dcim`, grouped by session number (design D3), plus every file that
/// matched neither an ignore glob nor the chapter pattern.
fn discover(dcim: &Path, ignore: &GlobSet) -> (SessionChapters, Vec<PathBuf>) {
    let mut sessions: HashMap<String, Vec<(u32, PathBuf)>> = HashMap::new();
    let mut unrecognized = Vec::new();

    for dir in gopro_dirs(dcim) {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if ignore.is_match(&path) {
                continue;
            }
            let matched = path
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(parse_chapter_name);
            match matched {
                Some((chapter, session_id)) => {
                    sessions
                        .entry(session_id)
                        .or_default()
                        .push((chapter, path));
                }
                None => unrecognized.push(path),
            }
        }
    }

    (sessions, unrecognized)
}

/// Lists `dcim`'s immediate subdirectories matching the `1*GOPRO`
/// naming GoPro cards use (e.g. `100GOPRO`, `101GOPRO`); returns
/// nothing if `dcim` doesn't exist.
fn gopro_dirs(dcim: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dcim) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(is_gopro_dir_name)
        })
        .collect()
}

fn is_gopro_dir_name(name: &str) -> bool {
    name.starts_with('1') && name.ends_with("GOPRO")
}

/// Parses a HERO8 chapter file name (`G[XH]ccnnnn.MP4`, case-
/// insensitive extension) into its chapter and session numbers
/// (design D3). `cc`/`nnnn` are kept as strings/`u32` respectively:
/// the session number is a layout-context value (always a string),
/// the chapter number is only ever used to sort.
fn parse_chapter_name(file_name: &str) -> Option<(u32, String)> {
    if file_name.len() != 12 {
        return None;
    }
    let (stem, ext) = file_name.split_at(8);
    if !ext.eq_ignore_ascii_case(".mp4") {
        return None;
    }
    let stem = stem.as_bytes();
    if stem[0] != b'G' || (stem[1] != b'X' && stem[1] != b'H') {
        return None;
    }
    let digits = std::str::from_utf8(&stem[2..8]).ok()?;
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let chapter: u32 = digits[0..2].parse().ok()?;
    Some((chapter, digits[2..6].to_string()))
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Converts a camera-clock civil datetime to a real instant by
/// interpreting it in the configured timezone (design D3 /
/// unify-timestamps D3). Camera clocks record local time; without
/// this step, a non-UTC zone would double-shift the wall reading.
fn civil_to_instant(dt: CivilDateTime, tz: &TimeZone) -> Timestamp {
    dt.to_zoned(tz.clone())
        .map(|z| z.timestamp())
        .unwrap_or_else(|_| dt.to_zoned(TimeZone::UTC).unwrap().timestamp())
}

/// The camera-clock civil creation time for one chapter (design D5):
/// `moov/mvhd` if it can be read, else the file's modification time
/// converted back to a civil datetime using UTC (a best-effort
/// fallback — the mtime is a real instant, not a wall clock, so we
/// return it as a civil value at UTC). A chapter that can't be
/// timestamped precisely still gets imported rather than blocking the
/// run.
fn chapter_civil_time(path: &Path) -> CivilDateTime {
    match File::open(path)
        .map_err(|e| e.to_string())
        .and_then(|mut f| mp4::read_creation_time(&mut f).map_err(|e| e.to_string()))
    {
        // mp4::read_creation_time already returns a Timestamp; convert it to
        // CivilDateTime in UTC as a best proxy for the camera's civil reading
        // (the real MVHD value is a civil time, but the existing API returns
        // it as a Timestamp — round-trip via UTC gives the same civil value
        // the camera wrote).
        Ok(timestamp) => timestamp.to_zoned(TimeZone::UTC).datetime(),
        Err(error) => {
            let fallback = fs::metadata(path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|modified| Timestamp::try_from(modified).ok())
                .unwrap_or(Timestamp::UNIX_EPOCH)
                .to_zoned(TimeZone::UTC)
                .datetime();
            tracing::warn!(
                file = %path.display(),
                %error,
                "could not read camera-clock creation time; using file modification time instead"
            );
            fallback
        }
    }
}

/// HiLight marker offsets for one chapter (design D7): a parse failure
/// degrades to zero markers with a warning rather than aborting the
/// scan.
fn chapter_markers(path: &Path) -> Vec<u32> {
    match File::open(path)
        .map_err(|e| e.to_string())
        .and_then(|mut f| mp4::read_hilights(&mut f).map_err(|e| e.to_string()))
    {
        Ok(offsets) => offsets,
        Err(error) => {
            tracing::warn!(
                file = %path.display(),
                %error,
                "could not parse HiLight markers; treating chapter as unmarked"
            );
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::ScanContext;

    /// A deterministic `ScanContext` for tests: UTC zone, epoch
    /// imported_at, empty ignore set.
    fn test_ctx_with_tz(tz: TimeZone) -> (globset::GlobSet, TimeZone, Timestamp) {
        (
            globset::GlobSetBuilder::new().build().unwrap(),
            tz,
            Timestamp::UNIX_EPOCH,
        )
    }

    fn make_ctx<'a>(
        ignore: &'a globset::GlobSet,
        tz: &'a TimeZone,
        imported_at: Timestamp,
    ) -> ScanContext<'a> {
        ScanContext {
            ignore,
            tz,
            imported_at,
        }
    }

    #[test]
    fn parses_hero8_chapter_names() {
        assert_eq!(
            parse_chapter_name("GX010123.MP4"),
            Some((1, "0123".to_string()))
        );
        assert_eq!(
            parse_chapter_name("GH020123.mp4"),
            Some((2, "0123".to_string()))
        );
    }

    #[test]
    fn rejects_non_chapter_names() {
        assert_eq!(parse_chapter_name("GOPR0042.JPG"), None);
        assert_eq!(parse_chapter_name("GX010123.LRV"), None);
        assert_eq!(parse_chapter_name("GX0101234.MP4"), None);
        assert_eq!(parse_chapter_name("random.txt"), None);
    }

    #[test]
    fn gopro_dir_name_matches_1_star_gopro() {
        assert!(is_gopro_dir_name("100GOPRO"));
        assert!(is_gopro_dir_name("101GOPRO"));
        assert!(!is_gopro_dir_name("MISC"));
        assert!(!is_gopro_dir_name("200GOPRO"));
    }

    #[test]
    fn chapters_group_by_session_across_gopro_dirs() {
        // Spec: "Session split across DCIM subdirectories" — chapters
        // sharing a session number in different 1*GOPRO dirs form one
        // group (design D3).
        let dir = tempfile::tempdir().unwrap();
        let dcim = dir.path().join("DCIM");
        fs::create_dir_all(dcim.join("100GOPRO")).unwrap();
        fs::create_dir_all(dcim.join("101GOPRO")).unwrap();
        fs::write(dcim.join("100GOPRO/GX010200.MP4"), b"").unwrap();
        fs::write(dcim.join("101GOPRO/GX020200.MP4"), b"").unwrap();

        let (sessions, unrecognized) = discover(&dcim, &GlobSet::empty());
        assert!(unrecognized.is_empty());
        assert_eq!(sessions.len(), 1, "both chapters share one session");
        let chapters = &sessions["0200"];
        assert_eq!(chapters.len(), 2);
        let names: Vec<String> = chapters.iter().map(|(_, p)| file_name(p)).collect();
        assert!(names.contains(&"GX010200.MP4".to_string()));
        assert!(names.contains(&"GX020200.MP4".to_string()));
    }

    #[test]
    fn require_marker_false_keeps_unmarked_session() {
        // Spec: "require_marker false keeps everything" (design D8) — a
        // session with no markers is Keep, and still gets a sidecar.
        let source = GoproSource {
            require_marker: false,
        };
        let dir = tempfile::tempdir().unwrap();
        let chapter = dir.path().join("GX010124.MP4");
        fs::write(&chapter, b"").unwrap();

        let (ignore, tz, imported_at) = test_ctx_with_tz(TimeZone::UTC);
        let ctx = make_ctx(&ignore, &tz, imported_at);
        let (group, verdict) = source.build_session("0124", &[(1, chapter)], &ctx);
        assert_eq!(verdict, Verdict::Keep);
        assert!(group.markers.is_empty());
        assert!(
            group.sidecar.is_some(),
            "kept sessions carry an import.json even with zero markers"
        );
        assert_eq!(
            group.sidecar.unwrap().filename,
            "import.json",
            "sidecar must be named import.json, not markers.json"
        );
    }

    #[test]
    fn detects_hero8_card_layout() {
        let dir = tempfile::tempdir().unwrap();
        let gopro_dir = dir.path().join("DCIM/100GOPRO");
        fs::create_dir_all(&gopro_dir).unwrap();
        fs::write(gopro_dir.join("GX010123.MP4"), b"").unwrap();

        let source = GoproSource {
            require_marker: true,
        };
        assert!(source.detect(dir.path()));
    }

    #[test]
    fn does_not_detect_empty_dcim() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("DCIM")).unwrap();

        let source = GoproSource {
            require_marker: true,
        };
        assert!(!source.detect(dir.path()));
    }

    #[test]
    fn does_not_detect_teslacam_root() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("TeslaCam/SavedClips")).unwrap();

        let source = GoproSource {
            require_marker: true,
        };
        assert!(!source.detect(dir.path()));
    }

    // --- GPS telemetry fixtures & tests (gopro-telemetry design D3-D7) ---

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

    /// A `yymmddhhmmss.sss` GPSU string for `ts` (must be a valid RFC
    /// 3339 UTC instant).
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

    /// The `mvhd`-encoded (MAC-epoch) creation time for `ts`.
    fn mac_time(ts: &str) -> u32 {
        (ts.parse::<Timestamp>().unwrap().as_second() + MAC_EPOCH_OFFSET_SECS) as u32
    }

    /// One GPMF payload: `DEVC { STRM { SCAL, GPSU?, GPSF, GPSP, GPS5 } }`.
    /// `usable` controls `GPSF`/`GPSP` so tests can produce payloads that
    /// fail fix-quality gating (design D3) without needing bogus samples.
    fn gps_payload(gpsu: Option<&str>, usable: bool, gps5: &[[i32; 5]]) -> Vec<u8> {
        let scal = klv_item(
            b"SCAL",
            b'l',
            4,
            &be_i32s(&[10_000_000, 10_000_000, 1000, 1000, 1000]),
        );
        let (fix, precision) = if usable {
            (3u32, 150u32)
        } else {
            (0u32, 9999u32)
        };
        let mut strm_children = vec![scal];
        if let Some(ts) = gpsu {
            strm_children.push(klv_item(b"GPSU", b'U', 16, gpsu_string(ts).as_bytes()));
        }
        strm_children.push(klv_item(b"GPSF", b'L', 4, &fix.to_be_bytes()));
        strm_children.push(klv_item(
            b"GPSP",
            b'S',
            2,
            &(precision as u16).to_be_bytes(),
        ));
        let raw: Vec<i32> = gps5.iter().flat_map(|s| s.iter().copied()).collect();
        strm_children.push(klv_item(b"GPS5", b'l', 4, &be_i32s(&raw)));
        let strm = nested(b"STRM", &strm_children);
        nested(b"DEVC", &[strm])
    }

    /// Writes a synthetic HERO8 chapter file: `moov/mvhd`, optionally
    /// `moov/udta/HMMT`, and — when `gpmd_payloads` is non-empty — a
    /// `gpmd` track with one payload per (1-second-spaced) sample.
    fn write_chapter(
        path: &Path,
        creation_time: &str,
        marker_offsets_ms: &[u32],
        gpmd_payloads: &[Vec<u8>],
    ) {
        let mut moov_children = vec![mvhd_v0(mac_time(creation_time))];
        if !marker_offsets_ms.is_empty() {
            moov_children.push(make_container(b"udta", &[hmmt(marker_offsets_ms)]));
        }

        let mut trailing_payloads = Vec::new();
        if !gpmd_payloads.is_empty() {
            let sentinels: Vec<u32> = (0..gpmd_payloads.len() as u32)
                .map(|i| 0xA000_0000 + i)
                .collect();
            let sizes: Vec<u32> = gpmd_payloads.iter().map(|p| p.len() as u32).collect();
            let stsc_entries: Vec<(u32, u32, u32)> = (0..gpmd_payloads.len() as u32)
                .map(|i| (i + 1, 1, 1))
                .collect();
            let stbl = make_container(
                b"stbl",
                &[
                    stsd(b"gpmd"),
                    stsz(&sizes),
                    stsc(&stsc_entries),
                    stco(&sentinels),
                    stts(&[(gpmd_payloads.len() as u32, 1000)]),
                ],
            );
            let minf = make_container(b"minf", &[stbl]);
            let mdia = make_container(b"mdia", &[hdlr(b"meta"), mdhd(1000), minf]);
            let trak = make_container(b"trak", &[mdia]);
            moov_children.push(trak);
            trailing_payloads = sentinels
                .into_iter()
                .zip(gpmd_payloads.iter().cloned())
                .collect();
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

        fs::write(path, moov_bytes).unwrap();
    }

    #[test]
    fn offset_math_corrects_drifted_camera_clock() {
        // Camera clock reads 2026-07-10T00:20 (drift + local-time
        // absorption, gopro-telemetry design D4); GPS says the true UTC
        // instant is an hour and 12 seconds earlier. The single payload
        // (time_s 0) supplies the fix, so the corrected session
        // timestamp lands exactly on the GPSU instant.
        let dir = tempfile::tempdir().unwrap();
        let chapter = dir.path().join("chapter.mp4");
        let payload = gps_payload(
            Some("2026-07-09T23:19:48Z"),
            true,
            &[[515_012_340, -1_234_567, 100_000, 0, 0]],
        );
        write_chapter(&chapter, "2026-07-10T00:20:00Z", &[500], &[payload]);

        let source = GoproSource {
            require_marker: true,
        };
        let (ignore, tz, imported_at) = test_ctx_with_tz(TimeZone::UTC);
        let ctx = make_ctx(&ignore, &tz, imported_at);
        let (group, verdict) = source.build_session("0001", &[(1, chapter)], &ctx);

        assert_eq!(verdict, Verdict::Keep);
        assert_eq!(
            group.timestamp,
            "2026-07-09T23:19:48Z".parse::<Timestamp>().unwrap()
        );
        let (lat, lon) = group.geo.expect("session should have a GPS fix");
        assert!((lat - 51.5012340).abs() < 1e-9);
        assert!((lon - (-0.1234567)).abs() < 1e-9);

        let sidecar = group.sidecar.unwrap();
        assert_eq!(sidecar.content["time_source"], "gps");
        let offset = sidecar.content["gopro"]["clock_offset_s"].as_f64().unwrap();
        assert!((offset - (-3612.0)).abs() < 0.01, "offset was {offset}");

        let marker = &sidecar.content["events"][0];
        assert!(marker.get("lat").is_some(), "marker should get coordinates");
        // The marker time is the corrected instant rendered in UTC at second precision.
        let marker_time = marker["time"].as_str().unwrap();
        let parsed: Timestamp = marker_time.parse().unwrap();
        // corrected = camera_wall(00:20:00Z) + 500ms + offset(-3612s)
        //           = 00:20:00.5Z - 3612s = 23:19:48.5Z → rounded to 23:19:48Z
        let expected: Timestamp = "2026-07-09T23:19:48Z".parse().unwrap();
        assert_eq!(parsed, expected, "marker time was {marker_time}");
    }

    #[test]
    fn offset_derived_from_later_chapter() {
        // Chapter 1 has no gpmd track at all; chapter 2 does. The
        // offset comes from chapter 2's fix but still applies to the
        // whole session's (chapter 1's) timestamp (spec: "Offset from a
        // later chapter").
        let dir = tempfile::tempdir().unwrap();
        let chapter1 = dir.path().join("chapter1.mp4");
        let chapter2 = dir.path().join("chapter2.mp4");
        write_chapter(&chapter1, "2026-07-10T00:20:00Z", &[], &[]);
        let payload = gps_payload(
            Some("2026-07-10T01:00:00Z"),
            true,
            &[[515_012_340, -1_234_567, 100_000, 0, 0]],
        );
        write_chapter(&chapter2, "2026-07-10T02:00:12Z", &[], &[payload]);

        let source = GoproSource {
            require_marker: false,
        };
        let (ignore, tz, imported_at) = test_ctx_with_tz(TimeZone::UTC);
        let ctx = make_ctx(&ignore, &tz, imported_at);
        let (group, _) = source.build_session("0002", &[(1, chapter1), (2, chapter2)], &ctx);

        // offset = GPSU(01:00:00) - (chapter2 mvhd 02:00:12 + 0s) = -3612s
        // session timestamp = chapter1 mvhd (00:20:00) + offset
        assert_eq!(
            group.timestamp,
            "2026-07-09T23:19:48Z".parse::<Timestamp>().unwrap()
        );
    }

    #[test]
    fn marker_without_nearby_fix_omits_coordinates() {
        // Payload 0 supplies the session's clock offset. Payloads 1-6
        // are all unusable, so a marker whose covering payload (index
        // 4) has no usable fix within ±2 gets a corrected UTC time but
        // no coordinates (spec: "No usable fix near marker").
        let dir = tempfile::tempdir().unwrap();
        let chapter = dir.path().join("chapter.mp4");
        let sample = [515_012_340, -1_234_567, 100_000, 0, 0];
        let mut payloads = vec![gps_payload(Some("2026-07-10T00:00:00Z"), true, &[sample])];
        for _ in 1..7 {
            payloads.push(gps_payload(None, false, &[sample]));
        }
        write_chapter(&chapter, "2026-07-10T00:00:00Z", &[4500], &payloads);

        let source = GoproSource {
            require_marker: true,
        };
        let (ignore, tz, imported_at) = test_ctx_with_tz(TimeZone::UTC);
        let ctx = make_ctx(&ignore, &tz, imported_at);
        let (group, _) = source.build_session("0003", &[(1, chapter)], &ctx);

        let sidecar = group.sidecar.unwrap();
        let marker = &sidecar.content["events"][0];
        assert!(marker.get("time").is_some());
        assert!(
            marker.get("lat").is_none() && marker.get("lon").is_none(),
            "marker should have no coordinates: {marker:?}"
        );
    }

    #[test]
    fn gps_sidecar_shape_differs_from_camera_sidecar() {
        let dir = tempfile::tempdir().unwrap();

        let gps_chapter = dir.path().join("gps.mp4");
        let payload = gps_payload(
            Some("2026-07-10T00:00:00Z"),
            true,
            &[[515_012_340, -1_234_567, 100_000, 0, 0]],
        );
        write_chapter(&gps_chapter, "2026-07-10T00:00:00Z", &[1000], &[payload]);

        let camera_chapter = dir.path().join("camera.mp4");
        write_chapter(&camera_chapter, "2026-07-10T00:00:00Z", &[1000], &[]);

        let source = GoproSource {
            require_marker: true,
        };
        let (ignore, tz, imported_at) = test_ctx_with_tz(TimeZone::UTC);
        let ctx = make_ctx(&ignore, &tz, imported_at);

        let (gps_group, _) = source.build_session("0004", &[(1, gps_chapter)], &ctx);
        let gps_sidecar = gps_group.sidecar.unwrap();
        assert_eq!(gps_sidecar.content["time_source"], "gps");
        // clock_offset_s lives in the "gopro" device block.
        assert!(gps_sidecar.content["gopro"].get("clock_offset_s").is_some());
        // GPS events have "time" (not "utc" or "camera_time").
        assert!(gps_sidecar.content["events"][0].get("time").is_some());
        assert!(
            gps_sidecar.content["events"][0]
                .get("camera_time")
                .is_none()
        );

        let (camera_group, _) = source.build_session("0005", &[(1, camera_chapter)], &ctx);
        let camera_sidecar = camera_group.sidecar.unwrap();
        assert_eq!(camera_sidecar.content["time_source"], "camera");
        // No clock_offset_s for camera sessions.
        assert!(
            camera_sidecar.content["gopro"]
                .get("clock_offset_s")
                .is_none()
        );
        // Camera events also use "time" (unified field).
        assert!(camera_sidecar.content["events"][0].get("time").is_some());
        assert!(camera_group.geo.is_none());
    }
}
