// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use assembly_container::WalkPaths;
use camino::Utf8PathBuf;
use moniker::Moniker;
use schemars::JsonSchema;
use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::str::FromStr;

pub use diagnostics_log_types::Severity;

/// Diagnostics configuration options for the diagnostics area.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema, WalkPaths)]
#[serde(default, deny_unknown_fields)]
pub struct DiagnosticsConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archivist: Option<ArchivistConfig>,

    /// The set of pipeline config files to supply to archivist.
    #[walk_paths]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub archivist_pipelines: Vec<ArchivistPipeline>,

    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub additional_serial_log_components: Vec<String>,

    /// Additional set of entries to be incorporated into a deny list for logs
    /// being forwarded to the serial port.
    ///
    /// Entries in this list take precedence over those in `additional_serial_log_components`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub additional_denied_serial_log_components: Vec<String>,

    #[walk_paths]
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub sampler: SamplerConfig,

    /// Defines whether or not the Persistence component should skip its update check.
    ///
    /// This is a privacy and security sensitive setting that is rarely used and always
    /// set to false on user builds.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub persistence: PersistenceConfig,

    #[walk_paths]
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub memory_monitor: MemoryMonitorConfig,

    /// The set of log levels components will receive as their initial interest.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub component_log_initial_interests: Vec<ComponentInitialInterest>,

    /// Disable the console.
    #[serde(default)]
    pub no_console: bool,
}

/// Diagnostics configuration options for the archivist configuration area.
#[derive(Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub enum ArchivistConfig {
    Default,
    LowMem,
}

/// A single archivist pipeline config.
#[derive(Debug, Deserialize, Serialize, PartialEq, JsonSchema, WalkPaths)]
#[serde(deny_unknown_fields)]
pub struct ArchivistPipeline {
    /// The name of the pipeline.
    pub name: PipelineType,
    /// The files to add to the pipeline.
    /// Zero files is not valid.
    #[schemars(schema_with = "crate::vec_path_schema")]
    #[walk_paths]
    pub files: Vec<Utf8PathBuf>,
}

#[derive(Debug, PartialEq, Clone, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[serde(into = "String")]
pub enum PipelineType {
    /// A pipeline for feedback data.
    Feedback,
    /// A pipeline for previous boot data.
    PreviousBoot,
    /// A custom pipeline.
    Custom(String),
}

impl Serialize for PipelineType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Feedback => serializer.serialize_str("feedback"),
            Self::PreviousBoot => serializer.serialize_str("previous_boot"),
            Self::Custom(s) => serializer.serialize_str(s),
        }
    }
}

impl<'de> Deserialize<'de> for PipelineType {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let variant = String::deserialize(de)?.to_lowercase();
        match variant.as_str() {
            "feedback" => Ok(PipelineType::Feedback),
            "previous_boot" => Ok(PipelineType::PreviousBoot),
            "lowpan" => Ok(PipelineType::Custom("lowpan".into())),
            "legacy_metrics" => Ok(PipelineType::Custom("legacy_metrics".into())),
            other => Err(D::Error::unknown_variant(
                other,
                &["feedback", "previous_boot", "lowpan", "legacy_metrics"],
            )),
        }
    }
}

impl std::fmt::Display for PipelineType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Feedback => "feedback",
                Self::PreviousBoot => "previous_boot",
                Self::Custom(name) => &name,
            }
        )
    }
}

impl From<PipelineType> for String {
    fn from(t: PipelineType) -> Self {
        t.to_string()
    }
}

/// Configuration options for the Persistence component.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default)]
pub struct PersistenceConfig {
    /// Determines whether the component waits for the update check before publishing
    /// the previous boot's data.
    ///
    /// This is privacy and security sensitive, rarely used, and always set to false on
    /// user builds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_update_check: Option<bool>,

    /// Controls how long persistence will wait when it is idle before it
    /// escrows its FIDL connections back to the component framework and exits.
    /// By default, persistence runs indefinitely and does not escrow on idle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_on_idle_timeout_millis: Option<i64>,

    /// Controls the duration in between snapshots on
    /// fuchsia.diagnostics.ArchiveAccessor.previous_boot that get recorded to disk.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub persistence_period_seconds: Option<i64>,

    /// Battery percentage threshold to trigger an inspect snapshot.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub low_battery_threshold_percent: Option<u64>,
}

/// Diagnostics configuration options for the sampler configuration area.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema, WalkPaths)]
#[serde(default, deny_unknown_fields)]
pub struct SamplerConfig {
    /// The metrics configs to pass to sampler.
    #[schemars(schema_with = "crate::vec_path_schema")]
    #[walk_paths]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub project_configs: Vec<Utf8PathBuf>,

    /// The FIRE config to pass to sampler.
    #[walk_paths]
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub fire: FireConfig,

