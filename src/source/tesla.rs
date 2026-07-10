//! Tesla dashcam device module (roadmap changeset 4): TeslaCam card
//! detection, SavedClips/SentryClips event-folder discovery,
//! `event.json` parsing, category/reason filtering, and the
//! `import.json` sidecar. See
//! `openspec/changes/add-tesla-import/design.md` for the decisions
//! (D1-D8) this module implements.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use globset::GlobSet;
use jiff::Timestamp;
use jiff::civil;
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::Result;
use crate::source::{ImportSource, MediaFile, MediaGroup, Sidecar, Verdict};

/// Length of the `YYYY-MM-DD_HH-MM-SS` timestamp stem shared by event
/// folder names and per-minute clip filenames.
const STEM_LEN: usize = "2026-07-04_18-23-51".len();
const STEM_FORMAT: &str = "%Y-%m-%d_%H-%M-%S";

/// Which event categories a Tesla profile imports (design D5). Drives
/// both `events` config parsing and the `event_type` layout-context
/// value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventCategory {
    Saved,
    Sentry,
    Recent,
}

impl EventCategory {
    fn as_str(self) -> &'static str {
        match self {
            EventCategory::Saved => "saved",
            EventCategory::Sentry => "sentry",
            EventCategory::Recent => "recent",
        }
    }
}

/// Default `events` list (spec: "Tesla profile loads with defaults"):
/// SavedClips and SentryClips are imported; RecentClips is opt-in.
pub fn default_events() -> Vec<EventCategory> {
    vec![EventCategory::Saved, EventCategory::Sentry]
}

/// Trigger-reason allow/deny list (design D5). Modeled as an enum
/// rather than two `Option<Vec<String>>` fields so "both set" and
/// "neither set" are illegal states the type system rules out: serde's
/// externally tagged representation for a newtype variant accepts
/// exactly one of `allow`/`deny` as a map key, matching the YAML shape
/// and rejecting both-or-neither at deserialization (spec: "reasons
/// allow and deny are mutually exclusive").
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reasons {
    Allow(Vec<String>),
    Deny(Vec<String>),
}

impl Reasons {
    /// `Some(reason for filtering)` if `reason` should be excluded;
    /// `None` if it passes. Only ever called with a known reason (D4:
    /// unknown reasons fail open and never reach this).
    fn filtered_reason(&self, reason: &str) -> Option<String> {
        match self {
            Reasons::Allow(list) => (!list.iter().any(|r| r == reason))
                .then(|| format!("reason '{reason}' not in allow list")),
            Reasons::Deny(list) => list
                .iter()
                .any(|r| r == reason)
                .then(|| format!("reason '{reason}' denied")),
        }
    }
}

pub struct TeslaSource {
    pub events: Vec<EventCategory>,
    pub reasons: Option<Reasons>,
}

impl ImportSource for TeslaSource {
    fn detect(&self, root: &Path) -> bool {
        let teslacam = root.join("TeslaCam");
        ["SavedClips", "SentryClips", "RecentClips"]
            .iter()
            .any(|dir| teslacam.join(dir).is_dir())
    }

    fn scan(&self, root: &Path, ignore: &GlobSet) -> Result<Vec<(MediaGroup, Verdict)>> {
        let teslacam = root.join("TeslaCam");
        let mut groups = Vec::new();
        let mut stray_files: Vec<PathBuf> = Vec::new();

        for (category, dir_name) in [
            (EventCategory::Saved, "SavedClips"),
            (EventCategory::Sentry, "SentryClips"),
        ] {
            let clips_dir = teslacam.join(dir_name);
            let Ok(entries) = fs::read_dir(&clips_dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    groups.push(self.build_event_group(category, &path, ignore));
                } else if path.is_file() && !ignore.is_match(&path) {
                    stray_files.push(path);
                }
            }
        }

        if let Ok(entries) = fs::read_dir(&teslacam) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() && !ignore.is_match(&path) {
                    stray_files.push(path);
                }
            }
        }

        if self.events.contains(&EventCategory::Recent) {
            let (recent_groups, recent_unrecognized) =
                scan_recent_clips(&teslacam.join("RecentClips"), ignore);
            groups.extend(recent_groups);
            stray_files.extend(recent_unrecognized);
        }

        if !stray_files.is_empty() {
            groups.push(unrecognized_group(stray_files));
        }

        Ok(groups)
    }
}

