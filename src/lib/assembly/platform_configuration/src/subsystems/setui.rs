// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fs::File;

use crate::subsystems::prelude::*;
use anyhow::{Context, Result};
use assembly_config_schema::platform_settings::setui_config::{ICUType, SetUiConfig};
use assembly_constants::FileEntry;

pub(crate) struct SetUiSubsystem;
impl DefineSubsystemConfiguration<SetUiConfig> for SetUiSubsystem {
    fn define_configuration(
        context: &ConfigurationContext<'_>,
        config: &SetUiConfig,
        builder: &mut dyn ConfigurationBuilder,
    ) -> Result<()> {
        // setui is always added to Standard feature set level system.
        if *context.feature_set_level == FeatureSetLevel::Standard {
            let bundle_name = if config.with_camera { "setui_with_camera" } else { "setui" };
            match config.use_icu {
                ICUType::Flavored => {
                    builder
                        .icu_platform_bundle(bundle_name)
                        .context("while configuring the 'Intl' subsystem with ICU")?;
                }
                ICUType::Unflavored => {
                    builder.platform_bundle(bundle_name)?;
                }
            };

            if let Some(display) = &config.display {
                builder.package("setui_service").config_data(FileEntry {
                    source: display.clone(),
                    destination: "display_configuration.json".into(),
                })?;
            }

            if let Some(interface) = &config.interface {
                builder.package("setui_service").config_data(FileEntry {
                    source: interface.clone(),
                    destination: "interface_configuration.json".into(),
                })?;
            }

            if let Some(agent) = &config.agent {
                builder.package("setui_service").config_data(FileEntry {
                    source: agent.clone(),
                    destination: "agent_configuration.json".into(),
                })?;
            }

            if config.external_brightness_controller {
                // if an external brightness controller is in use, write a service_flags.json
                // file that specifies that.

                let service_flags_json = serde_json::json!({
                    "controller_flags": ["ExternalBrightnessControl"]
                });

                let service_flags_config = context
                    .get_gendir()
                    .context("getting gen dir for setui service_flags.json config")?
                    .join("service_flags.json");
                let config_file = File::create(&service_flags_config).with_context(|| {
                    format!("Creating service_flags.json file: {service_flags_config}")
                })?;
                serde_json::to_writer_pretty(config_file, &service_flags_json).with_context(
                    || format!("Writing service_flags.json file: {service_flags_config}"),
                )?;

                builder
                    .package("setui_service")
                    .config_data(FileEntry {
                        source: service_flags_config,
                        destination: "service_flags.json".into(),
                    })
                    .context("Adding service_flags.json config data entry")?;
            }

            if let Some(input_device_config) = &config.input_device_config {
                builder
                    .package("setui_service")
                    .config_data(FileEntry {
                        source: input_device_config.clone(),
                        destination: "input_device_config.json".into(),
                    })
                    .context("Adding input_device_config.json")?;
            }

            if let Some(light_hardware_config) = &config.light_hardware_config {
                builder
                    .package("setui_service")
                    .config_data(FileEntry {
                        source: light_hardware_config.clone(),
                        destination: "light_hardware_config.json".into(),
                    })
                    .context("Adding light_hardware_config.json")?;
            }
        }
        Ok(())
    }
}
