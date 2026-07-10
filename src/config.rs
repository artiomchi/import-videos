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
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};

pub use layout::LayoutTemplate;

use crate::error::{Error, Result};
use crate::source::gopro::GoproSource;
use crate::source::tesla::{self, EventCategory, Reasons, TeslaSource};
use crate::source::{GenericSource, ImportSource};

/// Device-specific profile fields, an internally tagged enum on `type`
/// (design D1): serde rejects an unrecognized `type` at load for free,
/// which is how "unknown profile type" (spec) gets its exit-2 error
/// without extra validation code of our own.
///
/// `Generic` is the zero-extra-fields placeholder from `add-core-cli`
/// that exercises the flatten + internally-tagged-enum mechanism
/// (design Risks). `Gopro` is the first real device (design D1):
/// `require_marker` is a device-specific knob that rides on the
/// variant itself rather than polluting the common `Profile` surface.
/// `add-tesla-import` adds a sibling variant here plus an
/// `ImportSource` impl under `src/source/`.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SourceKind {
    Generic,
    Gopro {
        #[serde(default = "default_require_marker")]
        require_marker: bool,
    },
    Tesla {
        #[serde(default = "tesla::default_events")]
        events: Vec<EventCategory>,
        #[serde(default)]
        reasons: Option<Reasons>,
    },
}

fn default_require_marker() -> bool {
    true
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
            SourceKind::Gopro { require_marker } => Box::new(GoproSource {
                require_marker: *require_marker,
            }),
            SourceKind::Tesla { events, reasons } => Box::new(TeslaSource {
                events: events.clone(),
                reasons: reasons.clone(),
            }),
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
    /// When `false`, `Quarantine` groups are never copied; their source
    /// files are left exactly where they are. The `Quarantine` verdict
    /// is still produced and reported — only what execution does with
    /// it changes. Defaults to `true` (verified-copy behavior, ADR
    /// 0003) when omitted from the YAML.
    pub copy_quarantine: bool,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub profiles: HashMap<String, Profile>,
    pub mount_roots: Vec<PathBuf>,
    /// Resolved IANA timezone (default: system local). Every rendered
    /// timestamp — layout paths, sidecar fields, logs — formats through
    /// this zone. Device wall clocks are also *interpreted* in it.
    pub timezone: TimeZone,
}

#[derive(Debug, Deserialize, Serialize)]
struct RawConfig {
    #[serde(default)]
    mount_roots: Option<Vec<String>>,
    /// Optional IANA timezone name. Unset → system local; unknown name →
    /// `Error::Config` (exit 2).
    #[serde(default)]
    timezone: Option<String>,
    // Kept as raw YAML values, not `RawProfile`, so `load` can check
    // for `require_marker` on the wrong device type before that field
    // gets silently swallowed by the flattened, internally-tagged
    // `SourceKind` enum (a unit variant like `Generic` ignores extra
    // content rather than rejecting it — design Risks).
    #[serde(default)]
    profiles: HashMap<String, serde_yaml_ng::Value>,
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
    #[serde(default = "default_copy_quarantine")]
    copy_quarantine: bool,
}