impl TeslaSource {
    /// Builds one `SavedClips`/`SentryClips` event folder's group and
    /// verdict (design D2-D5).
    fn build_event_group(
        &self,
        category: EventCategory,
        dir: &Path,
        ignore: &GlobSet,
    ) -> (MediaGroup, Verdict) {
        let dir_name = file_name(dir);
        let parsed = parse_event_json(&dir.join("event.json"));

        let (wall_clock, time_source) = match parsed.timestamp {
            Some(dt) => (Some(dt), "event_json"),
            None => (parse_stem_exact(&dir_name), "folder_name"),
        };

        let Some(wall_clock) = wall_clock else {
            return (
                MediaGroup {
                    name: format!("{}-{}", category.as_str(), dir_name),
                    files: collect_group_files(dir, ignore, Timestamp::UNIX_EPOCH),
                    timestamp: Timestamp::UNIX_EPOCH,
                    markers: Vec::new(),
                    geo: None,
                    context: HashMap::new(),
                    sidecar: None,
                },
                Verdict::Ignore("unparseable event folder".to_string()),
            );
        };

        let event_instant = resolve_instant(wall_clock);
        let files = collect_group_files(dir, ignore, event_instant);
        let context = build_context(category, wall_clock);

        let verdict = if !self.events.contains(&category) {
            Verdict::Ignore(format!("event type '{}' not enabled", category.as_str()))
        } else if let Some(msg) = self
            .reasons
            .as_ref()
            .zip(parsed.reason.as_deref())
            .and_then(|(reasons, reason)| reasons.filtered_reason(reason))
        {
            Verdict::Ignore(msg)
        } else {
            Verdict::Keep
        };

        let sidecar = matches!(verdict, Verdict::Keep).then(|| {
            build_sidecar(
                category,
                dir,
                Some(&parsed),
                wall_clock,
                event_instant,
                time_source,
                &files,
            )
        });

        (
            MediaGroup {
                name: format!("{}-{}", category.as_str(), dir_name),
                files,
                timestamp: event_instant,
                markers: Vec::new(),
                geo: parsed.geo,
                context,
                sidecar,
            },
            verdict,
        )
    }
}

/// One event folder's tolerantly-parsed `event.json` (design D4): any
/// missing or malformed field degrades to `None` individually rather
/// than dropping the whole event.
#[derive(Debug, Default, Clone)]
struct ParsedEvent {
    timestamp: Option<civil::DateTime>,
    city: Option<String>,
    geo: Option<(f64, f64)>,
    reason: Option<String>,
}

fn parse_event_json(path: &Path) -> ParsedEvent {
    let Ok(text) = fs::read_to_string(path) else {
        return ParsedEvent::default();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return ParsedEvent::default();
    };

    let timestamp = value
        .get("timestamp")
        .and_then(|v| v.as_str())
        .and_then(|s| civil::DateTime::strptime("%Y-%m-%dT%H:%M:%S", s).ok());
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
    let geo = lat.zip(lon);
    let reason = value
        .get("reason")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    ParsedEvent {
        timestamp,
        city,
        geo,
        reason,
    }
}

/// Parses a folder name / stem that must match `STEM_FORMAT` exactly
/// end-to-end (event folder names — design D4).
fn parse_stem_exact(name: &str) -> Option<civil::DateTime> {
    if name.len() != STEM_LEN {
        return None;
    }
    civil::DateTime::strptime(STEM_FORMAT, name).ok()
}

/// Parses the leading `STEM_LEN` bytes of a clip filename as a
/// timestamp stem (design D8) — the remainder (`-front.mp4`, etc.) is
/// ignored.
fn parse_stem_prefix(file_name: &str) -> Option<civil::DateTime> {
    if file_name.len() < STEM_LEN || !file_name.is_char_boundary(STEM_LEN) {
        return None;
    }
    civil::DateTime::strptime(STEM_FORMAT, &file_name[..STEM_LEN]).ok()
}

