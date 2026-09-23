// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::subsystems::prelude::*;
use assembly_config_schema::platform_settings::battery_config::BatteryConfig;

pub(crate) struct BatterySubsystemConfig;
impl DefineSubsystemConfiguration<BatteryConfig> for BatterySubsystemConfig {
    fn define_configuration(
        _context: &ConfigurationContext<'_>,
        config: &BatteryConfig,
        builder: &mut dyn ConfigurationBuilder,
    ) -> anyhow::Result<()> {
        if config.enabled {
            builder.platform_bundle("battery_manager")?;
        }
        Ok(())
    }
}
