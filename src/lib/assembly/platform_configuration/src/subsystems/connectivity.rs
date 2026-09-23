// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::subsystems::prelude::*;
use crate::util;
use anyhow::{bail, ensure};
use assembly_config_capabilities::{Config, ConfigValueType};
use assembly_config_schema::platform_settings::connectivity_config::{
    NetstackVersion, NetworkingConfig, PlatformConnectivityConfig, WlanPolicyLayer,
    WlanRecoveryProfile, WlanRoamingMode, WlanRoamingPolicy, WlanRoamingProfile,
};
use assembly_constants::{BoardFeature, FileEntry, PackageDestination, PackageSetDestination};

pub(crate) struct ConnectivitySubsystemConfig;
impl DefineSubsystemConfiguration<PlatformConnectivityConfig> for ConnectivitySubsystemConfig {
    fn define_configuration(
        context: &ConfigurationContext<'_>,
        connectivity_config: &PlatformConnectivityConfig,
        builder: &mut dyn ConfigurationBuilder,
    ) -> anyhow::Result<()> {
        match (context.feature_set_level, context.build_type) {
            (FeatureSetLevel::Bootstrap | FeatureSetLevel::Embeddable, BuildType::User) => {}
            (
                FeatureSetLevel::Bootstrap | FeatureSetLevel::Embeddable,
                BuildType::UserDebug | BuildType::Eng,
            ) => {
                builder.platform_bundle("network_drivers_boot")?;
            }
            (FeatureSetLevel::Utility | FeatureSetLevel::Standard, _) => {
                builder.platform_bundle("network_drivers_base")?;
            }
        }

        let publish_fuchsia_dev_wired_service = match (
            context.feature_set_level,
            context.build_type,
            connectivity_config.mdns.publish_fuchsia_dev_wired_service,
        ) {
            // FFX discovery is not enabled on bootstrap, therefore we do not need the wired
            // udp service.
            (FeatureSetLevel::Bootstrap, _, _) => false,
            (FeatureSetLevel::Embeddable, _, _) => false,

            // User builds cannot have this service enabled.
            (_, BuildType::User, None) => false,
            (_, BuildType::User, Some(true)) => {
                bail!("A MDNS wired udp service cannot be enabled on user builds")
            }
            // Userdebug and eng builds have this service enabled by default.
            (_, _, None) => true,
            // The product can override the default only on userdebug and eng builds.
            (_, _, Some(b)) => b,
        };
        if publish_fuchsia_dev_wired_service {
            builder.platform_bundle("mdns_fuchsia_device_wired_service")?;
        }
        if let Some(mdns_config) = &connectivity_config.mdns.config {
            builder.package("mdns").config_data(FileEntry {
                source: mdns_config.clone(),
                destination: "assembly.config".into(),
            })?;
        }

        // The configuration of networking is dependent on all three of:
        // - the feature_set_level
        // - the build_type
        // - the requested configuration type
        let networking = match (
            context.feature_set_level,
            context.build_type,
            &connectivity_config.network.networking,
        ) {
            // bootstrap must not attempt to configure it, it's always None
            (FeatureSetLevel::Bootstrap, _, Some(_)) => {
                bail!("The configuration of networking is not an option for `bootstrap`")
            }
            (FeatureSetLevel::Bootstrap, _, None) => None,
            (FeatureSetLevel::Embeddable, _, _) => None,

            // utility, in user mode, only gets networking if requested.
            (FeatureSetLevel::Utility, BuildType::User, networking) => networking.as_ref(),

            // all other combinations get the network package that they request
            (_, _, Some(networking)) => Some(networking),

            // otherwise, the 'standard' networking package is used
            (_, _, None) => Some(&NetworkingConfig::Standard),
        };
        if let Some(networking) = networking {
            let maybe_gub_bundle = |bundle| {
                if connectivity_config.network.use_unified_binary {
                    format!("{bundle}_gub").into()
                } else {
                    std::borrow::Cow::Borrowed(bundle)
                }
            };

            // The 'core_realm_networking' and 'network_realm' bundles are
            // required if networking is enabled.
            builder.platform_bundle("core_realm_networking")?;
            builder.platform_bundle("network_realm")?;
            builder.platform_bundle(maybe_gub_bundle("network_realm_packages").as_ref())?;

            // Which specific network package is selectable by the product.
            match networking {
                NetworkingConfig::Standard => {
                    builder.platform_bundle("networking_with_virtualization")?;
                }
                NetworkingConfig::Basic => {
                    builder.platform_bundle("networking_basic")?;
                    builder
                        .platform_bundle(maybe_gub_bundle("networking_basic_packages").as_ref())?;
                }
            }

            let config_src = connectivity_config
                .network
                .netcfg_config_path
                .clone()
                .unwrap_or_else(|| context.get_resource("netcfg_default.json5"));
            let config_dir = builder
                .add_domain_config(PackageSetDestination::Blob(PackageDestination::NetcfgConfig))
                .directory("netcfg-config");
            config_dir.entry(FileEntry {
                source: config_src,
                destination: "netcfg_default.json5".into(),
            })?;

            if let Some(netstack_config_path) = &connectivity_config.network.netstack_config_path {
                builder.package("netstack").config_data(FileEntry {
                    source: netstack_config_path.clone(),
                    destination: "default.json".into(),
                })?;
            }

            if let Some(google_maps_api_key_path) =
                &connectivity_config.network.google_maps_api_key_path
            {
                builder.package("emergency").config_data(FileEntry {
                    source: google_maps_api_key_path.clone(),
                    destination: "google_maps_api_key.txt".into(),
                })?;
            }

            if let Some(timeout) =
                connectivity_config.network.http_client_stop_on_idle_timeout_millis
            {
                builder.set_config_capability(
                    "fuchsia.http-client.StopOnIdleTimeoutMillis",
                    Config::new(ConfigValueType::Int64, timeout.into()),
                )?;
            } else {
                builder.set_config_capability(
                    "fuchsia.http-client.StopOnIdleTimeoutMillis",
                    Config::new_void(),
                )?;
            }

            if let Some(receive_buffer_size) =
                connectivity_config.network.http_client_tcp_receive_buffer_size
            {
                if context.build_type == &BuildType::User {
                    bail!(
                        "'http_client_tcp_receive_buffer_size' cannot be overridden on User builds"
                    );
                }
                builder.set_config_capability(
                    "fuchsia.http-client.TcpReceiveBufferSizeBytes",
                    Config::new(ConfigValueType::Uint64, receive_buffer_size.get().into()),
                )?;
            } else {
                builder.set_config_capability(
                    "fuchsia.http-client.TcpReceiveBufferSizeBytes",
                    Config::new_void(),
                )?;
            }

            // The use of netstack3 can be forcibly required by the board,
            // otherwise it's selectable by the product.
            match (
                context.board_config.provides_feature(BoardFeature::NetworkRequireNetstack3),
                connectivity_config.network.netstack_version,
            ) {
                (true, _) | (false, NetstackVersion::Netstack3) => {
                    builder.platform_bundle("netstack3")?;
                    builder.platform_bundle(maybe_gub_bundle("netstack3_packages").as_ref())?;
                }
                (false, NetstackVersion::Netstack2) => {
                    if connectivity_config.network.max_rolling_capture_buffer_size.is_some() {
                        anyhow::bail!(
                            "max_rolling_capture_buffer_size only affects Netstack3, \
                             but Netstack2 was selected"
                        );
                    }
                    if connectivity_config.network.netstack_thread_count.is_some() {
                        anyhow::bail!(
                            "netstack_thread_count only affects Netstack3, \
                             but Netstack2 was selected"
                        );
                    }

                    builder.platform_bundle("netstack2")?;
                }
                (false, NetstackVersion::NetstackMigration) => {
                    builder.platform_bundle("netstack_migration")?;
                    builder.platform_bundle(
                        maybe_gub_bundle("netstack_migration_packages").as_ref(),
                    )?;
                }
            }

            // Define netstack3 structured configuration keys.
            //
            // It must be set twice, because netstack3 is both in the
            // netstack-migration package and the netstack3 package.
            for (package, component) in [
                ("netstack3", "meta/netstack3.cm"),
                ("netstack-migration", "meta/netstack-proxy.cm"),
            ] {
                builder
                    .package(package)
                    .component(component)?
                    .field(
                        "num_threads",
                        connectivity_config.network.netstack_thread_count.unwrap_or_default().get(),
                    )?
                    .field("debug_logs", false)?
                    .field("opaque_iids", true)?
                    .field(
                        "sampled_stats_enabled",
                        matches!(context.build_type, BuildType::Eng | BuildType::UserDebug),
                    )?
                    .field("multi_vmo", connectivity_config.network.netstack_multi_vmo)?
                    .field(
                        "max_rolling_capture_buffer_size",
                        connectivity_config
                            .network
                            .max_rolling_capture_buffer_size
                            .unwrap_or(16777216),
                    )?
                    // Routed from fuchsia.power.SuspendEnabled capability.
                    //
                    // TODO(https://fxbug.dev/368386068): This should not be
                    // necessary once we teach structured config and routed
                    // capabilities to coexist peacefully.
                    .field("suspend_enabled", false)?;
            }

            let has_fullmac = context.board_config.provides_feature(BoardFeature::WlanFullmac);
            let has_softmac = context.board_config.provides_feature(BoardFeature::WlanSoftmac);
            let has_virtio_hwsim =
                context.board_config.provides_feature(BoardFeature::Paravirtualization)
                    && has_softmac;
            if has_fullmac || has_softmac || has_virtio_hwsim {
                // Select the policy layer
                match connectivity_config.wlan.policy_layer {
                    WlanPolicyLayer::Platform => {
                        builder.platform_bundle("wlan_policy")?;

                        // Add in the recovery characteristics specified by the product config.
                        let recovery_profile = match connectivity_config.wlan.recovery_profile {
                            None => String::from(""),
                            Some(WlanRecoveryProfile::ThresholdedRecovery) => {
                                String::from("thresholded_recovery")
                            }
                        };
                        builder.set_config_capability(
                            "fuchsia.wlan.RecoveryEnabled",
                            Config::new(
                                ConfigValueType::Bool,
                                connectivity_config.wlan.recovery_enabled.into(),
                            ),
                        )?;
                        builder.set_config_capability(
                            "fuchsia.wlan.RecoveryProfile",
                            Config::new(
                                ConfigValueType::String { max_size: 512 },
                                recovery_profile.into(),
                            ),
                        )?;

                        let roaming_policy_str = match connectivity_config.wlan.roaming_policy {
                            // All roaming related behavior is disabled
                            WlanRoamingPolicy::Disabled => String::from("disabled"),
                            // Roaming fully enabled, based on the stationary profile implementation.
                            WlanRoamingPolicy::Enabled {
                                profile: WlanRoamingProfile::Stationary,
                                mode: WlanRoamingMode::CanRoam,
                            } => String::from("enabled_stationary_can_roam"),
                            // Only metrics are logged from the stationary profile implementation.
                            // Roam scanning can occur, but roams are not executed.
                            WlanRoamingPolicy::Enabled {
                                profile: WlanRoamingProfile::Stationary,
                                mode: WlanRoamingMode::MetricsOnly,
                            } => String::from("enabled_stationary_metrics_only"),
                        };
                        builder.set_config_capability(
                            "fuchsia.wlan.RoamingPolicy",
                            Config::new(
                                ConfigValueType::String { max_size: 512 },
                                roaming_policy_str.into(),
                            ),
                        )?;
                    }
                    WlanPolicyLayer::ViaWlanix => {
                        builder.platform_bundle("wlan_wlanix")?;

                        // Ensure we don't have invalid recovery settings
                        if connectivity_config.wlan.recovery_profile.is_some() {
                            bail!(
                                "wlan.recovery_profile is invalid with wlan.policy_layer ViaWlanix"
                            )
                        }
                        if connectivity_config.wlan.recovery_enabled {
                            bail!(
                                "wlan.recovery_enabled is invalid with wlan.policy_layer ViaWlanix"
                            )
                        }

                        // Ensure we don't have invalid roaming settings
                        if let WlanRoamingPolicy::Enabled { .. } =
                            connectivity_config.wlan.roaming_policy
                        {
                            bail!("wlan.roaming_policy is invalid with wlan.policy_layer ViaWlanix")
                        }
                    }
                    WlanPolicyLayer::None => {
                        // Ensure we only exclude a policy layer on an Eng build.
                        context.ensure_build_type_and_feature_set_level(
                            &[BuildType::Eng],
                            &[FeatureSetLevel::Standard],
                            "WLAN Policy None",
                        )?;

                        // Ensure we don't have invalid recovery settings
                        if connectivity_config.wlan.recovery_profile.is_some() {
                            bail!("wlan.recovery_profile is invalid without a wlan.policy_layer")
                        }
                        if connectivity_config.wlan.recovery_enabled {
                            bail!("wlan.recovery_enabled is invalid without a wlan.policy_layer")
                        }

                        // Ensure we don't have invalid roaming settings
                        if let WlanRoamingPolicy::Enabled { .. } =
                            connectivity_config.wlan.roaming_policy
                        {
                            bail!("wlan.roaming_policy is invalid without a wlan.policy_layer")
                        }
                    }
                }

                // Add development support on eng and userdebug systems if we have wlan.
                match context.build_type {
                    BuildType::Eng | BuildType::UserDebug => {
                        builder.platform_bundle("wlan_development")?
                    }
                    _ => {}
                }

                // Some products require legacy security types to be supported.
                // Otherwise, they are disabled by default.
                if connectivity_config.wlan.legacy_privacy_support {
                    builder.platform_bundle("wlan_legacy_privacy_support")?;
                } else {
                    builder.platform_bundle("wlan_contemporary_privacy_only_support")?;
                }

                if has_fullmac {
                    builder.platform_bundle("wlan_fullmac_support")?;
                }
                if has_softmac {
                    builder.platform_bundle("wlan_softmac_support")?;
                }
                if has_virtio_hwsim && context.build_type == &BuildType::Eng {
                    builder.platform_bundle("wlan_virtio_hwsim_driver")?;
                }
            }

            if connectivity_config.network.include_tun {
                builder.platform_bundle("network_tun")?;
            }

            if connectivity_config.thread.include_lowpan {
                builder.platform_bundle("thread_lowpan")?;
            }

            if connectivity_config.netpol.include_socket_proxy {
                builder.platform_bundle("socket-proxy-enabled")?;
                builder.platform_bundle("socket_proxy_packages")?;
            } else {
                builder.platform_bundle("socket-proxy-disabled")?;
            }
        }

        // Add the weave core shard if necessary.
        if let Some(url) = &connectivity_config.weave.component_url {
            util::add_platform_declared_product_provided_component(
                url,
                "weavestack.core_shard.cml.template",
                context,
                builder,
            )?;
        }

        if let Some(netsvc_interface) =
            &context.board_config.platform.connectivity.network.netsvc_interface
        {
            builder.set_config_capability(
                "fuchsia.network.PrimaryInterface",
                Config::new(
                    ConfigValueType::String { max_size: 200 },
                    netsvc_interface.clone().into(),
                ),
            )?;
        } else {
            builder
                .set_config_capability("fuchsia.network.PrimaryInterface", Config::new_void())?;
        }

        if let Some(boot_item_iface) =
            &context.board_config.platform.development_support.boot_item_interface_for_node_name
        {
            builder.set_config_capability(
                "fuchsia.device.BootItemInterfaceForNodeName",
                Config::new(ConfigValueType::Uint8, (*boot_item_iface).into()),
            )?;
        } else {
            builder.set_config_capability(
                "fuchsia.device.BootItemInterfaceForNodeName",
                Config::new_void(),
            )?;
        }

        builder.set_config_capability(
            "fuchsia.device.BootItemInterfaceFlipLowMacBit",
            Config::new(
                ConfigValueType::Bool,
                context
                    .board_config
                    .platform
                    .development_support
                    .boot_item_interface_flip_low_mac_bit
                    .into(),
            ),
        )?;

        if connectivity_config.location.enable_emergency_location_provider {
            ensure!(
                *context.feature_set_level == FeatureSetLevel::Standard,
                "Location services can only be enabled in the 'standard' feature set level."
            );
            builder.platform_bundle("location_emergency")?;
        }

        // Include realtek-8211f driver through a platform AIB.
        if context.board_config.provides_feature(BoardFeature::Realtek8211f) {
            // We only need this driver feature in the utility / standard feature set levels.
            builder.platform_bundle("realtek_8211f_driver")?;
        }

        // Include GNSS service through a platform AIB.
        if context.board_config.provides_feature(BoardFeature::Gnss) {
            builder.platform_bundle("gnss")?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subsystems::{ConfigurationBuilderImpl, ConfigurationContext, FeatureSetLevel};
    use assembly_config_schema::BuildType;
    use assembly_config_schema::platform_settings::connectivity_config::PlatformNetworkConfig;
    use std::num::NonZeroU64;

    #[test]
    fn test_http_client_tcp_receive_buffer_size_rejected_on_user() {
        let context = ConfigurationContext {
            feature_set_level: &FeatureSetLevel::Standard,
            build_type: &BuildType::User,
            ..ConfigurationContext::default_for_tests()
        };
        let config = PlatformConnectivityConfig {
            network: PlatformNetworkConfig {
                http_client_tcp_receive_buffer_size: NonZeroU64::new(2097152),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut builder = ConfigurationBuilderImpl::default();
        let result =
            ConnectivitySubsystemConfig::define_configuration(&context, &config, &mut builder);
        assert!(result.is_err());
        let error_message = result.unwrap_err().to_string();
        assert!(
            error_message.contains(
                "'http_client_tcp_receive_buffer_size' cannot be overridden on User builds"
            )
        );
    }
}