/// Resolves a vehicle-local civil datetime to a real instant via the
/// system timezone with jiff's default (compatible) DST disambiguation
/// (design D3). Falls back to interpreting the civil time as UTC in
/// the practically-never case that the system zone can't resolve it.
fn resolve_instant(wall_clock: civil::DateTime) -> Timestamp {
    wall_clock
        .to_zoned(TimeZone::system())
        .map(|z| z.timestamp())
        .unwrap_or_else(|_| wall_clock.to_zoned(TimeZone::UTC).unwrap().timestamp())
}

/// Layout-context fields formatted directly from the civil value —
/// pure wall clock, immune to timezone/DST (design D3).
fn build_context(category: EventCategory, wall_clock: civil::DateTime) -> HashMap<String, String> {
    let mut context = HashMap::new();
    context.insert("event_type".to_string(), category.as_str().to_string());
    context.insert(
        "event_date".to_string(),
        wall_clock.strftime("%Y-%m-%d").to_string(),
    );
    context.insert(
        "event_time".to_string(),
        wall_clock.strftime("%H-%M-%S").to_string(),
    );
    context
}

/// Collects one event folder's files (non-recursive), applying the
/// profile's `ignore` globs (design D2). Each clip's `recorded_at`
/// comes from its own filename stem; files without a parseable stem
/// (`event.json`, `thumb.png`, unrecognized files) use the event's own
/// instant instead (design D8).
fn collect_group_files(dir: &Path, ignore: &GlobSet, event_instant: Timestamp) -> Vec<MediaFile> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && !ignore.is_match(p))
        .collect();
    paths.sort();

    paths
        .into_iter()
        .map(|path| {
            let recorded_at = path
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(parse_stem_prefix)
                .map(resolve_instant)
                .unwrap_or(event_instant);
            MediaFile {
                size: fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
                path,
                recorded_at: Some(recorded_at),
            }
        })
        .collect()
}

/// Groups `RecentClips/` files by their filename-stem timestamp into
/// per-minute clusters (design D6). Only called when `recent` is
/// enabled (spec: "RecentClips import is opt-in"). Returns the
/// per-minute Keep groups plus any file whose name doesn't carry a
/// recognizable timestamp stem.
fn scan_recent_clips(dir: &Path, ignore: &GlobSet) -> (Vec<(MediaGroup, Verdict)>, Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return (Vec::new(), Vec::new());
    };

    let mut clusters: HashMap<String, Vec<PathBuf>> = HashMap::new();
    let mut unrecognized = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() || ignore.is_match(&path) {
            continue;
        }
        let matched = path
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|name| name.len() >= STEM_LEN && parse_stem_prefix(name).is_some())
            .map(|name| name[..STEM_LEN].to_string());
        match matched {
            Some(stem) => clusters.entry(stem).or_default().push(path),
            None => unrecognized.push(path),
        }
    }

    let mut stems: Vec<&String> = clusters.keys().collect();
    stems.sort();

    let groups = stems
        .into_iter()
        .map(|stem| {
            let wall_clock =
                parse_stem_exact(stem).expect("cluster key was parsed from a valid stem");
            let event_instant = resolve_instant(wall_clock);
            let mut paths = clusters[stem].clone();
            paths.sort();
            let files: Vec<MediaFile> = paths
                .into_iter()
                .map(|path| MediaFile {
                    size: fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
                    path,
                    recorded_at: Some(event_instant),
                })
                .collect();
            let context = build_context(EventCategory::Recent, wall_clock);
            let sidecar = build_sidecar(
                EventCategory::Recent,
                &dir.join(stem),
                None,
                wall_clock,
                event_instant,
                "folder_name",
                &files,
            );
            (
                MediaGroup {
                    name: format!("recent-{stem}"),
                    files,
                    timestamp: event_instant,
                    markers: Vec::new(),
                    geo: None,
                    context,
                    sidecar: Some(sidecar),
                },
                Verdict::Keep,
            )
        })
        .collect();

    (groups, unrecognized)
}

