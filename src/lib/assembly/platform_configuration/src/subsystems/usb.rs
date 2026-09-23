// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::subsystems::prelude::*;
use assembly_config_capabilities::{Config, ConfigNestedValueType, ConfigValueType};
use assembly_config_schema::platform_settings::starnix_config::PlatformStarnixConfig;
use assembly_config_schema::platform_settings::usb_config::{UsbConfig, UsbPeripheralFunction};
use assembly_constants::BoardFeature;

pub(crate) struct UsbSubsystem;

impl DefineSubsystemConfiguration<(&UsbConfig, &PlatformStarnixConfig)> for UsbSubsystem {
    fn define_configuration(
        context: &ConfigurationContext<'_>,
        (usb, starnix): &(&UsbConfig, &PlatformStarnixConfig),
        builder: &mut dyn ConfigurationBuilder,
    ) -> anyhow::Result<()> {
        let usb_peripheral_functions: Vec<String> =
            usb.peripheral.functions().iter().map(|x| x.to_string()).collect();

        builder.set_config_capability(
            "fuchsia.usb.PeripheralConfig.Functions",
            Config::new(
                ConfigValueType::Vector {
                    nested_type: ConfigNestedValueType::String { max_size: 32 },
                    max_count: 8,
                },
                usb_peripheral_functions.into(),
            ),
        )?;

        // Include xHCI driver through a platform AIB.
        if context.board_config.provides_feature(BoardFeature::Xhci) {
            builder.platform_bundle("xhci_driver")?;
        }
        if context.board_config.provides_feature(BoardFeature::UsbHost) {
            builder.platform_bundle("usb_host_drivers")?;
        }
        let enable_policy = usb.enable_policy.unwrap_or(
            matches!(
                context.feature_set_level,
                FeatureSetLevel::Utility | FeatureSetLevel::Standard
            ) && matches!(context.build_type, BuildType::UserDebug | BuildType::Eng),
        );

        if context.board_config.provides_feature(BoardFeature::UsbPeripheralSupport) {
            if enable_policy {
                builder.platform_bundle("usb_policy")?;
                if starnix.enabled {
                    builder.platform_bundle("usb_policy_starnix")?;
                }
            }
            for function in usb.peripheral.functions() {
                match (function, context.feature_set_level, context.build_type) {
                    (UsbPeripheralFunction::Adb, _, _) => {
                        builder.platform_bundle("usb_adb_function")?
                    }
                    (
                        UsbPeripheralFunction::Cdc,
                        FeatureSetLevel::Bootstrap | FeatureSetLevel::Embeddable,
                        BuildType::UserDebug | BuildType::Eng,
                    ) => {
                        builder.platform_bundle("usb_cdc_function_boot")?;
                    }
                    (
                        UsbPeripheralFunction::Cdc,
                        FeatureSetLevel::Utility | FeatureSetLevel::Standard,
                        _,
                    ) => {
                        builder.platform_bundle("usb_cdc_function_base")?;
                    }
                    (UsbPeripheralFunction::Fastboot, _, _) => {
                        builder.platform_bundle("fastbootd_usb_support")?
                    }
                    (
                        UsbPeripheralFunction::VsockBridge,
                        FeatureSetLevel::Utility | FeatureSetLevel::Standard,
                        BuildType::UserDebug | BuildType::Eng,
                    ) => {
                        builder.platform_bundle("core_realm_development_access_rcs_usb")?;
                        // Dependency of ^
                        builder.platform_bundle("vsock_service")?;
                    }
                    (UsbPeripheralFunction::Rndis, _, _) => {
                        builder.platform_bundle("usb_rndis_function")?
                    }
                    (
                        UsbPeripheralFunction::Test,
                        FeatureSetLevel::Utility | FeatureSetLevel::Standard,
                        BuildType::Eng,
                    ) => builder.platform_bundle("usb_zero_function_base")?,
                    (UsbPeripheralFunction::Ums, _, _) => {
                        builder.platform_bundle("usb_ums_function")?
                    }
                    _ => (),
                }
            }
        }

        match context.feature_set_level {
            FeatureSetLevel::Bootstrap | FeatureSetLevel::Embeddable => {
                builder.platform_bundle("usb_peripheral_drivers_boot")?;
            }
            FeatureSetLevel::Utility | FeatureSetLevel::Standard => {
                builder.platform_bundle("usb_peripheral_drivers_base")?;
            }
        }
        Ok(())
    }
}
