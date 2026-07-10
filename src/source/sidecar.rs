//! Unified `import.json` sidecar builder (design D6). Every device
//! module hands this builder structured pieces — envelope facts,
//! `events[]`, and an optional device block — and the builder renders
//! all timestamps through the configured timezone and serialises to
//! `serde_json::Value`. The file is always named `import.json`.
//!
//! Timestamp format: ISO-8601 with a numeric offset and no zone-name
//! suffix (design D7). jiff's `Zoned` `Display` appends an
//! `[IANA/Name]` suffix that we must avoid; `strftime("%Y-%m-%dT%H:%M:%S%:z")`
//! on a `Zoned` value produces exactly the right form.

use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde_json::{Value, json};

use super::Sidecar;

const SIDECAR_TIMESTAMP_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%:z";

fn format_ts(ts: Timestamp, tz: &TimeZone) -> String {
    let zoned = ts.to_zoned(tz.clone());
    jiff::fmt::strtime::format(SIDECAR_TIMESTAMP_FORMAT, &zoned)
        .expect("SIDECAR_TIMESTAMP_FORMAT is a constant, always valid")
}

/// One entry in `events[]`. The `type` field uses namespaced form,
/// e.g. `gopro:marker` or `tesla:saved`.
pub struct EventEntry {
    pub event_type: String,
    pub time: Timestamp,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    /// Tesla trigger reason; `None` for GoPro markers and events whose
    /// reason is unknown.
    pub reason: Option<String>,
    /// Millisecond offset from session start (GoPro markers only).
    pub offset_ms: Option<u32>,
}

/// Inputs that every device provides to build the common envelope.
pub struct SidecarEnvelope<'a> {
    /// Device model identifier, e.g. `"gopro-hero8"` or `"tesla"`.
    pub camera: &'a str,
    /// Source folder path (display string).
    pub source: String,
    /// When the import run started (from `ScanContext::imported_at`).
    pub imported_at: Timestamp,
    /// The configured IANA timezone name (or `""` when the zone has no
    /// IANA name, e.g. a fixed-offset system zone).
    pub timezone_name: String,
    /// The group's recording instant.
    pub recorded_at: Timestamp,
    /// Timestamp provenance: e.g. `"gps"`, `"camera"`, `"event_json"`,
    /// `"folder_name"`.
    pub time_source: &'a str,
    /// Imported file names (base names only).
    pub files: Vec<String>,
}

/// Builds the `import.json` `Sidecar` from structured pieces. The
/// device block (`gopro: {…}` or `tesla: {…}`) holds only fields that
/// have no common-envelope or per-event home; pass `None` when there
/// is nothing device-specific to record.
pub fn build(
    tz: &TimeZone,
    envelope: SidecarEnvelope<'_>,
    events: Vec<EventEntry>,
    device_block: Option<(&str, Value)>,
) -> Sidecar {
    let events_json: Vec<Value> = events
        .into_iter()
        .map(|e| {
            let mut entry = json!({
                "type": e.event_type,
                "time": format_ts(e.time, tz),
            });
            if let Some(lat) = e.lat {
                entry["lat"] = json!(lat);
            }
            if let Some(lon) = e.lon {
                entry["lon"] = json!(lon);
            }
            if let Some(reason) = e.reason {
                entry["reason"] = json!(reason);
            }
            if let Some(offset_ms) = e.offset_ms {
                entry["offset_ms"] = json!(offset_ms);
            }
            entry
        })
        .collect();

    let mut content = json!({
        "camera": envelope.camera,
        "source": envelope.source,
        "imported_at": format_ts(envelope.imported_at, tz),
        "timezone": envelope.timezone_name,
        "recorded_at": format_ts(envelope.recorded_at, tz),
        "time_source": envelope.time_source,
        "files": envelope.files,
        "events": events_json,
    });

    if let Some((key, block)) = device_block {
        content[key] = block;
    }

    Sidecar {
        filename: "import.json".to_string(),
        content,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::tz::TimeZone;

    fn vilnius() -> TimeZone {
        TimeZone::get("Europe/Vilnius").unwrap()
    }

    fn fixed_ts(secs: i64) -> Timestamp {
        Timestamp::from_second(secs).unwrap()
    }

    fn envelope(
        tz_name: &str,
        imported_at: Timestamp,
        recorded_at: Timestamp,
    ) -> SidecarEnvelope<'static> {
        SidecarEnvelope {
            camera: "gopro-hero8",
            source: "/media/GOPRO/DCIM/100GOPRO".to_string(),
            imported_at,
            timezone_name: tz_name.to_string(),
            recorded_at,
            time_source: "gps",
            files: vec!["GX010123.MP4".to_string()],
        }
    }

    #[test]
    fn offset_format_no_zone_name_suffix() {
        // Spec scenario: "Sidecar timestamps carry the zone offset" —
        // no `[Europe/Vilnius]` suffix, just `+03:00`.
        let ts = fixed_ts(1751641431); // 2026-07-04T15:23:51Z → +03:00 in Vilnius
        let tz = vilnius();
        let sidecar = build(&tz, envelope("Europe/Vilnius", ts, ts), vec![], None);
        let recorded_at = sidecar.content["recorded_at"].as_str().unwrap();
        assert!(
            recorded_at.ends_with("+03:00"),
            "expected +03:00 suffix, got: {recorded_at}"
        );
        assert!(
            !recorded_at.contains('['),
            "must not contain zone-name suffix, got: {recorded_at}"
        );
    }

    #[test]
    fn empty_events_array() {
        // Spec scenario: "Group without discrete events has an empty array"
        let ts = fixed_ts(0);
        let sidecar = build(&TimeZone::UTC, envelope("UTC", ts, ts), vec![], None);
        assert_eq!(sidecar.content["events"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn device_block_stored_under_its_key() {
        // Spec scenario: "GoPro device block holds session-only fields"
        let ts = fixed_ts(0);
        let block = json!({ "session": "0123", "clock_offset_s": -12.4 });
        let sidecar = build(
            &TimeZone::UTC,
            envelope("UTC", ts, ts),
            vec![],
            Some(("gopro", block.clone())),
        );
        assert_eq!(sidecar.content["gopro"], block);
        assert!(sidecar.content.get("tesla").is_none());
    }

    #[test]
    fn no_event_json_embedded() {
        // Spec scenario: "Raw event.json is not duplicated into the sidecar"
        let ts = fixed_ts(0);
        let sidecar = build(&TimeZone::UTC, envelope("UTC", ts, ts), vec![], None);
        // The sidecar content must not have an "event" key that holds a
        // copy of the event.json structure.
        assert!(sidecar.content.get("event").is_none());
    }

    #[test]
    fn sidecar_filename_is_import_json() {
        let ts = fixed_ts(0);
        let sidecar = build(&TimeZone::UTC, envelope("UTC", ts, ts), vec![], None);
        assert_eq!(sidecar.filename, "import.json");
    }
}