/// Assembles the `import.json` sidecar for a kept group (design D7):
/// device type, event type, source folder path, parsed event metadata
/// (when there is any — `RecentClips` clusters have none), resolved
/// wall-clock and UTC times with timestamp provenance, and the file
/// list.
fn build_sidecar(
    category: EventCategory,
    source_dir: &Path,
    parsed: Option<&ParsedEvent>,
    wall_clock: civil::DateTime,
    recorded_at: Timestamp,
    time_source: &str,
    files: &[MediaFile],
) -> Sidecar {
    let file_names: Vec<String> = files.iter().map(|f| file_name(&f.path)).collect();

    let event = parsed.map(|p| {
        json!({
            "timestamp": p.timestamp.map(|t| t.to_string()),
            "city": p.city,
            "lat": p.geo.map(|(lat, _)| lat),
            "lon": p.geo.map(|(_, lon)| lon),
            "reason": p.reason,
        })
    });

    Sidecar {
        filename: "import.json".to_string(),
        content: json!({
            "camera": "tesla",
            "event_type": category.as_str(),
            "source": source_dir.display().to_string(),
            "event": event,
            "time_source": time_source,
            "wall_clock": wall_clock.to_string(),
            "recorded_at": recorded_at.to_string(),
            "files": file_names,
        }),
    }
}

