// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::subsystems::prelude::*;
use anyhow::{Context, anyhow, ensure};
use assembly_config_capabilities::{Config, ConfigValueType};
use assembly_config_schema::platform_settings::sysmem_config::{
    BoardSysmemConfig, MemorySize, PlatformSysmemConfig,
};
use assembly_constants::{BootfsPackageDestination, PackageSetDestination};
use camino::Utf8PathBuf;
use std::collections::HashSet;

const CONTIGUOUS_MEMORY_SIZE_FIELD: &str = "contiguous_memory_size";
const CONTIGUOUS_MEMORY_SIZE_CAPABILITY_SUFFIX: &str = "ContiguousMemorySize";
const FIXED_CONTIGUOUS_MEMORY_SIZE_CAPABILITY: &str = "fuchsia.sysmem.FixedContiguousMemorySize";
#[allow(unused)]
const PERCENT_CONTIGUOUS_MEMORY_SIZE_CAPABILITY: &str =
    "fuchsia.sysmem.PercentContiguousMemorySize";

const PROTECTED_MEMORY_SIZE_FIELD: &str = "protected_memory_size";
const PROTECTED_MEMORY_SIZE_CAPABILITY_SUFFIX: &str = "ProtectedMemorySize";
const FIXED_PROTECTED_MEMORY_SIZE_CAPABILITY: &str = "fuchsia.sysmem.FixedProtectedMemorySize";
#[allow(unused)]
const PERCENT_PROTECTED_MEMORY_SIZE_CAPABILITY: &str = "fuchsia.sysmem.PercentProtectedMemorySize";

const CONTIGUOUS_GUARD_PAGES_UNUSED_CAPABILITY: &str = "fuchsia.sysmem.ContiguousGuardPagesUnused";

const FIXED_CAPABILITY: &str = "Fixed";
const PERCENT_CAPABILITY: &str = "Percent";
const CAPABILITY_PREFIX: &str = "fuchsia.sysmem.";

// Directory of sysmem config (for sysmem domain config).
const SYSMEM_CONFIG_DIRECTORY: &str = "sysmem-config";
// Filename of SysmemConfig file within sysmem domain config.
const SYSMEM_CONFIG_FILENAME: &str = "config.sysmem_config_persistent_fidl";

