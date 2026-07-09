//! YAML profile configuration (ADR 0004). Parsing is split into a raw
//! serde model (`RawConfig`/`RawProfile`, close to the YAML shape) and
//! a validated domain model (`Config`/`Profile`, with paths expanded,
//! globs compiled, and the layout template parsed) so validation
//! failures can name the profile and field without serde's error type
//! getting in the way.

mod layout;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};

pub use layout::LayoutTemplate;

use crate::error::{Error, Result};
use crate::source::{GenericSource, ImportSource};

/// Device-specific profile fields, an internally tagged enum on `type`
/// (design D1): serde rejects an unrecognized `type` at load for free,
/// which is how "unknown profile type" (spec) gets its exit-2 error
/// without extra validation code of our own.
///
/// This changeset ships no device modules yet (ADR 0005) — `Generic`
/// is the zero-extra-fields placeholder that exercises the flatten +
/// internally-tagged-enum mechanism (design Risks) and lets `scan`
/// run the pipeline end-to-end. `add-gopro-import` / `add-tesla-import`
/// each add a sibling variant here plus an `ImportSource` impl under
/// `src/source/`.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SourceKind {
    Generic,
}

impl SourceKind {
    /// Maps a profile's `type` to its `ImportSource` implementation.
    /// This exhaustive match *is* the registry (spec: "Device
    /// implementations plug in via the ImportSource trait"): the
    /// compiler forces an update here whenever a variant is added,
    /// which a runtime `HashMap<&str, _>` would not guarantee for a
    /// device set that's fixed at compile time (design D3).
    pub fn build(&self) -> Box<dyn ImportSource> {
        match self {
            SourceKind::Generic => Box::new(GenericSource),
        }
    }
}

/// A profile's `source:` field: either `auto` (mount-root probing) or
/// an explicit path (design D6).
#[derive(Debug, Clone, PartialEq)]
pub enum SourceLocation {
    Auto,
    Path(PathBuf),
}

/// One named profile from the config file, validated: paths
/// tilde-expanded, `ignore` globs compiled, `layout` parsed.
#[derive(Debug, Clone)]
pub struct Profile {
    pub kind: SourceKind,
    pub source: SourceLocation,
    pub destination: PathBuf,
    pub layout: LayoutTemplate,
    pub ignore: GlobSet,
    pub quarantine: Option<PathBuf>,
    pub delete_source: bool,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub profiles: HashMap<String, Profile>,
    pub mount_roots: Vec<PathBuf>,
}

#[derive(Debug, Deserialize, Serialize)]
struct RawConfig {
    #[serde(default)]
    mount_roots: Option<Vec<String>>,
    #[serde(default)]
    profiles: HashMap<String, RawProfile>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
struct RawProfile {
    #[serde(flatten)]
    kind: SourceKind,
    #[serde(default = "default_source")]
    source: String,
    destination: String,
    layout: String,
    #[serde(default)]
    ignore: Vec<String>,
    #[serde(default)]
    quarantine: Option<String>,
    #[serde(default)]
    delete_source: bool,
}

fn default_source() -> String {
    "auto".to_string()
}

/// Default config path per ADR 0004: `$XDG_CONFIG_HOME/import-videos/config.yaml`
/// (`~/.config/import-videos/config.yaml` on Linux).
pub fn default_config_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "import-videos")
        .map(|dirs| dirs.config_dir().join("config.yaml"))
}

/// Loads and validates the config at `path`. Every failure mode named
/// in the spec — unreadable file, invalid YAML, unknown `type`,
/// invalid glob, invalid layout template — is reported here with the
/// offending profile and field named.
pub fn load(path: &Path) -> Result<Config> {
    // An unreadable config file is a usage error (spec: exit 2), unlike
    // most I/O failures elsewhere in the crate (which are runtime
    // failures during scan/import and exit 1) — so this maps to
    // `Error::Config`, not `Error::io`.
    let text = std::fs::read_to_string(path)
        .map_err(|e| Error::Config(format!("{}: {e}", path.display())))?;
    let raw: RawConfig = serde_yaml_ng::from_str(&text)
        .map_err(|e| Error::Config(format!("{}: {e}", path.display())))?;

    let mount_roots = raw
        .mount_roots
        .unwrap_or_else(default_mount_roots)
        .iter()
        .map(|s| expand_tilde(s))
        .collect();

    let mut profiles = HashMap::with_capacity(raw.profiles.len());
    for (name, raw_profile) in raw.profiles {
        let profile = validate_profile(&name, raw_profile)?;
        profiles.insert(name, profile);
    }

    Ok(Config {
        profiles,
        mount_roots,
    })
}

