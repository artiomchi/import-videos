//! The `ImportSource` trait through which device modules plug into the
//! core pipeline (ADR 0005). Core defines the contract; it contains no
//! device-specific logic itself.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use jiff::Timestamp;

use crate::error::Result;

/// A single file belonging to a `MediaGroup` (a clip, a sidecar, ...).
#[derive(Debug, Clone, PartialEq)]
pub struct MediaFile {
    pub path: PathBuf,
    pub size: u64,
}

/// A point of interest within a group's footage (e.g. a GoPro HiLight
/// button press). Not consumed by core in this changeset; device
/// modules will attach these during `scan`.
#[derive(Debug, Clone, PartialEq)]
pub struct Marker {
    pub timestamp: Timestamp,
    pub label: Option<String>,
}

/// One unit of import planning: a set of related files (e.g. all the
/// chapter files for one recording session) sharing a timestamp and,
/// optionally, a location. `context` supplies the values that layout
/// templates resolve non-`date` fields from (design D2).
#[derive(Debug, Clone, PartialEq)]
pub struct MediaGroup {
    pub name: String,
    pub files: Vec<MediaFile>,
    pub timestamp: Timestamp,
    pub markers: Vec<Marker>,
    pub geo: Option<(f64, f64)>,
    pub context: HashMap<String, String>,
}

/// What the pipeline should do with a `MediaGroup`, decided by the
/// device implementation during `scan`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Keep,
    Quarantine,
    Ignore(String),
}

/// Implemented once per device type (`src/source/<device>.rs`, added by
/// its own changeset). The core pipeline — planning, transfer,
/// quarantine, reporting — only ever talks to this trait, never to a
/// concrete device type (spec: "Device implementations plug in via the
/// ImportSource trait").
pub trait ImportSource {
    /// Does `root` look like a card/volume this device produces?
    /// Used during `source: auto` mount-root probing (design D6).
    fn detect(&self, root: &Path) -> bool;

    /// Discovers media under `root`, grouping files and assigning a
    /// verdict to each group. Must not modify anything under `root`
    /// (spec: "Scan produces a reviewable plan without side effects").
    fn scan(&self, root: &Path) -> Result<Vec<(MediaGroup, Verdict)>>;
}

/// Placeholder `ImportSource`: never detects a card and never finds
/// media. Stands in for the device modules `add-gopro-import` and
/// `add-tesla-import` will add, so `scan`/`import` exercise the full
/// pipeline end-to-end in this changeset while correctly reporting "no
/// matching sources" (proposal).
pub struct GenericSource;

impl ImportSource for GenericSource {
    fn detect(&self, _root: &Path) -> bool {
        false
    }

    fn scan(&self, _root: &Path) -> Result<Vec<(MediaGroup, Verdict)>> {
        Ok(Vec::new())
    }
}
