// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::subsystems::prelude::*;
use anyhow::ensure;
use assembly_config_schema::platform_settings::memory_monitor_config::{
    MemoryMonitorVersion, PlatformMemoryMonitorConfig,
};

pub(crate) struct MemoryMonitorSubsystem;
impl DefineSubsystemConfiguration<PlatformMemoryMonitorConfig> for MemoryMonitorSubsystem {
    fn define_configuration(
        context: &ConfigurationContext<'_>,
        memory_monitor_config: &PlatformMemoryMonitorConfig,
        builder: &mut dyn ConfigurationBuilder,
    ) -> anyhow::Result<()> {
        for version in &memory_monitor_config.memory_monitor_versions {
            match version {
                MemoryMonitorVersion::Default => {
                    if *context.feature_set_level != FeatureSetLevel::Standard {
                        continue;
                    }
                    match context.build_type {
                        BuildType::Eng => {
                            builder.platform_bundle("memory_monitor_with_memory_sampler")?;
                        }
                        BuildType::UserDebug => {
                            builder.platform_bundle("memory_monitor_with_memory_sampler")?;
                            // Memory monitor looks for a file named
                            // "send_critical_pressure_crash_reports" to trigger sending crash
                            // reports when entering the critical memory pressure state.
                            // TODO: https://fxbug.dev/371555480 - Use structured configuration.
                            builder.platform_bundle("memory_monitor_critical_reports")?;
                        }
                        BuildType::User => {
                            builder.platform_bundle("memory_monitor")?;
                        }
                    }
                }
                MemoryMonitorVersion::V1 => {
                    builder.platform_bundle("memory_monitor")?;
                }
                MemoryMonitorVersion::V1WithProfiling => {
                    ensure!(*context.build_type != BuildType::User);
                    builder.platform_bundle("memory_monitor_with_memory_sampler")?;
                }
                MemoryMonitorVersion::V2 => {
                    builder.platform_bundle("memory_monitor2")?;
                }
            }
        }
        Ok(())
    }
}
