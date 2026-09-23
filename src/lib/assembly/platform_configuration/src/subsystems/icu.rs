// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::subsystems::prelude::*;
use anyhow::{Context, Result};
use assembly_config_schema::platform_settings::icu_config::ICUConfig;
use assembly_constants::FileEntry;

const DATA_VERSION: &str = "44";
const FORMAT: &str = "le";

pub(crate) struct IcuSubsystem;
impl DefineSubsystemConfiguration<ICUConfig> for IcuSubsystem {
    fn define_configuration(
        context: &ConfigurationContext<'_>,
        config: &ICUConfig,
        builder: &mut dyn ConfigurationBuilder,
    ) -> Result<()> {
        for package in &config.legacy_tzdata_packages {
            builder
                .package(package)
                .config_data(FileEntry {
                    source: context.get_resource("metaZones.res"),
                    destination: format!("tzdata/icu/{DATA_VERSION}/{FORMAT}/metaZones.res"),
                })
                .context(format!("Providing metaZones.res to {package}"))?
                .config_data(FileEntry {
                    source: context.get_resource("timezoneTypes.res"),
                    destination: format!("tzdata/icu/{DATA_VERSION}/{FORMAT}/timezoneTypes.res"),
                })
                .context(format!("Providing timezoneTypes.res to {package}"))?
                .config_data(FileEntry {
                    source: context.get_resource("zoneinfo64.res"),
                    destination: format!("tzdata/icu/{DATA_VERSION}/{FORMAT}/zoneinfo64.res"),
                })
                .context(format!("Providing zoneinfo64.res to {package}"))?
                .config_data(FileEntry {
                    source: context.get_resource("revision.txt"),
                    destination: "tzdata/revision.txt".to_string(),
                })
                .context(format!("Providing revision.txt to {package}"))?;
        }

        Ok(())
    }
}