pub(crate) struct SysmemConfig;
impl DefineSubsystemConfiguration<PlatformSysmemConfig> for SysmemConfig {
    fn define_configuration(
        context: &ConfigurationContext<'_>,
        platform_sysmem_config: &PlatformSysmemConfig,
        builder: &mut dyn ConfigurationBuilder,
    ) -> anyhow::Result<()> {
        // For format costs provided from the board, we plumb those via BoardProvidedConfig (not via
        // BoardSysmemConfig). Here, to homogenize the handling of the two layers (board and
        // platform/product), we put any board-provideed format costs into a synthesized board-level
        // PlatformSysmemConfig, before we combine the two PlatformSysmemConfig(s) together below.
        let sysmem_defaults: &BoardSysmemConfig = &context.board_config.platform.sysmem_defaults;
        let board_platform_sysmem_config = PlatformSysmemConfig {
            contiguous_memory_size: sysmem_defaults.contiguous_memory_size,
            protected_memory_size: sysmem_defaults.protected_memory_size,
            contiguous_guard_pages_unused: sysmem_defaults.contiguous_guard_pages_unused,
            format_costs: context.board_config.configuration.sysmem_format_costs.clone(),
            // Please don't use ..Default::default here - we intentionally want the build to fail
            // when we add a new field to BoardSysmemConfig so we can handle that field here (or in
            // this function).
        };

        // Combine the board's PlatformSysmemConfig (synthesized above) and platform
        // PlatformSysmemConfig(s) to a single PlatformSysmemConfig.
        let mut settings = PlatformSysmemConfig::default();
        for platform_sysmem_config in [&board_platform_sysmem_config, platform_sysmem_config] {
            // All the fields are Option<T>. If a field is still None after overrides from
            // board_config and platform config have been applied, the config capability will be
            // absent and the default value of the field will be as specified in
            // src/sysmem/drivers/sysmem/BUILD.gn.
            settings.format_costs.append(&mut platform_sysmem_config.format_costs.clone());

            // Please don't use ..Default::default() here; we want this to fail to build when new field(s)
            // are added.
            settings = PlatformSysmemConfig {
                contiguous_memory_size: platform_sysmem_config
                    .contiguous_memory_size
                    .or(settings.contiguous_memory_size),
                protected_memory_size: platform_sysmem_config
                    .protected_memory_size
                    .or(settings.protected_memory_size),
                contiguous_guard_pages_unused: platform_sysmem_config.contiguous_guard_pages_unused,
                format_costs: settings.format_costs,
            }
        }
        // The settings.format_costs list can be empty here if neither board info nor assembly
        // platform config had format_costs field set - but we always convert to a (possibly-empty)
        // vec (never None even if both inputs had None).

        if *context.feature_set_level == FeatureSetLevel::Embeddable {
            // At least for now, Embeddable --> zero contiguous or protected memory reservations,
            // ignoring PlatformSysmemConfig::contiguous_memory_size and protected_memory_size.
            builder.set_config_capability(
                FIXED_CONTIGUOUS_MEMORY_SIZE_CAPABILITY,
                Config::new(ConfigValueType::Int64, 0.into()),
            )?;
            builder.set_config_capability(
                PERCENT_CONTIGUOUS_MEMORY_SIZE_CAPABILITY,
                Config::new_void(),
            )?;
            builder.set_config_capability(
                FIXED_PROTECTED_MEMORY_SIZE_CAPABILITY,
                Config::new(ConfigValueType::Int64, 0.into()),
            )?;
            builder.set_config_capability(
                PERCENT_PROTECTED_MEMORY_SIZE_CAPABILITY,
                Config::new_void(),
            )?;
        } else {
            let mut apply_memory_size = |field_reference: &Option<MemorySize>,
                                         field_name: &str,
                                         config_capability_name_suffix: &str|
             -> anyhow::Result<()> {
                let fixed_capability =
                    format!("{CAPABILITY_PREFIX}{FIXED_CAPABILITY}{config_capability_name_suffix}");
                let percent_capability = format!(
                    "{CAPABILITY_PREFIX}{PERCENT_CAPABILITY}{config_capability_name_suffix}"
                );

                builder
                    .set_config_capability(
                        fixed_capability.as_str(),
                        if let Some(MemorySize::Fixed(fixed_memory_size)) = &field_reference {
                            Config::new(ConfigValueType::Int64, (*fixed_memory_size).into())
                        } else {
                            Config::new_void()
                        },
                    )
                    .context(fixed_capability)?;

                builder
                    .set_config_capability(
                        percent_capability.as_str(),
                        if let Some(MemorySize::Percent(percent_memory_size)) = &field_reference {
                            ensure!(
                                *percent_memory_size <= 99,
                                "{} Percent must be <= 99 - got {}",
                                field_name,
                                percent_memory_size
                            );
                            Config::new(ConfigValueType::Int32, (*percent_memory_size).into())
                        } else {
                            Config::new_void()
                        },
                    )
                    .context(percent_capability)?;

                Ok(())
            };

            // fuchsia.sysmem.FixedContiguousMemorySize or fuchsia.sysmem.PercentContiguousMemorySize
            apply_memory_size(
                &settings.contiguous_memory_size,
                CONTIGUOUS_MEMORY_SIZE_FIELD,
                CONTIGUOUS_MEMORY_SIZE_CAPABILITY_SUFFIX,
            )?;
            // fuchsia.sysmem.FixedProtectedMemorySize or fuchsia.sysmem.PercentProtectedMemorySize
            apply_memory_size(
                &settings.protected_memory_size,
                PROTECTED_MEMORY_SIZE_FIELD,
                PROTECTED_MEMORY_SIZE_CAPABILITY_SUFFIX,
            )?;
        }

        builder.set_config_capability(
            CONTIGUOUS_GUARD_PAGES_UNUSED_CAPABILITY,
            Config::new(ConfigValueType::Bool, settings.contiguous_guard_pages_unused.into()),
        )?;

        // Sysmem has a domain config with a config.sysmem_config_persistent_fidl file containing a
        // fuchsia.sysemm2.Config. This function builds the fuchsia.sysmem2.Config from an input
        // fuchsia.sysmem2.FormatCosts sysmem.format_costs_persistent_fidl file (and in future,
        // potentially other input config files). This mechanism is intended for aspects of sysmem
        // config that are too verbose/repetitive for direct inclusion in BoardSysmemConfig and/or
        // PlatformSysmemConfig.
        let format_costs_files: Vec<Utf8PathBuf> = settings.format_costs.to_vec();

        // On error, load_and_merge_pixel_format_costs_files has already attached context.
        let format_costs = load_and_merge_pixel_format_costs_files(&format_costs_files)?;
        let domain_config_fidl =
            fidl_fuchsia_sysmem2::Config { format_costs: Some(format_costs), ..Default::default() };

        let persisted_sysmem_config = fidl::persist(&domain_config_fidl)?;
        let sysmem_config_dir = builder
            .add_domain_config(PackageSetDestination::Boot(BootfsPackageDestination::SysmemConfig))
            .directory(SYSMEM_CONFIG_DIRECTORY);
        sysmem_config_dir
            .entry_from_binary_contents(SYSMEM_CONFIG_FILENAME, persisted_sysmem_config)?;

        Ok(())
    }
}