    // TODO(https://fxbug.dev/395159893): these are obsolete and unused. Remove.
    #[schemars(schema_with = "crate::vec_path_schema")]
    #[walk_paths]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub metrics_configs: Vec<Utf8PathBuf>,

    #[schemars(schema_with = "crate::vec_path_schema")]
    #[walk_paths]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fire_configs: Vec<Utf8PathBuf>,
}

/// Diagnostics configuration options for Sampler FIRE projects.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema, WalkPaths)]
#[serde(default, deny_unknown_fields)]
pub struct FireConfig {
    /// Component configuration files containing mapping from component attribution
    /// to Cobalt metric ID.
    #[schemars(schema_with = "crate::vec_path_schema")]
    #[walk_paths]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub component_configs: Vec<Utf8PathBuf>,

    /// FIRE project templates.
    #[schemars(schema_with = "crate::vec_path_schema")]
    #[walk_paths]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub project_templates: Vec<Utf8PathBuf>,
}

/// Diagnostics configuration options for the memory monitor configuration area.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema, WalkPaths)]
#[serde(default, deny_unknown_fields)]
pub struct MemoryMonitorConfig {
    /// The memory buckets config file to provide to memory monitor.
    #[schemars(schema_with = "crate::option_path_schema")]
    #[walk_paths]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buckets: Option<Utf8PathBuf>,

    /// Control whether a pressure change should trigger a capture.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub capture_on_pressure_change: bool,

    /// Expected delay between scheduled captures upon imminent OOM, in
    /// seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imminent_oom_capture_delay_s: Option<u32>,

    /// Expected delay between scheduled captures when the memory
    /// pressure is critical, in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub critical_capture_delay_s: Option<u32>,

    /// Expected delay between scheduled captures when the memory
    /// pressure is abnormal, in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning_capture_delay_s: Option<u32>,

    /// Expected delay between scheduled captures when the memory
    /// pressure is normal, in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub normal_capture_delay_s: Option<u32>,
}

// LINT.IfChange
/// The initial log interest that a component should receive upon starting up.
#[derive(Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ComponentInitialInterest {
    /// The URL or moniker for the component which should receive the initial interest.
    pub component: UrlOrMoniker,

    /// The log severity the initial interest should specify.
    pub log_severity: Severity,
}

impl ComponentInitialInterest {
    pub fn for_structured_config(&self) -> String {
        format!("{}:{}", self.component, self.log_severity)
    }
}
// LINT.ThenChange(/src/diagnostics/archivist/src/logs/repository.rs)

#[derive(Debug, PartialEq, JsonSchema)]
pub enum UrlOrMoniker {
    /// An absolute fuchsia url to a component.
    Url(String),
    /// The absolute moniker for a component.
    Moniker(String),
}

impl std::fmt::Display for UrlOrMoniker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Url(u) => write!(f, "{u}"),
            Self::Moniker(m) => write!(f, "{m}"),
        }
    }
}

impl Serialize for UrlOrMoniker {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(match self {
            Self::Url(s) | Self::Moniker(s) => s.as_str(),
        })
    }
}

