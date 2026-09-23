// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::subsystems::prelude::*;
use anyhow::ensure;
use assembly_config_capabilities::{Config, ConfigValueType};
use assembly_config_schema::platform_settings::fonts_config::FontsConfig;

pub(crate) struct FontsSubsystem;
impl DefineSubsystemConfiguration<FontsConfig> for FontsSubsystem {
    fn define_configuration(
        context: &ConfigurationContext<'_>,
        fonts_config: &FontsConfig,
        builder: &mut dyn ConfigurationBuilder,
    ) -> anyhow::Result<()> {
        // Adding the platform bundle conditionally allows us to soft-migrate
        // products that use the packages from the `fonts` bundle.
        if *context.feature_set_level == FeatureSetLevel::Standard {
            if let Some(ref font_collection_name) = fonts_config.font_collection {
                ensure!(
                    fonts_config.enabled,
                    "fonts.enabled must be `true` if `fonts.font_collection` is set"
                );
                ensure!(
                    !font_collection_name.is_empty(),
                    "fonts.font_collection must not be empty",
                );
                builder.platform_bundle("fonts_hermetic")?;

                builder.set_config_capability(
                    "fuchsia.fonts.VerboseLogging",
                    Config::new(
                        ConfigValueType::Bool,
                        matches!(context.build_type, BuildType::Eng | BuildType::UserDebug).into(),
                    ),
                )?;

                let font_manifest = format!(
                    "/fonts/data/assets/{font_collection_name}_all.hermetic_assets.font_manifest.json"
                );
                builder.set_config_capability(
                    "fuchsia.fonts.FontManifest",
                    Config::new(ConfigValueType::String { max_size: 1024 }, font_manifest.into()),
                )?;
            } else if fonts_config.enabled {
                builder.platform_bundle("fonts")?;
                builder.set_config_capability(
                    "fuchsia.fonts.VerboseLogging",
                    Config::new(
                        ConfigValueType::Bool,
                        matches!(context.build_type, BuildType::Eng | BuildType::UserDebug).into(),
                    ),
                )?;
                // Fallback to using fonts from `config-data` since a font collection was not
                // specified. Signified by an empty value in `font_manifest`.
                builder.set_config_capability(
                    "fuchsia.fonts.FontManifest",
                    Config::new(ConfigValueType::String { max_size: 1024 }, "".into()),
                )?;
            }
        }
        Ok(())
    }
}