#[derive(PartialEq, Eq, Hash)]
struct FormatCostKey {
    pixel_format: fidl_fuchsia_images2::PixelFormat,
    pixel_format_modifier: fidl_fuchsia_images2::PixelFormatModifier,
    usage_none: u32,
    usage_cpu: u32,
    usage_vulkan: u32,
    usage_display: u32,
    usage_video: u32,
}

impl From<fidl_fuchsia_sysmem2::FormatCostKey> for FormatCostKey {
    fn from(value: fidl_fuchsia_sysmem2::FormatCostKey) -> Self {
        assert!(value.pixel_format.is_some());
        let usage = value.buffer_usage_bits.unwrap_or_default();
        FormatCostKey {
            pixel_format: value.pixel_format.expect("pixel_format"),
            pixel_format_modifier: value
                .pixel_format_modifier
                .unwrap_or(fidl_fuchsia_images2::PixelFormatModifier::Linear),
            usage_none: usage.none.unwrap_or(0),
            usage_cpu: usage.cpu.unwrap_or(0),
            usage_vulkan: usage.vulkan.unwrap_or(0),
            usage_display: usage.display.unwrap_or(0),
            usage_video: usage.video.unwrap_or(0),
        }
    }
}

fn load_and_merge_pixel_format_costs_files(
    sources: &Vec<Utf8PathBuf>,
) -> anyhow::Result<Vec<fidl_fuchsia_sysmem2::FormatCostEntry>> {
    if sources.is_empty() {
        return Ok(vec![]);
    }

    let mut format_costs_to_process = vec![];
    for format_costs_filepath in sources {
        let file_bytes = std::fs::read(format_costs_filepath)
            .with_context(|| format!("reading {format_costs_filepath}"))?;
        let mut format_costs = fidl::unpersist::<fidl_fuchsia_sysmem2::FormatCosts>(&file_bytes)
            .with_context(|| format!("load_and_merge_pixel_format_costs_files fidl::unpersist failed - see also fuchsia.sysmem2.FormatCosts and fidl::persist - file: {format_costs_filepath}"))?;
        let mut format_costs_to_check = format_costs.format_costs.take().unwrap_or_else(Vec::new);
        for format_cost in &format_costs_to_check {
            let key = format_cost
                .key
                .as_ref()
                .ok_or_else(|| anyhow!("missing key in {}", format_costs_filepath))?;
            key.pixel_format
                .as_ref()
                .ok_or_else(|| anyhow!("missing pixel_format in {}", format_costs_filepath))?;
            format_cost
                .cost
                .as_ref()
                .ok_or_else(|| anyhow!("missing cost in {}", format_costs_filepath))?;
        }
        format_costs_to_process.append(&mut format_costs_to_check);
    }

    let mut format_cost_keys_hashmap: HashSet<FormatCostKey> = HashSet::new();
    // We process this way to preserve ordering of retained entries (in contrast to conversion
    // to/from a HashMap which would not). Later retained entries take priority over earlier
    // retained entries when there are ties in sysmem where a buffer collection's usage has the same
    // number of usage bits in common with two different format cost entries.
    let format_costs: Vec<fidl_fuchsia_sysmem2::FormatCostEntry> = format_costs_to_process
        .drain(..)
        .rev()
        .filter(|format_cost_entry| {
            let format_cost_key =
                FormatCostKey::from(format_cost_entry.key.as_ref().expect("key").clone());
            if format_cost_keys_hashmap.contains(&format_cost_key) {
                return false;
            }
            format_cost_keys_hashmap.insert(format_cost_key);
            true
        })
        // The .collect().drain(..) is here so that the filter lambda gets called in the reversed
        // order which is what we want.
        .collect::<Vec<_>>()
        .drain(..)
        .rev()
        .collect();

    Ok(format_costs)
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::subsystems::ConfigurationBuilderImpl;
    use crate::{DomainConfig, DomainConfigDirectory, FileOrContents};
    use assembly_config_schema::{BoardConfig, BoardProvidedConfig};
    use serde_json::{Number, Value};

    #[test]
    fn test_contiguous_memory_size() {
        let context = ConfigurationContext {
            feature_set_level: &FeatureSetLevel::Standard,
            build_type: &BuildType::Eng,
            board_config: &Default::default(),
            gendir: Default::default(),
            resource_dir: Default::default(),
            developer_only_options: Default::default(),
        };
        let platform_sysmem_config = PlatformSysmemConfig {
            contiguous_memory_size: Some(MemorySize::Fixed(123)),
            protected_memory_size: Some(MemorySize::Fixed(456)),
            ..Default::default()
        };
        let mut builder: ConfigurationBuilderImpl = Default::default();
        let result =
            SysmemConfig::define_configuration(&context, &platform_sysmem_config, &mut builder);
        assert!(result.is_ok());
        let config = builder.build();
        assert_eq!(
            config.configuration_capabilities[FIXED_CONTIGUOUS_MEMORY_SIZE_CAPABILITY].value(),
            Value::Number(Number::from(123))
        );
        assert_eq!(
            config.configuration_capabilities[PERCENT_CONTIGUOUS_MEMORY_SIZE_CAPABILITY].value(),
            Value::Null
        );
        assert_eq!(
            config.configuration_capabilities[FIXED_PROTECTED_MEMORY_SIZE_CAPABILITY].value(),
            Value::Number(Number::from(456))
        );
        assert_eq!(
            config.configuration_capabilities[PERCENT_PROTECTED_MEMORY_SIZE_CAPABILITY].value(),
            Value::Null
        );
        assert_eq!(
            config.configuration_capabilities[CONTIGUOUS_GUARD_PAGES_UNUSED_CAPABILITY].value(),
            Value::Bool(false)
        );
    }

    #[test]
    fn test_contiguous_memory_size_percent() {
        let context = ConfigurationContext {
            feature_set_level: &FeatureSetLevel::Standard,
            build_type: &BuildType::Eng,
            board_config: &Default::default(),
            gendir: Default::default(),
            resource_dir: Default::default(),
            developer_only_options: Default::default(),
        };
        let platform_sysmem_config = PlatformSysmemConfig {
            contiguous_memory_size: Some(MemorySize::Percent(5)),
            protected_memory_size: Some(MemorySize::Percent(12)),
            ..Default::default()
        };
        let mut builder: ConfigurationBuilderImpl = Default::default();
        let result =
            SysmemConfig::define_configuration(&context, &platform_sysmem_config, &mut builder);
        assert!(result.is_ok());
        let config = builder.build();
        assert_eq!(
            config.configuration_capabilities[FIXED_CONTIGUOUS_MEMORY_SIZE_CAPABILITY].value(),
            Value::Null
        );
        assert_eq!(
            config.configuration_capabilities[PERCENT_CONTIGUOUS_MEMORY_SIZE_CAPABILITY].value(),
            Value::Number(Number::from(5))
        );
        assert_eq!(
            config.configuration_capabilities[FIXED_PROTECTED_MEMORY_SIZE_CAPABILITY].value(),
            Value::Null
        );
        assert_eq!(
            config.configuration_capabilities[PERCENT_PROTECTED_MEMORY_SIZE_CAPABILITY].value(),
            Value::Number(Number::from(12))
        );
        assert_eq!(
            config.configuration_capabilities[CONTIGUOUS_GUARD_PAGES_UNUSED_CAPABILITY].value(),
            Value::Bool(false)
        );
    }

    #[test]
    fn test_contiguous_memory_size_percentage_too_high() {
        let context = ConfigurationContext {
            feature_set_level: &FeatureSetLevel::Standard,
            build_type: &BuildType::Eng,
            board_config: &Default::default(),
            gendir: Default::default(),
            resource_dir: Default::default(),
            developer_only_options: Default::default(),
        };
        let platform_sysmem_config = PlatformSysmemConfig {
            contiguous_memory_size: Some(MemorySize::Percent(100)),
            ..Default::default()
        };
        let mut builder: ConfigurationBuilderImpl = Default::default();
        let result =
            SysmemConfig::define_configuration(&context, &platform_sysmem_config, &mut builder);

        let error_message = format!("{:#}", result.unwrap_err());
        assert!(
            error_message.contains("contiguous_memory_size"),
            "Faulty message `{error_message}`"
        );
        assert!(error_message.contains("got 100"));
    }

    #[test]
    fn test_protected_memory_size_percentage_too_high() {
        let context = ConfigurationContext {
            feature_set_level: &FeatureSetLevel::Standard,
            build_type: &BuildType::Eng,
            board_config: &Default::default(),
            gendir: Default::default(),
            resource_dir: Default::default(),
            developer_only_options: Default::default(),
        };
        let platform_sysmem_config = PlatformSysmemConfig {
            protected_memory_size: Some(MemorySize::Percent(100)),
            ..Default::default()
        };
        let mut builder: ConfigurationBuilderImpl = Default::default();
        let result =
            SysmemConfig::define_configuration(&context, &platform_sysmem_config, &mut builder);

        let error_message = format!("{:#}", result.unwrap_err());
        assert!(
            error_message.contains("protected_memory_size"),
            "Faulty message `{error_message}`"
        );
        assert!(error_message.contains("got 100"));
    }

    #[test]
    fn test_board_defaults_no_platform_overrides() {
        let context = ConfigurationContext {
            feature_set_level: &FeatureSetLevel::Standard,
            build_type: &BuildType::Eng,
            board_config: &BoardConfig {
                platform: assembly_config_schema::board_config::PlatformSettings {
                    sysmem_defaults: BoardSysmemConfig {
                        contiguous_memory_size: Some(MemorySize::Fixed(123)),
                        protected_memory_size: Some(MemorySize::Fixed(456)),
                        contiguous_guard_pages_unused: true,
                    },
                    ..Default::default()
                },
                ..Default::default()
            },
            gendir: Default::default(),
            resource_dir: Default::default(),
            developer_only_options: Default::default(),
        };
        let platform_sysmem_config = PlatformSysmemConfig {
            contiguous_memory_size: None,
            protected_memory_size: None,
            contiguous_guard_pages_unused: true,
            format_costs: Default::default(),
        };
        let mut builder: ConfigurationBuilderImpl = Default::default();
        let result =
            SysmemConfig::define_configuration(&context, &platform_sysmem_config, &mut builder);
        assert!(result.is_ok());
        let config = builder.build();
        assert_eq!(
            config.configuration_capabilities[FIXED_CONTIGUOUS_MEMORY_SIZE_CAPABILITY].value(),
            Value::Number(Number::from(123))
        );
        assert_eq!(
            config.configuration_capabilities[PERCENT_CONTIGUOUS_MEMORY_SIZE_CAPABILITY].value(),
            Value::Null
        );
        assert_eq!(
            config.configuration_capabilities[FIXED_PROTECTED_MEMORY_SIZE_CAPABILITY].value(),
            Value::Number(Number::from(456))
        );
        assert_eq!(
            config.configuration_capabilities[PERCENT_PROTECTED_MEMORY_SIZE_CAPABILITY].value(),
            Value::Null
        );
        assert_eq!(
            config.configuration_capabilities[CONTIGUOUS_GUARD_PAGES_UNUSED_CAPABILITY].value(),
            Value::Bool(true)
        );
    }

    #[test]
    fn test_board_defaults_with_platform_overrides() {
        let context = ConfigurationContext {
            feature_set_level: &FeatureSetLevel::Standard,
            build_type: &BuildType::Eng,
            board_config: &BoardConfig {
                platform: assembly_config_schema::board_config::PlatformSettings {
                    sysmem_defaults: BoardSysmemConfig {
                        contiguous_memory_size: Some(MemorySize::Fixed(123)),
                        protected_memory_size: Some(MemorySize::Fixed(456)),
                        contiguous_guard_pages_unused: true,
                    },
                    ..Default::default()
                },
                ..Default::default()
            },
            gendir: Default::default(),
            resource_dir: Default::default(),
            developer_only_options: Default::default(),
        };
        let platform_sysmem_config = PlatformSysmemConfig {
            contiguous_memory_size: Some(MemorySize::Percent(12)),
            protected_memory_size: Some(MemorySize::Percent(34)),
            contiguous_guard_pages_unused: false,
            format_costs: Default::default(),
        };
        let mut builder: ConfigurationBuilderImpl = Default::default();
        let result =
            SysmemConfig::define_configuration(&context, &platform_sysmem_config, &mut builder);
        assert!(result.is_ok());
        let config = builder.build();
        assert_eq!(
            config.configuration_capabilities[FIXED_CONTIGUOUS_MEMORY_SIZE_CAPABILITY].value(),
            Value::Null
        );
        assert_eq!(
            config.configuration_capabilities[PERCENT_CONTIGUOUS_MEMORY_SIZE_CAPABILITY].value(),
            Value::Number(Number::from(12))
        );
        assert_eq!(
            config.configuration_capabilities[FIXED_PROTECTED_MEMORY_SIZE_CAPABILITY].value(),
            Value::Null
        );
        assert_eq!(
            config.configuration_capabilities[PERCENT_PROTECTED_MEMORY_SIZE_CAPABILITY].value(),
            Value::Number(Number::from(34))
        );
        assert_eq!(
            config.configuration_capabilities[CONTIGUOUS_GUARD_PAGES_UNUSED_CAPABILITY].value(),
            Value::Bool(false)
        );
    }

    #[test]
    fn test_no_overrides() {
        let context = ConfigurationContext {
            feature_set_level: &FeatureSetLevel::Standard,
            build_type: &BuildType::Eng,
            board_config: &BoardConfig {
                platform: assembly_config_schema::board_config::PlatformSettings {
                    sysmem_defaults: BoardSysmemConfig {
                        contiguous_memory_size: None,
                        protected_memory_size: None,
                        contiguous_guard_pages_unused: false,
                    },
                    ..Default::default()
                },
                ..Default::default()
            },
            gendir: Default::default(),
            resource_dir: Default::default(),
            developer_only_options: Default::default(),
        };
        let platform_sysmem_config = PlatformSysmemConfig {
            contiguous_memory_size: None,
            protected_memory_size: None,
            contiguous_guard_pages_unused: false,
            format_costs: Default::default(),
        };
        let mut builder: ConfigurationBuilderImpl = Default::default();
        let result =
            SysmemConfig::define_configuration(&context, &platform_sysmem_config, &mut builder);
        assert!(result.is_ok());
        let config = builder.build();
        assert_eq!(
            config.configuration_capabilities[FIXED_CONTIGUOUS_MEMORY_SIZE_CAPABILITY].value(),
            Value::Null
        );
        assert_eq!(
            config.configuration_capabilities[PERCENT_CONTIGUOUS_MEMORY_SIZE_CAPABILITY].value(),
            Value::Null
        );
        assert_eq!(
            config.configuration_capabilities[FIXED_PROTECTED_MEMORY_SIZE_CAPABILITY].value(),
            Value::Null
        );
        assert_eq!(
            config.configuration_capabilities[PERCENT_PROTECTED_MEMORY_SIZE_CAPABILITY].value(),
            Value::Null
        );
        assert_eq!(
            config.configuration_capabilities[CONTIGUOUS_GUARD_PAGES_UNUSED_CAPABILITY].value(),
            Value::Bool(false)
        );
    }

    #[test]
    fn test_format_costs_defaults_with_platform_overrides() {
        const BOARD_FORMAT_COSTS_FILENAME: &str = "./board.format_costs_persistent_fidl";
        const PLATFORM_FORMAT_COSTS_FILENAME: &str = "./platform.format_costs_persistent_fidl";

        let board_format_costs = fidl_fuchsia_sysmem2::FormatCosts {
            format_costs: Some(vec![
                // Where this entry specifies None, defaults are applied. This entry will be
                // overridden by the platform R8G8B8A8 entry which explicitly specifies the fields.
                fidl_fuchsia_sysmem2::FormatCostEntry {
                    key: Some(fidl_fuchsia_sysmem2::FormatCostKey {
                        pixel_format: Some(fidl_fuchsia_images2::PixelFormat::R8G8B8A8),
                        // default is Linear
                        pixel_format_modifier: None,
                        // default is all 0
                        buffer_usage_bits: None,
                        ..Default::default()
                    }),
                    cost: Some(100.0),
                    ..Default::default()
                },
                // This YV12 entry is not overridden by the platform YV12 entry because the platform
                // YV12 entry requires the CPU_USAGE_READ_OFTEN bit while this entry doesn't.
                fidl_fuchsia_sysmem2::FormatCostEntry {
                    key: Some(fidl_fuchsia_sysmem2::FormatCostKey {
                        pixel_format: Some(fidl_fuchsia_images2::PixelFormat::Yv12),
                        pixel_format_modifier: None,
                        buffer_usage_bits: None,
                        ..Default::default()
                    }),
                    cost: Some(300.0),
                    ..Default::default()
                },
                // This B8G8R8A8 entry is not override by the platform B8G8R8A8 entry because this
                // entry has a non-Linear PixelFormatModifier while the platform B8G8R8A8 entry
                // defaults to Linear.
                fidl_fuchsia_sysmem2::FormatCostEntry {
                    key: Some(fidl_fuchsia_sysmem2::FormatCostKey {
                        pixel_format: Some(fidl_fuchsia_images2::PixelFormat::B8G8R8A8),
                        pixel_format_modifier: Some(
                            fidl_fuchsia_images2::PixelFormatModifier::IntelI915XTiled,
                        ),
                        buffer_usage_bits: None,
                        ..Default::default()
                    }),
                    cost: Some(500.0),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };
        let platform_format_costs = fidl_fuchsia_sysmem2::FormatCosts {
            format_costs: Some(vec![
                // This entry will take the place of board R8G8B8A8 entry due to matching key after
                // defaults are applied to board R8G8B8A8 entry.
                fidl_fuchsia_sysmem2::FormatCostEntry {
                    // Same overall key as above, due to defaults applied to above entry.
                    key: Some(fidl_fuchsia_sysmem2::FormatCostKey {
                        pixel_format: Some(fidl_fuchsia_images2::PixelFormat::R8G8B8A8),
                        // Linear here matches None above since Linear is the default.
                        pixel_format_modifier: Some(
                            fidl_fuchsia_images2::PixelFormatModifier::Linear,
                        ),
                        // Empty BufferUsage here effectively matches None above.
                        buffer_usage_bits: Some(fidl_fuchsia_sysmem2::BufferUsage {
                            none: Some(0),
                            cpu: Some(0),
                            vulkan: Some(0),
                            display: Some(0),
                            video: Some(0),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                    cost: Some(200.0),
                    ..Default::default()
                },
                // This platform YV12 entry does not override the board YV12 entry because this
                // entry requires CPU_USAGE_READ_OFTEN while the board entry doesn't.
                fidl_fuchsia_sysmem2::FormatCostEntry {
                    key: Some(fidl_fuchsia_sysmem2::FormatCostKey {
                        pixel_format: Some(fidl_fuchsia_images2::PixelFormat::Yv12),
                        pixel_format_modifier: None,
                        buffer_usage_bits: Some(fidl_fuchsia_sysmem2::BufferUsage {
                            cpu: Some(fidl_fuchsia_sysmem2::CPU_USAGE_READ_OFTEN),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                    cost: Some(400.0),
                    ..Default::default()
                },
                // This B8G8R8A8 entry does not override the board B8G8R8A8 entry because this
                // entry defaults to PixelFormatModifier::Linear while the board entry has a
                // non-Linear PixelFormatModifier.
                fidl_fuchsia_sysmem2::FormatCostEntry {
                    key: Some(fidl_fuchsia_sysmem2::FormatCostKey {
                        pixel_format: Some(fidl_fuchsia_images2::PixelFormat::B8G8R8A8),
                        pixel_format_modifier: None,
                        buffer_usage_bits: None,
                        ..Default::default()
                    }),
                    cost: Some(600.0),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };

        let board_format_costs_vec: Vec<u8> =
            fidl::persist(&board_format_costs).expect("persist board_format_costs");
        std::fs::write(BOARD_FORMAT_COSTS_FILENAME, board_format_costs_vec)
            .expect("write board_format_costs_vec");

        let platform_format_costs_vec: Vec<u8> =
            fidl::persist(&platform_format_costs).expect("persist platform_format_costs");
        std::fs::write(PLATFORM_FORMAT_COSTS_FILENAME, platform_format_costs_vec)
            .expect("write platform_format_costs_vec");

        let context = ConfigurationContext {
            feature_set_level: &FeatureSetLevel::Standard,
            build_type: &BuildType::Eng,
            board_config: &BoardConfig {
                configuration: BoardProvidedConfig {
                    sysmem_format_costs: vec![BOARD_FORMAT_COSTS_FILENAME.into()],
                    ..Default::default()
                },
                ..Default::default()
            },
            gendir: Default::default(),
            resource_dir: Default::default(),
            developer_only_options: Default::default(),
        };

        let platform_sysmem_config = PlatformSysmemConfig {
            format_costs: vec![PLATFORM_FORMAT_COSTS_FILENAME.into()],
            ..Default::default()
        };

        let mut builder: ConfigurationBuilderImpl = Default::default();
        let result =
            SysmemConfig::define_configuration(&context, &platform_sysmem_config, &mut builder);
        assert!(result.is_ok());
        let config = builder.build();

        let maybe_domain_config: Option<&DomainConfig> = config
            .domain_configs
            .entries
            .get(&PackageSetDestination::Boot(BootfsPackageDestination::SysmemConfig));
        assert!(maybe_domain_config.is_some());
        let domain_config: &DomainConfig = maybe_domain_config.expect("domain_config");
        assert_eq!(
            domain_config.name,
            PackageSetDestination::Boot(BootfsPackageDestination::SysmemConfig)
        );
        let maybe_domain_config_directory: Option<&DomainConfigDirectory> =
            domain_config.directories.entries.get("sysmem-config");
        assert!(maybe_domain_config_directory.is_some());
        let domain_config_directory: &DomainConfigDirectory =
            maybe_domain_config_directory.expect("domain_config_directory");
        let maybe_file_or_contents: Option<&FileOrContents> =
            domain_config_directory.entries.get(SYSMEM_CONFIG_FILENAME);
        assert!(maybe_file_or_contents.is_some());
        let file_or_contents: &FileOrContents = maybe_file_or_contents.expect("file_or_contents");
        let binary_contents: &Vec<u8> = match &file_or_contents {
            FileOrContents::BinaryContents(binary_contents) => binary_contents,
            _ => panic!("FileOrContents::BinaryContents expected"),
        };
        let sysmem_config = fidl::unpersist::<fidl_fuchsia_sysmem2::Config>(binary_contents)
            .expect("unpersist sysmem_config");

        let expected_format_costs = vec![
            // from board
            //
            // This YV12 entry is not overridden by the platform YV12 entry because the platform
            // YV12 entry requires the CPU_USAGE_READ_OFTEN bit while this entry doesn't.
            fidl_fuchsia_sysmem2::FormatCostEntry {
                key: Some(fidl_fuchsia_sysmem2::FormatCostKey {
                    pixel_format: Some(fidl_fuchsia_images2::PixelFormat::Yv12),
                    pixel_format_modifier: None,
                    buffer_usage_bits: None,
                    ..Default::default()
                }),
                cost: Some(300.0),
                ..Default::default()
            },
            // from board
            //
            // This B8G8R8A8 entry is not override by the platform B8G8R8A8 entry because this
            // entry has a non-Linear PixelFormatModifier while the platform B8G8R8A8 entry
            // defaults to Linear.
            fidl_fuchsia_sysmem2::FormatCostEntry {
                key: Some(fidl_fuchsia_sysmem2::FormatCostKey {
                    pixel_format: Some(fidl_fuchsia_images2::PixelFormat::B8G8R8A8),
                    pixel_format_modifier: Some(
                        fidl_fuchsia_images2::PixelFormatModifier::IntelI915XTiled,
                    ),
                    buffer_usage_bits: None,
                    ..Default::default()
                }),
                cost: Some(500.0),
                ..Default::default()
            },
            // from platform
            //
            // This entry will take the place of board R8G8B8A8 entry due to matching key after
            // defaults are applied to board R8G8B8A8 entry.
            fidl_fuchsia_sysmem2::FormatCostEntry {
                // Same overall key as above, due to defaults applied to above entry.
                key: Some(fidl_fuchsia_sysmem2::FormatCostKey {
                    pixel_format: Some(fidl_fuchsia_images2::PixelFormat::R8G8B8A8),
                    // Linear here matches None above since Linear is the default.
                    pixel_format_modifier: Some(fidl_fuchsia_images2::PixelFormatModifier::Linear),
                    // Empty BufferUsage here effectively matches None above.
                    buffer_usage_bits: Some(fidl_fuchsia_sysmem2::BufferUsage {
                        none: Some(0),
                        cpu: Some(0),
                        vulkan: Some(0),
                        display: Some(0),
                        video: Some(0),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                cost: Some(200.0),
                ..Default::default()
            },
            // from platform
            //
            // This platform YV12 entry does not override the board YV12 entry because this
            // entry requires CPU_USAGE_READ_OFTEN while the board entry doesn't.
            fidl_fuchsia_sysmem2::FormatCostEntry {
                key: Some(fidl_fuchsia_sysmem2::FormatCostKey {
                    pixel_format: Some(fidl_fuchsia_images2::PixelFormat::Yv12),
                    pixel_format_modifier: None,
                    buffer_usage_bits: Some(fidl_fuchsia_sysmem2::BufferUsage {
                        cpu: Some(fidl_fuchsia_sysmem2::CPU_USAGE_READ_OFTEN),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                cost: Some(400.0),
                ..Default::default()
            },
            // from platform
            //
            // This B8G8R8A8 entry does not override the board B8G8R8A8 entry because this
            // entry defaults to PixelFormatModifier::Linear while the board entry has a
            // non-Linear PixelFormatModifier.
            fidl_fuchsia_sysmem2::FormatCostEntry {
                key: Some(fidl_fuchsia_sysmem2::FormatCostKey {
                    pixel_format: Some(fidl_fuchsia_images2::PixelFormat::B8G8R8A8),
                    pixel_format_modifier: None,
                    buffer_usage_bits: None,
                    ..Default::default()
                }),
                cost: Some(600.0),
                ..Default::default()
            },
        ];

        let actual_format_costs = sysmem_config.format_costs.expect("format_costs");

        println!("expected:\n{:#?}\n", expected_format_costs);
        println!("actual:\n{:#?}\n", actual_format_costs);

        assert_eq!(expected_format_costs, actual_format_costs);
    }
}