/// One `Ignore("unrecognized file(s)")` group for stray files found
/// outside any event folder or timestamp cluster, mirroring the GoPro
/// pattern.
fn unrecognized_group(files: Vec<PathBuf>) -> (MediaGroup, Verdict) {
    (
        MediaGroup {
            name: "unrecognized".to_string(),
            files: files
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
    )
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn event_json(reason: &str) -> String {
        format!(
            r#"{{"timestamp":"2026-07-04T18:23:51","city":"London","est_lat":"51.5012","est_lon":"-0.1246","reason":"{reason}","camera":"0"}}"#
        )
    }

    fn source(events: Vec<EventCategory>, reasons: Option<Reasons>) -> TeslaSource {
        TeslaSource { events, reasons }
    }

    #[test]
    fn detects_teslacam_with_saved_clips() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("TeslaCam/SavedClips")).unwrap();
        assert!(source(default_events(), None).detect(dir.path()));
    }

    #[test]
    fn does_not_detect_bare_teslacam_dir() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("TeslaCam")).unwrap();
        assert!(!source(default_events(), None).detect(dir.path()));
    }

    #[test]
    fn does_not_detect_gopro_card() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("DCIM/100GOPRO")).unwrap();
        assert!(!source(default_events(), None).detect(dir.path()));
    }

    #[test]
    fn does_not_detect_empty_root() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!source(default_events(), None).detect(dir.path()));
    }

    #[test]
    fn event_folder_becomes_one_group_with_all_files() {
        let dir = tempfile::tempdir().unwrap();
        let event_dir = dir.path().join("TeslaCam/SavedClips/2026-07-04_18-23-51");
        write(
            &event_dir.join("event.json"),
            &event_json("user_interaction_honk"),
        );
        write(&event_dir.join("thumb.png"), "thumb");
        write(&event_dir.join("2026-07-04_18-18-32-front.mp4"), "front");
        write(&event_dir.join("2026-07-04_18-18-32-back.mp4"), "back");

        let (group, verdict) = source(default_events(), None).build_event_group(
            EventCategory::Saved,
            &event_dir,
            &GlobSet::empty(),
        );

        assert_eq!(verdict, Verdict::Keep);
        assert_eq!(group.files.len(), 4);
        assert_eq!(group.context["event_type"], "saved");
        assert_eq!(group.context["event_date"], "2026-07-04");
        assert_eq!(group.context["event_time"], "18-23-51");
    }

    #[test]
    fn category_not_enabled_yields_ignore() {
        let dir = tempfile::tempdir().unwrap();
        let event_dir = dir.path().join("TeslaCam/SentryClips/2026-07-04_18-23-51");
        write(
            &event_dir.join("event.json"),
            &event_json("sentry_aware_object_detection"),
        );

        let (_, verdict) = source(vec![EventCategory::Saved], None).build_event_group(
            EventCategory::Sentry,
            &event_dir,
            &GlobSet::empty(),
        );
        assert_eq!(
            verdict,
            Verdict::Ignore("event type 'sentry' not enabled".to_string())
        );
    }

    #[test]
    fn deny_list_filters_matching_reason() {
        let dir = tempfile::tempdir().unwrap();
        let event_dir = dir.path().join("TeslaCam/SentryClips/2026-07-04_18-23-51");
        write(
            &event_dir.join("event.json"),
            &event_json("sentry_aware_object_detection"),
        );

        let reasons = Reasons::Deny(vec!["sentry_aware_object_detection".to_string()]);
        let (_, verdict) = source(default_events(), Some(reasons)).build_event_group(
            EventCategory::Sentry,
            &event_dir,
            &GlobSet::empty(),
        );
        assert_eq!(
            verdict,
            Verdict::Ignore("reason 'sentry_aware_object_detection' denied".to_string())
        );
    }

    #[test]
    fn allow_list_keeps_only_listed_reason() {
        let dir = tempfile::tempdir().unwrap();
        let event_dir = dir.path().join("TeslaCam/SentryClips/2026-07-04_18-23-51");
        write(
            &event_dir.join("event.json"),
            &event_json("sentry_aware_object_detection"),
        );

        let reasons = Reasons::Allow(vec!["user_interaction_honk".to_string()]);
        let (_, verdict) = source(default_events(), Some(reasons)).build_event_group(
            EventCategory::Sentry,
            &event_dir,
            &GlobSet::empty(),
        );
        assert!(matches!(verdict, Verdict::Ignore(_)));
    }

    #[test]
    fn unknown_reason_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let event_dir = dir.path().join("TeslaCam/SavedClips/2026-07-04_18-23-51");
        write(
            &event_dir.join("event.json"),
            r#"{"timestamp":"2026-07-04T18:23:51"}"#,
        );

        let reasons = Reasons::Allow(vec!["user_interaction_honk".to_string()]);
        let (_, verdict) = source(default_events(), Some(reasons)).build_event_group(
            EventCategory::Saved,
            &event_dir,
            &GlobSet::empty(),
        );
        assert_eq!(verdict, Verdict::Keep);
    }

    #[test]
    fn corrupt_event_json_falls_back_to_folder_name() {
        let dir = tempfile::tempdir().unwrap();
        let event_dir = dir.path().join("TeslaCam/SavedClips/2026-07-04_18-23-51");
        write(&event_dir.join("event.json"), "{not valid json");

        let (group, verdict) = source(default_events(), None).build_event_group(
            EventCategory::Saved,
            &event_dir,
            &GlobSet::empty(),
        );
        assert_eq!(verdict, Verdict::Keep);
        assert_eq!(group.context["event_date"], "2026-07-04");
        let sidecar = group.sidecar.unwrap();
        assert_eq!(sidecar.content["time_source"], "folder_name");
    }

    #[test]
    fn unparseable_folder_and_no_timestamp_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let event_dir = dir.path().join("TeslaCam/SavedClips/not-a-timestamp");
        write(&event_dir.join("event.json"), "{not valid json");

        let (_, verdict) = source(default_events(), None).build_event_group(
            EventCategory::Saved,
            &event_dir,
            &GlobSet::empty(),
        );
        assert_eq!(
            verdict,
            Verdict::Ignore("unparseable event folder".to_string())
        );
    }

    #[test]
    fn coordinates_parsed_from_string_fields() {
        let dir = tempfile::tempdir().unwrap();
        let event_dir = dir.path().join("TeslaCam/SavedClips/2026-07-04_18-23-51");
        write(
            &event_dir.join("event.json"),
            &event_json("user_interaction_honk"),
        );

        let (group, _) = source(default_events(), None).build_event_group(
            EventCategory::Saved,
            &event_dir,
            &GlobSet::empty(),
        );
        let (lat, lon) = group.geo.unwrap();
        assert!((lat - 51.5012).abs() < 1e-9);
        assert!((lon - (-0.1246)).abs() < 1e-9);
    }

    #[test]
    fn recent_clips_skipped_without_recent_category() {
        let dir = tempfile::tempdir().unwrap();
        let recent = dir.path().join("TeslaCam/RecentClips");
        write(&recent.join("2026-07-04_18-40-00-front.mp4"), "front");

        let groups = source(default_events(), None)
            .scan(dir.path(), &GlobSet::empty())
            .unwrap();
        assert!(groups.is_empty());
    }

    #[test]
    fn recent_clips_cluster_by_minute_when_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let recent = dir.path().join("TeslaCam/RecentClips");
        for angle in ["front", "back", "left_repeater", "right_repeater"] {
            write(
                &recent.join(format!("2026-07-04_18-40-00-{angle}.mp4")),
                angle,
            );
            write(
                &recent.join(format!("2026-07-04_18-41-00-{angle}.mp4")),
                angle,
            );
        }

        let mut events = default_events();
        events.push(EventCategory::Recent);
        let groups = source(events, None)
            .scan(dir.path(), &GlobSet::empty())
            .unwrap();

        assert_eq!(groups.len(), 2);
        for (group, verdict) in &groups {
            assert_eq!(*verdict, Verdict::Keep);
            assert_eq!(group.files.len(), 4);
            assert_eq!(group.context["event_type"], "recent");
        }
    }

    #[test]
    fn stray_file_outside_event_folder_is_ignored_not_touched() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("TeslaCam/SavedClips/stray.mp4"), "stray");

        let groups = source(default_events(), None)
            .scan(dir.path(), &GlobSet::empty())
            .unwrap();
        assert_eq!(groups.len(), 1);
        let (group, verdict) = &groups[0];
        assert_eq!(
            verdict,
            &Verdict::Ignore("unrecognized file(s)".to_string())
        );
        assert_eq!(group.files.len(), 1);
    }

    #[test]
    fn unrecognized_file_inside_event_folder_travels_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let event_dir = dir.path().join("TeslaCam/SavedClips/2026-07-04_18-23-51");
        write(
            &event_dir.join("event.json"),
            &event_json("user_interaction_honk"),
        );
        write(&event_dir.join("notes.txt"), "notes");

        let (group, verdict) = source(default_events(), None).build_event_group(
            EventCategory::Saved,
            &event_dir,
            &GlobSet::empty(),
        );
        assert_eq!(verdict, Verdict::Keep);
        assert!(
            group
                .files
                .iter()
                .any(|f| file_name(&f.path) == "notes.txt")
        );
    }

    #[test]
    fn reasons_round_trips_outside_flatten() {
        // `Reasons` serializes as a YAML-tagged newtype variant
        // (`!deny [...]`) — symmetric on its own, but that tag form
        // can't be re-deserialized through `RawProfile`'s
        // `#[serde(flatten)]` (a documented serde limitation: nested
        // enums don't survive flatten's generic buffering). Config
        // loading never serializes, so this only matters in isolation
        // — see `config::tests::tesla_variant_serde_round_trips`.
        let original = Reasons::Deny(vec!["sentry_aware_object_detection".to_string()]);
        let yaml = serde_yaml_ng::to_string(&original).unwrap();
        let round_tripped: Reasons = serde_yaml_ng::from_str(&yaml).unwrap();
        assert_eq!(original, round_tripped);
    }

    #[test]
    fn never_produces_quarantine_verdict() {
        let dir = tempfile::tempdir().unwrap();
        let saved = dir.path().join("TeslaCam/SavedClips/2026-07-04_18-23-51");
        write(
            &saved.join("event.json"),
            &event_json("sentry_aware_object_detection"),
        );
        let sentry = dir.path().join("TeslaCam/SentryClips/2026-07-04_18-24-00");
        write(&sentry.join("event.json"), "not json");
        write(&dir.path().join("TeslaCam/SavedClips/stray.mp4"), "stray");

        let reasons = Reasons::Deny(vec!["sentry_aware_object_detection".to_string()]);
        let groups = source(vec![EventCategory::Saved], Some(reasons))
            .scan(dir.path(), &GlobSet::empty())
            .unwrap();

        assert!(!groups.is_empty());
        assert!(
            groups
                .iter()
                .all(|(_, verdict)| !matches!(verdict, Verdict::Quarantine)),
            "Tesla verdicts must never be Quarantine"
        );
    }
}
