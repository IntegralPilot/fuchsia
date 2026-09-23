// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::subsystems::prelude::*;
use anyhow::Context;
use assembly_config_schema::platform_settings::system_sounds_config::SystemSoundsConfig;
use assembly_constants::FileEntry;

pub(crate) struct SystemSoundsSubsystem;
impl DefineSubsystemConfiguration<SystemSoundsConfig> for SystemSoundsSubsystem {
    fn define_configuration(
        context: &ConfigurationContext<'_>,
        system_sounds_config: &SystemSoundsConfig,
        builder: &mut dyn ConfigurationBuilder,
    ) -> anyhow::Result<()> {
        if let Some(earcons) = &system_sounds_config.earcons {
            context.ensure_feature_set_level(&[FeatureSetLevel::Standard], "earcons")?;

            builder.package("setui_service").optional_config_data_files(vec![
                (&earcons.bluetooth_connected, "bluetooth-connected.wav"),
                (&earcons.bluetooth_disconnected, "bluetooth-disconnected.wav"),
                (&earcons.volume_changed, "volume-changed.wav"),
                (&earcons.volume_max_reached, "volume-max.wav"),
            ])?;

            if let Some(path) = &earcons.system_start {
                builder
                    .package("scene_manager")
                    .config_data(FileEntry {
                        source: path.clone(),
                        destination: "chirp-start-tone.wav".into(),
                    })
                    .context("Setting scene-manager start sound")?;
            }
        }

        Ok(())
    }
}
