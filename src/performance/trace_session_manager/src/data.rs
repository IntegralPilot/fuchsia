// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_tracing::BufferingMode;
use fidl_fuchsia_tracing_controller::{
    Action, FxtVersion, ProviderSpec, TraceConfig, TraceOptions, Trigger,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct TriggerData {
    pub action: Option<String>,
    pub alert: Option<String>,
}

impl From<Trigger> for TriggerData {
    fn from(trigger: Trigger) -> Self {
        Self {
            action: trigger.action.map(|a| match a {
                Action::Terminate => "TERMINATE".to_string(),
                _ => "UNKNOWN".to_string(),
            }),
            alert: trigger.alert,
        }
    }
}

impl From<TriggerData> for Trigger {
    fn from(data: TriggerData) -> Self {
        Self {
            alert: data.alert,
            action: data.action.map(|s| match s.as_str() {
                "TERMINATE" => Action::Terminate,
                _ => Action::unknown(),
            }),
            // Don't use  ..Default::default() here so if fields are added, the compilation errors
            // will remind us to add the new fields to TraceOptionsData.
            __source_breaking: Default::default(),
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct ProviderSpecData {
    pub name: Option<String>,
    pub buffer_size_megabytes_hint: Option<u32>,
    pub categories: Option<Vec<String>>,
}

impl From<ProviderSpec> for ProviderSpecData {
    fn from(spec: ProviderSpec) -> Self {
        Self {
            name: spec.name,
            buffer_size_megabytes_hint: spec.buffer_size_megabytes_hint,
            categories: spec.categories,
        }
    }
}

impl From<ProviderSpecData> for ProviderSpec {
    fn from(data: ProviderSpecData) -> Self {
        Self {
            name: data.name,
            buffer_size_megabytes_hint: data.buffer_size_megabytes_hint,
            categories: data.categories,
            // Don't use  ..Default::default() here so if fields are added, the compilation errors
            // will remind us to add the new fields to TraceOptionsData.
            __source_breaking: Default::default(),
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct TraceOptionsData {
    pub duration_ns: Option<i64>,
    pub triggers: Option<Vec<TriggerData>>,
    pub requested_categories: Option<Vec<String>>,
    pub compression: Option<String>,
}

impl From<TraceOptions> for TraceOptionsData {
    fn from(options: TraceOptions) -> Self {
        Self {
            duration_ns: options.duration_ns,
            triggers: options
                .triggers
                .map(|triggers| triggers.into_iter().map(|t| t.into()).collect()),
            requested_categories: options.requested_categories,
            compression: options.compression.map(|c| match c {
                fidl_fuchsia_tracing_controller::CompressionType::Zstd => "ZSTD".to_string(),
                fidl_fuchsia_tracing_controller::CompressionType::None => "NONE".to_string(),
                _ => {
                    log::error!("Unknown compression type: {c:?} using NONE");
                    "NONE".to_string()
                }
            }),
        }
    }
}

impl From<TraceOptionsData> for TraceOptions {
    fn from(data: TraceOptionsData) -> Self {
        Self {
            duration_ns: data.duration_ns,
            triggers: data
                .triggers
                .map(|triggers| triggers.into_iter().map(|t| t.into()).collect()),
            requested_categories: data.requested_categories,
            compression: data.compression.map(|s| match s.as_str() {
                "ZSTD" => fidl_fuchsia_tracing_controller::CompressionType::Zstd,
                "NONE" => fidl_fuchsia_tracing_controller::CompressionType::None,
                _ => {
                    log::error!("Unknown compression type: {s:?} using NONE");
                    fidl_fuchsia_tracing_controller::CompressionType::None
                }
            }),
            // Don't use  ..Default::default() here so if fields are added, the compilation errors
            // will remind us to add the new fields to TraceOptionsData.
            __source_breaking: Default::default(),
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct FxtVersionData {
    pub major: Option<u32>,
    pub minor: Option<u32>,
}

impl From<FxtVersion> for FxtVersionData {
    fn from(version: FxtVersion) -> Self {
        Self { major: version.major, minor: version.minor }
    }
}

impl From<FxtVersionData> for FxtVersion {
    fn from(data: FxtVersionData) -> Self {
        Self {
            major: data.major,
            minor: data.minor,
            // Don't use  ..Default::default() here so if fields are added, the compilation errors
            // will remind us to add the new fields to TraceOptionsData.
            __source_breaking: Default::default(),
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct TraceConfigData {
    pub categories: Option<Vec<String>>,
    pub buffer_size_megabytes_hint: Option<u32>,
    pub start_timeout_milliseconds: Option<u64>,
    pub buffering_mode: Option<String>,
    pub provider_specs: Option<Vec<ProviderSpecData>>,
    pub version: Option<FxtVersionData>,
    pub defer_transfer: Option<bool>,
}

impl From<TraceConfig> for TraceConfigData {
    fn from(config: TraceConfig) -> Self {
        Self {
            categories: config.categories,
            buffer_size_megabytes_hint: config.buffer_size_megabytes_hint,
            start_timeout_milliseconds: config.start_timeout_milliseconds,
            buffering_mode: config.buffering_mode.map(|m| match m {
                BufferingMode::Oneshot => "ONESHOT".to_string(),
                BufferingMode::Circular => "CIRCULAR".to_string(),
                BufferingMode::Streaming => "STREAMING".to_string(),
            }),
            provider_specs: config
                .provider_specs
                .map(|specs| specs.into_iter().map(|s| s.into()).collect()),
            version: config.version.map(|v| v.into()),
            defer_transfer: config.defer_transfer,
        }
    }
}

impl From<TraceConfigData> for TraceConfig {
    fn from(data: TraceConfigData) -> Self {
        Self {
            categories: data.categories,
            buffer_size_megabytes_hint: data.buffer_size_megabytes_hint,
            start_timeout_milliseconds: data.start_timeout_milliseconds,
            buffering_mode: data.buffering_mode.map(|s| match s.as_str() {
                "ONESHOT" => BufferingMode::Oneshot,
                "CIRCULAR" => BufferingMode::Circular,
                "STREAMING" => BufferingMode::Streaming,
                // This should not happen if we are the ones writing the file.
                _ => BufferingMode::Oneshot,
            }),
            provider_specs: data
                .provider_specs
                .map(|specs| specs.into_iter().map(|s| s.into()).collect()),
            version: data.version.map(|v| v.into()),
            defer_transfer: data.defer_transfer,
            // Don't use  ..Default::default() here so if fields are added, the compilation errors
            // will remind us to add the new fields to TraceOptionsData.
            __source_breaking: Default::default(),
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct OnBootTraceConfig {
    pub options: TraceOptionsData,
    pub config: TraceConfigData,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_trigger_eq(
        t1: &fidl_fuchsia_tracing_controller::Trigger,
        t2: &fidl_fuchsia_tracing_controller::Trigger,
    ) {
        assert_eq!(t1.action, t2.action);
        assert_eq!(t1.alert, t2.alert);
    }

    fn assert_provider_spec_eq(
        p1: &fidl_fuchsia_tracing_controller::ProviderSpec,
        p2: &fidl_fuchsia_tracing_controller::ProviderSpec,
    ) {
        assert_eq!(p1.name, p2.name);
        assert_eq!(p1.buffer_size_megabytes_hint, p2.buffer_size_megabytes_hint);
        assert_eq!(p1.categories, p2.categories);
    }

    #[test]
    fn test_trigger_conversion() {
        let trigger = || fidl_fuchsia_tracing_controller::Trigger {
            action: Some(fidl_fuchsia_tracing_controller::Action::Terminate),
            alert: Some("alert".to_string()),
            ..Default::default()
        };

        let data: TriggerData = trigger().into();
        let converted_trigger: fidl_fuchsia_tracing_controller::Trigger = data.into();

        assert_trigger_eq(&converted_trigger, &trigger());
    }

    #[test]
    fn test_trigger_conversion_unknown_action() {
        let trigger = || fidl_fuchsia_tracing_controller::Trigger {
            action: Some(fidl_fuchsia_tracing_controller::Action::unknown()),
            alert: Some("alert".to_string()),
            ..Default::default()
        };

        let data: TriggerData = trigger().into();
        let converted_trigger: fidl_fuchsia_tracing_controller::Trigger = data.into();

        assert_trigger_eq(&converted_trigger, &trigger());
    }

    #[test]
    fn test_provider_spec_conversion() {
        let spec = || fidl_fuchsia_tracing_controller::ProviderSpec {
            name: Some("provider".to_string()),
            buffer_size_megabytes_hint: Some(1),
            categories: Some(vec!["c1".to_string(), "c2".to_string()]),
            ..Default::default()
        };

        let data: ProviderSpecData = spec().into();
        let converted_spec: fidl_fuchsia_tracing_controller::ProviderSpec = data.into();

        assert_provider_spec_eq(&converted_spec, &spec());
    }

    #[test]
    fn test_trace_options_conversion() {
        let options = || fidl_fuchsia_tracing_controller::TraceOptions {
            duration_ns: Some(100),
            triggers: Some(vec![fidl_fuchsia_tracing_controller::Trigger {
                action: Some(fidl_fuchsia_tracing_controller::Action::Terminate),
                alert: Some("alert".to_string()),
                ..Default::default()
            }]),
            requested_categories: Some(vec!["cat1".to_string()]),
            compression: Some(fidl_fuchsia_tracing_controller::CompressionType::Zstd),
            ..Default::default()
        };

        let data: TraceOptionsData = options().into();
        let converted_options: fidl_fuchsia_tracing_controller::TraceOptions = data.into();

        let expected_options = options();
        assert_eq!(converted_options.duration_ns, expected_options.duration_ns);
        assert_eq!(converted_options.triggers.as_ref().map(|t| t.len()), Some(1));
        assert_trigger_eq(
            &converted_options.triggers.as_ref().unwrap()[0],
            &expected_options.triggers.as_ref().unwrap()[0],
        );
        assert_eq!(converted_options.requested_categories, expected_options.requested_categories);
        assert_eq!(converted_options.compression, expected_options.compression);
    }

    #[test]
    fn test_trace_options_conversion_none_compression() {
        let options = || fidl_fuchsia_tracing_controller::TraceOptions {
            duration_ns: None,
            triggers: None,
            requested_categories: None,
            compression: Some(fidl_fuchsia_tracing_controller::CompressionType::None),
            ..Default::default()
        };

        let data: TraceOptionsData = options().into();
        let converted_options: fidl_fuchsia_tracing_controller::TraceOptions = data.into();

        assert_eq!(
            converted_options.compression,
            Some(fidl_fuchsia_tracing_controller::CompressionType::None)
        );
    }

    #[test]
    fn test_on_boot_trace_config_serde_roundtrip() {
        let on_boot_config = OnBootTraceConfig {
            options: TraceOptionsData {
                duration_ns: Some(5_000_000_000),
                triggers: Some(vec![TriggerData {
                    action: Some("TERMINATE".to_string()),
                    alert: Some("high_memory".to_string()),
                }]),
                requested_categories: Some(vec!["kernel:sched".to_string(), "gfx".to_string()]),
                compression: Some("ZSTD".to_string()),
            },
            config: TraceConfigData {
                categories: Some(vec!["kernel:sched".to_string(), "gfx".to_string()]),
                buffer_size_megabytes_hint: Some(16),
                start_timeout_milliseconds: Some(2500),
                buffering_mode: Some("CIRCULAR".to_string()),
                provider_specs: Some(vec![ProviderSpecData {
                    name: Some("archivist".to_string()),
                    buffer_size_megabytes_hint: Some(2),
                    categories: Some(vec!["log".to_string()]),
                }]),
                version: Some(FxtVersionData { major: Some(1), minor: Some(0) }),
                defer_transfer: Some(false),
            },
        };

        let json_str = serde_json::to_string_pretty(&on_boot_config)
            .expect("Failed to serialize OnBootTraceConfig");
        let deserialized: OnBootTraceConfig =
            serde_json::from_str(&json_str).expect("Failed to deserialize OnBootTraceConfig");

        assert_eq!(deserialized.options.duration_ns, on_boot_config.options.duration_ns);
        assert_eq!(
            deserialized.options.requested_categories,
            on_boot_config.options.requested_categories
        );
        assert_eq!(deserialized.options.compression, on_boot_config.options.compression);
        assert_eq!(deserialized.options.triggers.as_ref().map(|t| t.len()), Some(1));
        assert_eq!(
            deserialized.options.triggers.as_ref().unwrap()[0].action,
            Some("TERMINATE".to_string())
        );
        assert_eq!(
            deserialized.options.triggers.as_ref().unwrap()[0].alert,
            Some("high_memory".to_string())
        );

        assert_eq!(deserialized.config.categories, on_boot_config.config.categories);
        assert_eq!(
            deserialized.config.buffer_size_megabytes_hint,
            on_boot_config.config.buffer_size_megabytes_hint
        );
        assert_eq!(
            deserialized.config.start_timeout_milliseconds,
            on_boot_config.config.start_timeout_milliseconds
        );
        assert_eq!(deserialized.config.buffering_mode, on_boot_config.config.buffering_mode);
        assert_eq!(deserialized.config.provider_specs.as_ref().map(|p| p.len()), Some(1));
        assert_eq!(
            deserialized.config.provider_specs.as_ref().unwrap()[0].name,
            Some("archivist".to_string())
        );
        assert_eq!(deserialized.config.version.as_ref().unwrap().major, Some(1));
        assert_eq!(deserialized.config.version.as_ref().unwrap().minor, Some(0));
        assert_eq!(deserialized.config.defer_transfer, Some(false));

        // Also verify converting deserialized config back to FIDL types
        let fidl_options: fidl_fuchsia_tracing_controller::TraceOptions =
            deserialized.options.into();
        assert_eq!(fidl_options.duration_ns, Some(5_000_000_000));
        assert_eq!(
            fidl_options.compression,
            Some(fidl_fuchsia_tracing_controller::CompressionType::Zstd)
        );

        let fidl_config: fidl_fuchsia_tracing_controller::TraceConfig = deserialized.config.into();
        assert_eq!(fidl_config.buffer_size_megabytes_hint, Some(16));
        assert_eq!(fidl_config.buffering_mode, Some(BufferingMode::Circular));
    }

    #[test]
    fn test_fxt_version_conversion() {
        let version = || fidl_fuchsia_tracing_controller::FxtVersion {
            major: Some(1),
            minor: Some(2),
            ..Default::default()
        };

        let data: FxtVersionData = version().into();
        let converted_version: fidl_fuchsia_tracing_controller::FxtVersion = data.into();

        let expected_version = version();
        assert_eq!(converted_version.major, expected_version.major);
        assert_eq!(converted_version.minor, expected_version.minor);
    }

    #[test]
    fn test_trace_config_conversion() {
        let config = || fidl_fuchsia_tracing_controller::TraceConfig {
            categories: Some(vec!["cat1".to_string()]),
            buffer_size_megabytes_hint: Some(4),
            start_timeout_milliseconds: Some(1000),
            buffering_mode: Some(fidl_fuchsia_tracing::BufferingMode::Circular),
            provider_specs: Some(vec![fidl_fuchsia_tracing_controller::ProviderSpec {
                name: Some("provider".to_string()),
                buffer_size_megabytes_hint: Some(1),
                categories: Some(vec!["c1".to_string(), "c2".to_string()]),
                ..Default::default()
            }]),
            version: Some(fidl_fuchsia_tracing_controller::FxtVersion {
                major: Some(1),
                minor: Some(2),
                ..Default::default()
            }),
            defer_transfer: Some(true),
            ..Default::default()
        };

        let data: TraceConfigData = config().into();
        let converted_config: fidl_fuchsia_tracing_controller::TraceConfig = data.into();

        let expected_config = config();
        assert_eq!(converted_config.categories, expected_config.categories);
        assert_eq!(
            converted_config.buffer_size_megabytes_hint,
            expected_config.buffer_size_megabytes_hint
        );
        assert_eq!(
            converted_config.start_timeout_milliseconds,
            expected_config.start_timeout_milliseconds
        );
        assert_eq!(converted_config.buffering_mode, expected_config.buffering_mode);
        assert_eq!(converted_config.provider_specs.as_ref().map(|p| p.len()), Some(1));
        assert_provider_spec_eq(
            &converted_config.provider_specs.as_ref().unwrap()[0],
            &expected_config.provider_specs.as_ref().unwrap()[0],
        );
        assert_eq!(
            converted_config.version.as_ref().unwrap().major,
            expected_config.version.as_ref().unwrap().major
        );
        assert_eq!(
            converted_config.version.as_ref().unwrap().minor,
            expected_config.version.as_ref().unwrap().minor
        );
        assert_eq!(converted_config.defer_transfer, expected_config.defer_transfer);
    }
}
