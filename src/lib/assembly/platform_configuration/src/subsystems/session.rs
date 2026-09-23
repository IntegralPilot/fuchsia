// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::subsystems::prelude::*;
use anyhow::{Context, ensure};
use assembly_config_capabilities::{Config, ConfigValueType};
use assembly_config_schema::platform_settings::session_config::PlatformSessionConfig;
use assembly_config_schema::platform_settings::swd_config::SwdConfig;
use assembly_config_schema::product_settings::ProductSessionConfig;
use fuchsia_url::fuchsia_pkg::AbsoluteComponentUrl;

pub(crate) struct SessionConfig;
impl
    DefineSubsystemConfiguration<(
        &PlatformSessionConfig,
        &Option<ProductSessionConfig>,
        &SwdConfig,
    )> for SessionConfig
{
    fn define_configuration(
        context: &ConfigurationContext<'_>,
        config: &(&PlatformSessionConfig, &Option<ProductSessionConfig>, &SwdConfig),
        builder: &mut dyn ConfigurationBuilder,
    ) -> anyhow::Result<()> {
        let (platform_config, product_config, swd_config) = *config;

        if platform_config.enabled {
            ensure!(
                *context.feature_set_level == FeatureSetLevel::Standard,
                "The platform session manager is only supported in the default feature set level"
            );
            builder.platform_bundle("session_manager")?;
            if swd_config.enable_upgradable_packages {
                context.ensure_build_type_and_feature_set_level(
                    &[BuildType::Eng, BuildType::UserDebug],
                    &[FeatureSetLevel::Standard],
                    "Upgradable packages",
                )?;
                builder.platform_bundle("session_manager_enable_pkg_cache")?;
            } else {
                builder.platform_bundle("session_manager_disable_pkg_cache")?;
            }
        }

        let (url, collection, element_url, view_id_annotation) = match (
            context.feature_set_level,
            product_config,
        ) {
            (FeatureSetLevel::Standard, Some(config)) => {
                let config_url = AbsoluteComponentUrl::parse(&config.url)
                    .with_context(|| format!("valid session URLs given by session.url must start with `fuchsia-pkg://`, got `{}`", config.url))?
                    .to_string();
                if let Some(element) = &config.initial_element {
                    (
                        config_url,
                        Config::new(
                            ConfigValueType::String { max_size: 32 },
                            element.collection.to_owned().into(),
                        ),
                        Config::new(
                            ConfigValueType::String { max_size: 256 },
                            AbsoluteComponentUrl::parse(&element.url)
                                .with_context(|| {
                                    format!("valid initial element URLs given by session.initial_element.url must start with `fuchsia-pkg://`, got `{}`", element.url)
                                })?
                                .to_string()
                                .into(),
                        ),
                        Config::new(
                            ConfigValueType::String { max_size: 32 },
                            element.view_id_annotation.to_owned().into(),
                        ),
                    )
                } else {
                    (config_url, Config::new_void(), Config::new_void(), Config::new_void())
                }
            }
            _ => {
                ensure!(
                    product_config.is_none(),
                    "sessions are only supported with the 'Standard' feature set level."
                );
                (String::new(), Config::new_void(), Config::new_void(), Config::new_void())
            }
        };

        builder.set_config_capability(
            "fuchsia.session.SessionUrl",
            Config::new(ConfigValueType::String { max_size: 512 }, url.into()),
        )?;
        builder.set_config_capability(
            "fuchsia.session.AutoLaunch",
            Config::new(ConfigValueType::Bool, platform_config.autolaunch.into()),
        )?;
        builder.set_config_capability(
            "fuchsia.session.EnableDebugShortcut",
            Config::new(ConfigValueType::Bool, platform_config.enable_debug_shortcut.into()),
        )?;

        if platform_config.include_element_manager {
            ensure!(
                *context.feature_set_level == FeatureSetLevel::Standard,
                "The platform element manager is only supported in the default feature set level"
            );
            builder.platform_bundle("element_manager")?;
        }

        // Window manager configs
        builder.set_config_capability("fuchsia.session.window.Collection", collection)?;
        builder.set_config_capability("fuchsia.session.window.InitialElementUrl", element_url)?;
        builder.set_config_capability(
            "fuchsia.session.window.InitialViewIdAnnotation",
            view_id_annotation,
        )?;

        Ok(())
    }
}