fn validate_profile(name: &str, raw: RawProfile) -> Result<Profile> {
    let source = if raw.source == "auto" {
        SourceLocation::Auto
    } else {
        SourceLocation::Path(expand_tilde(&raw.source))
    };

    let layout = LayoutTemplate::parse(&raw.layout)
        .map_err(|e| Error::Config(format!("profile '{name}': layout: {e}")))?;

    let ignore = build_globset(&raw.ignore)
        .map_err(|e| Error::Config(format!("profile '{name}': ignore: {e}")))?;

    Ok(Profile {
        kind: raw.kind,
        source,
        destination: expand_tilde(&raw.destination),
        layout,
        ignore,
        quarantine: raw.quarantine.map(|s| expand_tilde(&s)),
        delete_source: raw.delete_source,
    })
}

fn build_globset(patterns: &[String]) -> std::result::Result<GlobSet, globset::Error> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(Glob::new(pattern)?);
    }
    builder.build()
}

fn default_mount_roots() -> Vec<String> {
    let mut roots = Vec::new();
    if let Ok(user) = std::env::var("USER") {
        roots.push(format!("/run/media/{user}"));
    }
    roots.push("/media".to_string());
    roots.push("/mnt".to_string());
    roots
}

fn expand_tilde(path: &str) -> PathBuf {
    let rest = if path == "~" {
        Some("")
    } else {
        path.strip_prefix("~/")
    };
    match rest.and_then(|rest| directories::BaseDirs::new().map(|base| base.home_dir().join(rest)))
    {
        Some(expanded) => expanded,
        None => PathBuf::from(path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_yaml() -> &'static str {
        r#"
profiles:
  cam:
    type: generic
    source: auto
    destination: ~/Videos/cam
    layout: "{date:%Y}/{date:%Y-%m-%d}"
    ignore: ["*.tmp"]
    delete_source: true
"#
    }

    #[test]
    fn loads_valid_profile() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(&path, sample_yaml()).unwrap();

        let cfg = load(&path).unwrap();
        let profile = cfg.profiles.get("cam").unwrap();
        assert_eq!(profile.kind, SourceKind::Generic);
        assert_eq!(profile.source, SourceLocation::Auto);
        assert!(profile.delete_source);
    }

    #[test]
    fn unknown_type_fails_at_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(
            &path,
            r#"
profiles:
  drone:
    type: quadcopter
    source: auto
    destination: /tmp/dest
    layout: "{date}"
"#,
        )
        .unwrap();

        let err = load(&path).unwrap_err();
        assert!(matches!(err, Error::Config(_)));
    }

    #[test]
    fn missing_config_file_fails() {
        // Spec: missing config file exits 2 (usage/config error), not 1.
        let err = load(Path::new("/nonexistent/config.yaml")).unwrap_err();
        assert!(matches!(err, Error::Config(_)));
        assert!(err.to_string().contains("/nonexistent/config.yaml"));
        assert_eq!(err.exit_code(), crate::error::ExitCode::UsageOrConfig);
    }

    #[test]
    fn malformed_layout_fails_at_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(
            &path,
            r#"
profiles:
  cam:
    type: generic
    source: auto
    destination: /tmp/dest
    layout: "{date:%Y"
"#,
        )
        .unwrap();

        let err = load(&path).unwrap_err();
        assert!(err.to_string().contains("layout"));
    }

    #[test]
    fn raw_profile_serde_round_trips() {
        // Design Risk: serde flatten + internally-tagged enum has known
        // edge cases. Round-trip a RawProfile through YAML to confirm
        // the flattened `type` tag and common fields survive together.
        let original = RawProfile {
            kind: SourceKind::Generic,
            source: "auto".to_string(),
            destination: "/tmp/dest".to_string(),
            layout: "{date}".to_string(),
            ignore: vec!["*.tmp".to_string()],
            quarantine: Some("/tmp/quarantine".to_string()),
            delete_source: true,
        };

        let yaml = serde_yaml_ng::to_string(&original).unwrap();
        let round_tripped: RawProfile = serde_yaml_ng::from_str(&yaml).unwrap();
        assert_eq!(original, round_tripped);
    }
}
