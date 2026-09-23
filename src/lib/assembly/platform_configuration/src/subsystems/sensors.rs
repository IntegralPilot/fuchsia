// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use assembly_config_schema::platform_settings::starnix_config::PlatformStarnixConfig;

use crate::subsystems::prelude::*;

pub(crate) struct SensorsSubsystemConfig;
impl DefineSubsystemConfiguration<PlatformStarnixConfig> for SensorsSubsystemConfig {
    fn define_configuration(
        context: &ConfigurationContext<'_>,
        starnix_config: &PlatformStarnixConfig,
        builder: &mut dyn ConfigurationBuilder,
    ) -> anyhow::Result<()> {
        // TODO(https://fxbug.dev/397439065): Consider adding a separate platform configuration to
        // enable sensors.
        if starnix_config.enable_android_support
            && *context.feature_set_level == FeatureSetLevel::Standard
        {
            // TODO(https://fxbug.dev/370576398): Remove sensors playback from UserDebug.
            if *context.build_type == BuildType::Eng || *context.build_type == BuildType::UserDebug
            {
                builder.platform_bundle("sensors_framework_eng")?;
            } else {
                builder.platform_bundle("sensors_framework")?;
            }
        }

        Ok(())
    }
}
