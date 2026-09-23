// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::subsystems::prelude::*;
use anyhow::{Context, Result, anyhow, ensure};
use assembly_config_schema::platform_settings::intl_config::{IntlConfig, Type};
use assembly_config_schema::platform_settings::session_config::PlatformSessionConfig;

pub(crate) struct IntlSubsystem;
impl DefineSubsystemConfiguration<(&IntlConfig, &PlatformSessionConfig)> for IntlSubsystem {
    fn define_configuration(
        context: &ConfigurationContext<'_>,
        platform_config: &(&IntlConfig, &PlatformSessionConfig),
        builder: &mut dyn ConfigurationBuilder,
    ) -> Result<()> {
        let (config, session_config) = *platform_config;

        // These settings require a core realm.
        if *context.feature_set_level != FeatureSetLevel::Standard {
            if config.config_type.is_some() {
                return Err(anyhow!(concat!(
                    "setting config_type for `intl` is only supported",
                    " on `standard` feature support level"
                )));
            }
            if config.include_zoneinfo_files {
                return Err(anyhow!(concat!(
                    "setting include_zoneinfo_files for `intl` only supported",
                    " only on `standard` feature support level"
                )));
            }
        }

        // Only perform configuration in the standard feature set level
        if *context.feature_set_level == FeatureSetLevel::Standard {
            match config.config_type.clone().unwrap_or_default() {
                Type::Default => {
                    builder
                        .icu_platform_bundle("intl_services")
                        .context("while configuring the 'Intl' subsystem")?;
                }
                Type::Small => {
                    ensure!(
                        session_config.enabled,
                        "Internationalization small is only supported with a session manager"
                    );
                    builder
                        .icu_platform_bundle("intl_services_small")
                        .context("while configuring the 'small Intl' subsystem")?;
                }
                Type::SmallWithTimezone => {
                    ensure!(
                        session_config.enabled,
                        "Internationalization small_with_timezone is only supported with a session manager"
                    );
                    builder
                        .icu_platform_bundle("intl_services_small_with_timezone")
                        .context("while configuring the 'small Intl with timezone' subsystem")?;
                }
                Type::None => {
                    if session_config.enabled {
                        builder.platform_bundle("no_intl_timezones")?;
                    } else {
                        // Since there's no session-manager there's no need to add the "void"
                        // route.
                    }
                }
            }

            if config.include_zoneinfo_files {
                builder.icu_platform_bundle("zoneinfo").context("while configuring `zoneinfo`")?;
            }
        }
        Ok(())
    }
}