fn default_copy_quarantine() -> bool {
    true
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

    let timezone = match raw.timezone {
        None => TimeZone::system(),
        Some(ref name) => TimeZone::get(name).map_err(|_| {
            Error::Config(format!(
                "timezone: unrecognized IANA timezone name '{name}'"
            ))
        })?,
    };

    let mut profiles = HashMap::with_capacity(raw.profiles.len());
    for (name, raw_value) in raw.profiles {
        let has_require_marker = raw_value
            .as_mapping()
            .map(|m| m.get("require_marker").is_some())
            .unwrap_or(false);
        let has_tesla_field = raw_value
            .as_mapping()
            .map(|m| m.get("events").is_some() || m.get("reasons").is_some())
            .unwrap_or(false);

        let raw_profile: RawProfile = serde_yaml_ng::from_value(raw_value)
            .map_err(|e| Error::Config(format!("profile '{name}': {e}")))?;

        if has_require_marker && !matches!(raw_profile.kind, SourceKind::Gopro { .. }) {
            return Err(Error::Config(format!(
                "profile '{name}': require_marker is only valid for profiles of type gopro"
            )));
        }
        if has_tesla_field && !matches!(raw_profile.kind, SourceKind::Tesla { .. }) {
            return Err(Error::Config(format!(
                "profile '{name}': events/reasons are only valid for profiles of type tesla"
            )));
        }

        let profile = validate_profile(&name, raw_profile)?;
        profiles.insert(name, profile);
    }

    Ok(Config {
        profiles,
        mount_roots,
        timezone,
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

    let destination = expand_tilde(&raw.destination);
    // A relative `quarantine` (e.g. `./_quarantine`) is resolved against
    // `destination`, not the process's current directory — the profile
    // already implies "relative to where kept footage lands" via the
    // unset-quarantine default (`destination.join("_quarantine")`), so an
    // explicit relative path follows the same rule instead of depending on
    // wherever the CLI happened to be invoked from.
    let quarantine = raw.quarantine.map(|s| {
        let path = expand_tilde(&s);
        if path.is_absolute() {
            path
        } else {
            destination.join(path)
        }
    });

    Ok(Profile {
        kind: raw.kind,
        source,
        destination,
        layout,
        ignore,
        quarantine,
        delete_source: raw.delete_source,
        copy_quarantine: raw.copy_quarantine,
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
    fn relative_quarantine_resolves_against_destination() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(
            &path,
            format!(
                "profiles:\n  cam:\n    type: generic\n    source: auto\n    destination: {}\n    layout: \"{{date}}\"\n    quarantine: ./_quarantine\n",
                dir.path().join("dest").display()
            ),
        )
        .unwrap();

        let cfg = load(&path).unwrap();
        let profile = cfg.profiles.get("cam").unwrap();
        assert_eq!(
            profile.quarantine,
            Some(dir.path().join("dest").join("_quarantine"))
        );
    }

    #[test]
    fn absolute_quarantine_is_left_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        let quarantine_path = dir.path().join("elsewhere/_quarantine");
        std::fs::write(
            &path,
            format!(
                "profiles:\n  cam:\n    type: generic\n    source: auto\n    destination: {}\n    layout: \"{{date}}\"\n    quarantine: {}\n",
                dir.path().join("dest").display(),
                quarantine_path.display()
            ),
        )
        .unwrap();

        let cfg = load(&path).unwrap();
        let profile = cfg.profiles.get("cam").unwrap();
        assert_eq!(profile.quarantine, Some(quarantine_path));
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
            copy_quarantine: true,
        };

        let yaml = serde_yaml_ng::to_string(&original).unwrap();
        let round_tripped: RawProfile = serde_yaml_ng::from_str(&yaml).unwrap();
        assert_eq!(original, round_tripped);
    }

    #[test]
    fn gopro_variant_serde_round_trips() {
        // Same risk as the Generic round-trip above, but for a variant
        // that actually carries a field (design D1) — the flatten +
        // internally-tagged-enum combination is exactly where that's
        // most likely to misbehave.
        let original = RawProfile {
            kind: SourceKind::Gopro {
                require_marker: false,
            },
            source: "auto".to_string(),
            destination: "/tmp/dest".to_string(),
            layout: "{date}".to_string(),
            ignore: vec!["*.LRV".to_string()],
            quarantine: None,
            delete_source: false,
            copy_quarantine: false,
        };

        let yaml = serde_yaml_ng::to_string(&original).unwrap();
        let round_tripped: RawProfile = serde_yaml_ng::from_str(&yaml).unwrap();
        assert_eq!(original, round_tripped);
    }

    #[test]
    fn require_marker_rejected_on_non_gopro_profile() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(
            &path,
            format!(
                "profiles:\n  cam:\n    type: generic\n    require_marker: false\n    source: auto\n    destination: {}\n    layout: \"{{date}}\"\n",
                dir.path().join("dest").display()
            ),
        )
        .unwrap();

        let err = load(&path).unwrap_err();
        assert!(matches!(err, Error::Config(_)));
        assert!(err.to_string().contains("cam"));
        assert!(err.to_string().contains("require_marker"));
        assert_eq!(err.exit_code(), crate::error::ExitCode::UsageOrConfig);
    }

    #[test]
    fn gopro_profile_defaults_require_marker_to_true() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(
            &path,
            format!(
                "profiles:\n  gopro:\n    type: gopro\n    source: auto\n    destination: {}\n    layout: \"{{date}}\"\n",
                dir.path().join("dest").display()
            ),
        )
        .unwrap();

        let cfg = load(&path).unwrap();
        assert_eq!(
            cfg.profiles.get("gopro").unwrap().kind,
            SourceKind::Gopro {
                require_marker: true
            }
        );
    }

    // --- Tesla profile (add-tesla-import, design D5) ---

    #[test]
    fn tesla_profile_defaults_events_and_no_reason_filter() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(
            &path,
            format!(
                "profiles:\n  tesla:\n    type: tesla\n    source: auto\n    destination: {}\n    layout: \"{{event_type}}\"\n",
                dir.path().join("dest").display()
            ),
        )
        .unwrap();

        let cfg = load(&path).unwrap();
        assert_eq!(
            cfg.profiles.get("tesla").unwrap().kind,
            SourceKind::Tesla {
                events: tesla::default_events(),
                reasons: None,
            }
        );
    }

    #[test]
    fn tesla_reasons_allow_and_deny_together_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(
            &path,
            format!(
                "profiles:\n  tesla:\n    type: tesla\n    source: auto\n    destination: {}\n    layout: \"{{event_type}}\"\n    reasons:\n      allow: [user_interaction_honk]\n      deny: [sentry_aware_object_detection]\n",
                dir.path().join("dest").display()
            ),
        )
        .unwrap();

        let err = load(&path).unwrap_err();
        assert!(matches!(err, Error::Config(_)));
        assert!(err.to_string().contains("tesla"));
        assert_eq!(err.exit_code(), crate::error::ExitCode::UsageOrConfig);
    }

    #[test]
    fn tesla_reasons_empty_block_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(
            &path,
            format!(
                "profiles:\n  tesla:\n    type: tesla\n    source: auto\n    destination: {}\n    layout: \"{{event_type}}\"\n    reasons: {{}}\n",
                dir.path().join("dest").display()
            ),
        )
        .unwrap();

        let err = load(&path).unwrap_err();
        assert!(matches!(err, Error::Config(_)));
    }

    #[test]
    fn tesla_fields_rejected_on_non_tesla_profile() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(
            &path,
            format!(
                "profiles:\n  cam:\n    type: gopro\n    events: [saved]\n    source: auto\n    destination: {}\n    layout: \"{{date}}\"\n",
                dir.path().join("dest").display()
            ),
        )
        .unwrap();

        let err = load(&path).unwrap_err();
        assert!(matches!(err, Error::Config(_)));
        assert!(err.to_string().contains("cam"));
        assert!(err.to_string().contains("events"));
        assert_eq!(err.exit_code(), crate::error::ExitCode::UsageOrConfig);
    }

    #[test]
    fn tesla_variant_serde_round_trips() {
        // Same risk as the Generic/Gopro round-trips above (design
        // Risks): flatten + internally-tagged enum. `reasons` stays
        // `None` here deliberately: `Reasons` is itself a data-carrying
        // enum, and serde's `flatten` combinator cannot round-trip a
        // *nested* enum serialized in its default (tag-based) YAML form
        // — a documented serde limitation, not a bug in `Reasons`
        // itself (see `reasons_round_trips_outside_flatten` in
        // `source::tesla`, and the config-loading tests above, which
        // exercise the real load path: hand-written YAML through
        // `serde_yaml_ng::Value`, never through `Serialize`).
        let original = RawProfile {
            kind: SourceKind::Tesla {
                events: vec![EventCategory::Saved, EventCategory::Recent],
                reasons: None,
            },
            source: "auto".to_string(),
            destination: "/tmp/dest".to_string(),
            layout: "{event_type}".to_string(),
            ignore: vec![],
            quarantine: None,
            delete_source: false,
            copy_quarantine: true,
        };

        let yaml = serde_yaml_ng::to_string(&original).unwrap();
        let round_tripped: RawProfile = serde_yaml_ng::from_str(&yaml).unwrap();
        assert_eq!(original, round_tripped);
    }

    #[test]
    fn copy_quarantine_defaults_to_true_when_omitted() {
        // Spec scenario: "Quarantine copy defaults to enabled" — a
        // profile that omits `copy_quarantine` must load with the field
        // set to `true` (i.e. today's verified-copy behavior).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(
            &path,
            format!(
                "profiles:\n  cam:\n    type: generic\n    source: auto\n    destination: {}\n    layout: \"{{date}}\"\n",
                dir.path().join("dest").display()
            ),
        )
        .unwrap();

        let cfg = load(&path).unwrap();
        assert!(
            cfg.profiles.get("cam").unwrap().copy_quarantine,
            "omitting copy_quarantine should default to true"
        );
    }

    #[test]
    fn copy_quarantine_false_loads_correctly() {
        // Spec scenario: "Quarantine copy can be disabled" — a profile
        // with `copy_quarantine: false` must load with the field `false`.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(
            &path,
            format!(
                "profiles:\n  cam:\n    type: generic\n    source: auto\n    destination: {}\n    layout: \"{{date}}\"\n    copy_quarantine: false\n",
                dir.path().join("dest").display()
            ),
        )
        .unwrap();

        let cfg = load(&path).unwrap();
        assert!(
            !cfg.profiles.get("cam").unwrap().copy_quarantine,
            "copy_quarantine: false must load as false"
        );
    }

    // --- Timezone config (unify-timestamps-and-sidecars) ---

    #[test]
    fn explicit_valid_timezone_loads() {
        // Spec scenario: "Explicit timezone loads"
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(
            &path,
            format!(
                "timezone: Europe/Vilnius\nprofiles:\n  cam:\n    type: generic\n    source: auto\n    destination: {}\n    layout: \"{{date}}\"\n",
                dir.path().join("dest").display()
            ),
        )
        .unwrap();

        let cfg = load(&path).unwrap();
        // The zone should be Europe/Vilnius; round-trip via IANA name.
        assert_eq!(cfg.timezone.iana_name(), Some("Europe/Vilnius"));
    }

    #[test]
    fn invalid_timezone_rejected_at_load() {
        // Spec scenario: "Invalid timezone rejected at load"
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(
            &path,
            format!(
                "timezone: Mars/Olympus\nprofiles:\n  cam:\n    type: generic\n    source: auto\n    destination: {}\n    layout: \"{{date}}\"\n",
                dir.path().join("dest").display()
            ),
        )
        .unwrap();

        let err = load(&path).unwrap_err();
        assert!(matches!(err, Error::Config(_)));
        assert!(err.to_string().contains("timezone"));
        assert_eq!(err.exit_code(), crate::error::ExitCode::UsageOrConfig);
    }

    #[test]
    fn unset_timezone_defaults_to_system_local() {
        // Spec scenario: "Unset timezone defaults to system local"
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(
            &path,
            format!(
                "profiles:\n  cam:\n    type: generic\n    source: auto\n    destination: {}\n    layout: \"{{date}}\"\n",
                dir.path().join("dest").display()
            ),
        )
        .unwrap();

        // Should load without error; we can't assert the exact zone
        // (system-dependent), just that it loads.
        let cfg = load(&path).unwrap();
        // system() zones may not have an IANA name on all hosts, so
        // just confirm the config loads successfully.
        let _ = cfg.timezone;
    }
}
