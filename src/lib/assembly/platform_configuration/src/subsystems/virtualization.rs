// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::subsystems::prelude::*;
use anyhow::ensure;
use assembly_config_schema::platform_settings::virtualization_config::PlatformVirtualizationConfig;

pub(crate) struct VirtualizationSubsystem;

impl DefineSubsystemConfiguration<PlatformVirtualizationConfig> for VirtualizationSubsystem {
    fn define_configuration(
        context: &ConfigurationContext<'_>,
        virtualization_config: &PlatformVirtualizationConfig,
        builder: &mut dyn ConfigurationBuilder,
    ) -> anyhow::Result<()> {
        if virtualization_config.enabled {
            ensure!(
                *context.feature_set_level == FeatureSetLevel::Standard,
                "Virtualization is only supported in the default feature set level"
            );
            builder.platform_bundle("virtualization_support")?;
        }
        Ok(())
    }
}
