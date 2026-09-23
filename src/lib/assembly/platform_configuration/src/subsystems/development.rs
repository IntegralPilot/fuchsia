// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::subsystems::prelude::*;
use anyhow::Context;
use assembly_config_schema::platform_settings::development_support_config::DevelopmentSupportConfig;
use assembly_constants::{BootfsDestination, FileEntry, KernelArg};

pub(crate) struct DevelopmentConfig;
impl DefineSubsystemConfiguration<DevelopmentSupportConfig> for DevelopmentConfig {
    fn define_configuration(
        context: &ConfigurationContext<'_>,
        config: &DevelopmentSupportConfig,
        builder: &mut dyn ConfigurationBuilder,
    ) -> anyhow::Result<()> {
        // Select the correct AIB based on the user-provided setting if present
        // and fall-back to the default by build-type.
        builder.platform_bundle(match (context.build_type, config.enabled) {
            (BuildType::User, Some(_)) => {
                anyhow::bail!("Development support cannot be enabled on user builds");
            }

            // User-provided development setting for non-user builds.
            (_, Some(true)) => "kernel_args_eng",
            (_, Some(false)) => "kernel_args_user",

            // Default development setting by build-type.
            (BuildType::Eng, None) => "kernel_args_eng",
            (BuildType::UserDebug, None) => "kernel_args_userdebug",
            (BuildType::User, None) => "kernel_args_user",
        })?;

        if config.include_netsvc {
            if context.build_type == &BuildType::User {
                anyhow::bail!("netsvc can't be included in user builds");
            }
            builder.platform_bundle("netsvc")?;
        };

        if config.enable_netsvc_netboot {
            if context.build_type == &BuildType::User {
                anyhow::bail!("netsvc can't be included in user builds");
            }
            builder.kernel_arg(KernelArg::NetsvcNetboot(true));
        };

        if config.tracing_enabled(*context.feature_set_level, *context.build_type)? {
            builder.platform_bundle("tracing")?;
        }

        if matches!(context.build_type, BuildType::Eng | BuildType::UserDebug) {
            builder.platform_bundle("ptysvc")?;
            builder.platform_bundle("kernel_debug_broker_userdebug")?;
        } else {
            builder.platform_bundle("kernel_debug_broker_user")?;
        }

        if config.vsock_development
            && matches!(context.feature_set_level, FeatureSetLevel::Embeddable)
            && matches!(context.build_type, BuildType::Eng | BuildType::UserDebug)
        {
            builder.platform_bundle("bootstrap_realm_vsock_development_access")?;
        }

        match (context.build_type, &config.authorized_ssh_keys_path) {
            (BuildType::User, Some(_)) => {
                anyhow::bail!("authorized_ssh_keys cannot be provided on user builds")
            }
            (_, Some(authorized_ssh_keys_path)) => {
                if config.vsock_development {
                    builder
                        .bootfs()
                        .file(FileEntry {
                            source: authorized_ssh_keys_path.clone(),
                            destination: BootfsDestination::SshAuthorizedKeys,
                        })
                        .context("Setting authorized_keys")?;
                } else {
                    builder
                        .package("sshd-host")
                        .config_data(FileEntry {
                            source: authorized_ssh_keys_path.clone(),
                            destination: "authorized_keys".into(),
                        })
                        .context("Setting authorized_keys")?;
                }
            }
            _ => {}
        }

        match (context.build_type, &config.authorized_ssh_ca_certs_path) {
            (BuildType::User, Some(_)) => {
                anyhow::bail!("authorized_ssh_ca_certs_path cannot be provided on user builds")
            }
            (_, Some(authorized_ssh_ca_certs_path)) => {
                builder
                    .package("sshd-host")
                    .config_data(FileEntry {
                        source: authorized_ssh_ca_certs_path.clone(),
                        destination: "ssh_ca_pub_keys".into(),
                    })
                    .context("Setting authorized ssh ca certs")?;
            }
            _ => {}
        }

        if config.include_sl4f {
            builder.platform_bundle("sl4f")?;
        }

        let is_embeddable = matches!(context.feature_set_level, FeatureSetLevel::Embeddable);
        match (context.build_type, &config.include_bin_clock, is_embeddable) {
            (BuildType::User, true, _) => {
                anyhow::bail!("bin/clock cannot be provided on user builds")
            }
            (_, true, true) => {
                anyhow::bail!("bin/clock cannot be provided on embeddable products");
            }
            (BuildType::Eng, _, false) | (BuildType::UserDebug, true, false) => {
                builder.platform_bundle("clock_development_tools")?
            }
            (_, false, _) => {}
        }

        if let Some(soc) =
            &context.board_config.platform.development_support.enable_debug_access_port_for_soc
        {
            builder.kernel_arg(KernelArg::Arm64DebugDap(soc.clone()));
        }

        if config.tools.audio.driver_tools {
            builder.platform_bundle("audio_driver_development_tools")?;
        }
        if config.tools.audio.legacy_driver_tools {
            builder.platform_bundle("audio_legacy_driver_tools")?;
        }
        if config.tools.audio.is_virtual_audio_modern_enabled() {
            builder.platform_bundle("virtual_audio_driver")?;
            if config.tools.audio.is_virtual_audio_tools_enabled() {
                builder.platform_bundle("virtual_audio_util")?;
            }
        }
        if config.tools.audio.is_virtual_audio_legacy_enabled() {
            builder.platform_bundle("virtual_audio_legacy_driver")?;
            if config.tools.audio.is_virtual_audio_tools_enabled() {
                builder.platform_bundle("virtual_audio_legacy_util")?;
            }
        }
        if config.tools.audio.full_stack_tools {
            builder.platform_bundle("audio_full_stack_development_tools")?;
        }

        if config.tools.connectivity.enable_networking {
            context.ensure_build_type_and_feature_set_level(
                &[BuildType::Eng, BuildType::UserDebug],
                &[FeatureSetLevel::Utility, FeatureSetLevel::Standard],
                "Networking tools",
            )?;
            builder.platform_bundle("development_support_tools_connectivity_networking")?;
        }
        if config.tools.connectivity.enable_wlan {
            context.ensure_build_type_and_feature_set_level(
                &[BuildType::Eng, BuildType::UserDebug],
                &[FeatureSetLevel::Utility, FeatureSetLevel::Standard],
                "WLAN tools",
            )?;
            builder.platform_bundle("development_support_tools_connectivity_wlan")?;
        }
        if config.tools.connectivity.enable_thread {
            context.ensure_build_type_and_feature_set_level(
                &[BuildType::Eng, BuildType::UserDebug],
                &[FeatureSetLevel::Utility, FeatureSetLevel::Standard],
                "Thread (protocol) tools",
            )?;
            builder.platform_bundle("development_support_tools_connectivity_thread")?;
        }

        if config.tools.display.driver_tools {
            context.ensure_build_type_and_feature_set_level(
                &[BuildType::Eng],
                &[FeatureSetLevel::Standard],
                "Display driver development tools",
            )?;
            builder.platform_bundle("display_driver_development_tools")?;
        }

        if config.tools.storage.enable_partitioning_tools {
            context.ensure_build_type_and_feature_set_level(
                &[BuildType::Eng],
                &[FeatureSetLevel::Bootstrap, FeatureSetLevel::Utility, FeatureSetLevel::Standard],
                "Partitioning tools",
            )?;
            builder.platform_bundle("partitioning_tools")?;
        }

        if config.include_bootstrap_testing_framework {
            context.ensure_feature_set_level(
                &[FeatureSetLevel::Bootstrap, FeatureSetLevel::Embeddable],
                "Bootstrap Testing Framework",
            )?;
            builder.platform_bundle("testing_support_bootstrap")?;
        }

        if config.enable_userboot_next_component_manager {
            context.ensure_build_type(&[BuildType::Eng], "userboot.next")?;
            builder.kernel_arg(KernelArg::UserbootNext("bin/component_manager+--boot".to_string()));
        }

        Ok(())
    }
}