impl<'de> Deserialize<'de> for UrlOrMoniker {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let variant = String::deserialize(de)?.to_lowercase();
        if fuchsia_url::fuchsia_pkg::AbsoluteComponentUrl::from_str(&variant).is_ok()
            || fuchsia_url::boot::AbsoluteComponentUrl::parse(variant.as_str()).is_ok()
        {
            Ok(UrlOrMoniker::Url(variant))
        } else if Moniker::from_str(&variant).is_ok() {
            Ok(UrlOrMoniker::Moniker(variant))
        } else {
            Err(D::Error::custom(format_args!("Expected a moniker or url. {variant} was neither",)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform_settings::PlatformSettings;

    use assembly_util as util;

    #[test]
    fn test_diagnostics_archivist_default_service() {
        let json5 = r#""default""#;
        let mut cursor = std::io::Cursor::new(json5);
        let archivist: ArchivistConfig = util::from_reader(&mut cursor).unwrap();
        assert_eq!(archivist, ArchivistConfig::Default);
    }

    #[test]
    fn test_diagnostics_archivist_low_mem() {
        let json5 = r#""low-mem""#;
        let mut cursor = std::io::Cursor::new(json5);
        let archivist: ArchivistConfig = util::from_reader(&mut cursor).unwrap();
        assert_eq!(archivist, ArchivistConfig::LowMem);
    }

    #[test]
    fn test_diagnostics_missing() {
        let json5 = r#"
        {
          build_type: "eng",
        }
    "#;

        let mut cursor = std::io::Cursor::new(json5);
        let config: PlatformSettings = util::from_reader(&mut cursor).unwrap();
        assert_eq!(
            config.diagnostics,
            DiagnosticsConfig {
                archivist: None,
                archivist_pipelines: Vec::new(),
                additional_serial_log_components: Vec::new(),
                sampler: SamplerConfig::default(),
                memory_monitor: MemoryMonitorConfig::default(),
                ..Default::default()
            }
        );
    }

    #[test]
    fn test_diagnostics_empty() {
        let json5 = r#"
        {
          build_type: "eng",
          diagnostics: {}
        }
    "#;

        let mut cursor = std::io::Cursor::new(json5);
        let config: PlatformSettings = util::from_reader(&mut cursor).unwrap();
        assert_eq!(
            config.diagnostics,
            DiagnosticsConfig {
                archivist: None,
                archivist_pipelines: Vec::new(),
                additional_serial_log_components: Vec::new(),
                sampler: SamplerConfig::default(),
                memory_monitor: MemoryMonitorConfig::default(),
                ..Default::default()
            }
        );
    }

    #[test]
    fn test_diagnostics_with_additional_log_tags() {
        let json5 = r#"
        {
          build_type: "eng",
          diagnostics: {
            additional_serial_log_components: [
                "/foo",
            ]
          }
        }
    "#;

        let mut cursor = std::io::Cursor::new(json5);
        let config: PlatformSettings = util::from_reader(&mut cursor).unwrap();
        assert_eq!(
            config.diagnostics,
            DiagnosticsConfig {
                archivist: None,
                archivist_pipelines: Vec::new(),
                additional_serial_log_components: vec!["/foo".to_string()],
                sampler: SamplerConfig::default(),
                memory_monitor: MemoryMonitorConfig::default(),
                ..Default::default()
            }
        );
    }

    #[test]
    fn test_diagnostics_with_component_log_initial_interests() {
        let json5 = r#"
        {
          build_type: "eng",
          diagnostics: {
            component_log_initial_interests: [
                {
                    component: "fuchsia-boot:///driver_host#meta/driver_host.cm",
                    log_severity: "debug",
                },
                {
                    component: "fuchsia-pkg://fuchsia.com/trace_manager#meta/trace_manager.cm",
                    log_severity: "error",
                },
                {
                    component: "/bootstrap/driver_manager",
                    log_severity: "fatal",
                },
            ]
          }
        }
    "#;

        let mut cursor = std::io::Cursor::new(json5);
        let config: PlatformSettings = util::from_reader(&mut cursor).unwrap();
        assert_eq!(
            config.diagnostics,
            DiagnosticsConfig {
                component_log_initial_interests: vec![
                    ComponentInitialInterest {
                        component: UrlOrMoniker::Url(
                            "fuchsia-boot:///driver_host#meta/driver_host.cm".to_string()
                        ),
                        log_severity: Severity::Debug,
                    },
                    ComponentInitialInterest {
                        component: UrlOrMoniker::Url(
                            "fuchsia-pkg://fuchsia.com/trace_manager#meta/trace_manager.cm"
                                .to_string()
                        ),
                        log_severity: Severity::Error,
                    },
                    ComponentInitialInterest {
                        component: UrlOrMoniker::Moniker("/bootstrap/driver_manager".to_string()),
                        log_severity: Severity::Fatal,
                    },
                ],
                ..Default::default()
            }
        );
    }

    #[test]
    fn test_serialize_component_log_initial_interests() {
        assert_eq!(
            serde_json::to_string(&ComponentInitialInterest {
                component: UrlOrMoniker::Url(
                    "fuchsia-boot:///driver_host#meta/driver_host.cm".to_string()
                ),
                log_severity: Severity::Debug,
            })
            .unwrap(),
            r#"{"component":"fuchsia-boot:///driver_host#meta/driver_host.cm","log_severity":"DEBUG"}"#,
        );
        assert_eq!(
            serde_json::to_string(&ComponentInitialInterest {
                component: UrlOrMoniker::Moniker("/bootstrap/driver_manager".to_string()),
                log_severity: Severity::Fatal,
            })
            .unwrap(),
            r#"{"component":"/bootstrap/driver_manager","log_severity":"FATAL"}"#,
        );
    }

    #[test]
    fn serialize_deserialize_component_log_initial_interest() {
        let original = ComponentInitialInterest {
            component: UrlOrMoniker::Url(
                "fuchsia-boot:///driver_host#meta/driver_host.cm".to_string(),
            ),
            log_severity: Severity::Debug,
        };
        let serialized = serde_json::to_string(&original).expect("serialize interest");
        let deserialized: ComponentInitialInterest =
            serde_json::from_str(&serialized).expect("deserialize interest");
        assert_eq!(deserialized, original);

        let original = ComponentInitialInterest {
            component: UrlOrMoniker::Moniker("/bootstrap/driver_manager".to_string()),
            log_severity: Severity::Fatal,
        };
        let serialized = serde_json::to_string(&original).expect("serialize interest");
        let deserialized: ComponentInitialInterest =
            serde_json::from_str(&serialized).expect("deserialize interest");
        assert_eq!(deserialized, original);
    }
}
