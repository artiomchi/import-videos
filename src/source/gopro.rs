//! GoPro HERO8 device module (roadmap changeset 2): card detection,
//! chapter-file session grouping, HiLight-marker-driven keep/quarantine
//! verdicts, and the `markers.json` sidecar. See
//! `openspec/changes/add-gopro-import/design.md` for the decisions
//! (D1-D8) this module implements.

use std::collections::HashMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

use globset::GlobSet;
use jiff::{Span, Timestamp};
use serde_json::json;

use crate::error::Result;
use crate::media::mp4;
use crate::source::{ImportSource, Marker, MediaFile, MediaGroup, Sidecar, Verdict};

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

    fn scan(&self, root: &Path, ignore: &GlobSet) -> Result<Vec<(MediaGroup, Verdict)>> {
        let (sessions, unrecognized) = discover(&root.join("DCIM"), ignore);

        let mut groups = Vec::with_capacity(sessions.len() + 1);
        let mut session_ids: Vec<&String> = sessions.keys().collect();
        session_ids.sort();

        for session_id in session_ids {
            let mut chapters = sessions[session_id].clone();
            chapters.sort_by_key(|(chapter, _)| *chapter);
            groups.push(self.build_session(session_id, &chapters));
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
    /// chapter-ordered file list (design D3, D5-D8).
    fn build_session(
        &self,
        session_id: &str,
        chapters: &[(u32, PathBuf)],
    ) -> (MediaGroup, Verdict) {
        let mut all_markers: Vec<MarkerHit> = Vec::new();
        for (_, path) in chapters {
            let chapter_time = chapter_timestamp(path);
            for offset_ms in chapter_markers(path) {
                all_markers.push(MarkerHit {
                    file_name: file_name(path),
                    offset_ms,
                    wall_time: chapter_time + Span::new().milliseconds(offset_ms as i64),
                });
            }
        }

        let verdict = if !self.require_marker || !all_markers.is_empty() {
            Verdict::Keep
        } else {
            Verdict::Quarantine
        };

        // Session timestamp is always the first chapter's camera-clock
        // time (design D5), independent of the verdict.
        let session_timestamp = chapter_timestamp(&chapters[0].1);

        let files: Vec<MediaFile> = chapters
            .iter()
            .map(|(_, path)| MediaFile {
                size: fs::metadata(path).map(|m| m.len()).unwrap_or(0),
                path: path.clone(),
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

        let sidecar =
            (verdict == Verdict::Keep).then(|| build_sidecar(session_id, chapters, &all_markers));

        (
            MediaGroup {
                name: format!("session-{session_id}"),
                files,
                timestamp: session_timestamp,
                markers,
                geo: None,
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
    file_name: String,
    offset_ms: u32,
    wall_time: Timestamp,
}

fn build_sidecar(session_id: &str, chapters: &[(u32, PathBuf)], markers: &[MarkerHit]) -> Sidecar {
    let files: Vec<String> = chapters.iter().map(|(_, path)| file_name(path)).collect();
    let markers: Vec<serde_json::Value> = markers
        .iter()
        .map(|hit| {
            json!({
                "file": hit.file_name,
                "offset_ms": hit.offset_ms,
                "camera_time": hit.wall_time.to_string(),
            })
        })
        .collect();

    Sidecar {
        filename: "markers.json".to_string(),
        content: json!({
            "camera": CAMERA_MODEL,
            "session": session_id,
            "files": files,
            "time_source": "camera",
            "markers": markers,
        }),
    }
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

/// The camera-clock creation time for one chapter (design D5):
/// `moov/mvhd` if it can be read, else the file's modification time
/// with a warning — a chapter that can't be timestamped precisely
/// still gets imported rather than blocking the run.
fn chapter_timestamp(path: &Path) -> Timestamp {
    match File::open(path)
        .map_err(|e| e.to_string())
        .and_then(|mut f| mp4::read_creation_time(&mut f).map_err(|e| e.to_string()))
    {
        Ok(timestamp) => timestamp,
        Err(error) => {
            let fallback = fs::metadata(path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|modified| Timestamp::try_from(modified).ok())
                .unwrap_or(Timestamp::UNIX_EPOCH);
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

        let (group, verdict) = source.build_session("0124", &[(1, chapter)]);
        assert_eq!(verdict, Verdict::Keep);
        assert!(group.markers.is_empty());
        assert!(
            group.sidecar.is_some(),
            "kept sessions carry a markers.json even with zero markers"
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
}
