// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::subsystems::prelude::*;
use anyhow::ensure;
use assembly_config_capabilities::{Config, ConfigNestedValueType, ConfigValueType};
use assembly_config_schema::platform_settings::connectivity_config::{
    PlatformConnectivityConfig, WlanPolicyLayer,
};
use assembly_config_schema::platform_settings::starnix_config::{
    NetworkManagerTreatment, PlatformStarnixConfig, SocketMarkTreatment,
};
use assembly_constants::BoardFeature;
use starnix_features::{Feature, FeatureAndArgs};

pub(crate) struct StarnixSubsystem;
impl DefineSubsystemConfiguration<(&PlatformStarnixConfig, &PlatformConnectivityConfig)>
    for StarnixSubsystem
{
    fn define_configuration(
        context: &ConfigurationContext<'_>,
        configs: &(&PlatformStarnixConfig, &PlatformConnectivityConfig),
        builder: &mut dyn ConfigurationBuilder,
    ) -> anyhow::Result<()> {
        let (starnix_config, connectivity_config) = *configs;
        let PlatformStarnixConfig {
            enabled,
            enable_android_support,
            socket_mark,
            network_manager,
            enable_wakeup_test,
            prefetch_kernel,
        } = starnix_config;

        if *enabled {
            ensure!(
                *context.feature_set_level == FeatureSetLevel::Standard,
                "Starnix is only supported in the default feature set level"
            );
            ensure!(
                *context.build_type != BuildType::User,
                "Starnix is not supported on user builds."
            );
            builder.platform_bundle("starnix_support")?;

            let has_fullmac = context.board_config.provides_feature(BoardFeature::WlanFullmac);
            let has_softmac = context.board_config.provides_feature(BoardFeature::WlanSoftmac);
            let has_wifi = *enable_android_support && (has_fullmac || has_softmac);
            let has_wakeup_test = if *enable_wakeup_test {
                ensure!(
                    *context.build_type != BuildType::User,
                    "The wakeup_test feature is not supported on user builds."
                );
                true
            } else {
                false
            };
            if has_wifi {
                ensure!(
                    matches!(connectivity_config.wlan.policy_layer, WlanPolicyLayer::ViaWlanix),
                    "Android Wi-fi requires the Wlanix policy layer to be enabled"
                );
            }
            if *enable_android_support {
                builder.set_config_capability(
                    "fuchsia.starnix.runner.EnableDataCollection",
                    Config::new(
                        ConfigValueType::Bool,
                        (*context.build_type == BuildType::UserDebug).into(),
                    ),
                )?;
                builder.platform_bundle("hvdcp_opti_support")?;
                builder.platform_bundle("nanohub_support")?;
                builder.platform_bundle("fastrpc_support")?;
                builder.platform_bundle("erofs_support")?;
            } else {
                builder.set_config_capability(
                    "fuchsia.starnix.runner.EnableDataCollection",
                    Config::new(ConfigValueType::Bool, false.into()),
                )?;
            }
            builder.set_config_capability(
                "fuchsia.starnix.mcu.ExpectReady",
                Config::new(ConfigValueType::Bool, (*enable_android_support).into()),
            )?;
            builder.set_config_capability(
                "fuchsia.starnix.fastrpc.ExpectReady",
                Config::new(ConfigValueType::Bool, (*enable_android_support).into()),
            )?;
            builder.set_config_capability(
                "fuchsia.starnix.config.Prefetch",
                Config::new(ConfigValueType::Bool, (*prefetch_kernel).into()),
            )?;
            builder.set_config_capability(
                "fuchsia.starnix.config.container.ExtraFeatures",
                Config::new(
                    ConfigValueType::Vector {
                        nested_type: ConfigNestedValueType::String { max_size: 1024 },
                        max_count: 1024,
                    },
                    [
                        match socket_mark {
                            SocketMarkTreatment::SharedWithNetstack => None,
                        },
                        match network_manager {
                            NetworkManagerTreatment::Disabled => None,
                            NetworkManagerTreatment::Enabled => Some(FeatureAndArgs {
                                feature: Feature::NetworkManager,
                                raw_args: None,
                            }),
                        },
                        has_wifi
                            .then_some(FeatureAndArgs { feature: Feature::Wifi, raw_args: None }),
                        has_wakeup_test.then_some(FeatureAndArgs {
                            feature: Feature::WakeupTest,
                            raw_args: None,
                        }),
                    ]
                    .into_iter()
                    .flatten()
                    .map(|feature: FeatureAndArgs| feature.to_string())
                    .collect::<Vec<_>>()
                    .into(),
                ),
            )?;
        } else {
            builder.set_config_capability(
                "fuchsia.starnix.mcu.ExpectReady",
                Config::new(ConfigValueType::Bool, false.into()),
            )?;
            builder.set_config_capability(
                "fuchsia.starnix.fastrpc.ExpectReady",
                Config::new(ConfigValueType::Bool, false.into()),
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subsystems::ConfigurationBuilderImpl;
    use assembly_config_schema::BoardConfig;

    #[test]
    fn test_define_configuration_with_wifi_support_requires_wlanix() {
        let mut board_config: BoardConfig = Default::default();
        board_config.provided_features.push("fuchsia::wlan_fullmac".into());

        let context = ConfigurationContext {
            feature_set_level: &FeatureSetLevel::Standard,
            build_type: &BuildType::Eng,
            board_config: &board_config,
            gendir: Default::default(),
            resource_dir: Default::default(),
            developer_only_options: Default::default(),
        };

        let starnix_config = PlatformStarnixConfig {
            enabled: true,
            enable_android_support: true,
            ..Default::default()
        };

        let connectivity_config = PlatformConnectivityConfig { ..Default::default() };

        let mut builder: ConfigurationBuilderImpl = Default::default();

        let result = StarnixSubsystem::define_configuration(
            &context,
            &(&starnix_config, &connectivity_config),
            &mut builder,
        );

        assert!(result.is_err());
        let error_message = result.unwrap_err().to_string();
        assert!(
            error_message.contains("Android Wi-fi requires the Wlanix policy layer to be enabled")
        );
    }
}
