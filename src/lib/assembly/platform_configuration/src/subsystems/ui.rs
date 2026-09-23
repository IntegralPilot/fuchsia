// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::subsystems::prelude::*;
use anyhow::ensure;
use assembly_config_capabilities::{Config, ConfigNestedValueType, ConfigValueType};
use assembly_config_schema::platform_settings::ui_config::{
    PlatformUiConfig, UnsignedIntegerRangeInclusive,
};
use assembly_constants::{BoardFeature, FileEntry, PackageDestination, PackageSetDestination};

pub(crate) struct UiSubsystem;

impl DefineSubsystemConfiguration<PlatformUiConfig> for UiSubsystem {
    fn define_configuration(
        context: &ConfigurationContext<'_>,
        ui_config: &PlatformUiConfig,
        builder: &mut dyn ConfigurationBuilder,
    ) -> anyhow::Result<()> {
        if context.board_config.provides_feature(BoardFeature::Input) {
            builder.platform_bundle("input_drivers")?;
        }

        let visual_debugging_level: u8 = ui_config.visual_debugging_level.clone().into();
        builder.set_config_capability(
            "fuchsia.ui.VisualDebuggingLevel",
            Config::new(ConfigValueType::Uint8, visual_debugging_level.into()),
        )?;
        builder.set_config_capability(
            "fuchsia.ui.Prefetch",
            Config::new(ConfigValueType::Bool, ui_config.prefetch.into()),
        )?;

        if !ui_config.enabled {
            return Ok(());
        }

        if ui_config.enable_dso {
            // Supports ui_*_dso bundles below
            builder.platform_bundle("dso_runner")?;
        }

        match context.build_type {
            BuildType::Eng => {
                builder.platform_bundle("ui")?;
                if ui_config.enable_dso {
                    builder.icu_platform_bundle("ui_eng_dso")?;
                } else {
                    builder.icu_platform_bundle("ui_eng")?;
                }
                match &ui_config.with_synthetic_device_support {
                    true => {
                        builder.platform_bundle(
                            "ui_package_eng_userdebug_with_synthetic_device_support",
                        )?;
                    }
                    false => {
                        builder.platform_bundle("ui_package_eng")?;
                    }
                }
            }
            BuildType::UserDebug => {
                builder.platform_bundle("ui")?;
                if ui_config.enable_dso {
                    builder.icu_platform_bundle("ui_userdebug_dso")?;
                } else {
                    builder.icu_platform_bundle("ui_user_and_userdebug")?;
                }
                match &ui_config.with_synthetic_device_support {
                    true => {
                        builder.platform_bundle(
                            "ui_package_eng_userdebug_with_synthetic_device_support",
                        )?;
                    }
                    false => {
                        builder.platform_bundle("ui_package_user_and_userdebug")?;
                    }
                }
            }
            BuildType::User => {
                ensure!(
                    !ui_config.enable_dso,
                    "experimental dso must not be enabled on user builds"
                );
                builder.platform_bundle("ui")?;
                builder.icu_platform_bundle("ui_user_and_userdebug")?;
                builder.platform_bundle("ui_package_user_and_userdebug")?;
            }
        }

        ensure!(
            *context.feature_set_level == FeatureSetLevel::Standard,
            "UI is only supported in the default feature set level"
        );

        fn get_px_range(new: &UnsignedIntegerRangeInclusive) -> (i32, i32) {
            let start = new.start.map(|i| i as i32).unwrap_or(-1);
            let end = new.end.map(|i| i as i32).unwrap_or(-1);
            (start, end)
        }

        let (horizontal_res_min, horizontal_res_max) =
            get_px_range(&ui_config.display_mode.horizontal_resolution_px_range);
        let (vertical_res_min, vertical_res_max) =
            get_px_range(&ui_config.display_mode.vertical_resolution_px_range);
        let (refresh_rate_min, refresh_rate_max) =
            get_px_range(&ui_config.display_mode.refresh_rate_millihertz_range);

        // We should only configure scenic here when it has been added to assembly.
        builder.set_config_capability(
            "fuchsia.scenic.Renderer",
            Config::new(
                ConfigValueType::String { max_size: 16 },
                serde_json::to_value(ui_config.renderer.clone())?,
            ),
        )?;
        builder.set_config_capability(
            "fuchsia.scenic.FrameSchedulerMinPredictedFrameDurationInUs",
            Config::new(
                ConfigValueType::Uint64,
                ui_config.frame_scheduler_min_predicted_frame_duration_in_us.into(),
            ),
        )?;
        builder.set_config_capability(
            "fuchsia.scenic.FramePredictionMarginInUs",
            Config::new(ConfigValueType::Uint64, ui_config.frame_prediction_margin_in_us.into()),
        )?;
        builder.set_config_capability(
            "fuchsia.scenic.PointerAutoFocus",
            Config::new(ConfigValueType::Bool, ui_config.pointer_auto_focus.into()),
        )?;
        builder.set_config_capability(
            "fuchsia.scenic.UseSeparateInputThread",
            Config::new(ConfigValueType::Bool, ui_config.use_separate_input_thread.into()),
        )?;
        if ui_config.display_composition_enable_heuristics {
            ensure!(
                ui_config.display_composition,
                "display_composition_enable_heuristics can only be enabled if display_composition is enabled"
            );
        }
        builder.set_config_capability(
            "fuchsia.scenic.DisplayComposition",
            Config::new(ConfigValueType::Bool, ui_config.display_composition.into()),
        )?;
        builder.set_config_capability(
            "fuchsia.scenic.DisplayCompositionEnableHeuristics",
            Config::new(
                ConfigValueType::Bool,
                ui_config.display_composition_enable_heuristics.into(),
            ),
        )?;
        builder.set_config_capability(
            "fuchsia.scenic.ICanHazDisplayId",
            Config::new(ConfigValueType::Int64, (-1i64).into()),
        )?;
        builder.set_config_capability(
            "fuchsia.scenic.ICanHazDisplayMode",
            Config::new(ConfigValueType::Int64, (-1i64).into()),
        )?;
        builder.set_config_capability(
            "fuchsia.scenic.DisplayRotation",
            Config::new(ConfigValueType::Uint64, ui_config.display_rotation.into()),
        )?;
        builder.set_config_capability(
            "fuchsia.scenic.MinDisplayHorizontalResolutionPx",
            Config::new(ConfigValueType::Int32, horizontal_res_min.into()),
        )?;
        builder.set_config_capability(
            "fuchsia.scenic.MaxDisplayHorizontalResolutionPx",
            Config::new(ConfigValueType::Int32, horizontal_res_max.into()),
        )?;
        builder.set_config_capability(
            "fuchsia.scenic.MinDisplayVerticalResolutionPx",
            Config::new(ConfigValueType::Int32, vertical_res_min.into()),
        )?;
        builder.set_config_capability(
            "fuchsia.scenic.MaxDisplayVerticalResolutionPx",
            Config::new(ConfigValueType::Int32, vertical_res_max.into()),
        )?;
        builder.set_config_capability(
            "fuchsia.scenic.MinDisplayRefreshRateMillihertz",
            Config::new(ConfigValueType::Int32, refresh_rate_min.into()),
        )?;
        builder.set_config_capability(
            "fuchsia.scenic.MaxDisplayRefreshRateMillihertz",
            Config::new(ConfigValueType::Int32, refresh_rate_max.into()),
        )?;
        builder.set_config_capability(
            "fuchsia.scenic.FrameCounterOverlay",
            Config::new(ConfigValueType::Bool, ui_config.scenic_frame_counter_overlay.into()),
        )?;

        builder.set_config_capability(
            "fuchsia.ui.SupportedInputDevices",
            Config::new(
                ConfigValueType::Vector {
                    nested_type: ConfigNestedValueType::String { max_size: 12 },
                    max_count: 6,
                },
                ui_config
                    .supported_input_devices
                    .iter()
                    .filter_map(|d| serde_json::to_value(d).ok())
                    .collect::<serde_json::Value>(),
            ),
        )?;

        builder.set_config_capability(
            "fuchsia.ui.DisplayPixelDensity",
            Config::new(
                ConfigValueType::String { max_size: 8 },
                ui_config.display_pixel_density.clone().into(),
            ),
        )?;

        builder.set_config_capability(
            "fuchsia.ui.ViewingDistance",
            Config::new(
                ConfigValueType::String { max_size: 8 },
                ui_config.viewing_distance.as_ref().to_string().into(),
            ),
        )?;

        builder.set_config_capability(
            "fuchsia.ui.AttachA11yView",
            Config::new(ConfigValueType::Bool, ui_config.attach_a11y_view.into()),
        )?;

        builder.set_config_capability(
            "fuchsia.ui.IdleThresholdMs",
            Config::new(ConfigValueType::Uint64, {
                match context.build_type {
                    BuildType::Eng => 5000,
                    BuildType::UserDebug | BuildType::User => 5000,
                }
                .into()
            }),
        )?;

        let config_dir = builder
            .add_domain_config(PackageSetDestination::Blob(PackageDestination::SensorConfig))
            .directory("sensor-config");
        if let Some(sensor_config_path) = &ui_config.sensor_config {
            config_dir.entry(FileEntry {
                source: sensor_config_path.clone(),
                destination: "config.json".into(),
            })?;
        }

        let mut manage_display_power = false;
        let mut power_on_display_millis = 0u16;
        let mut power_off_display_millis = 0u16;
        if let Some(brightness_manager) = &ui_config.brightness_manager {
            builder.platform_bundle("brightness_manager")?;
            if brightness_manager.with_display_power {
                manage_display_power = true;
                power_on_display_millis = 35;
                power_off_display_millis = 85;
            }
        }
        builder.set_config_capability(
            "fuchsia.ui.ManageDisplayPower",
            Config::new(ConfigValueType::Bool, manage_display_power.into()),
        )?;
        builder.set_config_capability(
            "fuchsia.ui.PowerOnDelayMillis",
            Config::new(ConfigValueType::Uint16, power_on_display_millis.into()),
        )?;
        builder.set_config_capability(
            "fuchsia.ui.PowerOffDelayMillis",
            Config::new(ConfigValueType::Uint16, power_off_display_millis.into()),
        )?;
        builder.set_config_capability(
            "fuchsia.ui.EnableMergeTouchEvents",
            Config::new(ConfigValueType::Bool, ui_config.enable_merge_touch_events.into()),
        )?;

        Ok(())
    }
}
