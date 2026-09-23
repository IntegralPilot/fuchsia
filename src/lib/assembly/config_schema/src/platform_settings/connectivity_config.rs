// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::num::NonZeroU8;

use assembly_container::WalkPaths;
use camino::Utf8PathBuf;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::num::NonZeroU64;

/// Platform configuration options for the connectivity area.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema, WalkPaths)]
#[serde(default, deny_unknown_fields)]
pub struct PlatformConnectivityConfig {
    #[walk_paths]
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub network: PlatformNetworkConfig,

    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub wlan: PlatformWlanConfig,

    #[walk_paths]
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub mdns: MdnsConfig,

    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub thread: ThreadConfig,

    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub weave: WeaveConfig,

    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub netpol: NetpolConfig,

    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub location: LocationConfig,
}

/// Platform configuration options for the network area.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema, WalkPaths)]
#[serde(default, deny_unknown_fields)]
pub struct PlatformNetworkConfig {
    /// Only used to control networking for the `utility` and `minimal`
    /// feature_set_levels.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub networking: Option<NetworkingConfig>,

    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub netstack_version: NetstackVersion,

    #[walk_paths]
    #[schemars(schema_with = "crate::option_path_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub netcfg_config_path: Option<Utf8PathBuf>,

    #[walk_paths]
    #[schemars(schema_with = "crate::option_path_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub netstack_config_path: Option<Utf8PathBuf>,

    /// Number of threads used by Netstack3.
    ///
    /// Platform default will be used if unspecified.
    ///
    /// NOTE: An error will be thrown if this is set and Netstack2 is selected
    /// as the system netstack.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub netstack_thread_count: Option<NetstackThreadCount>,

    #[walk_paths]
    #[schemars(schema_with = "crate::option_path_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub google_maps_api_key_path: Option<Utf8PathBuf>,

    /// Controls how long the http client will wait when it is idle before it
    /// escrows its FIDL connections back to the component framework and exits.
    /// If the value is negative or left out, then the http client will not
    /// stop after it idles.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_client_stop_on_idle_timeout_millis: Option<i64>,

    /// Controls the TCP receive buffer size (SO_RCVBUF) in bytes for HTTP client sockets.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_client_tcp_receive_buffer_size: Option<NonZeroU64>,

    /// Controls whether the unified binary for networking should be used.
    ///
    /// The unified binary provides space savings for space-constrained
    /// products, trading off multiple small binaries for one large binary that
    /// is smaller than the sum of its separate parts thanks to linking
    /// optimizations.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub use_unified_binary: bool,

    /// Whether to include network-tun.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub include_tun: bool,

    /// Whether to enable multi-VMO data path for network device sessions in
    /// Netstack3.
    // TODO(https://fxbug.dev/545307151): Remove this flag.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub netstack_multi_vmo: bool,

    /// Maximum size of the rolling packet capture buffer in bytes.
    ///
    /// Platform default (16MB) will be used if unspecified.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_rolling_capture_buffer_size: Option<u32>,
}

#[derive(Debug, Serialize, Copy, Clone, Deserialize, JsonSchema, PartialEq)]
#[serde(try_from = "u8")]
pub struct NetstackThreadCount(NonZeroU8);

impl TryFrom<u8> for NetstackThreadCount {
    type Error = &'static str;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        NonZeroU8::new(value)
            .map(NetstackThreadCount)
            .ok_or("netstack thread count must be nonzero")
    }
}

impl Default for NetstackThreadCount {
    fn default() -> Self {
        Self(NonZeroU8::new(4).unwrap())
    }
}

impl NetstackThreadCount {
    pub fn get(&self) -> u8 {
        let Self(v) = self;
        v.get()
    }
}

/// Network stack version to use.
#[derive(Debug, Default, Copy, Clone, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NetstackVersion {
    Netstack2,
    #[default]
    Netstack3,
    NetstackMigration,
}

/// Which networking type to use (standard or basic).
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum NetworkingConfig {
    /// The standard network configuration
    #[default]
    Standard,

    /// A more-basic networking configuration for constrained devices
    Basic,
}

/// Platform configuration options for the wlan area.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PlatformWlanConfig {
    /// Enable the use of legacy security types like WEP and/or WPA1
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub legacy_privacy_support: bool,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery_profile: Option<WlanRecoveryProfile>,

    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub recovery_enabled: bool,

    /// Defines roaming behavior for device.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub roaming_policy: WlanRoamingPolicy,

    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub policy_layer: WlanPolicyLayer,
}

#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WlanPolicyLayer {
    /// Use the Fuchsia platform's built-in policy layer, `wlancfg`, to configure and manage the
    /// Fuchsia WLAN stack.
    #[default]
    Platform,
    /// Use an external policy layer, which uses `wlanix` to communicate with the Fuchsia WLAN
    /// stack.
    ViaWlanix,
    /// Do not include a policy layer. Intended for testing only and only valid on Eng
    /// products.
    None,
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WlanRecoveryProfile {
    ThresholdedRecovery,
}

// LINT.IfChange
// Configures whether any roaming behavior is enabled.
#[derive(Copy, Clone, Default, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WlanRoamingPolicy {
    #[default]
    Disabled,
    Enabled {
        #[serde(default)]
        #[serde(skip_serializing_if = "crate::common::is_default")]
        profile: WlanRoamingProfile,

        #[serde(default)]
        #[serde(skip_serializing_if = "crate::common::is_default")]
        mode: WlanRoamingMode,
    },
}

// Configures implementation details for roaming (e.g. what data is considered,
// thresholds, maximum scan frequency, etc.).
#[derive(Copy, Clone, Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WlanRoamingProfile {
    #[default]
    Stationary,
}

// Configures what roaming behavior is allowed for enabled platform roaming. Defaults to
// 'CanRoam', which does not restrict any roaming behavior in the profile.
#[derive(Copy, Clone, Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WlanRoamingMode {
    MetricsOnly,
    #[default]
    CanRoam,
}
// LINT.ThenChange(//src/connectivity/wlan/wlancfg/src/client/roaming/lib.rs)

/// Platform configuration options to use for the mdns area.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema, WalkPaths)]
#[serde(deny_unknown_fields)]
pub struct MdnsConfig {
    /// Enable a wired service so that ffx can discover the device.
    /// If nothing is set, a reasonable default is chosen based on the build type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub publish_fuchsia_dev_wired_service: Option<bool>,

    /// Service config file.
    #[walk_paths]
    #[schemars(schema_with = "crate::option_path_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<Utf8PathBuf>,
}

/// Platform configuration options to use for the thread area.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ThreadConfig {
    /// Include the LoWPAN service.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub include_lowpan: bool,
}

/// Platform configuration options to use for the weave area.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct WeaveConfig {
    /// The URL of the weave component.
    ///   e.g. fuchsia-pkg://fuchsia.com/weavestack#meta/weavestack.cm
    #[serde(skip_serializing_if = "Option::is_none")]
    pub component_url: Option<String>,
}

/// Platform configuration options to use for the Network Policy area.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct NetpolConfig {
    /// Whether to include network-socket-proxy.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub include_socket_proxy: bool,
}

/// Platform configuration options for the location services
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct LocationConfig {
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub enable_emergency_location_provider: bool,
}
