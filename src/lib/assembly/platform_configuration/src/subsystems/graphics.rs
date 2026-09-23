// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::subsystems::prelude::*;
use assembly_config_capabilities::{Config, ConfigNestedValueType, ConfigValueType};
use assembly_config_schema::board_config::Architecture;
use assembly_config_schema::platform_settings::graphics_config::{
    FakeDisplayConfig, GraphicsConfig,
};
use assembly_config_schema::platform_settings::ui_config::PlatformUiConfig;
use assembly_constants::BoardFeature;

pub(crate) struct GraphicsSubsystemConfig;
impl DefineSubsystemConfiguration<(&GraphicsConfig, &PlatformUiConfig)>
    for GraphicsSubsystemConfig
{
    fn define_configuration(
        context: &ConfigurationContext<'_>,
        config: &(&GraphicsConfig, &PlatformUiConfig),
        builder: &mut dyn ConfigurationBuilder,
    ) -> anyhow::Result<()> {
        let (graphics_config, ui_config) = *config;
        let virtcon_config = &graphics_config.virtual_console;

        let enable_virtual_console =
            match (context.build_type, context.feature_set_level, virtcon_config.enable) {
                // Use the value if one was specified.
                (_, _, Some(enable_virtual_console)) => enable_virtual_console,
                // If unspecified, virtcon is disabled if it's a user build-type
                (assembly_config_schema::BuildType::User, _, _) => false,
                // If neither of those, disable if we're targeting embeddable as well.
                (_, FeatureSetLevel::Embeddable, _) => false,
                // Otherwise, enable virtcon.
                (_, _, _) => true,
            };

        if enable_virtual_console {
            builder.platform_bundle("virtcon")?;
        }

        if *context.feature_set_level == FeatureSetLevel::Standard
            && context.board_config.provides_feature(BoardFeature::VulkanGpu)
        {
            builder.platform_bundle("vulkan_loader")?;

            // LINT.IfChange(vulkan_loader_lavapipe)
            let (default_magma, default_goldfish, default_lavapipe) =
                match (&context.board_config.arch, context.build_type) {
                    (Architecture::X64, BuildType::Eng) => {
                        // Magma is unsupported on x64, but goldfish and lavapipe
                        // are permitted on eng builds only.
                        (false, true, true)
                    }
                    (Architecture::X64, _) => {
                        // In non-eng variants, we do not allow lavapipe.
                        (false, true, false)
                    }
                    _ => {
                        // We support magma on other architectures, so we prefer it.
                        (true, true, false)
                    }
                };
            // LINT.ThenChange(//src/lib/assembly/platform_configuration/src/subsystems/component.rs:vulkan_loader_lavapipe)

            let allow_magma = graphics_config.vulkan_icd.allow_magma.unwrap_or(default_magma);
            let allow_goldfish =
                graphics_config.vulkan_icd.allow_goldfish.unwrap_or(default_goldfish);
            let allow_lavapipe =
                graphics_config.vulkan_icd.allow_lavapipe.unwrap_or(default_lavapipe);

            // TODO(https://fxbug.dev/541271630): Restrict lavapipe to eng build types.
            if allow_lavapipe {
                context.ensure_build_type(
                    &[BuildType::Eng, BuildType::UserDebug],
                    "vulkan_icd lavapipe",
                )?;
                builder.platform_bundle("lavapipe_pkg")?;
                builder.core_shard(&context.get_resource("lavapipe.core_shard.cml"));
            } else {
                builder.core_shard(&context.get_resource("lavapipe-disabled.core_shard.cml"));
            }

            builder
                .package("vulkan_loader")
                .component("meta/vulkan_loader.cm")?
                .field("allow_magma_icds", allow_magma)?
                .field("allow_goldfish_icd", allow_goldfish)?
                .field("allow_lavapipe_icd", allow_lavapipe)?
                .field(
                    "lavapipe_icd_url",
                    "fuchsia-pkg://fuchsia.com/libvulkan_lavapipe#meta/vulkan.cm",
                )?;
        }

        match &graphics_config.fake_display {
            FakeDisplayConfig::Values(values) => {
                if !ui_config.enabled {
                    anyhow::bail!(
                        "graphics.fake_display cannot be configured when ui.enabled is false"
                    );
                }
                builder.platform_bundle("fake_display_stack_host")?;
                let mut component = builder
                    .package("fake-display-stack-host")
                    .component("meta/fake-display-stack-host.cm")?;
                component.field("active_width_px", values.width)?;
                component.field("active_height_px", values.height)?;
            }
            FakeDisplayConfig::OnWithDefaultValues => {
                if !ui_config.enabled {
                    anyhow::bail!(
                        "graphics.fake_display cannot be configured when ui.enabled is false"
                    );
                }
                builder.platform_bundle("fake_display_stack_host")?;
            }

            // TODO(https://fxbug.dev/543145106): Remove BoardFeature::FakeDisplay and force product configs
            // to set the fake display settings.
            FakeDisplayConfig::Off => {
                if context.board_config.provides_feature(BoardFeature::FakeDisplay)
                    && ui_config.enabled
                {
                    builder.platform_bundle("fake_display_stack_host")?;
                }
            }
        }

        builder.set_config_capability("fuchsia.virtcon.BufferCount", Config::new_void())?;

        if let Some(scheme) = &virtcon_config.color_scheme {
            builder.set_config_capability(
                "fuchsia.virtcon.ColorScheme",
                Config::new(ConfigValueType::String { max_size: 20 }, scheme.to_string().into()),
            )?;
        } else {
            builder.set_config_capability("fuchsia.virtcon.ColorScheme", Config::new_void())?;
        }

        builder.set_config_capability(
            "fuchsia.virtcon.Disable",
            Config::new(ConfigValueType::Bool, (!enable_virtual_console).into()),
        )?;

        if let Some(rotation) = context.board_config.platform.graphics.display.rotation {
            builder.set_config_capability(
                "fuchsia.virtcon.DisplayRotation",
                Config::new(ConfigValueType::Uint32, rotation.into()),
            )?;
        } else {
            builder.set_config_capability("fuchsia.virtcon.DisplayRotation", Config::new_void())?;
        }

        if !virtcon_config.dpi.is_empty() {
            builder.set_config_capability(
                "fuchsia.virtcon.DotsPerInch",
                Config::new(
                    ConfigValueType::Vector {
                        nested_type: ConfigNestedValueType::Uint32,
                        max_count: 10,
                    },
                    virtcon_config.dpi.clone().into(),
                ),
            )?;
        } else {
            builder.set_config_capability("fuchsia.virtcon.DotsPerInch", Config::new_void())?;
        }

        builder.set_config_capability("fuchsia.virtcon.FontSize", Config::new_void())?;
        builder.set_config_capability("fuchsia.virtcon.KeepLogVisible", Config::new_void())?;
        builder.set_config_capability("fuchsia.virtcon.ShowLogo", Config::new_bool(true))?;
        if let Some(keymap) = &virtcon_config.keymap {
            builder.set_config_capability(
                "fuchsia.virtcon.KeyMap",
                Config::new(ConfigValueType::String { max_size: 10 }, keymap.as_str().into()),
            )?;
        } else {
            builder.set_config_capability("fuchsia.virtcon.KeyMap", Config::new_void())?;
        }

        builder.set_config_capability("fuchsia.virtcon.KeyRepeat", Config::new_void())?;

        let rounded_corners = context.board_config.platform.graphics.display.rounded_corners;
        builder.set_config_capability(
            "fuchsia.virtcon.RoundedCorners",
            Config::new(ConfigValueType::Bool, rounded_corners.into()),
        )?;

        builder.set_config_capability("fuchsia.virtcon.ScrollbackRows", Config::new_void())?;

        // TODO(https://fxbug.dev/540969321): Fallback default sizes match historical 160mm x 90mm.
        let fallback_horizontal_size_mm = context
            .board_config
            .platform
            .graphics
            .display
            .fallback_horizontal_size_mm
            .unwrap_or(160);
        builder.set_config_capability(
            "fuchsia.display.FallbackHorizontalSizeMm",
            Config::new(ConfigValueType::Uint32, fallback_horizontal_size_mm.into()),
        )?;

        // TODO(https://fxbug.dev/540969321): Fallback default sizes match historical 160mm x 90mm.
        let fallback_vertical_size_mm =
            context.board_config.platform.graphics.display.fallback_vertical_size_mm.unwrap_or(90);
        builder.set_config_capability(
            "fuchsia.display.FallbackVerticalSizeMm",
            Config::new(ConfigValueType::Uint32, fallback_vertical_size_mm.into()),
        )?;

        match context.feature_set_level {
            FeatureSetLevel::Bootstrap | FeatureSetLevel::Embeddable => {
                builder.platform_bundle("display_drivers_boot")?;
            }
            FeatureSetLevel::Utility | FeatureSetLevel::Standard => {
                builder.platform_bundle("display_drivers_base")?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::ConfigurationBuilderImpl;
    use assembly_config_schema::BoardConfig;
    use assembly_config_schema::platform_settings::graphics_config::{VirtconConfig, VulkanIcd};

    #[test]
    fn test_user_default() {
        let context = ConfigurationContext {
            feature_set_level: &FeatureSetLevel::Standard,
            build_type: &BuildType::User,
            ..ConfigurationContext::default_for_tests()
        };
        let config = GraphicsConfig { ..Default::default() };
        let mut builder = ConfigurationBuilderImpl::default();
        GraphicsSubsystemConfig::define_configuration(
            &context,
            &(&config, &PlatformUiConfig { enabled: true, ..Default::default() }),
            &mut builder,
        )
        .unwrap();
        let config = builder.build();
        assert_eq!(config.bundles, ["display_drivers_base".to_string()].into());
    }

    #[test]
    fn test_user_virtcon_disabled() {
        let context = ConfigurationContext {
            feature_set_level: &FeatureSetLevel::Standard,
            build_type: &BuildType::User,
            ..ConfigurationContext::default_for_tests()
        };
        let config = GraphicsConfig {
            virtual_console: VirtconConfig { enable: Some(false), ..Default::default() },
            ..Default::default()
        };
        let mut builder = ConfigurationBuilderImpl::default();
        GraphicsSubsystemConfig::define_configuration(
            &context,
            &(&config, &PlatformUiConfig { enabled: true, ..Default::default() }),
            &mut builder,
        )
        .unwrap();
        let config = builder.build();
        assert_eq!(config.bundles, ["display_drivers_base".to_string()].into());
    }

    #[test]
    fn test_user_virtcon_enabled() {
        let context = ConfigurationContext {
            feature_set_level: &FeatureSetLevel::Standard,
            build_type: &BuildType::User,
            ..ConfigurationContext::default_for_tests()
        };
        let config = GraphicsConfig {
            virtual_console: VirtconConfig { enable: Some(true), ..Default::default() },
            ..Default::default()
        };
        let mut builder = ConfigurationBuilderImpl::default();
        GraphicsSubsystemConfig::define_configuration(
            &context,
            &(&config, &PlatformUiConfig { enabled: true, ..Default::default() }),
            &mut builder,
        )
        .unwrap();
        let config = builder.build();
        assert_eq!(
            config.bundles,
            ["display_drivers_base".to_string(), "virtcon".to_string()].into()
        );
    }

    #[test]
    fn test_fake_display_ui_enabled() {
        let board_config = BoardConfig {
            provided_features: vec!["fuchsia::fake_display".to_string()],
            ..Default::default()
        };
        let context = ConfigurationContext {
            board_config: &board_config,
            ..ConfigurationContext::default_for_tests()
        };
        let config = GraphicsConfig { ..Default::default() };
        let mut builder = ConfigurationBuilderImpl::default();
        GraphicsSubsystemConfig::define_configuration(
            &context,
            &(&config, &PlatformUiConfig { enabled: true, ..Default::default() }),
            &mut builder,
        )
        .unwrap();
        let config = builder.build();
        assert_eq!(
            config.bundles,
            ["display_drivers_base".to_string(), "fake_display_stack_host".to_string()].into()
        );
    }

    #[test]
    fn test_fake_display_ui_disabled() {
        let board_config = BoardConfig {
            provided_features: vec!["fuchsia::fake_display".to_string()],
            ..Default::default()
        };
        let context = ConfigurationContext {
            board_config: &board_config,
            ..ConfigurationContext::default_for_tests()
        };
        let config = GraphicsConfig { ..Default::default() };
        let mut builder = ConfigurationBuilderImpl::default();
        GraphicsSubsystemConfig::define_configuration(
            &context,
            &(&config, &PlatformUiConfig { enabled: false, ..Default::default() }),
            &mut builder,
        )
        .unwrap();
        let config = builder.build();
        assert_eq!(config.bundles, ["display_drivers_base".to_string()].into());
    }

    #[test]
    fn test_vulkan_loader_default() {
        let board_config = BoardConfig {
            provided_features: vec![BoardFeature::VulkanGpu.as_ref().to_string()],
            ..Default::default()
        };
        let context = ConfigurationContext {
            feature_set_level: &FeatureSetLevel::Standard,
            build_type: &BuildType::User,
            board_config: &board_config,
            ..ConfigurationContext::default_for_tests()
        };
        let config = GraphicsConfig { ..Default::default() };
        let mut builder = ConfigurationBuilderImpl::default();
        GraphicsSubsystemConfig::define_configuration(
            &context,
            &(&config, &PlatformUiConfig::default()),
            &mut builder,
        )
        .unwrap();
        let config = builder.build();
        assert_eq!(
            config.bundles,
            ["display_drivers_base".to_string(), "vulkan_loader".to_string()].into()
        );
    }

    #[test]
    fn test_vulkan_loader_lavapipe() {
        let board_config = BoardConfig {
            provided_features: vec![BoardFeature::VulkanGpu.as_ref().to_string()],
            ..Default::default()
        };
        let context = ConfigurationContext {
            feature_set_level: &FeatureSetLevel::Standard,
            build_type: &BuildType::UserDebug,
            board_config: &board_config,
            ..ConfigurationContext::default_for_tests()
        };
        let config = GraphicsConfig {
            vulkan_icd: VulkanIcd {
                allow_magma: Some(false),
                allow_goldfish: Some(false),
                allow_lavapipe: Some(true),
            },
            virtual_console: VirtconConfig { enable: Some(false), ..Default::default() },
            ..Default::default()
        };
        let mut builder = ConfigurationBuilderImpl::default();
        GraphicsSubsystemConfig::define_configuration(
            &context,
            &(&config, &PlatformUiConfig::default()),
            &mut builder,
        )
        .unwrap();
        let config = builder.build();
        assert_eq!(
            config.bundles,
            [
                "display_drivers_base".to_string(),
                "vulkan_loader".to_string(),
                "lavapipe_pkg".to_string()
            ]
            .into()
        );
    }

    #[test]
    fn test_fake_display_custom_dimensions() {
        use assembly_config_schema::platform_settings::graphics_config::{
            FakeDisplayConfig, FakeDisplayValues,
        };

        let board_config = BoardConfig {
            provided_features: vec!["fuchsia::fake_display".to_string()],
            ..Default::default()
        };
        let context = ConfigurationContext {
            board_config: &board_config,
            ..ConfigurationContext::default_for_tests()
        };
        let config = GraphicsConfig {
            fake_display: FakeDisplayConfig::Values(FakeDisplayValues {
                width: 1080,
                height: 2410,
            }),
            ..Default::default()
        };
        let mut builder = ConfigurationBuilderImpl::default();
        GraphicsSubsystemConfig::define_configuration(
            &context,
            &(&config, &PlatformUiConfig { enabled: true, ..Default::default() }),
            &mut builder,
        )
        .unwrap();
        let config = builder.build();
        let component_config = config
            .package_configs
            .get("fake-display-stack-host")
            .unwrap()
            .components
            .get("meta/fake-display-stack-host.cm")
            .unwrap();
        assert_eq!(
            component_config.fields.get("active_width_px").unwrap(),
            &serde_json::json!(1080)
        );
        assert_eq!(
            component_config.fields.get("active_height_px").unwrap(),
            &serde_json::json!(2410)
        );
    }

    // TODO(https://fxbug.dev/543145106): Remove BoardFeature::FakeDisplay and force product configs
    // to set the fake display settings.
    #[test]
    fn test_fake_display_from_product_config_without_board_feature() {
        use assembly_config_schema::platform_settings::graphics_config::{
            FakeDisplayConfig, FakeDisplayValues,
        };

        let board_config = BoardConfig::default();
        let context = ConfigurationContext {
            board_config: &board_config,
            ..ConfigurationContext::default_for_tests()
        };
        let config = GraphicsConfig {
            fake_display: FakeDisplayConfig::Values(FakeDisplayValues {
                width: 1080,
                height: 2410,
            }),
            ..Default::default()
        };
        let mut builder = ConfigurationBuilderImpl::default();
        GraphicsSubsystemConfig::define_configuration(
            &context,
            &(&config, &PlatformUiConfig { enabled: true, ..Default::default() }),
            &mut builder,
        )
        .unwrap();
        let config = builder.build();
        assert!(config.bundles.contains("fake_display_stack_host"));
        let component_config = config
            .package_configs
            .get("fake-display-stack-host")
            .unwrap()
            .components
            .get("meta/fake-display-stack-host.cm")
            .unwrap();
        assert_eq!(
            component_config.fields.get("active_width_px").unwrap(),
            &serde_json::json!(1080)
        );
        assert_eq!(
            component_config.fields.get("active_height_px").unwrap(),
            &serde_json::json!(2410)
        );
    }

    #[test]
    fn test_fake_display_on_with_default_values() {
        use assembly_config_schema::platform_settings::graphics_config::FakeDisplayConfig;

        let board_config = BoardConfig::default();
        let context = ConfigurationContext {
            board_config: &board_config,
            ..ConfigurationContext::default_for_tests()
        };
        let config = GraphicsConfig {
            fake_display: FakeDisplayConfig::OnWithDefaultValues,
            ..Default::default()
        };
        let mut builder = ConfigurationBuilderImpl::default();
        GraphicsSubsystemConfig::define_configuration(
            &context,
            &(&config, &PlatformUiConfig { enabled: true, ..Default::default() }),
            &mut builder,
        )
        .unwrap();
        let config = builder.build();
        assert!(config.bundles.contains("fake_display_stack_host"));
        assert!(!config.package_configs.contains_key("fake-display-stack-host"));
    }

    #[test]
    fn test_fake_display_from_board_config_backward_compatibility() {
        let board_config = BoardConfig {
            provided_features: vec!["fuchsia::fake_display".to_string()],
            ..Default::default()
        };
        let context = ConfigurationContext {
            board_config: &board_config,
            ..ConfigurationContext::default_for_tests()
        };
        let config = GraphicsConfig::default();
        let mut builder = ConfigurationBuilderImpl::default();
        GraphicsSubsystemConfig::define_configuration(
            &context,
            &(&config, &PlatformUiConfig { enabled: true, ..Default::default() }),
            &mut builder,
        )
        .unwrap();
        let config = builder.build();
        assert!(config.bundles.contains("fake_display_stack_host"));
        assert!(!config.package_configs.contains_key("fake-display-stack-host"));
    }

    #[test]
    fn test_fake_display_error_when_ui_disabled() {
        use assembly_config_schema::platform_settings::graphics_config::{
            FakeDisplayConfig, FakeDisplayValues,
        };

        let context = ConfigurationContext::default_for_tests();
        let config = GraphicsConfig {
            fake_display: FakeDisplayConfig::Values(FakeDisplayValues { width: 720, height: 1280 }),
            ..Default::default()
        };
        let mut builder = ConfigurationBuilderImpl::default();
        let result = GraphicsSubsystemConfig::define_configuration(
            &context,
            &(&config, &PlatformUiConfig { enabled: false, ..Default::default() }),
            &mut builder,
        );
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "graphics.fake_display cannot be configured when ui.enabled is false"
        );
    }
}
