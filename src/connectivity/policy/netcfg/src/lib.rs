// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![deny(unused)]

mod devices;
mod dhcpv4;
mod dhcpv6;
mod dns;
mod errors;
mod filter;
mod interface;
mod masquerade;
pub mod network;
pub mod telemetry;
mod virtualization;

use ::dhcpv4::protocol::FromFidlExt as _;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::fmt::{self, Display};
use std::num::NonZeroU64;
use std::pin::{Pin, pin};
use std::str::FromStr;
use std::{fs, io, path};

use fidl::endpoints::{ProtocolMarker, RequestStream as _, Responder as _};
use fidl_fuchsia_net_ext::{self as fnet_ext, DisplayExt as _, IpExt as _};
use fidl_fuchsia_net_interfaces_ext::{self as fnet_interfaces_ext, Update as _};
use fuchsia_component::client::{clone_namespace_svc, new_protocol_connector_in_dir};
use fuchsia_component::server::{ServiceFs, ServiceFsDir};

use fidl_fuchsia_io as fio;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_dhcp as fnet_dhcp;
use fidl_fuchsia_net_dhcp_ext as fnet_dhcp_ext;
use fidl_fuchsia_net_dhcpv6 as fnet_dhcpv6;
use fidl_fuchsia_net_filter as fnet_filter;
use fidl_fuchsia_net_filter_deprecated as fnet_filter_deprecated;
use fidl_fuchsia_net_interfaces as fnet_interfaces;
use fidl_fuchsia_net_interfaces_admin as fnet_interfaces_admin;
use fidl_fuchsia_net_masquerade as fnet_masquerade;
use fidl_fuchsia_net_matchers_ext as fnet_matchers_ext;
use fidl_fuchsia_net_name as fnet_name;
use fidl_fuchsia_net_ndp as fnet_ndp;
use fidl_fuchsia_net_policy_properties as fnp_properties;
use fidl_fuchsia_net_policy_socketproxy as fnp_socketproxy;
use fidl_fuchsia_net_reachability as freachability;
use fidl_fuchsia_net_resources as fnet_resources;
use fidl_fuchsia_net_routes_admin as fnet_routes_admin;
use fidl_fuchsia_net_routes_ext as fnet_routes_ext;
use fidl_fuchsia_net_stack as fnet_stack;
use fidl_fuchsia_net_virtualization as fnet_virtualization;
use fuchsia_async as fasync;

use anyhow::{Context as _, anyhow, format_err};
use assert_matches::assert_matches;
use async_trait::async_trait;
use async_utils::stream::WithTag as _;
use dns_server_watcher::{DEFAULT_DNS_PORT, DnsServers, DnsServersUpdateSource};
use futures::stream::BoxStream;
use futures::{FutureExt, StreamExt as _, TryFutureExt as _, TryStreamExt as _};
use log::{debug, error, info, trace, warn};
use net_declare::fidl_ip_v4;
use net_declare::net::prefix_length_v4;
use net_types::ip::{IpAddress as _, Ipv4, Ipv6, PrefixLength};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use self::devices::DeviceInfo;
use self::errors::{ContextExt as _, accept_error};
use self::filter::{FilterControl, FilterEnabledState};
use self::interface::{
    DeviceInfoRef, InterfaceNamingIdentifier, NetstackManagedRoutesDesignation, ProvisioningAction,
    ProvisioningType,
};
use self::masquerade::MasqueradeHandler;

/// Interface Identifier
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct InterfaceId(NonZeroU64);

impl InterfaceId {
    const fn new(raw: u64) -> Option<Self> {
        // Note: use a match here because `Option::map` is non-const.
        match NonZeroU64::new(raw) {
            Some(id) => Some(InterfaceId(id)),
            None => None,
        }
    }

    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

impl Display for InterfaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fnet_interfaces_ext::TryFromMaybeNonZero for InterfaceId {
    fn try_from(value: u64) -> Result<Self, fnet_interfaces_ext::ZeroError> {
        Self::new(value).ok_or(fnet_interfaces_ext::ZeroError {})
    }
}

impl TryFrom<u64> for InterfaceId {
    type Error = std::num::TryFromIntError;
    fn try_from(val: u64) -> Result<Self, Self::Error> {
        Ok(Self(NonZeroU64::try_from(val)?))
    }
}

impl TryFrom<u32> for InterfaceId {
    type Error = std::num::TryFromIntError;
    fn try_from(val: u32) -> Result<Self, Self::Error> {
        Ok(Self(NonZeroU64::try_from(val as u64)?))
    }
}

impl From<NonZeroU64> for InterfaceId {
    fn from(val: NonZeroU64) -> Self {
        Self(val)
    }
}

impl From<InterfaceId> for NonZeroU64 {
    fn from(InterfaceId(val): InterfaceId) -> Self {
        val
    }
}

impl From<InterfaceId> for u64 {
    fn from(val: InterfaceId) -> Self {
        val.get()
    }
}

/// Interface metrics.
///
/// Interface metrics are used to sort the route table. An interface with a
/// lower metric is favored over one with a higher metric.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct Metric(u32);

impl Default for Metric {
    // A default value of 600 is chosen for Metric: this provides plenty of space for routing
    // policy on devices with many interfaces (physical or logical) while remaining in the same
    // magnitude as our current default ethernet metric.
    fn default() -> Self {
        Self(600)
    }
}

impl std::fmt::Display for Metric {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Metric(u) = self;
        write!(f, "{}", u)
    }
}

impl From<Metric> for u32 {
    fn from(Metric(u): Metric) -> u32 {
        u
    }
}

/// The prefix length for the address assigned to a WLAN AP interface.
const WLAN_AP_PREFIX_LEN: PrefixLength<Ipv4> = prefix_length_v4!(29);

/// The address for the network the WLAN AP interface is a part of.
const WLAN_AP_NETWORK_ADDR: fnet::Ipv4Address = fidl_ip_v4!("192.168.255.248");

/// The lease time for a DHCP lease.
///
/// 1 day in seconds.
const WLAN_AP_DHCP_LEASE_TIME_SECONDS: u32 = 24 * 60 * 60;

/// The DnsServerWatcher yields a collection of information used in recording available DNS servers.
/// The DnsServerUpdate consolidates all of that information.
#[derive(Debug)]
pub(crate) struct DnsServerUpdate {
    pub(crate) source: DnsServersUpdateSource,
    pub(crate) payload: DnsWatcherResultPayload,
    pub(crate) lifetime: DnsServerLifetime,
    pub(crate) result: Result<Vec<fnet_name::DnsServer_>, fidl::Error>,
}

/// A map of DNS server watcher streams that yields `DnsServerWatcherEvent` as DNS
/// server updates become available.
///
/// DNS server watcher streams may be added or removed at runtime as the watchers
/// are started or stopped.
type DnsServerWatchers<'a> =
    async_utils::stream::StreamMap<DnsServersUpdateSource, BoxStream<'a, DnsServerUpdate>>;

/// Defines log levels.
#[derive(Debug, Copy, Clone)]
pub struct LogLevel(diagnostics_log::Severity);

impl Default for LogLevel {
    fn default() -> Self {
        Self(diagnostics_log::Severity::Info)
    }
}

impl FromStr for LogLevel {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, anyhow::Error> {
        match s.to_uppercase().as_str() {
            "TRACE" => Ok(Self(diagnostics_log::Severity::Trace)),
            "DEBUG" => Ok(Self(diagnostics_log::Severity::Debug)),
            "INFO" => Ok(Self(diagnostics_log::Severity::Info)),
            "WARN" => Ok(Self(diagnostics_log::Severity::Warn)),
            "ERROR" => Ok(Self(diagnostics_log::Severity::Error)),
            "FATAL" => Ok(Self(diagnostics_log::Severity::Fatal)),
            _ => Err(anyhow::anyhow!("unrecognized log level = {}", s)),
        }
    }
}

/// Network Configuration tool.
///
/// Configures network components in response to events.
#[derive(argh::FromArgs, Debug)]
struct Opt {
    /// minimum severity for logs
    #[argh(option, default = "Default::default()")]
    min_severity: LogLevel,

    /// config file to use
    #[argh(option, default = "\"/netcfg-config/netcfg_default.json5\".to_string()")]
    config_data: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DnsConfig {
    pub servers: Vec<std::net::IpAddr>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilterConfig {
    pub rules: Vec<String>,
    pub nat_rules: Vec<String>,
    pub rdr_rules: Vec<String>,
}

#[derive(Debug, PartialEq, Eq, Hash, Copy, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "lowercase")]
pub enum InterfaceType {
    Ethernet,
    // NB: Serde alias provides backwards compatibility.
    #[serde(alias = "wlan")]
    WlanClient,
    // NB: Serde alias provides backwards compatibility.
    #[serde(alias = "ap")]
    WlanAp,
    Lowpan,
    Blackhole,
}

impl TryFrom<fidl_fuchsia_hardware_network::PortClass> for InterfaceType {
    type Error = UnknownPortClassError;
    fn try_from(port_class: fidl_fuchsia_hardware_network::PortClass) -> Result<Self, Self::Error> {
        let device_class = DeviceClass::try_from(port_class)?;
        Ok(device_class.into())
    }
}

impl From<DeviceClass> for InterfaceType {
    fn from(device_class: DeviceClass) -> Self {
        match device_class {
            DeviceClass::WlanClient => InterfaceType::WlanClient,
            DeviceClass::Blackhole => InterfaceType::Blackhole,
            DeviceClass::WlanAp => InterfaceType::WlanAp,
            DeviceClass::Ethernet
            | DeviceClass::Virtual
            | DeviceClass::Ppp
            | DeviceClass::Bridge => InterfaceType::Ethernet,
            DeviceClass::Lowpan => InterfaceType::Lowpan,
        }
    }
}

impl From<DeviceClass> for fnp_socketproxy::NetworkType {
    fn from(device_class: DeviceClass) -> Self {
        match device_class {
            DeviceClass::WlanClient | DeviceClass::WlanAp => fnp_socketproxy::NetworkType::Wifi,
            DeviceClass::Ethernet
            | DeviceClass::Bridge
            | DeviceClass::Virtual
            | DeviceClass::Lowpan => fnp_socketproxy::NetworkType::Ethernet,
            DeviceClass::Ppp | DeviceClass::Blackhole => fnp_socketproxy::NetworkType::Unknown,
        }
    }
}

#[cfg_attr(test, derive(PartialEq))]
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceMetrics {
    #[serde(default)]
    pub wlan_metric: Metric,
    #[serde(default)]
    pub eth_metric: Metric,
    #[serde(default)]
    pub blackhole_metric: Metric,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "lowercase")]
// LINT.IfChange(device_class_enum_tefmo)
pub enum DeviceClass {
    Virtual,
    Ethernet,
    // NB: Serde alias provides backwards compatibility.
    #[serde(alias = "wlan")]
    WlanClient,
    Ppp,
    Bridge,
    WlanAp,
    Lowpan,
    Blackhole,
}
// LINT.ThenChange(//tools/testing/tefmocheck/cdc_ethernet_state_check.go:device_class_enum_tefmo)

#[derive(Debug, Error)]
#[error("unknown port class with ordinal: {unknown_ordinal}")]
pub struct UnknownPortClassError {
    unknown_ordinal: u16,
}

impl TryFrom<fidl_fuchsia_hardware_network::PortClass> for DeviceClass {
    type Error = UnknownPortClassError;
    fn try_from(port_class: fidl_fuchsia_hardware_network::PortClass) -> Result<Self, Self::Error> {
        match port_class {
            fidl_fuchsia_hardware_network::PortClass::Virtual => Ok(DeviceClass::Virtual),
            fidl_fuchsia_hardware_network::PortClass::Ethernet => Ok(DeviceClass::Ethernet),
            fidl_fuchsia_hardware_network::PortClass::WlanClient => Ok(DeviceClass::WlanClient),
            fidl_fuchsia_hardware_network::PortClass::Ppp => Ok(DeviceClass::Ppp),
            fidl_fuchsia_hardware_network::PortClass::Bridge => Ok(DeviceClass::Bridge),
            fidl_fuchsia_hardware_network::PortClass::WlanAp => Ok(DeviceClass::WlanAp),
            fidl_fuchsia_hardware_network::PortClass::Lowpan => Ok(DeviceClass::Lowpan),
            fidl_fuchsia_hardware_network::PortClass::__SourceBreaking { unknown_ordinal } => {
                Err(UnknownPortClassError { unknown_ordinal })
            }
        }
    }
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(transparent)]
struct AllowedDeviceClasses(HashSet<DeviceClass>);

impl Default for AllowedDeviceClasses {
    fn default() -> Self {
        // When new variants are added, this exhaustive match will cause a compilation failure as a
        // reminder to add the new variant to the default array.
        match DeviceClass::Virtual {
            DeviceClass::Virtual
            | DeviceClass::Ethernet
            | DeviceClass::WlanClient
            | DeviceClass::Ppp
            | DeviceClass::Bridge
            | DeviceClass::WlanAp
            | DeviceClass::Lowpan
            | DeviceClass::Blackhole => {}
        }
        Self(HashSet::from([
            DeviceClass::Virtual,
            DeviceClass::Ethernet,
            DeviceClass::WlanClient,
            DeviceClass::Ppp,
            DeviceClass::Bridge,
            DeviceClass::WlanAp,
            DeviceClass::Lowpan,
            DeviceClass::Blackhole,
        ]))
    }
}

// TODO(https://github.com/serde-rs/serde/issues/368): use an inline literal for the default value
// rather than defining a one-off function.
fn dhcpv6_enabled_default() -> bool {
    true
}

#[derive(Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "lowercase")]
struct ForwardedDeviceClasses {
    #[serde(default)]
    pub ipv4: HashSet<DeviceClass>,
    #[serde(default)]
    pub ipv6: HashSet<DeviceClass>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    pub dns_config: DnsConfig,
    pub filter_config: FilterConfig,
    pub filter_enabled_interface_types: HashSet<InterfaceType>,
    // NB: `InterfaceMetrics` custom default behavior sets all included
    // interface types to the same metric. It is encouraged to set at least one
    // metric for predictable packet routing on a multinetworked system.
    #[serde(default)]
    pub interface_metrics: InterfaceMetrics,
    // NB: `AllowedDeviceClasses` custom default behavior is permissive. To
    // permit a subset of device classes, specify this in configuration.
    #[serde(default)]
    pub allowed_upstream_device_classes: AllowedDeviceClasses,
    // NB: See above.
    #[serde(default)]
    pub allowed_bridge_upstream_device_classes: AllowedDeviceClasses,
    // TODO(https://fxbug.dev/42173732): default to false.
    #[serde(default = "dhcpv6_enabled_default")]
    pub enable_dhcpv6: bool,
    // NB: `ForwardedDeviceClasses` custom default behavior is restrictive. To
    // permit a subset of device classes, specify this in configuration.
    #[serde(default)]
    pub forwarded_device_classes: ForwardedDeviceClasses,
    #[serde(default)]
    pub interface_naming_policy: Vec<interface::NamingRule>,
    #[serde(default)]
    pub interface_provisioning_policy: Vec<interface::ProvisioningRule>,
    /// The names of blackhole interfaces to install.
    ///
    /// The interfaces are named exactly as specified in the configuration, and are not under
    /// the influence of `interface_naming_policy`.
    #[serde(default)]
    pub blackhole_interfaces: Vec<String>,
    // Whether to use protocols provided by the socketproxy component.
    #[serde(default)]
    pub enable_socket_proxy: bool,
}

impl Config {
    pub fn load<P: AsRef<path::Path>>(path: P) -> Result<Self, anyhow::Error> {
        let path = path.as_ref();
        let file = fs::File::open(path)
            .with_context(|| format!("could not open the config file {}", path.display()))?;
        let config = serde_json5::from_reader(io::BufReader::new(file))
            .with_context(|| format!("could not deserialize the config file {}", path.display()))?;
        Ok(config)
    }
}

#[derive(Clone, Debug)]
struct InterfaceConfig {
    name: String,
    metric: u32,
    netstack_managed_routes_designation: Option<NetstackManagedRoutesDesignation>,
}

#[derive(Debug)]
struct InterfaceState {
    // Hold on to control to enforce interface ownership, even if unused.
    control: fidl_fuchsia_net_interfaces_ext::admin::Control,
    device_class: DeviceClass,
    config: InterfaceConfigState,
    provisioning: interface::ProvisioningType,
}

/// State for an interface.
#[derive(Debug)]
enum InterfaceConfigState {
    Host(HostInterfaceState),
    WlanAp(WlanApInterfaceState),
    Blackhole(BlackholeInterfaceState),
}

#[derive(Debug)]
enum Dhcpv4ClientState {
    NotRunning,
    Running(dhcpv4::ClientState),
    ScheduledRestart(Pin<Box<fasync::Timer>>),
}

#[derive(Debug)]
struct HostInterfaceState {
    dhcpv4_client: Dhcpv4ClientState,
    dhcpv6_client_state: Option<dhcpv6::ClientState>,
    // The PD configuration to use for the DHCPv6 client on this interface.
    dhcpv6_pd_config: Option<fnet_dhcpv6::PrefixDelegationConfig>,
    interface_admin_auth: fnet_resources::GrantForInterfaceAuthorization,
    interface_naming_id: interface::InterfaceNamingIdentifier,
}

#[derive(Debug)]
struct WlanApInterfaceState {
    interface_naming_id: interface::InterfaceNamingIdentifier,
}

#[derive(Debug)]
struct BlackholeInterfaceState;

impl InterfaceState {
    async fn new_host(
        interface_naming_id: interface::InterfaceNamingIdentifier,
        control: fidl_fuchsia_net_interfaces_ext::admin::Control,
        device_class: DeviceClass,
        dhcpv6_pd_config: Option<fnet_dhcpv6::PrefixDelegationConfig>,
        provisioning: interface::ProvisioningType,
    ) -> Result<Self, errors::Error> {
        let interface_admin_auth =
            control.get_authorization_for_interface().await.map_err(|e| {
                errors::Error::NonFatal(anyhow::anyhow!(
                    "error getting authorization for interface: {}",
                    e
                ))
            })?;
        Ok(Self {
            control,
            config: InterfaceConfigState::Host(HostInterfaceState {
                dhcpv4_client: Dhcpv4ClientState::NotRunning,
                dhcpv6_client_state: None,
                dhcpv6_pd_config,
                interface_admin_auth,
                interface_naming_id,
            }),
            device_class,
            provisioning,
        })
    }

    fn new_wlan_ap(
        interface_naming_id: interface::InterfaceNamingIdentifier,
        control: fidl_fuchsia_net_interfaces_ext::admin::Control,
        device_class: DeviceClass,
        provisioning: interface::ProvisioningType,
    ) -> Self {
        Self {
            control,
            device_class,
            config: InterfaceConfigState::WlanAp(WlanApInterfaceState { interface_naming_id }),
            provisioning,
        }
    }

    fn new_blackhole(
        control: fidl_fuchsia_net_interfaces_ext::admin::Control,
        provisioning: interface::ProvisioningType,
    ) -> Self {
        Self {
            control,
            device_class: DeviceClass::Blackhole,
            config: InterfaceConfigState::Blackhole(BlackholeInterfaceState),
            provisioning,
        }
    }

    fn is_wlan_ap(&self) -> bool {
        let Self { config, .. } = self;
        match config {
            InterfaceConfigState::Host(_) | InterfaceConfigState::Blackhole(_) => false,
            InterfaceConfigState::WlanAp(_) => true,
        }
    }

    fn interface_naming_id(&self) -> Option<&InterfaceNamingIdentifier> {
        match &self.config {
            InterfaceConfigState::Host(HostInterfaceState { interface_naming_id, .. })
            | InterfaceConfigState::WlanAp(WlanApInterfaceState { interface_naming_id, .. }) => {
                Some(interface_naming_id)
            }
            InterfaceConfigState::Blackhole(_) => None,
        }
    }

    /// Handles the interface being discovered.
    async fn on_discovery(
        &mut self,
        properties: &fnet_interfaces_ext::Properties<fnet_interfaces_ext::DefaultInterest>,
        dhcpv4_client_provider: Option<&fnet_dhcp::ClientProviderProxy>,
        dhcpv6_client_provider: Option<&fnet_dhcpv6::ClientProviderProxy>,
        dhcpv4_server: Option<&fnet_dhcp::Server_Proxy>,
        route_set_provider: &fnet_routes_admin::RouteTableV4Proxy,
        netpol_networks_service: &mut network::NetpolNetworksService,
        watchers: &mut DnsServerWatchers<'_>,
        dhcpv4_configuration_streams: &mut dhcpv4::ConfigurationStreamMap,
        dhcpv6_prefixes_streams: &mut dhcpv6::PrefixesStreamMap,
        enable_socket_proxy: bool,
    ) -> Result<(), errors::Error> {
        let Self { config, provisioning, device_class, .. } = self;
        let fnet_interfaces_ext::Properties {
            online,
            has_default_ipv4_route,
            has_default_ipv6_route,
            ..
        } = properties;

        // Netcfg won't handle interface update results for a delegated
        // interface.
        if !provisioning.track_in_network_registry() {
            return Ok(());
        }

        // Note: No discovery actions are needed for offline interfaces.
        if !online {
            return Ok(());
        }

        // TODO(https://fxbug.dev/498654191): Add Locally provisioned networks to
        // the networks service even when socket-proxy is absent.
        if enable_socket_proxy {
            // TODO(https://fxbug.dev/390709467): Involve Reachability state when
            // evaluating whether to add discovered interfaces with Internet to the
            // socketproxy.
            //
            // Verify that the interface has a v4 or v6 default route, as this
            // is a signal that there might be a higher layer of connectivity.
            if *has_default_ipv4_route || *has_default_ipv6_route {
                netpol_networks_service
                    .update(network::NetworkRegistryUpdate::ChangeNetwork(
                        network::NetworkId::fuchsia(properties.id),
                        network::NetworkUpdate::Properties(network::NetworkPropertiesChange {
                            added: true,
                            marks: None,
                            dns_servers: None,
                            connectivity_state: None,
                            name: Some(properties.name.clone()),
                            network_type: Some((*device_class).into()),
                        }),
                    ))
                    .await;
            }
        }

        match config {
            InterfaceConfigState::Host(HostInterfaceState {
                dhcpv4_client,
                dhcpv6_client_state,
                dhcpv6_pd_config,
                interface_admin_auth,
                interface_naming_id,
            }) => {
                let interface_id = properties.id.try_into().expect("should be nonzero");
                NetCfg::handle_dhcpv4_client_start(
                    interface_id,
                    &properties.name,
                    dhcpv4_client,
                    dhcpv4_client_provider,
                    route_set_provider,
                    interface_admin_auth,
                    dhcpv4_configuration_streams,
                )
                .await?;

                if let Some(dhcpv6_client_provider) = dhcpv6_client_provider {
                    let sockaddr = start_dhcpv6_client(
                        properties,
                        dhcpv6::duid(interface_naming_id.mac),
                        dhcpv6_client_provider,
                        dhcpv6_pd_config.clone(),
                        watchers,
                        dhcpv6_prefixes_streams,
                    )?;
                    *dhcpv6_client_state = sockaddr.map(dhcpv6::ClientState::new);
                }
            }
            InterfaceConfigState::WlanAp(WlanApInterfaceState { interface_naming_id: _ }) => {
                if let Some(dhcpv4_server) = dhcpv4_server {
                    dhcpv4::start_server(dhcpv4_server)
                        .await
                        .context("error starting DHCP server")?
                }
            }
            InterfaceConfigState::Blackhole(BlackholeInterfaceState) => {}
        }

        Ok(())
    }
}

/// Network Configuration state.
pub struct NetCfg<'a> {
    stack: fnet_stack::StackProxy,
    lookup_admin: fnet_name::LookupAdminProxy,
    filter_control: FilterControl,
    interface_state: fnet_interfaces::StateProxy,
    installer: fidl_fuchsia_net_interfaces_admin::InstallerProxy,
    dhcp_server: Option<fnet_dhcp::Server_Proxy>,
    dhcpv4_client_provider: Option<fnet_dhcp::ClientProviderProxy>,
    dhcpv6_client_provider: Option<fnet_dhcpv6::ClientProviderProxy>,
    route_set_v4_provider: fnet_routes_admin::RouteTableV4Proxy,

    locally_provisioned_network_rule_set:
        Option<(fnet_routes_admin::RuleSetV4Proxy, fnet_routes_admin::RuleSetV6Proxy)>,

    filter_enabled_state: FilterEnabledState,

    // TODO(https://fxbug.dev/42146318): These hashmaps are all indexed by
    // interface ID and store per-interface state, and should be merged.
    interface_states: HashMap<InterfaceId, InterfaceState>,
    interface_properties: HashMap<
        InterfaceId,
        fnet_interfaces_ext::PropertiesAndState<(), fnet_interfaces_ext::DefaultInterest>,
    >,
    interface_metrics: InterfaceMetrics,

    dns_servers: DnsServers,
    dns_server_watch_responders: dns::DnsServerWatchResponders,
    route_advertisement_watcher_provider:
        Option<fnet_ndp::RouterAdvertisementOptionWatcherProviderProxy>,

    forwarded_device_classes: ForwardedDeviceClasses,

    allowed_upstream_device_classes: &'a HashSet<DeviceClass>,

    dhcpv4_configuration_streams: dhcpv4::ConfigurationStreamMap,

    dhcpv6_prefix_provider_handler: Option<dhcpv6::PrefixProviderHandler>,
    dhcpv6_prefixes_streams: dhcpv6::PrefixesStreamMap,

    // Policy configuration to determine the name of an interface.
    interface_naming_config: interface::InterfaceNamingConfig,
    // Policy configuration to determine whether to provision an interface.
    interface_provisioning_policy: Vec<interface::ProvisioningRule>,

    // NetworkProperty Watchers
    netpol_networks_service: network::NetpolNetworksService,

    // Stores DNS servers learned from NDP Router Advertisements, keyed by
    // (InterfaceId, router address).
    ndp_dns_servers: HashMap<(InterfaceId, net_types::ip::Ipv6Addr), Vec<fnet_name::DnsServer_>>,

    // Manages the lifetime of NDP-learned DNS servers.
    ndp_dns_expiry_tracker: dns::NdpDnsExpiryTracker,

    telemetry_node: fuchsia_inspect::Node,
    enable_socket_proxy: bool,
    inspector: fuchsia_inspect::Inspector,
}

/// Returns a [`fnet_name::DnsServer_`] with a static source from a [`std::net::IpAddr`].
fn static_source_from_ip(f: std::net::IpAddr) -> fnet_name::DnsServer_ {
    let socket_addr = match fnet_ext::IpAddress(f).into() {
        fnet::IpAddress::Ipv4(addr) => fnet::SocketAddress::Ipv4(fnet::Ipv4SocketAddress {
            address: addr,
            port: DEFAULT_DNS_PORT,
        }),
        fnet::IpAddress::Ipv6(addr) => fnet::SocketAddress::Ipv6(fnet::Ipv6SocketAddress {
            address: addr,
            port: DEFAULT_DNS_PORT,
            zone_index: 0,
        }),
    };

    fnet_name::DnsServer_ {
        address: Some(socket_addr),
        source: Some(fnet_name::DnsServerSource::StaticSource(
            fnet_name::StaticDnsServerSource::default(),
        )),
        ..Default::default()
    }
}

/// Connect to a service, returning an error if the service does not exist in
/// the service directory.
async fn svc_connect<S: fidl::endpoints::DiscoverableProtocolMarker>(
    svc_dir: &fio::DirectoryProxy,
) -> Result<S::Proxy, anyhow::Error> {
    optional_svc_connect::<S>(svc_dir)
        .await?
        .ok_or_else(|| anyhow::anyhow!("service does not exist"))
}

/// Attempt to connect to a service, returning `None` if the service does not
/// exist in the service directory.
async fn optional_svc_connect<S: fidl::endpoints::DiscoverableProtocolMarker>(
    svc_dir: &fio::DirectoryProxy,
) -> Result<Option<S::Proxy>, anyhow::Error> {
    let req = new_protocol_connector_in_dir::<S>(&svc_dir);
    if !req.exists().await.context("error checking for service existence")? {
        Ok(None)
    } else {
        req.connect().context("error connecting to service").map(Some)
    }
}

/// Start a DHCPv6 client if there is a unicast link-local IPv6 address in `addresses` to use as
/// the address.
fn start_dhcpv6_client(
    fnet_interfaces_ext::Properties {
        id,
        online,
        name,
        addresses,
        port_class: _,
        has_default_ipv4_route: _,
        has_default_ipv6_route: _,
        port_identity_koid: _,
    }: &fnet_interfaces_ext::Properties<fnet_interfaces_ext::DefaultInterest>,
    duid: fnet_dhcpv6::Duid,
    dhcpv6_client_provider: &fnet_dhcpv6::ClientProviderProxy,
    pd_config: Option<fnet_dhcpv6::PrefixDelegationConfig>,
    watchers: &mut DnsServerWatchers<'_>,
    dhcpv6_prefixes_streams: &mut dhcpv6::PrefixesStreamMap,
) -> Result<Option<fnet::Ipv6SocketAddress>, errors::Error> {
    let id = InterfaceId::from(*id);
    if !online {
        return Ok(None);
    }

    let sockaddr = if let Some(sockaddr) = addresses.iter().find_map(
        |&fnet_interfaces_ext::Address {
             addr: fnet::Subnet { addr, prefix_len: _ },
             valid_until: _,
             preferred_lifetime_info: _,
             assignment_state,
         }| {
            assert_eq!(assignment_state, fnet_interfaces::AddressAssignmentState::Assigned);
            match addr {
                fnet::IpAddress::Ipv6(address) => {
                    if address.is_unicast_link_local() {
                        Some(fnet::Ipv6SocketAddress {
                            address,
                            port: fnet_dhcpv6::DEFAULT_CLIENT_PORT,
                            zone_index: id.get(),
                        })
                    } else {
                        None
                    }
                }
                fnet::IpAddress::Ipv4(_) => None,
            }
        },
    ) {
        sockaddr
    } else {
        return Ok(None);
    };

    if matches!(pd_config, Some(fnet_dhcpv6::PrefixDelegationConfig::Prefix(_))) {
        // We debug-log the `PrefixDelegationConfig` below. This is okay for
        // now because we do not use the prefix variant, but if we did we need
        // to support pretty-printing prefixes as it is considered PII and only
        // pretty-printed prefixes/addresses are properly redacted.
        todo!("https://fxbug.dev/42069036: Support pretty-printing configured prefix");
    }

    let source = DnsServersUpdateSource::Dhcpv6 { interface_id: id.get() };
    assert!(
        !watchers.contains_key(&source) && !dhcpv6_prefixes_streams.contains_key(&id),
        "interface with id={} already has a DHCPv6 client",
        id
    );

    let (dns_servers_stream, prefixes_stream) = dhcpv6::start_client(
        dhcpv6_client_provider,
        id,
        sockaddr,
        duid,
        pd_config.clone(),
    )
        .with_context(|| {
            format!(
                "failed to start DHCPv6 client on interface {} (id={}) w/ sockaddr {} and PD config {:?}",
                name,
                id,
                sockaddr.display_ext(),
                pd_config,
            )
        })?;
    let stream = dns_servers_stream
        .tagged(source)
        .map(|(source, result)| DnsServerUpdate {
            source,
            payload: DnsWatcherResultPayload::Generic,
            lifetime: DnsServerLifetime::Undefined,
            result,
        })
        .boxed();
    if let Some(o) = watchers.insert(source, stream) {
        let _: BoxStream<'_, _> = o;
        unreachable!("DNS server watchers must not contain key {:?}", source);
    }
    if let Some(o) = dhcpv6_prefixes_streams.insert(id, prefixes_stream.tagged(id)) {
        let _: Pin<Box<dhcpv6::InterfaceIdTaggedPrefixesStream>> = o;
        unreachable!("DHCPv6 prefixes streams must not contain key {:?}", id);
    }

    info!(
        "started DHCPv6 client on host interface {} (id={}) w/ sockaddr {} and PD config {:?}",
        name,
        id,
        sockaddr.display_ext(),
        pd_config,
    );

    Ok(Some(sockaddr))
}

enum RequestStream {
    Virtualization(fnet_virtualization::ControlRequestStream),
    Dhcpv6PrefixProvider(fnet_dhcpv6::PrefixProviderRequestStream),
    Masquerade(fnet_masquerade::FactoryRequestStream),
    DnsServerWatcher(fnet_name::DnsServerWatcherRequestStream),
    NetworkAttributes(fnp_properties::NetworksRequestStream),
    DelegatedNetworks(fnp_socketproxy::NetworkRegistryRequestStream),
    NetworkTokenResolver(fnp_properties::NetworkTokenResolverRequestStream),
    Reachability(freachability::MonitorRequestStream),
}

impl std::fmt::Debug for RequestStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            RequestStream::Virtualization(_) => write!(f, "Virtualization"),
            RequestStream::Dhcpv6PrefixProvider(_) => write!(f, "Dhcpv6PrefixProvider"),
            RequestStream::Masquerade(_) => write!(f, "Masquerade"),
            RequestStream::DnsServerWatcher(_) => write!(f, "DnsServerWatcher"),
            RequestStream::NetworkAttributes(_) => write!(f, "NetworkAttributes"),
            RequestStream::DelegatedNetworks(_) => write!(f, "DelegatedNetworks"),
            RequestStream::NetworkTokenResolver(_) => write!(f, "NetworkTokenResolver"),
            RequestStream::Reachability(_) => write!(f, "Reachability"),
        }
    }
}

#[derive(Debug, PartialEq)]
enum AllowClientRestart {
    No,
    Yes,
}

#[must_use]
#[derive(Debug, PartialEq)]
enum Dhcpv4ConfigurationHandlerResult {
    ContinueOperation,
    ClientStopped(AllowClientRestart),
}

#[derive(Debug, PartialEq)]
enum DnsWatcherResultPayload {
    Generic,
    Ndp { router_address: net_types::ip::Ipv6Addr },
}

#[derive(Debug, PartialEq)]
enum DnsServerLifetime {
    Undefined,
    Seconds(u32),
}

// Events associated with provisioning a device.
#[derive(Debug)]
enum ProvisioningEvent {
    InterfaceWatcherResult(
        Result<
            Option<fnet_interfaces_ext::EventWithInterest<fnet_interfaces_ext::DefaultInterest>>,
            fidl::Error,
        >,
    ),
    DnsWatcherResult(Option<DnsServerUpdate>),
    RequestStream(Option<RequestStream>),
    Dhcpv4Configuration(
        Option<(InterfaceId, Result<fnet_dhcp_ext::Configuration, fnet_dhcp_ext::Error>)>,
    ),
    Dhcpv4ClientDelayedStart(InterfaceId),
    Dhcpv6PrefixProviderRequest(Result<fnet_dhcpv6::PrefixProviderRequest, fidl::Error>),
    Dhcpv6PrefixControlRequest(
        Option<Result<Option<fnet_dhcpv6::PrefixControlRequest>, fidl::Error>>,
    ),
    Dhcpv6Prefixes(Option<(InterfaceId, Result<Vec<fnet_dhcpv6::Prefix>, fidl::Error>)>),
    DnsServerWatcherRequest(
        (dns::ConnectionId, Result<fnet_name::DnsServerWatcherRequest, fidl::Error>),
    ),
    VirtualizationEvent(virtualization::Event),
    MasqueradeEvent(masquerade::Event),
    NdpDnsExpired(InterfaceId, net_types::ip::Ipv6Addr),
}

// Per RFC 2131 "The client SHOULD wait a minimum of ten seconds before
// restarting the configuration process to avoid excessive network traffic in
// case of looping."
const DHCP_CLIENT_RESTART_WAIT_TIME: std::time::Duration = std::time::Duration::from_secs(10);

const TELEMETRY_INSPECT_NODE_NAME: &str = "telemetry";

impl<'a> NetCfg<'a> {
    async fn new(
        filter_enabled_interface_types: HashSet<InterfaceType>,
        interface_metrics: InterfaceMetrics,
        enable_dhcpv6: bool,
        forwarded_device_classes: ForwardedDeviceClasses,
        allowed_upstream_device_classes: &'a HashSet<DeviceClass>,
        interface_naming_policy: Vec<interface::NamingRule>,
        interface_provisioning_policy: Vec<interface::ProvisioningRule>,
        enable_socket_proxy: bool,
        inspector: fuchsia_inspect::Inspector,
    ) -> Result<NetCfg<'a>, anyhow::Error> {
        let svc_dir = clone_namespace_svc().context("error cloning svc directory handle")?;
        let stack = svc_connect::<fnet_stack::StackMarker>(&svc_dir)
            .await
            .context("could not connect to stack")?;
        let lookup_admin = svc_connect::<fnet_name::LookupAdminMarker>(&svc_dir)
            .await
            .context("could not connect to lookup admin")?;

        let filter_control = {
            let filter_deprecated =
                optional_svc_connect::<fnet_filter_deprecated::FilterMarker>(&svc_dir)
                    .await
                    .context("could not connect to filter deprecated")?;
            let filter_current = optional_svc_connect::<fnet_filter::ControlMarker>(&svc_dir)
                .await
                .context("could not connect to filter")?;
            filter::FilterControl::new(filter_deprecated, filter_current).await?
        };

        let interface_state = svc_connect::<fnet_interfaces::StateMarker>(&svc_dir)
            .await
            .context("could not connect to interfaces state")?;
        let dhcp_server = optional_svc_connect::<fnet_dhcp::Server_Marker>(&svc_dir)
            .await
            .context("could not connect to DHCP Server")?;
        let dhcpv4_client_provider = {
            let provider = optional_svc_connect::<fnet_dhcp::ClientProviderMarker>(&svc_dir)
                .await
                .context("could not connect to DHCPv4 client provider")?;
            match provider {
                Some(provider) => dhcpv4::probe_for_presence(&provider).await.then_some(provider),
                None => None,
            }
        };
        let dhcpv6_client_provider = if enable_dhcpv6 {
            let dhcpv6_client_provider =
                optional_svc_connect::<fnet_dhcpv6::ClientProviderMarker>(&svc_dir)
                    .await
                    .context("could not connect to DHCPv6 client provider")?;
            dhcpv6_client_provider
        } else {
            None
        };
        let route_set_v4_provider = svc_connect::<fnet_routes_admin::RouteTableV4Marker>(&svc_dir)
            .await
            .context("could not connect to fuchsia.net.routes.admin.RouteTableV4")?;
        let installer = svc_connect::<fnet_interfaces_admin::InstallerMarker>(&svc_dir)
            .await
            .context("could not connect to installer")?;
        let route_advertisement_watcher_provider = optional_svc_connect::<
            fnet_ndp::RouterAdvertisementOptionWatcherProviderMarker,
        >(&svc_dir)
        .await
        .context("could not connect to fuchsia.net.ndp.RouteAdvertisementOptionWatcherProvider")?;
        let interface_naming_config =
            interface::InterfaceNamingConfig::from_naming_rules(interface_naming_policy);
        let telemetry_node = inspector.root().create_child(TELEMETRY_INSPECT_NODE_NAME);
        let netpol_networks_service = network::NetpolNetworksService::default()
            .with_inspect(&telemetry_node, "operations", &telemetry_node, "network_registry")
            .context("failed to initialize network registry inspect")?;

        Ok(NetCfg {
            stack,
            lookup_admin,
            filter_control,
            interface_state,
            dhcp_server,
            installer,
            dhcpv4_client_provider,
            dhcpv6_client_provider,
            route_set_v4_provider,
            locally_provisioned_network_rule_set: None,
            interface_naming_config,
            filter_enabled_state: FilterEnabledState::new(filter_enabled_interface_types),
            interface_properties: Default::default(),
            interface_states: Default::default(),
            interface_metrics,
            dns_servers: Default::default(),
            dns_server_watch_responders: Default::default(),
            route_advertisement_watcher_provider,
            forwarded_device_classes,
            dhcpv4_configuration_streams: dhcpv4::ConfigurationStreamMap::empty(),
            dhcpv6_prefix_provider_handler: None,
            dhcpv6_prefixes_streams: dhcpv6::PrefixesStreamMap::empty(),
            allowed_upstream_device_classes,
            interface_provisioning_policy,
            netpol_networks_service,
            ndp_dns_servers: Default::default(),
            ndp_dns_expiry_tracker: dns::NdpDnsExpiryTracker::new(),
            telemetry_node,
            enable_socket_proxy,
            inspector,
        })
    }

    /// Updates the DNS servers used by the DNS resolver.
    async fn update_dns_servers(
        &mut self,
        source: DnsServersUpdateSource,
        servers: Vec<fnet_name::DnsServer_>,
    ) {
        dns::update_servers(
            &self.lookup_admin,
            &mut self.dns_servers,
            &mut self.dns_server_watch_responders,
            &mut self.netpol_networks_service,
            source,
            servers,
        )
        .await
    }

    /// Handles the completion of the DNS server watcher associated with `source`.
    ///
    /// Clears the servers for `source` and removes the watcher from `dns_watchers`.
    async fn handle_dns_server_watcher_done(
        &mut self,
        source: DnsServersUpdateSource,
        dns_watchers: &mut DnsServerWatchers<'_>,
    ) -> Result<(), anyhow::Error> {
        match source {
            DnsServersUpdateSource::Default => {
                panic!("should not have a DNS server watcher for the default source");
            }
            DnsServersUpdateSource::Dhcpv4 { interface_id } => {
                unreachable!(
                    "DHCPv4 configurations are not obtained through DNS server watcher; \
                     interface_id={}",
                    interface_id,
                )
            }
            DnsServersUpdateSource::Netstack => Ok(()),
            DnsServersUpdateSource::Dhcpv6 { interface_id } => {
                let interface_id = interface_id.try_into().expect("should be nonzero");
                let InterfaceState { config, provisioning, .. } = self
                    .interface_states
                    .get_mut(&interface_id)
                    .unwrap_or_else(|| panic!("no interface state found for id={}", interface_id));

                // Netcfg won't start a DHCPv6 client for a delegated interface.
                debug_assert!(provisioning == &interface::ProvisioningType::Local);

                match config {
                    InterfaceConfigState::Host(HostInterfaceState {
                        dhcpv4_client: _,
                        dhcpv6_client_state,
                        dhcpv6_pd_config: _,
                        interface_admin_auth: _,
                        interface_naming_id: _,
                    }) => {
                        let _: dhcpv6::ClientState =
                            dhcpv6_client_state.take().unwrap_or_else(|| {
                                panic!(
                                    "DHCPv6 was not being performed on host interface with id={}",
                                    interface_id
                                )
                            });

                        // If the DNS server watcher is done, that means the server-end
                        // of the channel is closed meaning DHCPv6 has been stopped.
                        // Perform the cleanup for our end of the DHCPv6 client
                        // (which will update DNS servers) and send an prefix update to any
                        // blocked watchers of fuchsia.net.dhcpv6/PrefixControl.WatchPrefix.
                        dhcpv6::stop_client(
                            &self.lookup_admin,
                            &mut self.dns_servers,
                            &mut self.dns_server_watch_responders,
                            &mut self.netpol_networks_service,
                            interface_id,
                            dns_watchers,
                            &mut self.dhcpv6_prefixes_streams,
                        )
                        .await;

                        dhcpv6::maybe_send_watch_prefix_response(
                            &self.interface_states,
                            &self.allowed_upstream_device_classes,
                            self.dhcpv6_prefix_provider_handler.as_mut(),
                        )
                    }
                    InterfaceConfigState::WlanAp(WlanApInterfaceState {
                        interface_naming_id: _,
                    }) => {
                        panic!(
                            "should not have a DNS watcher for a WLAN AP interface with id={}",
                            interface_id
                        );
                    }
                    InterfaceConfigState::Blackhole(BlackholeInterfaceState) => {
                        panic!(
                            "should not have a DNS watcher for a blackhole interface with id={}",
                            interface_id
                        );
                    }
                }
            }
            DnsServersUpdateSource::Ndp { interface_id } => {
                let interface_id = interface_id.try_into().expect("should be nonzero");
                let InterfaceState { config, provisioning, .. } = self
                    .interface_states
                    .get_mut(&interface_id)
                    .unwrap_or_else(|| panic!("no interface state found for id={}", interface_id));

                // Netcfg won't watch NDP servers for a delegated interface.
                debug_assert!(provisioning == &interface::ProvisioningType::Local);

                match config {
                    InterfaceConfigState::Host(HostInterfaceState { .. }) => {
                        dns::remove_rdnss_watcher(
                            &self.lookup_admin,
                            &mut self.dns_servers,
                            &mut self.dns_server_watch_responders,
                            &mut self.netpol_networks_service,
                            interface_id,
                            dns_watchers,
                        )
                        .await;
                        self.ndp_dns_servers.retain(|(iface_id, _), _| *iface_id != interface_id);
                        Ok(())
                    }
                    InterfaceConfigState::WlanAp(WlanApInterfaceState { .. }) => {
                        panic!(
                            "should not have a NDP DNS watcher for a WLAN AP interface with id={}",
                            interface_id
                        );
                    }
                    InterfaceConfigState::Blackhole(BlackholeInterfaceState) => {
                        panic!(
                            "should not have a NDP DNS watcher for a blackhole interface with id={}",
                            interface_id
                        );
                    }
                }
            }
            // TODO(https://fxbug.dev/475916525): Update the name of this source once these
            // servers are provided directly by Starnix.
            // SocketProxy DNS updates are pushed directly via the NetworkRegistry protocol
            // rather than an active DNS server watcher stream. Thus, the watcher completion
            // handler is a no-op.
            // We need to maintain this variant to allow for SocketProxy DNS servers to be
            // prioritized in dns-resolver over other DNS server sources.
            DnsServersUpdateSource::SocketProxy => Ok(()),
        }
    }

    /// Run the network configuration eventloop.
    ///
    /// The device directory will be monitored for device events and the netstack will be
    /// configured with a new interface on new device discovery.
    async fn run(
        &mut self,
        mut virtualization_handler: impl virtualization::Handler,
    ) -> Result<(), anyhow::Error> {
        let netdev_stream =
            self.create_device_stream().await.context("create netdevice stream")?.fuse();
        let mut netdev_stream = pin!(netdev_stream);

        let if_watcher_event_stream =
            fnet_interfaces_ext::event_stream_from_state(&self.interface_state, Default::default())
                .context("error creating interface watcher event stream")?
                .fuse();
        let mut if_watcher_event_stream = pin!(if_watcher_event_stream);

        let dns_server_watcher =
            fuchsia_component::client::connect_to_protocol::<fnet_name::DnsServerWatcherMarker>()
                .context("error connecting to dns server watcher")?;
        let netstack_dns_server_stream = dns_server_watcher::new_dns_server_stream(
            DnsServersUpdateSource::Netstack,
            dns_server_watcher,
        )
        .map(|(source, result)| DnsServerUpdate {
            source,
            payload: DnsWatcherResultPayload::Generic,
            lifetime: DnsServerLifetime::Undefined,
            result,
        })
        .boxed();

        let dns_watchers = DnsServerWatchers::empty();
        // `Fuse` (the return of `fuse`) guarantees that once the underlying stream is
        // exhausted, future attempts to poll the stream will return `None`. This would
        // be undesirable if we needed to support a scenario where all streams are
        // exhausted before adding a new stream to the `StreamMap`. However,
        // `netstack_dns_server_stream` is not expected to end so we can fuse the
        // `StreamMap` without issue.
        let mut dns_watchers = dns_watchers.fuse();

        assert!(
            dns_watchers
                .get_mut()
                .insert(DnsServersUpdateSource::Netstack, netstack_dns_server_stream)
                .is_none(),
            "dns watchers should be empty"
        );

        let mut masquerade_handler = MasqueradeHandler::default();

        // Serve fuchsia.net.virtualization/Control.
        let mut fs = ServiceFs::new_local();
        let _: &mut ServiceFsDir<'_, _> = fs
            .dir("svc")
            .add_fidl_service(RequestStream::Virtualization)
            .add_fidl_service(RequestStream::Dhcpv6PrefixProvider)
            .add_fidl_service(RequestStream::Masquerade)
            .add_fidl_service(RequestStream::DnsServerWatcher)
            .add_fidl_service(RequestStream::NetworkAttributes)
            .add_fidl_service(RequestStream::DelegatedNetworks)
            .add_fidl_service(RequestStream::NetworkTokenResolver)
            .add_fidl_service(RequestStream::Reachability);

        let _inspect_server_task =
            inspect_runtime::publish(&self.inspector, inspect_runtime::PublishOptions::default())
                .expect("publish Inspect task");

        let _: &mut ServiceFs<_> =
            fs.take_and_serve_directory_handle().context("take and serve directory handle")?;
        let mut fs = fs.fuse();

        let mut dhcpv6_prefix_provider_requests =
            futures::stream::SelectAll::<fnet_dhcpv6::PrefixProviderRequestStream>::new();

        // Maintain a queue of virtualization events to be dispatched to the virtualization handler.
        let mut virtualization_events =
            futures::stream::SelectAll::<virtualization::EventStream>::new();

        // Maintain a queue of masquerade events to be dispatched to the masquerade handler.
        let mut masquerade_events = futures::stream::SelectAll::<masquerade::EventStream>::new();

        let mut dns_server_watcher_incoming_requests =
            dns::DnsServerWatcherRequestStreams::default();

        let (telemetry_sender, telemetry_fut) = crate::telemetry::serve_telemetry(
            self.telemetry_node.clone_weak(),
            &format!("root/{TELEMETRY_INSPECT_NODE_NAME}"),
        );
        let telemetry_fut = telemetry_fut.fuse();
        let mut telemetry_fut = pin!(telemetry_fut);
        self.netpol_networks_service.set_telemetry(telemetry_sender);

        // Lifecycle handle takes no args, must be set to zero.
        // See zircon/processargs.h.
        const LIFECYCLE_HANDLE_ARG: u16 = 0;
        let lifecycle = fuchsia_runtime::take_startup_handle(fuchsia_runtime::HandleInfo::new(
            fuchsia_runtime::HandleType::Lifecycle,
            LIFECYCLE_HANDLE_ARG,
        ))
        .ok_or_else(|| anyhow::anyhow!("lifecycle handle not present"))?;
        let lifecycle = fuchsia_async::Channel::from_channel(lifecycle.into());
        let mut lifecycle = fidl_fuchsia_process_lifecycle::LifecycleRequestStream::from_channel(
            fidl::AsyncChannel::from(lifecycle),
        );

        debug!("starting eventloop...");

        enum Event {
            NetworkDeviceResult(Result<Option<devices::NetworkDeviceInstance>, anyhow::Error>),
            LifecycleRequest(
                Result<Option<fidl_fuchsia_process_lifecycle::LifecycleRequest>, fidl::Error>,
            ),
            NetpolNetworksServiceEvent(network::NetworkRequest),
            ProvisioningEvent(ProvisioningEvent),
        }

        loop {
            let mut dhcpv6_prefix_control_fut = futures::future::OptionFuture::from(
                self.dhcpv6_prefix_provider_handler
                    .as_mut()
                    .map(dhcpv6::PrefixProviderHandler::try_next_prefix_control_request),
            );
            let mut delayed_dhcpv4_client_starts = self
                .interface_states
                .iter_mut()
                .filter_map(|(id, InterfaceState { config, .. })| match config {
                    InterfaceConfigState::Host(HostInterfaceState {
                        dhcpv4_client,
                        dhcpv6_client_state: _,
                        dhcpv6_pd_config: _,
                        interface_admin_auth: _,
                        interface_naming_id: _,
                    }) => match dhcpv4_client {
                        Dhcpv4ClientState::NotRunning => None,
                        Dhcpv4ClientState::Running(_) => None,
                        Dhcpv4ClientState::ScheduledRestart(timer) => Some(timer.map(|()| *id)),
                    },
                    InterfaceConfigState::WlanAp(_) | InterfaceConfigState::Blackhole(_) => None,
                })
                .collect::<futures::stream::FuturesUnordered<_>>();
            let event = futures::select! {
                netdev_res = netdev_stream.try_next() => {
                    Event::NetworkDeviceResult(netdev_res)
                }
                req = lifecycle.try_next() => {
                    Event::LifecycleRequest(req)
                }
                if_watcher_res = if_watcher_event_stream.try_next() => {
                    Event::ProvisioningEvent(
                        ProvisioningEvent::InterfaceWatcherResult(if_watcher_res)
                    )
                }
                dns_watchers_res = dns_watchers.next() => {
                    Event::ProvisioningEvent(
                        ProvisioningEvent::DnsWatcherResult(dns_watchers_res)
                    )
                }
                ndp_dns_expiry = self.ndp_dns_expiry_tracker.next() => {
                    let (interface_id, router_address) =
                        ndp_dns_expiry.expect("expiry stream should never end");
                    Event::ProvisioningEvent(
                        ProvisioningEvent::NdpDnsExpired(interface_id, router_address)
                    )
                }
                req_stream = fs.next() => {
                    Event::ProvisioningEvent(
                        ProvisioningEvent::RequestStream(req_stream)
                    )
                }
                dhcpv4_configuration = self.dhcpv4_configuration_streams.next() => {
                    Event::ProvisioningEvent(
                        ProvisioningEvent::Dhcpv4Configuration(dhcpv4_configuration)
                    )
                }
                interface_id = delayed_dhcpv4_client_starts.select_next_some() => {
                    Event::ProvisioningEvent(
                        ProvisioningEvent::Dhcpv4ClientDelayedStart(interface_id)
                    )
                }
                dhcpv6_prefix_req = dhcpv6_prefix_provider_requests.select_next_some() => {
                    Event::ProvisioningEvent(
                        ProvisioningEvent::Dhcpv6PrefixProviderRequest(dhcpv6_prefix_req)
                    )
                }
                dhcpv6_prefix_control_req = dhcpv6_prefix_control_fut => {
                    Event::ProvisioningEvent(
                        ProvisioningEvent::Dhcpv6PrefixControlRequest(dhcpv6_prefix_control_req)
                    )
                }
                dhcpv6_prefixes = self.dhcpv6_prefixes_streams.next() => {
                    Event::ProvisioningEvent(
                        ProvisioningEvent::Dhcpv6Prefixes(dhcpv6_prefixes)
                    )
                }
                req = dns_server_watcher_incoming_requests.select_next_some() => {
                    Event::ProvisioningEvent(
                        ProvisioningEvent::DnsServerWatcherRequest(req)
                    )
                }
                virt_event = virtualization_events.select_next_some() => {
                    Event::ProvisioningEvent(
                        ProvisioningEvent::VirtualizationEvent(virt_event)
                    )
                }
                masq_event = masquerade_events.select_next_some() => {
                    Event::ProvisioningEvent(
                        ProvisioningEvent::MasqueradeEvent(
                            masq_event.context("error while receiving MasqueradeEvent")?)
                        )
                }
                netcfg_event = self.netpol_networks_service.select_next_some() => {
                    Event::NetpolNetworksServiceEvent(netcfg_event)
                }
                _telemetry_res = telemetry_fut => {
                    error!("unexpectedly stopped serving telemetry");
                    continue;
                }
                complete => return Err(anyhow::anyhow!("eventloop ended unexpectedly")),
            };

            // `delayed_dhcpv4_client_starts` mutably borrows the delayed-start timers from `self`'s
            // InterfaceState map, so we have to drop it here to regain access to `self`.
            drop(delayed_dhcpv4_client_starts);
            match event {
                Event::NetworkDeviceResult(netdev_res) => {
                    let instance =
                        netdev_res.context("error retrieving netdev instance")?.ok_or_else(
                            || anyhow::anyhow!("netdev instance watcher stream ended unexpectedly"),
                        )?;
                    // DNS watchers must be propagated to start an RA NDP watcher for the interface
                    // prior to the interface getting enabled in the Netstack.
                    self.handle_device_instance(instance, dns_watchers.get_mut())
                        .await
                        .context("handle netdev instance")?
                }
                Event::LifecycleRequest(req) => {
                    let req = req.context("lifecycle request")?.ok_or_else(|| {
                        anyhow::anyhow!("LifecycleRequestStream ended unexpectedly")
                    })?;
                    match req {
                        fidl_fuchsia_process_lifecycle::LifecycleRequest::Stop {
                            control_handle,
                        } => {
                            info!("received shutdown request");
                            // Shutdown request is acknowledged by the lifecycle
                            // channel shutting down. Intentionally leak the
                            // channel so it'll only be closed on process
                            // termination, allowing clean process termination
                            // to always be observed.

                            // Must drop the control_handle to unwrap the
                            // lifecycle channel.
                            std::mem::drop(control_handle);
                            let (inner, _terminated): (_, bool) = lifecycle.into_inner();
                            let inner = std::sync::Arc::try_unwrap(inner).map_err(
                                |_: std::sync::Arc<_>| {
                                    anyhow::anyhow!("failed to retrieve lifecycle channel")
                                },
                            )?;
                            let inner: zx::Channel = inner.into_channel().into_zx_channel();
                            std::mem::forget(inner);

                            return Ok(());
                        }
                    }
                }
                Event::NetpolNetworksServiceEvent(event) => {
                    match self.netpol_networks_service.handle_event(event).await {
                        Ok(network::DelegatedNetworkUpdateResult {
                            dns_servers: Some(dns_servers),
                        }) => {
                            self.update_dns_servers(
                                dns_server_watcher::DnsServersUpdateSource::SocketProxy,
                                dns_servers,
                            )
                            .await;
                        }
                        Ok(network::DelegatedNetworkUpdateResult { dns_servers: None }) => {}
                        Err(e) => {
                            error!("Could not handle netpol networks service event: {e:?}");
                        }
                    }
                }
                Event::ProvisioningEvent(event) => {
                    self.handle_provisioning_event(
                        event,
                        dns_watchers.get_mut(),
                        &mut dhcpv6_prefix_provider_requests,
                        &mut dns_server_watcher_incoming_requests,
                        &mut virtualization_handler,
                        &mut virtualization_events,
                        &mut masquerade_handler,
                        &mut masquerade_events,
                    )
                    .await?
                }
            }
        }
    }

    async fn handle_provisioning_event(
        &mut self,
        event: ProvisioningEvent,
        dns_watchers: &mut DnsServerWatchers<'_>,
        dhcpv6_prefix_provider_requests: &mut futures::stream::SelectAll<
            fnet_dhcpv6::PrefixProviderRequestStream,
        >,
        dns_server_watcher_incoming_requests: &mut dns::DnsServerWatcherRequestStreams,
        virtualization_handler: &mut impl virtualization::Handler,
        virtualization_events: &mut futures::stream::SelectAll<virtualization::EventStream>,
        masquerade_handler: &mut MasqueradeHandler,
        masquerade_events: &mut futures::stream::SelectAll<masquerade::EventStream>,
    ) -> Result<(), anyhow::Error> {
        match event {
            ProvisioningEvent::InterfaceWatcherResult(if_watcher_res) => {
                let event = if_watcher_res
                    .unwrap_or_else(|err| exit_with_fidl_error(err))
                    .expect("watcher stream never returns None");
                trace!("got interfaces watcher event = {:?}", event);

                if let Some(removed_id) = self
                    .handle_interface_watcher_event(event, dns_watchers, virtualization_handler)
                    .await
                    .context("handle interface watcher event")?
                {
                    masquerade_handler.handle_interface_removed(removed_id);
                }
            }
            ProvisioningEvent::DnsWatcherResult(dns_watchers_res) => {
                let DnsServerUpdate { source, payload, lifetime, result } = dns_watchers_res
                    .ok_or_else(|| {
                        anyhow::anyhow!("dns watchers stream should never be exhausted")
                    })?;
                let servers = match result {
                    Ok(s) => s,
                    Err(e) => {
                        // TODO(https://fxbug.dev/42135335): Restart the DNS server watcher.
                        warn!(
                            "non-fatal error getting next event from DNS server watcher stream
                            with source = {:?}: {:?}",
                            source, e
                        );
                        self.handle_dns_server_watcher_done(source, dns_watchers)
                            .await
                            .with_context(|| {
                                format!(
                                    "error handling completion of DNS server watcher for \
                                    {:?}",
                                    source
                                )
                            })?;
                        return Ok(());
                    }
                };

                match source {
                    DnsServersUpdateSource::Ndp { interface_id } => {
                        // Update DNS servers associated with the interface and router.
                        let interface_id_typed =
                            InterfaceId::new(interface_id).ok_or_else(|| {
                                anyhow::anyhow!("NDP update with invalid zero interface ID")
                            })?;
                        if let DnsWatcherResultPayload::Ndp { router_address } = payload {
                            let key = (interface_id_typed, router_address);
                            if servers.is_empty() {
                                let _: Option<Vec<fnet_name::DnsServer_>> =
                                    self.ndp_dns_servers.remove(&key);
                            } else {
                                let _: Option<Vec<fnet_name::DnsServer_>> =
                                    self.ndp_dns_servers.insert(key, servers.clone());
                            }
                        } else {
                            return Err(format_err!(
                                "Unexpected NDP DNS update payload: {:?}",
                                payload
                            ));
                        }
                        let flattened_servers = self
                            .ndp_dns_servers
                            .iter()
                            .filter(|((iface_id, _), _)| *iface_id == interface_id_typed)
                            .flat_map(|(_, servers)| servers.iter().cloned())
                            .collect::<Vec<_>>();
                        self.update_dns_servers(source, flattened_servers).await;

                        // If the router address and lifetime have been provided, update the DNS
                        // server expiration tracking.  Note that the lifetime may be 0.  In this
                        // case, the timer is allowed to immediately expire and remove the entry to
                        // unify these code paths.
                        if let (
                            DnsWatcherResultPayload::Ndp { router_address },
                            DnsServerLifetime::Seconds(lifetime_seconds),
                        ) = (payload, lifetime)
                        {
                            // Set the expiration timer for this latest batch of DNS servers.
                            self.ndp_dns_expiry_tracker.record_expiry(
                                interface_id_typed,
                                router_address,
                                lifetime_seconds,
                            );
                        }
                    }
                    DnsServersUpdateSource::Default
                    | DnsServersUpdateSource::Netstack
                    | DnsServersUpdateSource::Dhcpv4 { .. }
                    | DnsServersUpdateSource::Dhcpv6 { .. }
                    | DnsServersUpdateSource::SocketProxy => {
                        self.update_dns_servers(source, servers).await
                    }
                };
            }
            ProvisioningEvent::NdpDnsExpired(interface_id, router_address) => {
                self.handle_ndp_dns_expired(interface_id, router_address).await
            }
            // TODO(https://fxbug.dev/42080722): Add tests to ensure we do not offer
            // these services when interface has ProvisioningType::Delegated
            // state.
            ProvisioningEvent::RequestStream(req_stream) => {
                match req_stream.context("ServiceFs ended unexpectedly")? {
                    RequestStream::Virtualization(req_stream) => virtualization_handler
                        .handle_event(
                            virtualization::Event::ControlRequestStream(req_stream),
                            virtualization_events,
                        )
                        .await
                        .context("handle virtualization event")
                        .or_else(errors::Error::accept_non_fatal)?,
                    RequestStream::Dhcpv6PrefixProvider(req_stream) => {
                        dhcpv6_prefix_provider_requests.push(req_stream);
                    }
                    RequestStream::Masquerade(req_stream) => {
                        masquerade_handler
                            .handle_event(
                                masquerade::Event::FactoryRequestStream(req_stream),
                                masquerade_events,
                                &mut self.filter_control,
                                &mut self.filter_enabled_state,
                                &self.interface_states,
                            )
                            .await
                    }
                    RequestStream::DnsServerWatcher(req_stream) => {
                        dns_server_watcher_incoming_requests.handle_request_stream(req_stream);
                    }
                    RequestStream::NetworkAttributes(req_stream) => {
                        self.netpol_networks_service.add_stream(req_stream);
                    }
                    RequestStream::DelegatedNetworks(req_stream) => {
                        self.netpol_networks_service.add_stream(req_stream);
                    }
                    RequestStream::NetworkTokenResolver(req_stream) => {
                        self.netpol_networks_service.add_stream(req_stream);
                    }
                    RequestStream::Reachability(req_stream) => {
                        self.netpol_networks_service.add_stream(req_stream);
                    }
                };
            }
            ProvisioningEvent::Dhcpv4Configuration(config) => {
                let (interface_id, config) =
                    config.expect("DHCPv4 configuration stream is never exhausted");
                match self.handle_dhcpv4_configuration(interface_id, config).await {
                    Dhcpv4ConfigurationHandlerResult::ContinueOperation => (),
                    Dhcpv4ConfigurationHandlerResult::ClientStopped(allow_restart) => {
                        let interface_name = self
                            .interface_properties
                            .get(&interface_id)
                            .map(
                                |fnet_interfaces_ext::PropertiesAndState {
                                     state: (),
                                     properties: fnet_interfaces_ext::Properties { name, .. },
                                 }| name.as_str(),
                            )
                            .unwrap_or("<removed>");
                        let state = self
                            .interface_states
                            .get_mut(&interface_id)
                            .map(
                                |InterfaceState {
                                     control,
                                     device_class: _,
                                     config,
                                     provisioning: _,
                                 }| {
                                    match config {
                                        InterfaceConfigState::Host(HostInterfaceState {
                                            dhcpv4_client,
                                            dhcpv6_client_state: _,
                                            dhcpv6_pd_config: _,
                                            interface_admin_auth: _,
                                            interface_naming_id: _,
                                        }) => Some((dhcpv4_client, control)),
                                        InterfaceConfigState::WlanAp(_)
                                        | InterfaceConfigState::Blackhole(_) => None,
                                    }
                                },
                            )
                            .flatten();

                        match state {
                            None => {
                                log::error!(
                                    "Trying to handle DHCPv4 client shutdown \
                                (id={interface_id}), but no client is running on that interface"
                                );
                            }
                            Some((dhcpv4_client, control)) => {
                                Self::handle_dhcpv4_client_stop(
                                    interface_id,
                                    interface_name,
                                    dhcpv4_client,
                                    &mut self.dhcpv4_configuration_streams,
                                    &mut self.dns_servers,
                                    &mut self.dns_server_watch_responders,
                                    &mut self.netpol_networks_service,
                                    control,
                                    &self.lookup_admin,
                                    dhcpv4::AlreadyObservedClientExit::Yes,
                                )
                                .await;

                                match allow_restart {
                                    AllowClientRestart::No => (),
                                    AllowClientRestart::Yes => {
                                        // The client exited due to an unexpected error. Schedule it
                                        // to be restarted after waiting a backoff period.
                                        *dhcpv4_client =
                                            Dhcpv4ClientState::ScheduledRestart(Box::pin(
                                                fasync::Timer::new(DHCP_CLIENT_RESTART_WAIT_TIME),
                                            ));
                                    }
                                }
                            }
                        }
                    }
                }
            }
            ProvisioningEvent::Dhcpv4ClientDelayedStart(interface_id) => {
                self.on_delayed_dhcpv4_client_start(interface_id)
                    .await
                    .or_else(errors::Error::accept_non_fatal)?;
            }
            ProvisioningEvent::Dhcpv6PrefixProviderRequest(res) => {
                match res {
                    Ok(fnet_dhcpv6::PrefixProviderRequest::AcquirePrefix {
                        config,
                        prefix,
                        control_handle: _,
                    }) => {
                        self.handle_dhcpv6_acquire_prefix(config, prefix, dns_watchers)
                            .await
                            .or_else(errors::Error::accept_non_fatal)?;
                    }
                    Err(e) => {
                        error!("fuchsia.net.dhcpv6/PrefixProvider request error: {:?}", e)
                    }
                };
            }
            ProvisioningEvent::Dhcpv6PrefixControlRequest(req) => {
                let res = req.context(
                    "PrefixControl OptionFuture will only be selected if it is not None",
                )?;
                match res {
                    Err(e) => {
                        error!("fuchsia.net.dhcpv6/PrefixControl request stream error: {:?}", e);
                        self.on_dhcpv6_prefix_control_close(dns_watchers).await;
                    }
                    Ok(None) => {
                        info!("fuchsia.net.dhcpv6/PrefixControl closed by client");
                        self.on_dhcpv6_prefix_control_close(dns_watchers).await;
                    }
                    Ok(Some(fnet_dhcpv6::PrefixControlRequest::WatchPrefix { responder })) => {
                        self.handle_watch_prefix(responder, dns_watchers)
                            .await
                            .context("handle PrefixControl.WatchPrefix")
                            .unwrap_or_else(accept_error);
                    }
                    Ok(Some(fnet_dhcpv6::PrefixControlRequest::Stop { control_handle })) => {
                        self.on_dhcpv6_prefix_control_close(dns_watchers).await;
                        match control_handle
                            .send_on_exit(fnet_dhcpv6::PrefixControlExitReason::Stopped)
                        {
                            Ok(()) => {}
                            Err(e) if e.is_closed() => {}
                            Err(e) => warn!("failed to send OnExit on Stop: {:?}", e),
                        }
                    }
                };
            }
            ProvisioningEvent::Dhcpv6Prefixes(prefixes) => {
                let (interface_id, res) =
                    prefixes.context("DHCPv6 watch prefixes stream map can never be exhausted")?;
                self.handle_dhcpv6_prefixes(interface_id, res, dns_watchers)
                    .await
                    .unwrap_or_else(accept_error);
            }
            ProvisioningEvent::DnsServerWatcherRequest((id, req)) => {
                self.dns_server_watch_responders.handle_request(id, req, &self.dns_servers)?;
            }
            ProvisioningEvent::VirtualizationEvent(event) => {
                virtualization_handler
                    .handle_event(event, virtualization_events)
                    .await
                    .context("handle virtualization event")
                    .or_else(errors::Error::accept_non_fatal)?;
            }
            ProvisioningEvent::MasqueradeEvent(event) => {
                masquerade_handler
                    .handle_event(
                        event,
                        masquerade_events,
                        &mut self.filter_control,
                        &mut self.filter_enabled_state,
                        &self.interface_states,
                    )
                    .await
            }
        };
        return Ok(());
    }

    async fn handle_dhcpv4_client_stop(
        id: InterfaceId,
        name: &str,
        dhcpv4_client: &mut Dhcpv4ClientState,
        configuration_streams: &mut dhcpv4::ConfigurationStreamMap,
        dns_servers: &mut DnsServers,
        dns_server_watch_responders: &mut dns::DnsServerWatchResponders,
        netpol_networks_service: &mut network::NetpolNetworksService,
        control: &fnet_interfaces_ext::admin::Control,
        lookup_admin: &fnet_name::LookupAdminProxy,
        already_observed_client_exit: dhcpv4::AlreadyObservedClientExit,
    ) {
        match std::mem::replace(dhcpv4_client, Dhcpv4ClientState::NotRunning) {
            Dhcpv4ClientState::NotRunning => (),
            Dhcpv4ClientState::Running(c) => {
                dhcpv4::stop_client(
                    id,
                    name,
                    c,
                    configuration_streams,
                    dns_servers,
                    dns_server_watch_responders,
                    netpol_networks_service,
                    control,
                    lookup_admin,
                    already_observed_client_exit,
                )
                .await;
            }
            Dhcpv4ClientState::ScheduledRestart(_) => (),
        }
    }

    async fn handle_dhcpv4_client_start(
        id: InterfaceId,
        name: &str,
        dhcpv4_client: &mut Dhcpv4ClientState,
        dhcpv4_client_provider: Option<&fnet_dhcp::ClientProviderProxy>,
        route_set_provider: &fnet_routes_admin::RouteTableV4Proxy,
        interface_admin_auth: &fnet_resources::GrantForInterfaceAuthorization,
        configuration_streams: &mut dhcpv4::ConfigurationStreamMap,
    ) -> Result<(), errors::Error> {
        *dhcpv4_client = match dhcpv4_client_provider {
            None => Dhcpv4ClientState::NotRunning,
            Some(p) => Dhcpv4ClientState::Running(
                dhcpv4::start_client(
                    id,
                    name,
                    p,
                    route_set_provider,
                    interface_admin_auth,
                    configuration_streams,
                )
                .await?,
            ),
        };
        Ok(())
    }

    async fn handle_dhcpv4_client_update(
        id: InterfaceId,
        name: &str,
        online: bool,
        dhcpv4_client: &mut Dhcpv4ClientState,
        dhcpv4_client_provider: Option<&fnet_dhcp::ClientProviderProxy>,
        configuration_streams: &mut dhcpv4::ConfigurationStreamMap,
        dns_servers: &mut DnsServers,
        dns_server_watch_responders: &mut dns::DnsServerWatchResponders,
        netpol_networks_service: &mut network::NetpolNetworksService,
        control: &fnet_interfaces_ext::admin::Control,
        lookup_admin: &fnet_name::LookupAdminProxy,
        route_set_provider: &fnet_routes_admin::RouteTableV4Proxy,
        interface_admin_auth: &fnet_resources::GrantForInterfaceAuthorization,
    ) -> Result<(), errors::Error> {
        if online {
            Self::handle_dhcpv4_client_start(
                id,
                name,
                dhcpv4_client,
                dhcpv4_client_provider,
                route_set_provider,
                interface_admin_auth,
                configuration_streams,
            )
            .await?
        } else {
            Self::handle_dhcpv4_client_stop(
                id,
                name,
                dhcpv4_client,
                configuration_streams,
                dns_servers,
                dns_server_watch_responders,
                netpol_networks_service,
                control,
                lookup_admin,
                dhcpv4::AlreadyObservedClientExit::No,
            )
            .await
        }

        Ok(())
    }

    async fn on_delayed_dhcpv4_client_start(
        &mut self,
        interface_id: InterfaceId,
    ) -> Result<(), errors::Error> {
        let Self {
            dhcpv4_client_provider,
            route_set_v4_provider,
            interface_states,
            interface_properties,
            dhcpv4_configuration_streams,
            ..
        } = self;

        let (dhcpv4_client, interface_admin_auth) = match interface_states
            .get_mut(&interface_id)
            .and_then(|InterfaceState { config, .. }| match config {
                InterfaceConfigState::Host(HostInterfaceState {
                    dhcpv4_client,
                    dhcpv6_client_state: _,
                    dhcpv6_pd_config: _,
                    interface_admin_auth,
                    interface_naming_id: _,
                }) => Some((dhcpv4_client, interface_admin_auth)),
                InterfaceConfigState::WlanAp(_) | InterfaceConfigState::Blackhole(_) => None,
            }) {
            Some(state) => state,
            None => {
                // It's fine for the interface to have been removed before we
                // got around to restarting the client.
                return Ok(());
            }
        };

        match dhcpv4_client {
            Dhcpv4ClientState::NotRunning => (),
            Dhcpv4ClientState::Running(_) => {
                // We already restarted the client before reaching this point.
                return Ok(());
            }
            Dhcpv4ClientState::ScheduledRestart(_) => {
                *dhcpv4_client = Dhcpv4ClientState::NotRunning;
            }
        };

        let properties = match interface_properties.get(&interface_id) {
            Some(fnet_interfaces_ext::PropertiesAndState { properties, state: () }) => properties,
            None => return Ok(()),
        };

        Self::handle_dhcpv4_client_start(
            properties.id.into(),
            &properties.name,
            dhcpv4_client,
            dhcpv4_client_provider.as_ref(),
            route_set_v4_provider,
            interface_admin_auth,
            dhcpv4_configuration_streams,
        )
        .await
    }

    /// Handles an interface watcher event. Returns the ID if the interface was removed.
    async fn handle_interface_watcher_event(
        &mut self,
        event: fnet_interfaces_ext::EventWithInterest<fnet_interfaces_ext::DefaultInterest>,
        watchers: &mut DnsServerWatchers<'_>,
        virtualization_handler: &mut impl virtualization::Handler,
    ) -> Result<Option<InterfaceId>, anyhow::Error> {
        let Self {
            interface_properties,
            dns_servers,
            dns_server_watch_responders,
            netpol_networks_service,
            interface_states,
            lookup_admin,
            dhcp_server,
            dhcpv4_client_provider,
            dhcpv6_client_provider,
            route_set_v4_provider,
            dhcpv4_configuration_streams,
            dhcpv6_prefixes_streams,
            allowed_upstream_device_classes,
            dhcpv6_prefix_provider_handler,
            filter_enabled_state,
            ndp_dns_servers,
            enable_socket_proxy,
            ..
        } = self;
        let update_result = interface_properties
            .update(event)
            .context("failed to update interface properties with watcher event")?;

        // When the underlying interface id for the event represents an
        // interface netcfg installed, determine whether the event should
        // be ignored given the interface's provisioning policy.
        if let Some(id) = match &update_result {
            fnet_interfaces_ext::UpdateResult::NoChange => None,
            fnet_interfaces_ext::UpdateResult::Existing { properties, state: _ } => {
                Some(properties.id)
            }
            fnet_interfaces_ext::UpdateResult::Added { properties, state: _ } => {
                Some(properties.id)
            }
            fnet_interfaces_ext::UpdateResult::Changed { previous: _, current, state: _ } => {
                Some(current.id)
            }
            fnet_interfaces_ext::UpdateResult::Removed(
                fnet_interfaces_ext::PropertiesAndState { properties, state: _ },
            ) => Some(properties.id),
        } {
            if let Some(&InterfaceState { provisioning, .. }) = interface_states.get(&id.into()) {
                if provisioning == interface::ProvisioningType::Delegated {
                    // Ignore result handling, which prevents provisioning
                    // activity from starting such as DHCP and DHCPv6 clients.
                    debug!(
                        "ignoring interface watcher event because provisioning \
                    is delegated for this interface: {:?}",
                        update_result
                    );
                    return Ok(None);
                }
            }
        }

        let removed_id = Self::handle_interface_update_result(
            &update_result,
            watchers,
            dns_servers,
            dns_server_watch_responders,
            netpol_networks_service,
            interface_states,
            dhcpv4_configuration_streams,
            dhcpv6_prefixes_streams,
            lookup_admin,
            dhcp_server,
            dhcpv4_client_provider,
            dhcpv6_client_provider,
            route_set_v4_provider,
            filter_enabled_state,
            ndp_dns_servers,
            *enable_socket_proxy,
        )
        .await
        .context("handle interface update");

        let removed_id = match removed_id {
            Ok(id) => id,
            Err(e) => {
                e.accept_non_fatal()?;
                None
            }
        };

        // The interface watcher event may have disabled DHCPv6 on the interface
        // so respond accordingly.
        dhcpv6::maybe_send_watch_prefix_response(
            interface_states,
            allowed_upstream_device_classes,
            dhcpv6_prefix_provider_handler.as_mut(),
        )
        .context("maybe send PrefixControl.WatchPrefix response")?;

        virtualization_handler
            .handle_interface_update_result(&update_result)
            .await
            .context("handle interface update for virtualization")
            .or_else(errors::Error::accept_non_fatal)?;

        Ok(removed_id)
    }

    /// Processes the update result from the interface watcher, returning the
    /// ID if the interface was removed.
    // This method takes mutable references to several fields of `NetCfg` separately as parameters,
    // rather than `&mut self` directly, because `update_result` already holds a reference into
    // `self.interface_properties`.
    async fn handle_interface_update_result(
        update_result: &fnet_interfaces_ext::UpdateResult<
            '_,
            (),
            fnet_interfaces_ext::DefaultInterest,
        >,
        watchers: &mut DnsServerWatchers<'_>,
        dns_servers: &mut DnsServers,
        dns_server_watch_responders: &mut dns::DnsServerWatchResponders,
        netpol_networks_service: &mut network::NetpolNetworksService,
        interface_states: &mut HashMap<InterfaceId, InterfaceState>,
        dhcpv4_configuration_streams: &mut dhcpv4::ConfigurationStreamMap,
        dhcpv6_prefixes_streams: &mut dhcpv6::PrefixesStreamMap,
        lookup_admin: &fnet_name::LookupAdminProxy,
        dhcp_server: &Option<fnet_dhcp::Server_Proxy>,
        dhcpv4_client_provider: &Option<fnet_dhcp::ClientProviderProxy>,
        dhcpv6_client_provider: &Option<fnet_dhcpv6::ClientProviderProxy>,
        route_set_v4_provider: &fnet_routes_admin::RouteTableV4Proxy,
        filter_enabled_state: &mut FilterEnabledState,
        ndp_dns_servers: &mut HashMap<
            (InterfaceId, net_types::ip::Ipv6Addr),
            Vec<fnet_name::DnsServer_>,
        >,
        enable_socket_proxy: bool,
    ) -> Result<Option<InterfaceId>, errors::Error> {
        match update_result {
            fnet_interfaces_ext::UpdateResult::Added { properties, state: _ } => {
                if let Some(state) = interface_states.get_mut(&properties.id.into()) {
                    return state
                        .on_discovery(
                            properties,
                            dhcpv4_client_provider.as_ref(),
                            dhcpv6_client_provider.as_ref(),
                            dhcp_server.as_ref(),
                            route_set_v4_provider,
                            netpol_networks_service,
                            watchers,
                            dhcpv4_configuration_streams,
                            dhcpv6_prefixes_streams,
                            enable_socket_proxy,
                        )
                        .await
                        .context("failed to handle interface added event")
                        .map(|()| None);
                }
                // An interface netcfg won't be configuring was added, do nothing.
                return Ok(None);
            }
            fnet_interfaces_ext::UpdateResult::Existing { properties, state: _ } => {
                if let Some(state) = interface_states.get_mut(&properties.id.into()) {
                    return state
                        .on_discovery(
                            properties,
                            dhcpv4_client_provider.as_ref(),
                            dhcpv6_client_provider.as_ref(),
                            dhcp_server.as_ref(),
                            route_set_v4_provider,
                            netpol_networks_service,
                            watchers,
                            dhcpv4_configuration_streams,
                            dhcpv6_prefixes_streams,
                            enable_socket_proxy,
                        )
                        .await
                        .context("failed to handle existing interface event")
                        .map(|()| None);
                }
                // An interface netcfg won't be configuring was discovered, do nothing.
                return Ok(None);
            }
            fnet_interfaces_ext::UpdateResult::Changed {
                previous: previous_properties,
                current: current_properties,
                state: _,
            } => {
                let fnet_interfaces::Properties { online: previous_online, .. } =
                    previous_properties;
                let &fnet_interfaces_ext::Properties { id, name, online, addresses, .. } =
                    current_properties;
                match interface_states.get_mut(&(*id).into()) {
                    // An interface netcfg is not configuring was changed, do nothing.
                    None => return Ok(None),
                    Some(InterfaceState {
                        device_class,
                        config:
                            InterfaceConfigState::Host(HostInterfaceState {
                                dhcpv4_client,
                                dhcpv6_client_state,
                                dhcpv6_pd_config,
                                interface_admin_auth,
                                interface_naming_id,
                            }),
                        control,
                        provisioning,
                    }) => {
                        if previous_online.is_some() {
                            Self::handle_dhcpv4_client_update(
                                (*id).into(),
                                name,
                                *online,
                                dhcpv4_client,
                                dhcpv4_client_provider.as_ref(),
                                dhcpv4_configuration_streams,
                                dns_servers,
                                dns_server_watch_responders,
                                netpol_networks_service,
                                control,
                                lookup_admin,
                                route_set_v4_provider,
                                interface_admin_auth,
                            )
                            .await?;
                        }

                        // TODO(https://fxbug.dev/498654191): Add locally provisioned networks to
                        // the network registry even when socket-proxy is absent.
                        if enable_socket_proxy && provisioning.track_in_network_registry() {
                            // Determine whether the interface has gained or lost candidacy, and
                            // update the network registry accordingly.
                            match crate::network::determine_interface_state_changed(
                                &previous_properties,
                                &current_properties,
                            ) {
                                Some(true) => {
                                    info!(
                                        "Interface {name} (id={id}) is now a default candidate, \
                                         updating central registry"
                                    );

                                    netpol_networks_service
                                        .update(network::NetworkRegistryUpdate::ChangeNetwork(
                                            network::NetworkId::fuchsia(InterfaceId(*id)),
                                            network::NetworkUpdate::Properties(
                                                network::NetworkPropertiesChange {
                                                    added: true,
                                                    marks: None,
                                                    dns_servers: None,
                                                    connectivity_state: None,
                                                    name: Some(name.clone()),
                                                    network_type: Some((*device_class).into()),
                                                },
                                            ),
                                        ))
                                        .await;

                                    let updated_dns =
                                        netpol_networks_service.consolidated_dns_servers();
                                    dns::update_servers(
                                        lookup_admin,
                                        dns_servers,
                                        dns_server_watch_responders,
                                        netpol_networks_service,
                                        dns_server_watcher::DnsServersUpdateSource::SocketProxy,
                                        updated_dns,
                                    )
                                    .await;
                                }
                                Some(false) => {
                                    info!(
                                        "Interface {name} (id={id}) is no longer a default candidate, \
                                         removing from central registry"
                                    );

                                    netpol_networks_service
                                        .update(network::NetworkRegistryUpdate::ChangeNetwork(
                                            network::NetworkId::fuchsia(InterfaceId(*id)),
                                            network::NetworkUpdate::Remove,
                                        ))
                                        .await;

                                    let updated_dns =
                                        netpol_networks_service.consolidated_dns_servers();
                                    dns::update_servers(
                                        lookup_admin,
                                        dns_servers,
                                        dns_server_watch_responders,
                                        netpol_networks_service,
                                        dns_server_watcher::DnsServersUpdateSource::SocketProxy,
                                        updated_dns,
                                    )
                                    .await;
                                }
                                None => (),
                            }
                        }

                        let dhcpv6_client_provider =
                            if let Some(dhcpv6_client_provider) = dhcpv6_client_provider {
                                dhcpv6_client_provider
                            } else {
                                return Ok(None);
                            };

                        if !online {
                            // Stop DHCPv6 client if interface went down.
                            let dhcpv6::ClientState { sockaddr, prefixes: _ } =
                                match dhcpv6_client_state.take() {
                                    Some(s) => s,
                                    None => return Ok(None),
                                };

                            info!(
                                "host interface {} (id={}) went down \
                                so stopping DHCPv6 client w/ sockaddr = {}",
                                name,
                                id,
                                sockaddr.display_ext(),
                            );

                            dhcpv6::stop_client(
                                &lookup_admin,
                                dns_servers,
                                dns_server_watch_responders,
                                netpol_networks_service,
                                (*id).into(),
                                watchers,
                                dhcpv6_prefixes_streams,
                            )
                            .await;
                            return Ok(None);
                        }

                        // Stop the DHCPv6 client if its address can no longer be found on the
                        // interface.
                        if let Some(dhcpv6::ClientState { sockaddr, prefixes: _ }) =
                            dhcpv6_client_state
                        {
                            let &mut fnet::Ipv6SocketAddress { address, port: _, zone_index: _ } =
                                sockaddr;
                            if !addresses.iter().any(
                                |&fnet_interfaces_ext::Address {
                                     addr: fnet::Subnet { addr, prefix_len: _ },
                                     valid_until: _,
                                     preferred_lifetime_info: _,
                                     assignment_state,
                                 }| {
                                    assert_eq!(
                                        assignment_state,
                                        fnet_interfaces::AddressAssignmentState::Assigned
                                    );
                                    addr == fnet::IpAddress::Ipv6(address)
                                },
                            ) {
                                let sockaddr = *sockaddr;
                                *dhcpv6_client_state = None;

                                info!(
                                    "stopping DHCPv6 client on host interface {} (id={}) \
                                    w/ removed sockaddr = {}",
                                    name,
                                    id,
                                    sockaddr.display_ext(),
                                );

                                dhcpv6::stop_client(
                                    &lookup_admin,
                                    dns_servers,
                                    dns_server_watch_responders,
                                    netpol_networks_service,
                                    (*id).into(),
                                    watchers,
                                    dhcpv6_prefixes_streams,
                                )
                                .await;
                            }
                        }

                        // Start a DHCPv6 client if there isn't one.
                        if dhcpv6_client_state.is_none() {
                            let sockaddr = start_dhcpv6_client(
                                current_properties,
                                dhcpv6::duid(interface_naming_id.mac),
                                &dhcpv6_client_provider,
                                dhcpv6_pd_config.clone(),
                                watchers,
                                dhcpv6_prefixes_streams,
                            )?;
                            *dhcpv6_client_state = sockaddr.map(dhcpv6::ClientState::new);
                        }

                        return Ok(None);
                    }
                    Some(InterfaceState {
                        config:
                            InterfaceConfigState::WlanAp(WlanApInterfaceState {
                                interface_naming_id: _,
                            }),
                        ..
                    }) => {
                        // TODO(https://fxbug.dev/42133555): Stop the DHCP server when the address it is
                        // listening on is removed.
                        let dhcp_server = if let Some(dhcp_server) = dhcp_server {
                            dhcp_server
                        } else {
                            return Ok(None);
                        };

                        if previous_online
                            .map_or(true, |previous_online| previous_online == *online)
                        {
                            return Ok(None);
                        }

                        if *online {
                            info!(
                                "WLAN AP interface {} (id={}) came up so starting DHCP server",
                                name, id
                            );
                            dhcpv4::start_server(&dhcp_server)
                                .await
                                .context("error starting DHCP server")?;
                            return Ok(None);
                        } else {
                            info!(
                                "WLAN AP interface {} (id={}) went down so stopping DHCP server",
                                name, id
                            );
                            dhcpv4::stop_server(&dhcp_server)
                                .await
                                .context("error stopping DHCP server")?;
                            return Ok(None);
                        }
                    }
                    Some(InterfaceState {
                        config: InterfaceConfigState::Blackhole(BlackholeInterfaceState),
                        ..
                    }) => return Ok(None),
                }
            }
            fnet_interfaces_ext::UpdateResult::Removed(
                fnet_interfaces_ext::PropertiesAndState {
                    properties: fnet_interfaces_ext::Properties { id, name, .. },
                    state: (),
                },
            ) => {
                let interface_id: InterfaceId = (*id).into();
                match interface_states.remove(&interface_id) {
                    // An interface netcfg was not responsible for configuring was removed, do
                    // nothing.
                    None => return Ok(None),
                    Some(InterfaceState { config, control, provisioning, .. }) => {
                        filter_enabled_state.remove_interface(interface_id);
                        // TODO(https://fxbug.dev/498654191): Add locally provisioned networks to
                        // the network registry even when socket-proxy is absent.
                        if enable_socket_proxy {
                            if provisioning.track_in_network_registry() {
                                let network_id = network::NetworkId::fuchsia(*id);
                                netpol_networks_service
                                    .update(network::NetworkRegistryUpdate::ChangeNetwork(
                                        network_id,
                                        network::NetworkUpdate::Remove,
                                    ))
                                    .await;
                            }
                        }
                        let res = match config {
                            InterfaceConfigState::Host(HostInterfaceState {
                                mut dhcpv4_client,
                                mut dhcpv6_client_state,
                                ..
                            }) => {
                                Self::handle_dhcpv4_client_stop(
                                    interface_id,
                                    name,
                                    &mut dhcpv4_client,
                                    dhcpv4_configuration_streams,
                                    dns_servers,
                                    dns_server_watch_responders,
                                    netpol_networks_service,
                                    &control,
                                    lookup_admin,
                                    dhcpv4::AlreadyObservedClientExit::No,
                                )
                                .await;

                                // The NDP RDNSS watcher is registered
                                // unconditionally when the interface is
                                // added. We must remove the watcher prior
                                // to removing the DHCPv6 client state
                                // to avoid leaking the watcher.
                                dns::remove_rdnss_watcher(
                                    &lookup_admin,
                                    dns_servers,
                                    dns_server_watch_responders,
                                    netpol_networks_service,
                                    interface_id,
                                    watchers,
                                )
                                .await;
                                ndp_dns_servers
                                    .retain(|(iface_id, _), _| *iface_id != interface_id);

                                if let Some(dhcpv6::ClientState { sockaddr, prefixes: _ }) =
                                    dhcpv6_client_state.take()
                                {
                                    info!(
                                        "host interface {} (id={}) removed \
                                        so stopping DHCPv6 client w/ sockaddr = {}",
                                        name,
                                        id,
                                        sockaddr.display_ext()
                                    );

                                    dhcpv6::stop_client(
                                        &lookup_admin,
                                        dns_servers,
                                        dns_server_watch_responders,
                                        netpol_networks_service,
                                        interface_id,
                                        watchers,
                                        dhcpv6_prefixes_streams,
                                    )
                                    .await;
                                }

                                Ok(())
                            }
                            InterfaceConfigState::WlanAp(WlanApInterfaceState {
                                interface_naming_id: _,
                            }) => {
                                if let Some(dhcp_server) = dhcp_server {
                                    // The DHCP server should only run on the WLAN AP interface, so stop it
                                    // since the AP interface is removed.
                                    info!(
                                        "WLAN AP interface {} (id={}) is removed, stopping DHCP server",
                                        name, id
                                    );
                                    dhcpv4::stop_server(&dhcp_server)
                                        .await
                                        .context("error stopping DHCP server")
                                } else {
                                    Ok(())
                                }
                            }
                            InterfaceConfigState::Blackhole(BlackholeInterfaceState) => Ok(()),
                        };
                        return res
                            .context("failed to handle interface removed event")
                            .map(|()| Some(interface_id));
                    }
                }
            }
            fnet_interfaces_ext::UpdateResult::NoChange => return Ok(None),
        };
    }

    /// Handle an event from `D`'s device directory.
    async fn create_device_stream(
        &self,
    ) -> Result<
        impl futures::Stream<Item = Result<devices::NetworkDeviceInstance, anyhow::Error>> + use<>,
        anyhow::Error,
    > {
        let installer = self.installer.clone();
        let stream_of_streams =
            fuchsia_component::client::Service::open(fidl_fuchsia_hardware_network::ServiceMarker)
                .context("cannot open fuchsia.hardware.network.Service")?
                .watch()
                .await
                .context("cannot watch for devices")?
                .err_into()
                .try_filter_map(move |service: fidl_fuchsia_hardware_network::ServiceProxy| {
                    let installer = installer.clone();
                    async move {
                        let name = service.instance_name().to_string();
                        info!("found new network device {name}");
                        match devices::NetworkDeviceInstance::get_instance_stream(
                            &installer, service,
                        )
                        .await
                        .context("create instance stream")
                        {
                            Ok(stream) => Ok(Some(
                                stream
                                    .filter_map(move |r| {
                                        futures::future::ready(match r {
                                            Ok(instance) => Some(Ok(instance)),
                                            Err(errors::Error::NonFatal(nonfatal)) => {
                                                error!(
                                                    "non-fatal error operating device stream \
                                                    for {name:?}: {nonfatal:?}"
                                                );
                                                None
                                            }
                                            Err(errors::Error::Fatal(fatal)) => Some(Err(fatal)),
                                        })
                                    })
                                    // Need to box the stream to combine it
                                    // with flatten_unordered because it's
                                    // not Unpin.
                                    .boxed(),
                            )),
                            Err(errors::Error::NonFatal(nonfatal)) => {
                                error!(
                                    "non-fatal error fetching device stream for {:?}: {:?}",
                                    name, nonfatal
                                );
                                Ok(None)
                            }
                            Err(errors::Error::Fatal(fatal)) => Err(fatal),
                        }
                    }
                })
                .fuse()
                .try_flatten_unordered(None);
        Ok(stream_of_streams)
    }

    async fn handle_device_instance(
        &mut self,
        instance: devices::NetworkDeviceInstance,
        dns_watchers: &mut DnsServerWatchers<'_>,
    ) -> Result<(), anyhow::Error> {
        // Produce the identifier for the device to determine if it is already
        // known to Netcfg.
        let interface_naming_id =
            match self.get_interface_naming_identifier_for_instance(&instance).await {
                Ok(id) => id,
                Err(e) => {
                    error!("non-fatal error getting interface naming id {instance:?}: {e:?}");

                    return Ok(());
                }
            };

        // TODO(https://fxbug.dev/42086636): Add metrics/inspect data for devices
        // that encounter the situations below
        // Check if this device is known to Netcfg.
        // The device creation process removes an existing interface with
        // the same InterfaceNamingIdentifier. Therefore, two interfaces cannot
        // exist concurrently with the same InterfaceNamingIdentifier. This
        // should result in 0 or 1 interface to exist with the
        // provided identifier.
        match self.interface_states.iter().find_map(|(_id, state)| {
            // A device with the same InterfaceNamingId is considered to be
            // the same logical interface.
            if state.interface_naming_id() == Some(&interface_naming_id) {
                Some(state)
            } else {
                None
            }
        }) {
            // If Netcfg knows about the device already, then it's likely
            // flapped and we should uninstall the existing interface prior
            // to adding the new one.
            Some(interface_state @ InterfaceState { control, .. }) => {
                let interface_naming_id = interface_state.interface_naming_id();
                warn!(
                    "interface likely flapped. attempting to remove interface with \
                    interface naming id ({interface_naming_id:?}) prior to adding \
                    instance: {instance:?}"
                );
                match control.remove().await {
                    Ok(Ok(())) => {
                        // Successfully removed interface.
                        // It may have been that Netstack responded to the
                        // `remove()` request or Netstack noticed the flap
                        // and removed the interface on its own.
                        info!(
                            "interface removed due to reason: {:?}",
                            control.clone().wait_termination().await
                        );
                    }
                    Ok(Err(e)) => {
                        panic!("expected to be able to call remove on this interface: {e:?}")
                    }
                    Err(e) => {
                        // Do nothing. The interface was already removed.
                        info!("interface was already removed prior to calling `remove()`: {e:?}");
                    }
                }
            }
            None => {
                // Do nothing. The device is not known to Netcfg so
                // we should attempt to add the device.
            }
        }

        match self.add_new_device(&instance, dns_watchers).await {
            Ok(()) => {
                return Ok(());
            }
            Err(devices::AddDeviceError::AlreadyExists(name)) => {
                // Either the interface with this name was not installed
                // by Netcfg or another device installed by Netcfg used
                // the same name already. We will reject interface
                // installation.
                error!(
                    "interface with name ({name}) is already present in the Netstack, \
                    and the device was either not installed by Netcfg or a different \
                    device installed by Netcfg used the same name. rejecting \
                    installation for instance: {instance:?}"
                );
                return Ok(());
            }
            Err(devices::AddDeviceError::Other(errors::Error::NonFatal(e))) => {
                error!("non-fatal error adding device {:?}: {:?}", instance, e);

                return Ok(());
            }
            Err(devices::AddDeviceError::Other(errors::Error::Fatal(e))) => {
                return Err(e.context(format!("error adding new device {:?}", instance)));
            }
        }
    }

    /// Add a blackhole interface to the netstack.
    async fn add_blackhole_interface(&mut self, name: &str) -> Result<(), devices::AddDeviceError> {
        let (control, control_server_end) =
            fidl_fuchsia_net_interfaces_ext::admin::Control::create_endpoints()
                .context("create Control proxy")
                .map_err(errors::Error::NonFatal)?;
        let Metric(metric) = self.interface_metrics.blackhole_metric;
        self.installer
            .install_blackhole_interface(
                control_server_end,
                fnet_interfaces_admin::Options {
                    name: Some(name.to_owned()),
                    metric: Some(metric),
                    netstack_managed_routes_designation: None,
                    __source_breaking: fidl::marker::SourceBreaking,
                },
            )
            .context("error installing blackhole interface")
            .map_err(errors::Error::NonFatal)?;
        let interface_id = control.get_id().await.map_err(|err| {
            let other = match err {
                fidl_fuchsia_net_interfaces_ext::admin::TerminalError::Fidl(err) => err.into(),
                fidl_fuchsia_net_interfaces_ext::admin::TerminalError::Terminal(terminal_error) => {
                    match terminal_error {
                        fidl_fuchsia_net_interfaces_admin::InterfaceRemovedReason::DuplicateName => {
                            return devices::AddDeviceError::AlreadyExists(name.to_owned());
                        }
                        reason => {
                            anyhow::anyhow!("received terminal event {:?}", reason)
                        }
                    }
                }
            };
            devices::AddDeviceError::Other(
                errors::Error::NonFatal(other).context("calling Control get_id"),
            )
        })?;
        let _did_enable: bool = control
            .enable()
            .await
            .map_err(map_control_error("error sending enable request"))
            .and_then(|res| {
                // ControlEnableError is an empty *flexible* enum, so we can't match on it, but
                // the operation is infallible at the time of writing.
                res.map_err(|e: fidl_fuchsia_net_interfaces_admin::ControlEnableError| {
                    errors::Error::Fatal(anyhow::anyhow!("enable interface: {:?}", e))
                })
            })?;
        let interface_state =
            InterfaceState::new_blackhole(control, interface::ProvisioningType::Local);

        assert_matches!(
            self.interface_states
                .insert(InterfaceId::new(interface_id).expect("must be nonzero"), interface_state),
            None
        );

        info!("installed blackhole interface (id={} name={})", interface_id, name);

        Ok(())
    }

    /// Add a device at `filepath` to the netstack.
    async fn add_new_device(
        &mut self,
        device_instance: &devices::NetworkDeviceInstance,
        dns_watchers: &mut DnsServerWatchers<'_>,
    ) -> Result<(), devices::AddDeviceError> {
        let device_info =
            device_instance.get_device_info().await.context("error getting device info and MAC")?;

        let DeviceInfo { mac, port_class, topological_path } = &device_info;

        let mac = mac.ok_or_else(|| {
            warn!("devices without mac address not supported yet");
            devices::AddDeviceError::Other(errors::Error::NonFatal(anyhow::anyhow!(
                "device without mac not supported"
            )))
        })?;

        let device_class =
            DeviceClass::try_from(*port_class).map_err(|e: UnknownPortClassError| {
                devices::AddDeviceError::Other(errors::Error::NonFatal(anyhow::Error::new(e)))
            })?;
        if device_class == DeviceClass::Lowpan {
            info!(
                "lowpan interface addition detected: netcfg does not manage lowpan \
                 interfaces; filtering out"
            );
            return Ok(());
        }
        let device_info =
            DeviceInfoRef { device_class, mac: &mac, topological_path: &topological_path };

        let interface_type = device_info.interface_type();
        let metric = match interface_type {
            InterfaceType::WlanClient | InterfaceType::WlanAp => self.interface_metrics.wlan_metric,
            InterfaceType::Ethernet => self.interface_metrics.eth_metric,
            // Lowpan interfaces are not managed or assigned metrics by Netcfg.
            InterfaceType::Lowpan => unreachable!("unexpected lowpan interface type"),
            InterfaceType::Blackhole => self.interface_metrics.blackhole_metric,
        }
        .into();
        let (interface_name, interface_naming_id) = match self
            .interface_naming_config
            .generate_stable_name(&topological_path, &mac, device_class)
        {
            Ok((name, interface_naming_id)) => (name.to_string(), interface_naming_id),
            Err(interface::NameGenerationError::GenerationError(e)) => {
                return Err(devices::AddDeviceError::Other(errors::Error::Fatal(
                    e.context("error getting stable name"),
                )));
            }
        };

        info!(
            "adding {device_instance:?} to stack with name = {interface_name} and \
        naming id = {interface_naming_id:?}"
        );

        let provisioning_action = interface::find_provisioning_action_from_provisioning_rules(
            &self.interface_provisioning_policy,
            &device_info,
            &interface_name,
        );
        info!(
            "interface with name {:?} will have {:?} provisioning",
            interface_name, provisioning_action
        );

        let ProvisioningAction { provisioning, netstack_managed_routes_designation } =
            provisioning_action;

        let (interface_id, control) = device_instance
            .add_to_stack(
                self,
                InterfaceConfig {
                    name: interface_name.clone(),
                    metric,
                    netstack_managed_routes_designation,
                },
            )
            .await
            .context("error adding to stack")?;

        self.configure_eth_interface(
            interface_id.try_into().expect("interface ID should be nonzero"),
            control,
            interface_name,
            interface_naming_id,
            &device_info,
            dns_watchers,
            provisioning,
        )
        .await
        .context("error configuring ethernet interface")
        .map_err(devices::AddDeviceError::Other)
    }

    /// Configure an ethernet interface.
    ///
    /// If the device is a WLAN AP, it will be configured as a WLAN AP (see
    /// `configure_wlan_ap_and_dhcp_server` for more details). Otherwise, it
    /// will be configured as a host (see `configure_host` for more details).
    async fn configure_eth_interface(
        &mut self,
        interface_id: InterfaceId,
        control: fidl_fuchsia_net_interfaces_ext::admin::Control,
        interface_name: String,
        interface_naming_id: interface::InterfaceNamingIdentifier,
        device_info: &DeviceInfoRef<'_>,
        dns_watchers: &mut DnsServerWatchers<'_>,
        provisioning_type: ProvisioningType,
    ) -> Result<(), errors::Error> {
        let ForwardedDeviceClasses { ipv4, ipv6 } = &self.forwarded_device_classes;
        let ipv4_forwarding = ipv4.contains(&device_info.device_class);
        let ipv6_forwarding = ipv6.contains(&device_info.device_class);
        let config: fnet_interfaces_admin::Configuration = control
            .set_configuration(&fnet_interfaces_admin::Configuration {
                ipv6: Some(fnet_interfaces_admin::Ipv6Configuration {
                    unicast_forwarding: Some(ipv6_forwarding),
                    multicast_forwarding: Some(ipv6_forwarding),
                    ..Default::default()
                }),
                ipv4: Some(fnet_interfaces_admin::Ipv4Configuration {
                    unicast_forwarding: Some(ipv4_forwarding),
                    multicast_forwarding: Some(ipv4_forwarding),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .await
            .map_err(map_control_error("setting configuration"))
            .and_then(|res| {
                res.map_err(|e: fnet_interfaces_admin::ControlSetConfigurationError| {
                    errors::Error::Fatal(anyhow::anyhow!("{:?}", e))
                })
            })?;
        info!("installed configuration with result {:?}", config);

        if device_info.is_wlan_ap() {
            if let Some(id) = self
                .interface_states
                .iter()
                .find_map(|(id, state)| if state.is_wlan_ap() { Some(id) } else { None })
            {
                return Err(errors::Error::NonFatal(anyhow::anyhow!(
                    "multiple WLAN AP interfaces are not supported, \
                        have WLAN AP interface with id = {}",
                    id
                )));
            }
            let InterfaceState { control, .. } = match self.interface_states.entry(interface_id) {
                Entry::Occupied(entry) => {
                    panic!(
                        "multiple interfaces with the same ID = {}; \
                                attempting to add state for a WLAN AP, existing state = {:?}",
                        entry.key(),
                        entry.get(),
                    );
                }
                Entry::Vacant(entry) => entry.insert(InterfaceState::new_wlan_ap(
                    interface_naming_id,
                    control,
                    device_info.device_class,
                    provisioning_type,
                )),
            };

            info!("discovered WLAN AP (interface ID={})", interface_id);

            if let Some(dhcp_server) = &self.dhcp_server {
                info!("configuring DHCP server for WLAN AP (interface ID={})", interface_id);
                Self::configure_wlan_ap_and_dhcp_server(
                    &mut self.filter_enabled_state,
                    &mut self.filter_control,
                    interface_id,
                    dhcp_server,
                    control,
                    interface_name,
                    device_info,
                )
                .await
                .context("error configuring wlan ap and dhcp server")?;
            } else {
                warn!(
                    "cannot configure DHCP server for WLAN AP (interface ID={}) \
                        since DHCP server service is not available",
                    interface_id
                );
            }
        } else {
            let InterfaceState { control, .. } = match self.interface_states.entry(interface_id) {
                Entry::Occupied(entry) => {
                    panic!(
                        "multiple interfaces with the same ID = {}; \
                            attempting to add state for a host, existing state = {:?}",
                        entry.key(),
                        entry.get()
                    );
                }
                Entry::Vacant(entry) => {
                    let dhcpv6_pd_config = if !self
                        .allowed_upstream_device_classes
                        .contains(&device_info.device_class)
                    {
                        None
                    } else {
                        self.dhcpv6_prefix_provider_handler.as_ref().map(
                            |dhcpv6::PrefixProviderHandler {
                                 preferred_prefix_len,
                                 prefix_control_request_stream: _,
                                 watch_prefix_responder: _,
                                 interface_config: _,
                                 current_prefix: _,
                             }| {
                                preferred_prefix_len.map_or(
                                    fnet_dhcpv6::PrefixDelegationConfig::Empty(fnet_dhcpv6::Empty),
                                    |preferred_prefix_len| {
                                        fnet_dhcpv6::PrefixDelegationConfig::PrefixLength(
                                            preferred_prefix_len,
                                        )
                                    },
                                )
                            },
                        )
                    };
                    entry.insert(
                        InterfaceState::new_host(
                            interface_naming_id,
                            control,
                            device_info.device_class,
                            dhcpv6_pd_config,
                            provisioning_type,
                        )
                        .await?,
                    )
                }
            };

            info!("discovered host interface with id={}, configuring interface", interface_id);

            Self::configure_host(
                &mut self.filter_enabled_state,
                &mut self.filter_control,
                &self.stack,
                interface_id,
                device_info,
                // Disable in-stack DHCPv4 when provisioning is ignored.
                self.dhcpv4_client_provider.is_none()
                    && provisioning_type == interface::ProvisioningType::Local,
            )
            .await
            .context("error configuring host")?;

            if let Some(watcher_provider) = &self.route_advertisement_watcher_provider {
                dns::add_rdnss_watcher(&watcher_provider, interface_id, dns_watchers)
                    .await
                    .map_err(errors::Error::NonFatal)?;
            }

            if self.enable_socket_proxy && provisioning_type.track_in_network_registry() {
                if self.locally_provisioned_network_rule_set.is_none() {
                    self.locally_provisioned_network_rule_set = futures::try_join!(
                        install_locally_provisioned_network_rule_set::<Ipv4>(),
                        install_locally_provisioned_network_rule_set::<Ipv6>(),
                    )
                    .inspect_err(|err| {
                        log::error!(
                            "failed to create route rules for locally provisioined networks: {:?}",
                            err
                        );
                    })
                    .ok();
                }
            }

            let _did_enable: bool = control
                .enable()
                .await
                .map_err(map_control_error("error sending enable request"))
                .and_then(|res| {
                    // ControlEnableError is an empty *flexible* enum, so we can't match on it, but
                    // the operation is infallible at the time of writing.
                    res.map_err(|e: fidl_fuchsia_net_interfaces_admin::ControlEnableError| {
                        errors::Error::Fatal(anyhow::anyhow!("enable interface: {:?}", e))
                    })
                })?;
        }

        Ok(())
    }

    /// Configure host interface.
    async fn configure_host(
        filter_enabled_state: &mut FilterEnabledState,
        filter_control: &mut FilterControl,
        stack: &fnet_stack::StackProxy,
        interface_id: InterfaceId,
        device_info: &DeviceInfoRef<'_>,
        start_in_stack_dhcpv4: bool,
    ) -> Result<(), errors::Error> {
        // Handle on-demand interface enabling by using either implementation of Filter.
        filter_enabled_state
            .maybe_update(Some(device_info.interface_type()), interface_id, filter_control)
            .await
            .map_err(|e| {
                anyhow::anyhow!("failed to update filter on nic {interface_id} with error = {e:?}")
            })
            .map_err(errors::Error::NonFatal)?;

        // Enable DHCP.
        if start_in_stack_dhcpv4 {
            stack
                .set_dhcp_client_enabled(interface_id.get(), true)
                .await
                .unwrap_or_else(|err| exit_with_fidl_error(err))
                .map_err(|e| anyhow!("failed to start dhcp client: {:?}", e))
                .map_err(errors::Error::NonFatal)?;
        }

        Ok(())
    }

    /// Configure the WLAN AP and the DHCP server to serve requests on its network.
    ///
    /// Note, this method will not start the DHCP server, it will only be configured
    /// with the parameters so it is ready to be started when an interface UP event
    /// is received for the WLAN AP.
    async fn configure_wlan_ap_and_dhcp_server(
        filter_enabled_state: &mut FilterEnabledState,
        filter_control: &mut FilterControl,
        interface_id: InterfaceId,
        dhcp_server: &fnet_dhcp::Server_Proxy,
        control: &fidl_fuchsia_net_interfaces_ext::admin::Control,
        name: String,
        device_info: &DeviceInfoRef<'_>,
    ) -> Result<(), errors::Error> {
        let (address_state_provider, server_end) = fidl::endpoints::create_proxy::<
            fidl_fuchsia_net_interfaces_admin::AddressStateProviderMarker,
        >();

        // Handle on-demand interface enabling by using either implementation of Filter.
        filter_enabled_state
            .maybe_update(Some(device_info.interface_type()), interface_id, filter_control)
            .await
            .map_err(|e| {
                anyhow::anyhow!("failed to update filter on nic {interface_id} with error = {e:?}")
            })
            .map_err(errors::Error::NonFatal)?;

        // Calculate and set the interface address based on the network address.
        // The interface address should be the first available address.
        let network_addr_as_u32 = u32::from_be_bytes(WLAN_AP_NETWORK_ADDR.addr);
        let interface_addr_as_u32 = network_addr_as_u32 + 1;
        let ipv4 = fnet::Ipv4Address { addr: interface_addr_as_u32.to_be_bytes() };
        let addr = fidl_fuchsia_net::Subnet {
            addr: fnet::IpAddress::Ipv4(ipv4.clone()),
            prefix_len: WLAN_AP_PREFIX_LEN.get(),
        };

        control
            .add_address(
                &addr,
                &fidl_fuchsia_net_interfaces_admin::AddressParameters {
                    add_subnet_route: Some(true),
                    ..Default::default()
                },
                server_end,
            )
            .map_err(map_control_error("error sending add address request"))?;

        // Allow the address to outlive this scope. At the time of writing its lifetime is
        // identical to the interface's lifetime and no updates to its properties are made. We may
        // wish to retain the handle in the future to allow external address removal (e.g. by a
        // user) to be observed so that an error can be emitted (as such removal would break a
        // critical user journey).
        address_state_provider
            .detach()
            .map_err(Into::into)
            .map_err(map_address_state_provider_error("error sending detach request"))?;

        // Enable the interface to allow DAD to proceed.
        let _did_enable: bool = control
            .enable()
            .await
            .map_err(map_control_error("error sending enable request"))
            .and_then(|res| {
                // ControlEnableError is an empty *flexible* enum, so we can't match on it, but the
                // operation is infallible at the time of writing.
                res.map_err(|e: fidl_fuchsia_net_interfaces_admin::ControlEnableError| {
                    errors::Error::Fatal(anyhow::anyhow!("enable interface: {:?}", e))
                })
            })?;

        let state_stream =
            fidl_fuchsia_net_interfaces_ext::admin::assignment_state_stream(address_state_provider);
        let mut state_stream = pin!(state_stream);
        fidl_fuchsia_net_interfaces_ext::admin::wait_assignment_state(
            &mut state_stream,
            fidl_fuchsia_net_interfaces::AddressAssignmentState::Assigned,
        )
        .await
        .map_err(map_address_state_provider_error("failed to add interface address for WLAN AP"))?;

        // First we clear any leases that the server knows about since the server
        // will be used on a new interface. If leases exist, configuring the DHCP
        // server parameters may fail (AddressPool).
        debug!("clearing DHCP leases");
        dhcp_server
            .clear_leases()
            .await
            .context("error sending clear DHCP leases request")
            .map_err(errors::Error::NonFatal)?
            .map_err(zx::Status::err_from_raw)
            .context("error clearing DHCP leases request")
            .map_err(errors::Error::NonFatal)?;

        // Configure the DHCP server.
        let v = vec![ipv4];
        debug!("setting DHCP IpAddrs parameter to {:?}", v);
        dhcp_server
            .set_parameter(&fnet_dhcp::Parameter::IpAddrs(v))
            .await
            .context("error sending set DHCP IpAddrs parameter request")
            .map_err(errors::Error::NonFatal)?
            .map_err(zx::Status::err_from_raw)
            .context("error setting DHCP IpAddrs parameter")
            .map_err(errors::Error::NonFatal)?;

        let v = vec![name];
        debug!("setting DHCP BoundDeviceNames parameter to {:?}", v);
        dhcp_server
            .set_parameter(&fnet_dhcp::Parameter::BoundDeviceNames(v))
            .await
            .context("error sending set DHCP BoundDeviceName parameter request")
            .map_err(errors::Error::NonFatal)?
            .map_err(zx::Status::err_from_raw)
            .context("error setting DHCP BoundDeviceNames parameter")
            .map_err(errors::Error::NonFatal)?;

        let v = fnet_dhcp::LeaseLength {
            default: Some(WLAN_AP_DHCP_LEASE_TIME_SECONDS),
            max: Some(WLAN_AP_DHCP_LEASE_TIME_SECONDS),
            ..Default::default()
        };
        debug!("setting DHCP LeaseLength parameter to {:?}", v);
        dhcp_server
            .set_parameter(&fnet_dhcp::Parameter::Lease(v))
            .await
            .context("error sending set DHCP LeaseLength parameter request")
            .map_err(errors::Error::NonFatal)?
            .map_err(zx::Status::err_from_raw)
            .context("error setting DHCP LeaseLength parameter")
            .map_err(errors::Error::NonFatal)?;

        let host_mask = ::dhcpv4::configuration::SubnetMask::new(WLAN_AP_PREFIX_LEN);
        let broadcast_addr =
            host_mask.broadcast_of(&std::net::Ipv4Addr::from_fidl(WLAN_AP_NETWORK_ADDR));
        let broadcast_addr_as_u32: u32 = broadcast_addr.into();
        // The start address of the DHCP pool should be the first address after the WLAN AP
        // interface address.
        let dhcp_pool_start =
            fnet::Ipv4Address { addr: (interface_addr_as_u32 + 1).to_be_bytes() }.into();
        // The last address of the DHCP pool should be the last available address
        // in the interface's network. This is the address immediately before the
        // network's broadcast address.
        let dhcp_pool_end = fnet::Ipv4Address { addr: (broadcast_addr_as_u32 - 1).to_be_bytes() };

        let v = fnet_dhcp::AddressPool {
            prefix_length: Some(WLAN_AP_PREFIX_LEN.get()),
            range_start: Some(dhcp_pool_start),
            range_stop: Some(dhcp_pool_end),
            ..Default::default()
        };
        debug!("setting DHCP AddressPool parameter to {:?}", v);
        dhcp_server
            .set_parameter(&fnet_dhcp::Parameter::AddressPool(v))
            .await
            .context("error sending set DHCP AddressPool parameter request")
            .map_err(errors::Error::NonFatal)?
            .map_err(zx::Status::err_from_raw)
            .context("error setting DHCP AddressPool parameter")
            .map_err(errors::Error::NonFatal)
    }

    async fn handle_dhcpv6_acquire_prefix(
        &mut self,
        fnet_dhcpv6::AcquirePrefixConfig {
            interface_id,
            preferred_prefix_len,
            ..
        }: fnet_dhcpv6::AcquirePrefixConfig,
        prefix: fidl::endpoints::ServerEnd<fnet_dhcpv6::PrefixControlMarker>,
        dns_watchers: &mut DnsServerWatchers<'_>,
    ) -> Result<(), errors::Error> {
        let (prefix_control_request_stream, control_handle) =
            prefix.into_stream_and_control_handle();

        let dhcpv6_client_provider = if let Some(s) = self.dhcpv6_client_provider.as_ref() {
            s
        } else {
            warn!(
                "Attempted to acquire prefix when DHCPv6 is not \
                supported; interface_id={:?}, preferred_prefix_len={:?}",
                interface_id, preferred_prefix_len,
            );
            return control_handle
                .send_on_exit(fnet_dhcpv6::PrefixControlExitReason::NotSupported)
                .context("failed to send NotSupported terminal event")
                .map_err(errors::Error::NonFatal);
        };

        let interface_config = if let Some(interface_id) = interface_id {
            match self
                .interface_states
                .get(&interface_id.try_into().expect("interface ID should be nonzero"))
            {
                None => {
                    warn!(
                        "Attempted to acquire a prefix on unmanaged interface; \
                        id={:?}, preferred_prefix_len={:?}",
                        interface_id, preferred_prefix_len,
                    );
                    return control_handle
                        .send_on_exit(fnet_dhcpv6::PrefixControlExitReason::InvalidInterface)
                        .context("failed to send InvalidInterface terminal event")
                        .map_err(errors::Error::NonFatal);
                }
                Some(InterfaceState { config: InterfaceConfigState::WlanAp(_), .. }) => {
                    warn!(
                        "Attempted to acquire a prefix on AP interface; \
                        id={:?}, preferred_prefix_len={:?}",
                        interface_id, preferred_prefix_len,
                    );
                    return control_handle
                        .send_on_exit(fnet_dhcpv6::PrefixControlExitReason::InvalidInterface)
                        .context("failed to send InvalidInterface terminal event")
                        .map_err(errors::Error::NonFatal);
                }
                Some(InterfaceState { config: InterfaceConfigState::Blackhole(_), .. }) => {
                    warn!(
                        "Attempted to acquire a prefix on blackhole interface; \
                        id={:?}, preferred_prefix_len={:?}",
                        interface_id, preferred_prefix_len,
                    );
                    return control_handle
                        .send_on_exit(fnet_dhcpv6::PrefixControlExitReason::InvalidInterface)
                        .context("failed to send InvalidInterface terminal event")
                        .map_err(errors::Error::NonFatal);
                }
                Some(InterfaceState {
                    config:
                        InterfaceConfigState::Host(HostInterfaceState {
                            dhcpv4_client: _,
                            dhcpv6_client_state: _,
                            dhcpv6_pd_config: _,
                            interface_admin_auth: _,
                            interface_naming_id: _,
                        }),
                    ..
                }) => dhcpv6::AcquirePrefixInterfaceConfig::Id(interface_id),
            }
        } else {
            dhcpv6::AcquirePrefixInterfaceConfig::Upstreams
        };
        let pd_config = if let Some(preferred_prefix_len) = preferred_prefix_len {
            if preferred_prefix_len > net_types::ip::Ipv6Addr::BYTES * 8 {
                warn!(
                    "Preferred prefix length exceeds bits in IPv6 address; \
                    interface_id={:?}, preferred_prefix_len={:?}",
                    interface_id, preferred_prefix_len,
                );
                return control_handle
                    .send_on_exit(fnet_dhcpv6::PrefixControlExitReason::InvalidPrefixLength)
                    .context("failed to send InvalidPrefixLength terminal event")
                    .map_err(errors::Error::NonFatal);
            }
            fnet_dhcpv6::PrefixDelegationConfig::PrefixLength(preferred_prefix_len)
        } else {
            fnet_dhcpv6::PrefixDelegationConfig::Empty(fnet_dhcpv6::Empty)
        };

        // TODO(https://fxbug.dev/142065403): Support multiple clients asking
        // for IPv6 prefixes.
        if self.dhcpv6_prefix_provider_handler.is_some() {
            warn!(
                "Attempted to acquire a prefix while a prefix is already being acquired for \
                another iface; interface_id={:?}, preferred_prefix_len={:?}",
                interface_id, preferred_prefix_len,
            );
            return control_handle
                .send_on_exit(fnet_dhcpv6::PrefixControlExitReason::AlreadyAcquiring)
                .context("failed to send AlreadyAcquiring terminal event")
                .map_err(errors::Error::NonFatal);
        }

        let interface_state_iter = match interface_config {
            dhcpv6::AcquirePrefixInterfaceConfig::Id(want_id) => {
                let want_id = want_id.try_into().expect("interface ID should be nonzero");
                either::Either::Left(std::iter::once((
                    want_id,
                    self.interface_states
                        .get_mut(&want_id)
                        .unwrap_or_else(|| panic!("interface {} state not present", want_id)),
                )))
            }
            dhcpv6::AcquirePrefixInterfaceConfig::Upstreams => either::Either::Right(
                self.interface_states.iter_mut().filter_map(|(id, if_state)| {
                    self.allowed_upstream_device_classes
                        .contains(&if_state.device_class)
                        .then_some((*id, if_state))
                }),
            ),
        };
        // Look for all eligible interfaces and start/restart DHCPv6 client as needed.
        for (id, InterfaceState { config, .. }) in interface_state_iter {
            let HostInterfaceState {
                dhcpv4_client: _,
                dhcpv6_client_state,
                dhcpv6_pd_config,
                interface_admin_auth: _,
                interface_naming_id,
            } = match config {
                InterfaceConfigState::Host(state) => state,
                InterfaceConfigState::WlanAp(WlanApInterfaceState { interface_naming_id: _ })
                | InterfaceConfigState::Blackhole(_) => {
                    continue;
                }
            };

            // Save the config so that future DHCPv6 clients are started with PD.
            *dhcpv6_pd_config = Some(pd_config.clone());

            let fnet_interfaces_ext::PropertiesAndState { properties, state: _ } =
                if let Some(properties) = self.interface_properties.get(&id) {
                    properties
                } else {
                    // There is a delay between when netcfg installs and when said interface's
                    // properties are received via the interface watcher and ends up in
                    // `interface_properties`, so if the properties are not yet known, simply
                    // continue. The DHCPv6 client on such an interface will be started with PD
                    // configured per the usual process when handling interface watcher events.
                    continue;
                };

            // TODO(https://fxbug.dev/42068818): Reload configuration in-place rather than
            // restarting the DHCPv6 client with different configuration.
            // Stop DHCPv6 client if it's running.
            if let Some::<dhcpv6::ClientState>(_) = dhcpv6_client_state.take() {
                dhcpv6::stop_client(
                    &self.lookup_admin,
                    &mut self.dns_servers,
                    &mut self.dns_server_watch_responders,
                    &mut self.netpol_networks_service,
                    id,
                    dns_watchers,
                    &mut self.dhcpv6_prefixes_streams,
                )
                .await;
            }

            // Restart DHCPv6 client and configure it to perform PD.
            let sockaddr = match start_dhcpv6_client(
                properties,
                dhcpv6::duid(interface_naming_id.mac),
                &dhcpv6_client_provider,
                Some(pd_config.clone()),
                dns_watchers,
                &mut self.dhcpv6_prefixes_streams,
            )
            .context("starting DHCPv6 client with PD")
            {
                Ok(dhcpv6_client_addr) => dhcpv6_client_addr,
                Err(errors::Error::NonFatal(e)) => {
                    warn!("error restarting DHCPv6 client to perform PD: {:?}", e);
                    None
                }
                Err(errors::Error::Fatal(e)) => {
                    panic!("error restarting DHCPv6 client to perform PD: {:?}", e);
                }
            };
            *dhcpv6_client_state = sockaddr.map(dhcpv6::ClientState::new);
        }
        self.dhcpv6_prefix_provider_handler = Some(dhcpv6::PrefixProviderHandler {
            prefix_control_request_stream,
            watch_prefix_responder: None,
            current_prefix: None,
            interface_config,
            preferred_prefix_len,
        });
        Ok(())
    }

    async fn handle_watch_prefix(
        &mut self,
        responder: fnet_dhcpv6::PrefixControlWatchPrefixResponder,
        dns_watchers: &mut DnsServerWatchers<'_>,
    ) -> Result<(), anyhow::Error> {
        let dhcpv6::PrefixProviderHandler {
            watch_prefix_responder,
            interface_config: _,
            prefix_control_request_stream: _,
            preferred_prefix_len: _,
            current_prefix: _,
        } = self
            .dhcpv6_prefix_provider_handler
            .as_mut()
            .expect("DHCPv6 prefix provider handler must be present to handle WatchPrefix");
        if let Some(responder) = watch_prefix_responder.take() {
            warn!("Attempted to call WatchPrefix twice on PrefixControl channel, closing channel");
            match responder
                .control_handle()
                .send_on_exit(fnet_dhcpv6::PrefixControlExitReason::DoubleWatch)
            {
                Err(e) => {
                    warn!(
                        "failed to send DoubleWatch terminal event on PrefixControl channel: {:?}",
                        e
                    );
                }
                Ok(()) => {}
            }
            self.on_dhcpv6_prefix_control_close(dns_watchers).await;
            Ok(())
        } else {
            *watch_prefix_responder = Some(responder);

            dhcpv6::maybe_send_watch_prefix_response(
                &self.interface_states,
                &self.allowed_upstream_device_classes,
                self.dhcpv6_prefix_provider_handler.as_mut(),
            )
        }
    }

    async fn on_dhcpv6_prefix_control_close(&mut self, dns_watchers: &mut DnsServerWatchers<'_>) {
        let _: dhcpv6::PrefixProviderHandler = self
            .dhcpv6_prefix_provider_handler
            .take()
            .expect("DHCPv6 prefix provider handler must be present");
        let dhcpv6_client_provider =
            self.dhcpv6_client_provider.as_ref().expect("DHCPv6 client provider must be present");
        for (id, InterfaceState { config, .. }) in self.interface_states.iter_mut() {
            let (dhcpv6_client_state, interface_naming_id) = match config {
                InterfaceConfigState::WlanAp(WlanApInterfaceState { interface_naming_id: _ })
                | InterfaceConfigState::Blackhole(_) => {
                    continue;
                }
                InterfaceConfigState::Host(HostInterfaceState {
                    dhcpv4_client: _,
                    dhcpv6_client_state,
                    dhcpv6_pd_config,
                    interface_admin_auth: _,
                    interface_naming_id,
                }) => {
                    if dhcpv6_pd_config.take().is_none() {
                        continue;
                    }
                    match dhcpv6_client_state.take() {
                        Some(_) => (dhcpv6_client_state, interface_naming_id),
                        None => continue,
                    }
                }
            };

            // TODO(https://fxbug.dev/42068818): Reload configuration in-place rather than
            // restarting the DHCPv6 client with different configuration.
            // Stop DHCPv6 client if it's running.
            dhcpv6::stop_client(
                &self.lookup_admin,
                &mut self.dns_servers,
                &mut self.dns_server_watch_responders,
                &mut self.netpol_networks_service,
                *id,
                dns_watchers,
                &mut self.dhcpv6_prefixes_streams,
            )
            .await;

            let fnet_interfaces_ext::PropertiesAndState { properties, state: _ } =
                self.interface_properties.get(id).unwrap_or_else(|| {
                    panic!("interface {} has DHCPv6 client but properties unknown", id)
                });

            // Restart DHCPv6 client without PD.
            let sockaddr = match start_dhcpv6_client(
                properties,
                dhcpv6::duid(interface_naming_id.mac),
                &dhcpv6_client_provider,
                None,
                dns_watchers,
                &mut self.dhcpv6_prefixes_streams,
            )
            .context("starting DHCPv6 client with PD")
            {
                Ok(dhcpv6_client_addr) => dhcpv6_client_addr,
                Err(errors::Error::NonFatal(e)) => {
                    warn!("restarting DHCPv6 client to stop PD: {:?}", e);
                    None
                }
                Err(e @ errors::Error::Fatal(_)) => {
                    panic!("restarting DHCPv6 client to stop PD: {:?}", e);
                }
            };
            *dhcpv6_client_state = sockaddr.map(dhcpv6::ClientState::new);
        }
    }

    async fn handle_dhcpv4_configuration(
        &mut self,
        interface_id: InterfaceId,
        res: Result<fnet_dhcp_ext::Configuration, fnet_dhcp_ext::Error>,
    ) -> Dhcpv4ConfigurationHandlerResult {
        let configuration = match res {
            Err(error) => {
                let allow_restart = match error {
                    fnet_dhcp_ext::Error::UnexpectedExit(reason) => match reason {
                        Some(reason) => match reason {
                            fnet_dhcp::ClientExitReason::AddressRemovedByUser => {
                                log::warn!(
                                    "DHCP client exited because its \
                                    bound address was removed (iface={interface_id})"
                                );
                                // The user intentionally removed the DHCP client's address, so we
                                // shouldn't automatically restart the client.
                                AllowClientRestart::No
                            }
                            reason @ (
                                fnet_dhcp::ClientExitReason::ClientAlreadyExistsOnInterface
                                | fnet_dhcp::ClientExitReason::WatchConfigurationAlreadyPending
                                | fnet_dhcp::ClientExitReason::InvalidInterface
                                | fnet_dhcp::ClientExitReason::InvalidParams
                                | fnet_dhcp::ClientExitReason::NetworkUnreachable
                                | fnet_dhcp::ClientExitReason::UnableToOpenSocket
                                | fnet_dhcp::ClientExitReason::GracefulShutdown
                                | fnet_dhcp::ClientExitReason::AddressStateProviderError
                            ) => {
                                log::error!("DHCP client unexpectedly exited \
                                                (iface={interface_id}, reason={reason:?})");
                                // The exit was unexpected, so we should restart the client.
                                AllowClientRestart::Yes
                            }
                        },
                        None => {
                            log::error!(
                                "DHCP client unexpectedly exited without \
                                giving a reason (iface={interface_id})"
                            );
                            AllowClientRestart::Yes
                        }
                    },
                    error @ (fnet_dhcp_ext::Error::ApiViolation(_)
                    | fnet_dhcp_ext::Error::RouteSet(_)
                    | fnet_dhcp_ext::Error::Fidl(_)
                    | fnet_dhcp_ext::Error::WrongExitReason(_)
                    | fnet_dhcp_ext::Error::MissingExitReason) => {
                        log::error!("DHCP client exited due to error \
                                        (iface={interface_id}, error={error:?})");
                                        AllowClientRestart::Yes
                    }
                };
                return Dhcpv4ConfigurationHandlerResult::ClientStopped(allow_restart);
            }
            Ok(configuration) => configuration,
        };

        let (dhcpv4_client, control) = {
            let InterfaceState { config, control, .. } = self
                .interface_states
                .get_mut(&interface_id)
                .unwrap_or_else(|| panic!("interface {} not found", interface_id));

            match config {
                InterfaceConfigState::Host(HostInterfaceState {
                    dhcpv4_client,
                    dhcpv6_client_state: _,
                    dhcpv6_pd_config: _,
                    interface_admin_auth: _,
                    interface_naming_id: _,
                }) => (
                    match dhcpv4_client {
                        Dhcpv4ClientState::Running(client) => client,
                        Dhcpv4ClientState::NotRunning | Dhcpv4ClientState::ScheduledRestart(_) => {
                            panic!(
                                "interface {} does not have a running DHCPv4 client",
                                interface_id
                            )
                        }
                    },
                    control,
                ),
                InterfaceConfigState::WlanAp(wlan_ap_state) => {
                    panic!(
                        "interface {} expected to be host but is WLAN AP with state {:?}",
                        interface_id, wlan_ap_state
                    );
                }
                InterfaceConfigState::Blackhole(state) => {
                    panic!(
                        "interface {} expected to be host but is blackhole with state {:?}",
                        interface_id, state
                    );
                }
            }
        };

        dhcpv4::update_configuration(
            interface_id,
            dhcpv4_client,
            configuration,
            &mut self.dns_servers,
            &mut self.dns_server_watch_responders,
            &mut self.netpol_networks_service,
            control,
            &self.lookup_admin,
        )
        .await;

        Dhcpv4ConfigurationHandlerResult::ContinueOperation
    }

    async fn handle_dhcpv6_prefixes(
        &mut self,
        interface_id: InterfaceId,
        res: Result<Vec<fnet_dhcpv6::Prefix>, fidl::Error>,
        dns_watchers: &mut DnsServerWatchers<'_>,
    ) -> Result<(), anyhow::Error> {
        let new_prefixes = match res
            .context("DHCPv6 prefixes stream FIDL error")
            .and_then(|prefixes| dhcpv6::from_fidl_prefixes(&prefixes))
        {
            Err(e) => {
                warn!(
                    "DHCPv6 client on interface={} error on watch_prefixes: {:?}",
                    interface_id, e
                );
                dhcpv6::stop_client(
                    &self.lookup_admin,
                    &mut self.dns_servers,
                    &mut self.dns_server_watch_responders,
                    &mut self.netpol_networks_service,
                    interface_id,
                    dns_watchers,
                    &mut self.dhcpv6_prefixes_streams,
                )
                .await;
                HashMap::new()
            }
            Ok(prefixes_map) => prefixes_map,
        };
        let InterfaceState { config, .. } = self
            .interface_states
            .get_mut(&interface_id)
            .unwrap_or_else(|| panic!("interface {} not found", interface_id));
        let dhcpv6::ClientState { prefixes, sockaddr: _ } = match config {
            InterfaceConfigState::Host(HostInterfaceState {
                dhcpv4_client: _,
                dhcpv6_client_state,
                dhcpv6_pd_config: _,
                interface_admin_auth: _,
                interface_naming_id: _,
            }) => dhcpv6_client_state.as_mut().unwrap_or_else(|| {
                panic!("interface {} does not have a running DHCPv6 client", interface_id)
            }),
            InterfaceConfigState::WlanAp(wlan_ap_state) => {
                panic!(
                    "interface {} expected to be host but is WLAN AP with state {:?}",
                    interface_id, wlan_ap_state
                );
            }
            InterfaceConfigState::Blackhole(state) => {
                panic!(
                    "interface {} expected to be host but is blackhole with state {:?}",
                    interface_id, state
                );
            }
        };
        *prefixes = new_prefixes;

        dhcpv6::maybe_send_watch_prefix_response(
            &self.interface_states,
            &self.allowed_upstream_device_classes,
            self.dhcpv6_prefix_provider_handler.as_mut(),
        )
    }

    async fn get_interface_naming_identifier_for_instance(
        &self,
        device_instance: &devices::NetworkDeviceInstance,
    ) -> Result<interface::InterfaceNamingIdentifier, anyhow::Error> {
        let device_info = device_instance
            .get_device_info()
            .await
            .context("error getting device info and MAC")
            .map_err(|e| match e {
                errors::Error::NonFatal(e) | errors::Error::Fatal(e) => e,
            })?;

        let DeviceInfo { mac, port_class: _, topological_path } = &device_info;
        let mac = mac.ok_or_else(|| anyhow!("devices without mac not supported"))?;

        Ok(interface::generate_identifier(&mac, topological_path))
    }

    async fn handle_ndp_dns_expired(
        &mut self,
        interface_id: crate::InterfaceId,
        router_address: net_types::ip::Ipv6Addr,
    ) {
        info!(
            "NDP DNS servers expired for router {:?} on interface {:?}",
            router_address, interface_id
        );
        let key = (interface_id, router_address);
        let _: Option<Vec<fnet_name::DnsServer_>> = self.ndp_dns_servers.remove(&key);
        let flattened_servers = self
            .ndp_dns_servers
            .iter()
            .filter(|((iface_id, _), _)| *iface_id == interface_id)
            .flat_map(|(_, servers)| servers.iter().cloned())
            .collect::<Vec<_>>();
        self.update_dns_servers(
            DnsServersUpdateSource::Ndp { interface_id: interface_id.into() },
            flattened_servers,
        )
        .await;
    }
}

pub async fn run<M: Mode>() -> Result<(), anyhow::Error> {
    let opt: Opt = argh::from_env();
    let Opt { min_severity: LogLevel(min_severity), config_data } = &opt;

    // Use the diagnostics_log library directly rather than e.g. the #[fuchsia::main] macro on
    // the main function, so that we can specify the logging severity level at runtime based on a
    // command line argument.
    diagnostics_log::initialize(
        diagnostics_log::PublishOptions::default().minimum_severity(*min_severity),
    )?;

    info!("starting");
    debug!("starting with options = {:?}", opt);

    let Config {
        dns_config: DnsConfig { servers },
        filter_config,
        filter_enabled_interface_types,
        interface_metrics,
        allowed_upstream_device_classes: AllowedDeviceClasses(allowed_upstream_device_classes),
        allowed_bridge_upstream_device_classes:
            AllowedDeviceClasses(allowed_bridge_upstream_device_classes),
        enable_dhcpv6,
        forwarded_device_classes,
        interface_naming_policy,
        interface_provisioning_policy,
        blackhole_interfaces,
        enable_socket_proxy,
    } = Config::load(config_data)?;

    // LINT.IfChange(netcfg_naming_policy_tefmo)
    info!("using naming policy: {interface_naming_policy:?}");
    // LINT.ThenChange(//tools/testing/tefmocheck/cdc_ethernet_state_check.go:netcfg_naming_policy_tefmo)
    info!("using provisioning policy: {interface_provisioning_policy:?}");

    let inspector = fuchsia_inspect::component::inspector();
    // Report data on the size of the inspect VMO, and the number of allocation
    // failures encountered. (Allocation failures can lead to missing data.)
    fuchsia_inspect::component::serve_inspect_stats();

    let mut netcfg = NetCfg::new(
        filter_enabled_interface_types,
        interface_metrics,
        enable_dhcpv6,
        forwarded_device_classes,
        &allowed_upstream_device_classes,
        interface_naming_policy,
        interface_provisioning_policy,
        enable_socket_proxy,
        inspector.clone(),
    )
    .await
    .context("error creating new netcfg instance")?;

    for name in &blackhole_interfaces {
        let result = netcfg.add_blackhole_interface(name).await;

        match result {
            Ok(()) => {}
            Err(devices::AddDeviceError::AlreadyExists(name)) => {
                // Either the interface with this name was not installed
                // by Netcfg or another device installed by Netcfg used
                // the same name already. We will reject interface
                // installation.
                error!(
                    "interface with name ({name}) is already present in the Netstack, \
                    so we're rejecting installation of a blackhole interface with that name."
                );
            }
            Err(devices::AddDeviceError::Other(errors::Error::NonFatal(e))) => {
                error!("non-fatal error adding blackhole interface {:?}: {:?}", name, e);
            }
            Err(devices::AddDeviceError::Other(errors::Error::Fatal(e))) => {
                return Err(e.context(format!("error adding new blackhole interface {:?}", name)));
            }
        }
    }

    // TODO(https://fxbug.dev/42080661): Once non-Fuchsia components can control filtering rules, disable
    // setting filters when interfaces are in Delegated provisioning mode.
    netcfg
        .filter_control
        .update_filters(filter_config, &netcfg.filter_enabled_state)
        .await
        .context("update filters based on config")?;

    // TODO(https://fxbug.dev/42080096): Once non-Fuchsia components can control DNS servers, disable
    // setting default DNS servers when interfaces are in Delegated provisioning mode.
    let servers = servers.into_iter().map(static_source_from_ip).collect();
    debug!("updating default servers to {:?}", servers);
    netcfg.update_dns_servers(DnsServersUpdateSource::Default, servers).await;

    M::run(netcfg, allowed_bridge_upstream_device_classes)
        .map_err(|e| {
            let err_str = format!("fatal error running main: {:?}", e);
            error!("{}", err_str);
            anyhow!(err_str)
        })
        .await
}

/// Allows callers of `netcfg::run` to configure at compile time which features
/// should be enabled.
///
/// This trait may be expanded to support combinations of features that can be
/// assembled together for specific netcfg builds.
#[async_trait(?Send)]
pub trait Mode {
    async fn run<'a>(
        netcfg: NetCfg<'a>,
        allowed_bridge_upstream_device_classes: HashSet<DeviceClass>,
    ) -> Result<(), anyhow::Error>;
}

/// In this configuration, netcfg acts as the policy manager for netstack,
/// watching for device events and configuring the netstack with new interfaces
/// as needed on new device discovery. It does not implement any FIDL protocols.
pub enum BasicMode {}

#[async_trait(?Send)]
impl Mode for BasicMode {
    async fn run<'a>(
        mut netcfg: NetCfg<'a>,
        _allowed_bridge_upstream_device_classes: HashSet<DeviceClass>,
    ) -> Result<(), anyhow::Error> {
        netcfg.run(virtualization::Stub).await.context("event loop")
    }
}

/// In this configuration, netcfg implements the base functionality included in
/// `BasicMode`, and also serves the `fuchsia.net.virtualization/Control`
/// protocol, allowing clients to create virtual networks.
pub enum VirtualizationEnabled {}

#[async_trait(?Send)]
impl Mode for VirtualizationEnabled {
    async fn run<'a>(
        mut netcfg: NetCfg<'a>,
        allowed_bridge_upstream_device_classes: HashSet<DeviceClass>,
    ) -> Result<(), anyhow::Error> {
        let handler = virtualization::Virtualization::new(
            netcfg.allowed_upstream_device_classes,
            allowed_bridge_upstream_device_classes,
            virtualization::BridgeHandlerImpl::new(netcfg.stack.clone()),
            netcfg.installer.clone(),
        );
        netcfg.run(handler).await.context("event loop")
    }
}

fn map_control_error(
    ctx: &'static str,
) -> impl FnOnce(
    fnet_interfaces_ext::admin::TerminalError<fnet_interfaces_admin::InterfaceRemovedReason>,
) -> errors::Error {
    move |e| {
        let severity = match &e {
            fidl_fuchsia_net_interfaces_ext::admin::TerminalError::Fidl(e) => {
                if e.is_closed() {
                    // Control handle can close when interface is
                    // removed; not a fatal error.
                    errors::Error::NonFatal
                } else {
                    errors::Error::Fatal
                }
            }
            fidl_fuchsia_net_interfaces_ext::admin::TerminalError::Terminal(e) => match e {
                fidl_fuchsia_net_interfaces_admin::InterfaceRemovedReason::DuplicateName
                | fidl_fuchsia_net_interfaces_admin::InterfaceRemovedReason::PortAlreadyBound
                | fidl_fuchsia_net_interfaces_admin::InterfaceRemovedReason::BadPort => {
                    errors::Error::Fatal
                }
                fidl_fuchsia_net_interfaces_admin::InterfaceRemovedReason::PortClosed
                | fidl_fuchsia_net_interfaces_admin::InterfaceRemovedReason::User
                | _ => errors::Error::NonFatal,
            },
        };
        severity(anyhow::Error::new(e).context(ctx))
    }
}

fn map_address_state_provider_error(
    ctx: &'static str,
) -> impl FnOnce(fnet_interfaces_ext::admin::AddressStateProviderError) -> errors::Error {
    move |e| {
        let severity = match &e {
            fnet_interfaces_ext::admin::AddressStateProviderError::Fidl(e) => {
                if e.is_closed() {
                    // TODO(https://fxbug.dev/42170615): Reconsider whether this
                    // should be a fatal error, as it can be caused by a
                    // netstack bug.
                    errors::Error::NonFatal
                } else {
                    errors::Error::Fatal
                }
            }
            fnet_interfaces_ext::admin::AddressStateProviderError::ChannelClosed => {
                // TODO(https://fxbug.dev/42170615): Reconsider whether this should
                // be a fatal error, as it can be caused by a netstack bug.
                errors::Error::NonFatal
            }
            fnet_interfaces_ext::admin::AddressStateProviderError::AddressRemoved(e) => match e {
                fidl_fuchsia_net_interfaces_admin::AddressRemovalReason::Invalid
                | fidl_fuchsia_net_interfaces_admin::AddressRemovalReason::InvalidProperties => {
                    errors::Error::Fatal
                }
                fidl_fuchsia_net_interfaces_admin::AddressRemovalReason::AlreadyAssigned
                | fidl_fuchsia_net_interfaces_admin::AddressRemovalReason::DadFailed
                | fidl_fuchsia_net_interfaces_admin::AddressRemovalReason::Forfeited
                | fidl_fuchsia_net_interfaces_admin::AddressRemovalReason::InterfaceRemoved
                | fidl_fuchsia_net_interfaces_admin::AddressRemovalReason::UserRemoved => {
                    errors::Error::NonFatal
                }
            },
        };
        severity(anyhow::Error::new(e).context(ctx))
    }
}

/// If we can't reach netstack via fidl, log an error and exit.
//
// TODO(https://fxbug.dev/42070352): add a test that works as intended.
pub(crate) fn exit_with_fidl_error(cause: fidl::Error) -> ! {
    error!(cause:%; "exiting due to fidl error");
    std::process::exit(1);
}

/// Installs the rule that directs all unmarked sockets to lookup the main table
/// where all the locally-provisioned network related routes live.
///
/// This needs to have a high priority so that it is not overridden later.
async fn install_locally_provisioned_network_rule_set<
    I: fnet_routes_ext::rules::FidlRuleIpExt
        + fnet_routes_ext::rules::FidlRuleAdminIpExt
        + fnet_routes_ext::FidlRouteIpExt
        + fnet_routes_ext::admin::FidlRouteAdminIpExt,
>() -> Result<<I::RuleSetMarker as ProtocolMarker>::Proxy, anyhow::Error> {
    let rule_table = fuchsia_component::client::connect_to_protocol::<I::RuleTableMarker>()?;
    // The `RouteTableV{4,6}` protocol provides access to the main table.
    let main_route_table = fuchsia_component::client::connect_to_protocol::<I::RouteTableMarker>()?;

    let fidl_fuchsia_net_routes_admin::GrantForRouteTableAuthorization { table_id, token } =
        fnet_routes_ext::admin::get_authorization_for_route_table::<I>(&main_route_table)
            .await
            .context("failed to get authorization for the main table")?;

    const LOCALLY_PROVISIONED_NETWORK_RULE_SET_PRIORITY: u32 = 0;
    let rule_set = fnet_routes_ext::rules::new_rule_set::<I>(
        &rule_table,
        fnet_routes_ext::rules::RuleSetPriority::from(
            LOCALLY_PROVISIONED_NETWORK_RULE_SET_PRIORITY,
        ),
    )
    .context("failed to create a new rule set")?;

    fnet_routes_ext::rules::authenticate_for_route_table::<I>(&rule_set, table_id, token)
        .await
        .context("fidl error authenticating for route table")?
        .map_err(|err| anyhow::anyhow!("failed to authenticate for the route table: {err:?}"))?;

    const LOCALLY_PROVISIONED_NETWORK_RULE_INDEX: u32 = 0;
    fnet_routes_ext::rules::add_rule::<I>(
        &rule_set,
        fnet_routes_ext::rules::RuleIndex::from(LOCALLY_PROVISIONED_NETWORK_RULE_INDEX),
        fnet_routes_ext::rules::RuleMatcher {
            mark_1: Some(fnet_matchers_ext::Mark::Unmarked),
            mark_2: Some(fnet_matchers_ext::Mark::Unmarked),
            ..Default::default()
        },
        fnet_routes_ext::rules::RuleAction::Lookup(fnet_routes_ext::TableId::new(table_id)),
    )
    .await
    .context("fidl error adding rule")?
    .map_err(|err| anyhow::anyhow!("failed to add the rule: {err:?}"))?;

    Ok(rule_set)
}

#[cfg(test)]
mod tests {
    use fidl_fuchsia_net_dhcpv6_ext as fnet_dhcpv6_ext;
    use fidl_fuchsia_net_ext::FromExt as _;
    use fidl_fuchsia_net_routes as fnet_routes;
    use fidl_fuchsia_net_routes_admin as fnet_routes_admin;
    use fidl_fuchsia_net_routes_ext as fnet_routes_ext;

    use assert_matches::assert_matches;
    use futures::future;
    use futures::stream::FusedStream as _;
    use net_declare::{
        fidl_ip, fidl_ip_v4_with_prefix, fidl_ip_v6, fidl_ip_v6_with_prefix, fidl_mac, fidl_subnet,
    };
    use pretty_assertions::assert_eq;
    use std::task::Poll;
    use test_case::test_case;

    use super::*;
    use crate::interface::ProvisioningType::{Delegated, Local};

    impl Config {
        pub fn load_str(s: &str) -> Result<Self, anyhow::Error> {
            let config = serde_json5::from_str(s)
                .with_context(|| format!("could not deserialize the config data {}", s))?;
            Ok(config)
        }
    }

    struct ServerEnds {
        lookup_admin: fnet_name::LookupAdminRequestStream,
        dhcpv4_client_provider: fnet_dhcp::ClientProviderRequestStream,
        dhcpv6_client_provider: fnet_dhcpv6::ClientProviderRequestStream,
        route_set_v4_provider: fidl::endpoints::ServerEnd<fnet_routes_admin::RouteTableV4Marker>,
        dhcpv4_server: fidl::endpoints::ServerEnd<fnet_dhcp::Server_Marker>,
    }

    impl Into<anyhow::Error> for errors::Error {
        fn into(self) -> anyhow::Error {
            match self {
                errors::Error::NonFatal(e) => e,
                errors::Error::Fatal(e) => e,
            }
        }
    }

    impl Into<fidl_fuchsia_hardware_network::PortClass> for DeviceClass {
        fn into(self) -> fidl_fuchsia_hardware_network::PortClass {
            match self {
                Self::Virtual => fidl_fuchsia_hardware_network::PortClass::Virtual,
                Self::Ethernet => fidl_fuchsia_hardware_network::PortClass::Ethernet,
                Self::WlanClient => fidl_fuchsia_hardware_network::PortClass::WlanClient,
                Self::Ppp => fidl_fuchsia_hardware_network::PortClass::Ppp,
                Self::Bridge => fidl_fuchsia_hardware_network::PortClass::Bridge,
                Self::WlanAp => fidl_fuchsia_hardware_network::PortClass::WlanAp,
                Self::Lowpan => fidl_fuchsia_hardware_network::PortClass::Lowpan,
                Self::Blackhole => fidl_fuchsia_hardware_network::PortClass::Virtual,
            }
        }
    }

    static DEFAULT_ALLOWED_UPSTREAM_DEVICE_CLASSES: std::sync::LazyLock<HashSet<DeviceClass>> =
        std::sync::LazyLock::new(HashSet::new);

    #[derive(Default)]
    struct NetcfgTestArgs {
        with_dhcpv4_client_provider: bool,
        enable_socket_proxy: bool,
    }

    fn test_netcfg(args: NetcfgTestArgs) -> Result<(NetCfg<'static>, ServerEnds), anyhow::Error> {
        let (stack, _stack_server) = fidl::endpoints::create_proxy::<fnet_stack::StackMarker>();
        let (lookup_admin, lookup_admin_server) =
            fidl::endpoints::create_proxy::<fnet_name::LookupAdminMarker>();
        let (filter, _filter_server) =
            fidl::endpoints::create_proxy::<fnet_filter_deprecated::FilterMarker>();
        let filter_control = FilterControl::Deprecated(filter);
        let (interface_state, _interface_state_server) =
            fidl::endpoints::create_proxy::<fnet_interfaces::StateMarker>();
        let (dhcp_server, dhcp_server_server_end) =
            fidl::endpoints::create_proxy::<fnet_dhcp::Server_Marker>();
        let (dhcpv4_client_provider, dhcpv4_client_provider_server) =
            fidl::endpoints::create_proxy::<fnet_dhcp::ClientProviderMarker>();
        let (dhcpv6_client_provider, dhcpv6_client_provider_server) =
            fidl::endpoints::create_proxy::<fnet_dhcpv6::ClientProviderMarker>();
        let (installer, _installer_server) =
            fidl::endpoints::create_proxy::<fidl_fuchsia_net_interfaces_admin::InstallerMarker>();
        let (route_set_v4_provider, route_set_v4_provider_server) =
            fidl::endpoints::create_proxy::<fnet_routes_admin::RouteTableV4Marker>();
        Ok((
            NetCfg {
                stack,
                lookup_admin,
                filter_control,
                interface_state,
                installer,
                dhcp_server: Some(dhcp_server),
                dhcpv4_client_provider: args
                    .with_dhcpv4_client_provider
                    .then_some(dhcpv4_client_provider),
                dhcpv6_client_provider: Some(dhcpv6_client_provider),
                route_set_v4_provider,
                locally_provisioned_network_rule_set: None,
                filter_enabled_state: Default::default(),
                interface_properties: Default::default(),
                interface_states: Default::default(),
                interface_metrics: Default::default(),
                dns_servers: Default::default(),
                dns_server_watch_responders: Default::default(),
                route_advertisement_watcher_provider: Default::default(),
                forwarded_device_classes: Default::default(),
                dhcpv6_prefix_provider_handler: Default::default(),
                allowed_upstream_device_classes: &DEFAULT_ALLOWED_UPSTREAM_DEVICE_CLASSES,
                dhcpv4_configuration_streams: dhcpv4::ConfigurationStreamMap::empty(),
                dhcpv6_prefixes_streams: dhcpv6::PrefixesStreamMap::empty(),
                interface_naming_config: interface::InterfaceNamingConfig::from_naming_rules(
                    vec![],
                ),
                interface_provisioning_policy: Default::default(),
                netpol_networks_service: Default::default(),
                ndp_dns_servers: Default::default(),
                ndp_dns_expiry_tracker: dns::NdpDnsExpiryTracker::new(),
                telemetry_node: Default::default(),
                enable_socket_proxy: args.enable_socket_proxy,
                inspector: fuchsia_inspect::Inspector::default(),
            },
            ServerEnds {
                lookup_admin: lookup_admin_server.into_stream(),
                dhcpv4_client_provider: dhcpv4_client_provider_server.into_stream(),
                dhcpv6_client_provider: dhcpv6_client_provider_server.into_stream(),
                route_set_v4_provider: route_set_v4_provider_server,
                dhcpv4_server: dhcp_server_server_end,
            },
        ))
    }

    const INTERFACE_ID: InterfaceId = InterfaceId::new(1).unwrap();
    const DHCPV6_DNS_SOURCE: DnsServersUpdateSource =
        DnsServersUpdateSource::Dhcpv6 { interface_id: INTERFACE_ID.get() };
    const LINK_LOCAL_SOCKADDR1: fnet::Ipv6SocketAddress = fnet::Ipv6SocketAddress {
        address: fidl_ip_v6!("fe80::1"),
        port: fnet_dhcpv6::DEFAULT_CLIENT_PORT,
        zone_index: INTERFACE_ID.get(),
    };
    const LINK_LOCAL_SOCKADDR2: fnet::Ipv6SocketAddress = fnet::Ipv6SocketAddress {
        address: fidl_ip_v6!("fe80::2"),
        port: fnet_dhcpv6::DEFAULT_CLIENT_PORT,
        zone_index: INTERFACE_ID.get(),
    };
    const GLOBAL_ADDR: fnet::Subnet = fnet::Subnet { addr: fidl_ip!("2000::1"), prefix_len: 64 };
    const DNS_SERVER1: fnet::SocketAddress = fnet::SocketAddress::Ipv6(fnet::Ipv6SocketAddress {
        address: fidl_ip_v6!("2001::1"),
        port: fnet_dhcpv6::DEFAULT_CLIENT_PORT,
        zone_index: 0,
    });
    const DNS_SERVER2: fnet::SocketAddress = fnet::SocketAddress::Ipv6(fnet::Ipv6SocketAddress {
        address: fidl_ip_v6!("2001::2"),
        port: fnet_dhcpv6::DEFAULT_CLIENT_PORT,
        zone_index: 0,
    });

    fn test_addr(addr: fnet::Subnet) -> fnet_interfaces::Address {
        fnet_interfaces_ext::Address::<fnet_interfaces_ext::DefaultInterest> {
            addr,
            valid_until: fnet_interfaces_ext::NoInterest,
            preferred_lifetime_info: fnet_interfaces_ext::NoInterest,
            assignment_state: fnet_interfaces::AddressAssignmentState::Assigned,
        }
        .into()
    }

    fn ipv6addrs(a: Option<fnet::Ipv6SocketAddress>) -> Vec<fnet_interfaces::Address> {
        // The DHCPv6 client will only use a link-local address but we include a global address
        // and expect it to not be used.
        std::iter::once(test_addr(GLOBAL_ADDR))
            .chain(a.map(|fnet::Ipv6SocketAddress { address, port: _, zone_index: _ }| {
                test_addr(fnet::Subnet { addr: fnet::IpAddress::Ipv6(address), prefix_len: 64 })
            }))
            .collect()
    }

    /// Handle receiving a netstack interface changed event.
    async fn handle_interface_changed_event(
        netcfg: &mut NetCfg<'_>,
        dns_watchers: &mut DnsServerWatchers<'_>,
        online: Option<bool>,
        addresses: Option<Vec<fnet_interfaces::Address>>,
    ) -> Result<(), anyhow::Error> {
        let event = fnet_interfaces::Event::Changed(fnet_interfaces::Properties {
            id: Some(INTERFACE_ID.get()),
            online,
            addresses,
            ..Default::default()
        });
        netcfg
            .handle_interface_watcher_event(event.into(), dns_watchers, &mut virtualization::Stub)
            .await
            .map(|_| ())
    }

    /// Make sure that a new DHCPv6 client was requested, and verify its parameters.
    async fn check_new_dhcpv6_client(
        server: &mut fnet_dhcpv6::ClientProviderRequestStream,
        id: InterfaceId,
        sockaddr: fnet::Ipv6SocketAddress,
        prefix_delegation_config: Option<fnet_dhcpv6::PrefixDelegationConfig>,
        dns_watchers: &mut DnsServerWatchers<'_>,
    ) -> Result<fnet_dhcpv6::ClientRequestStream, anyhow::Error> {
        let evt =
            server.try_next().await.context("error getting next dhcpv6 client provider event")?;
        let mut client_server =
            match evt.ok_or_else(|| anyhow::anyhow!("expected dhcpv6 client provider request"))? {
                fnet_dhcpv6::ClientProviderRequest::NewClient {
                    params,
                    request,
                    control_handle: _,
                } => {
                    let stateful = prefix_delegation_config.is_some();
                    let params: fnet_dhcpv6_ext::NewClientParams = params.try_into()?;
                    assert_eq!(
                        params,
                        fnet_dhcpv6_ext::NewClientParams {
                            interface_id: id.get(),
                            address: sockaddr,
                            config: fnet_dhcpv6_ext::ClientConfig {
                                information_config: fnet_dhcpv6_ext::InformationConfig {
                                    dns_servers: true,
                                },
                                prefix_delegation_config,
                                non_temporary_address_config: fnet_dhcpv6_ext::AddressConfig {
                                    address_count: 0,
                                    preferred_addresses: None,
                                }
                            },
                            duid: stateful.then_some(fnet_dhcpv6::Duid::LinkLayerAddress(
                                fnet_dhcpv6::LinkLayerAddress::Ethernet(TEST_MAC)
                            )),
                        }
                    );

                    request.into_stream()
                }
            };
        assert!(dns_watchers.contains_key(&DHCPV6_DNS_SOURCE), "should have a watcher");
        // NetCfg always keeps the server hydrated with a pending hanging-get.
        expect_watch_prefixes(&mut client_server).await;
        Ok(client_server)
    }

    fn expect_watch_dhcpv4_configuration(
        stream: &mut fnet_dhcp::ClientRequestStream,
    ) -> fnet_dhcp::ClientWatchConfigurationResponder {
        assert_matches::assert_matches!(
            stream.try_next().now_or_never(),
            Some(Ok(Some(fnet_dhcp::ClientRequest::WatchConfiguration { responder }))) => {
                responder
            },
            "expect a watch_configuration call immediately"
        )
    }

    const TEST_MAC: fidl_fuchsia_net::MacAddress = fidl_mac!("00:01:02:03:04:05");
    const TEST_TOPOLOGICAL_PATH: &'static str = "/dev/foo/bar/network-device";

    fn test_interface_naming_id() -> interface::InterfaceNamingIdentifier {
        interface::generate_identifier(&TEST_MAC.into(), TEST_TOPOLOGICAL_PATH)
    }

    async fn expect_get_interface_auth(control: &mut fnet_interfaces_admin::ControlRequestStream) {
        let responder = control
            .by_ref()
            .try_next()
            .await
            .expect("get next interface control request")
            .expect("interface control request")
            .into_get_authorization_for_interface()
            .expect("should be interface authorization request");
        responder
            .send(fnet_resources::GrantForInterfaceAuthorization {
                interface_id: INTERFACE_ID.get(),
                token: zx::Event::create(),
            })
            .expect("respond should succeed");
    }

    #[fuchsia::test]
    async fn test_stopping_dhcpv6_with_down_lookup_admin() {
        let (
            mut netcfg,
            ServerEnds {
                lookup_admin,
                dhcpv4_client_provider: _,
                mut dhcpv6_client_provider,
                route_set_v4_provider: _,
                dhcpv4_server: _,
            },
        ) = test_netcfg(NetcfgTestArgs {
            with_dhcpv4_client_provider: false,
            enable_socket_proxy: false,
        })
        .expect("error creating test netcfg");
        let mut dns_watchers = DnsServerWatchers::empty();

        // Mock a new interface being discovered by NetCfg (we only need to make NetCfg aware of a
        // NIC with ID `INTERFACE_ID` to test DHCPv6).
        let (control, control_server_end) =
            fidl_fuchsia_net_interfaces_ext::admin::Control::create_endpoints()
                .expect("create endpoints");

        let mut control_request_stream = control_server_end.into_stream();

        let port_class = fidl_fuchsia_hardware_network::PortClass::Virtual;
        let (new_host_result, ()) = futures::join!(
            InterfaceState::new_host(
                test_interface_naming_id(),
                control,
                port_class.try_into().unwrap(),
                None,
                interface::ProvisioningType::Local,
            ),
            expect_get_interface_auth(&mut control_request_stream)
        );

        assert_matches::assert_matches!(
            netcfg
                .interface_states
                .insert(INTERFACE_ID, new_host_result.expect("new_host should succeed")),
            None
        );

        // Should start the DHCPv6 client when we get an interface changed event that shows the
        // interface as up with an link-local address.
        let _removed_interface_id: Option<InterfaceId> = netcfg
            .handle_interface_watcher_event(
                fnet_interfaces::Event::Added(fnet_interfaces::Properties {
                    id: Some(INTERFACE_ID.get()),
                    name: Some("testif01".to_string()),
                    port_class: Some(fnet_interfaces::PortClass::Device(port_class)),
                    online: Some(true),
                    addresses: Some(ipv6addrs(Some(LINK_LOCAL_SOCKADDR1))),
                    has_default_ipv4_route: Some(false),
                    has_default_ipv6_route: Some(false),
                    ..Default::default()
                })
                .into(),
                &mut dns_watchers,
                &mut virtualization::Stub,
            )
            .await
            .expect("error handling interface added event with interface up and sockaddr1");
        let _: fnet_dhcpv6::ClientRequestStream = check_new_dhcpv6_client(
            &mut dhcpv6_client_provider,
            INTERFACE_ID,
            LINK_LOCAL_SOCKADDR1,
            None,
            &mut dns_watchers,
        )
        .await
        .expect("error checking for new client with sockaddr1");

        // Drop the server-end of the lookup admin to simulate a down lookup admin service.
        std::mem::drop(lookup_admin);

        // Not having any more link local IPv6 addresses should terminate the client.
        handle_interface_changed_event(&mut netcfg, &mut dns_watchers, None, Some(ipv6addrs(None)))
            .await
            .expect("error handling interface changed event with sockaddr1 removed");

        // Another update without any link-local IPv6 addresses should do nothing
        // since the DHCPv6 client was already stopped.
        handle_interface_changed_event(&mut netcfg, &mut dns_watchers, None, Some(Vec::new()))
            .await
            .expect("error handling interface changed event with sockaddr1 removed");

        // Update interface with a link-local address to create a new DHCPv6 client.
        handle_interface_changed_event(
            &mut netcfg,
            &mut dns_watchers,
            None,
            Some(ipv6addrs(Some(LINK_LOCAL_SOCKADDR1))),
        )
        .await
        .expect("error handling interface changed event with sockaddr1 removed");
        let _: fnet_dhcpv6::ClientRequestStream = check_new_dhcpv6_client(
            &mut dhcpv6_client_provider,
            INTERFACE_ID,
            LINK_LOCAL_SOCKADDR1,
            None,
            &mut dns_watchers,
        )
        .await
        .expect("error checking for new client with sockaddr1");

        // Update offline status to down to stop DHCPv6 client.
        handle_interface_changed_event(&mut netcfg, &mut dns_watchers, Some(false), None)
            .await
            .expect("error handling interface changed event with sockaddr1 removed");

        // Update interface with new addresses but leave offline status as down.
        handle_interface_changed_event(&mut netcfg, &mut dns_watchers, None, Some(ipv6addrs(None)))
            .await
            .expect("error handling interface changed event with sockaddr1 removed")
    }

    async fn expect_watch_prefixes(client_server: &mut fnet_dhcpv6::ClientRequestStream) {
        assert_matches::assert_matches!(
            client_server.try_next().now_or_never(),
            Some(Ok(Some(fnet_dhcpv6::ClientRequest::WatchPrefixes { responder }))) => {
                responder.drop_without_shutdown();
            },
            "expect a watch_prefixes call immediately"
        )
    }

    async fn handle_update(
        netcfg: &mut NetCfg<'_>,
        online: Option<bool>,
        addresses: Option<Vec<fnet_interfaces::Address>>,
        dns_watchers: &mut DnsServerWatchers<'_>,
    ) {
        let _removed_interface_id: Option<InterfaceId> = netcfg
            .handle_interface_watcher_event(
                fnet_interfaces::Event::Changed(fnet_interfaces::Properties {
                    id: Some(INTERFACE_ID.get()),
                    online,
                    addresses,
                    ..Default::default()
                })
                .into(),
                dns_watchers,
                &mut virtualization::Stub,
            )
            .await
            .expect("error handling interface change event with interface online");
    }
    const DHCP_ADDRESS: fnet::Ipv4AddressWithPrefix = fidl_ip_v4_with_prefix!("192.0.2.254/24");

    fn dhcp_address_parameters() -> fnet_interfaces_admin::AddressParameters {
        fnet_interfaces_admin::AddressParameters {
            initial_properties: Some(fnet_interfaces_admin::AddressProperties {
                preferred_lifetime_info: None,
                valid_lifetime_end: Some(zx::MonotonicInstant::INFINITE.into_nanos()),
                ..Default::default()
            }),
            temporary: Some(true),
            add_subnet_route: Some(true),
            perform_dad: Some(true),
            ..Default::default()
        }
    }

    // Verify that the DHCPv4 server for a WlanAP is started when the interface
    // is observed to be online, regardless of whether that happens in an
    // `Added` or `Changed` event from the Netstack's interfaces watcher.
    #[test_case(true; "added online")]
    #[test_case(false; "added offline")]
    #[fuchsia::test]
    async fn test_dhcpv4_server_started(added_online: bool) {
        let (mut netcfg, ServerEnds { dhcpv4_server, .. }) = test_netcfg(NetcfgTestArgs {
            with_dhcpv4_client_provider: false,
            enable_socket_proxy: false,
        })
        .expect("error creating test netcfg");

        // A future representing the DHCPv4 server. Must be polled while feeding
        // updates to Netcfg.
        let mut dhcpv4_server_req_stream = dhcpv4_server.into_stream();
        let start_serving_fut = dhcpv4_server_req_stream.next().map(|req| {
            match req.expect("dhcpv4 request stream ended").expect("dhcpv4 request") {
                fnet_dhcp::Server_Request::StartServing { responder } => {
                    responder.send(Ok(())).expect("failed to respond");
                }
                _ => panic!("unexpected DHCPv4 server request"),
            }
        });

        // Mock a new WlanAP interface being discovered by NetCfg.
        let port_class = fidl_fuchsia_hardware_network::PortClass::WlanAp;
        let (control_client, _control_server_end) =
            fidl_fuchsia_net_interfaces_ext::admin::Control::create_endpoints()
                .expect("create endpoints");
        let wlan_ap = InterfaceState::new_wlan_ap(
            test_interface_naming_id(),
            control_client,
            port_class.try_into().unwrap(),
            interface::ProvisioningType::Local,
        );
        assert_matches::assert_matches!(
            netcfg.interface_states.insert(INTERFACE_ID, wlan_ap),
            None
        );

        // Have Netcfg observe the new interface being added.
        let mut dns_watchers = DnsServerWatchers::empty();
        let mut virt_stub = virtualization::Stub;
        let start_serving_fut = {
            let netcfg_fut = netcfg
                .handle_interface_watcher_event(
                    fnet_interfaces::Event::Added(fnet_interfaces::Properties {
                        id: Some(INTERFACE_ID.get()),
                        name: Some("testif01".to_string()),
                        port_class: Some(fnet_interfaces::PortClass::Device(port_class)),
                        online: Some(added_online),
                        addresses: Some(Vec::new()),
                        has_default_ipv4_route: Some(false),
                        has_default_ipv6_route: Some(false),
                        ..Default::default()
                    })
                    .into(),
                    &mut dns_watchers,
                    &mut virt_stub,
                )
                .map(|result| {
                    let _removed_interface_id: Option<InterfaceId> =
                        result.expect("handling interfaces watcher event");
                })
                .fuse();
            let netcfg_fut = pin!(netcfg_fut);
            if added_online {
                // Serving should be started. Expect both futures to terminate.
                let ((), ()) = futures::join!(netcfg_fut, start_serving_fut);
                None
            } else {
                match futures::future::select(netcfg_fut, start_serving_fut).await {
                    futures::future::Either::Left(((), start_serving_fut)) => {
                        Some(start_serving_fut)
                    }
                    futures::future::Either::Right(((), _netcfg_fut)) => {
                        panic!("serving unexpectedly started")
                    }
                }
            }
        };
        if let Some(start_serving_fut) = start_serving_fut {
            let netcfg_fut = handle_update(
                &mut netcfg,
                Some(true), /* online */
                None,       /* addresses */
                &mut dns_watchers,
            )
            .fuse();
            let netcfg_fut = pin!(netcfg_fut);
            // Serving should be started. Expect both futures to terminate.
            let ((), ()) = futures::join!(netcfg_fut, start_serving_fut);
        }
    }

    #[test_case(true, true; "added online and removed interface")]
    #[test_case(false, true; "added offline and removed interface")]
    #[test_case(true, false; "added online and disabled interface")]
    #[test_case(false, false; "added offline and disabled interface")]
    #[fuchsia::test]
    async fn test_dhcpv4(added_online: bool, remove_interface: bool) {
        let (
            mut netcfg,
            ServerEnds {
                mut lookup_admin,
                mut dhcpv4_client_provider,
                dhcpv6_client_provider: _,
                route_set_v4_provider,
                dhcpv4_server: _,
            },
        ) = test_netcfg(NetcfgTestArgs {
            with_dhcpv4_client_provider: true,
            enable_socket_proxy: false,
        })
        .expect("error creating test netcfg");
        let mut dns_watchers = DnsServerWatchers::empty();

        let mut route_set_request_stream =
            fnet_routes_ext::testutil::admin::serve_one_route_set::<Ipv4>(route_set_v4_provider);

        // Mock a new interface being discovered by NetCfg (we only need to make NetCfg aware of a
        // NIC with ID `INTERFACE_ID` to test DHCPv4).
        let (control_client, control_server_end) =
            fidl_fuchsia_net_interfaces_ext::admin::Control::create_endpoints()
                .expect("create endpoints");
        let port_class = fidl_fuchsia_hardware_network::PortClass::Virtual;
        let mut control = control_server_end.into_stream();

        let (new_host_result, ()) = futures::join!(
            InterfaceState::new_host(
                test_interface_naming_id(),
                control_client,
                port_class.try_into().unwrap(),
                None,
                interface::ProvisioningType::Local,
            ),
            expect_get_interface_auth(&mut control)
        );

        assert_matches::assert_matches!(
            netcfg
                .interface_states
                .insert(INTERFACE_ID, new_host_result.expect("new_host should succeed")),
            None
        );

        let handle_route_set_fut = async {
            let (_proof, responder) = route_set_request_stream
                .try_next()
                .await
                .expect("get next route set request")
                .expect("route set request stream should not have ended")
                .into_authenticate_for_interface()
                .expect("should be AuthenticateForInterface request");
            responder.send(Ok(())).expect("responding should succeed");
        };

        let handle_interface_watcher_event_fut = async {
            let _removed_interface_id: Option<InterfaceId> = netcfg
                .handle_interface_watcher_event(
                    fnet_interfaces::Event::Added(fnet_interfaces::Properties {
                        id: Some(INTERFACE_ID.get()),
                        name: Some("testif01".to_string()),
                        port_class: Some(fnet_interfaces::PortClass::Device(
                            fidl_fuchsia_hardware_network::PortClass::Virtual,
                        )),
                        online: Some(added_online),
                        addresses: Some(Vec::new()),
                        has_default_ipv4_route: Some(false),
                        has_default_ipv6_route: Some(false),
                        ..Default::default()
                    })
                    .into(),
                    &mut dns_watchers,
                    &mut virtualization::Stub,
                )
                .await
                .expect("error handling interface added event");

            if !added_online {
                assert_matches::assert_matches!(
                    dhcpv4_client_provider.try_next().now_or_never(),
                    None
                );
                handle_update(
                    &mut netcfg,
                    Some(true), /* online */
                    None,       /* addresses */
                    &mut dns_watchers,
                )
                .await;
            }

            let mut client_req_stream = match dhcpv4_client_provider
                .try_next()
                .await
                .expect("get next dhcpv4 client provider event")
                .expect("dhcpv4 client provider request")
            {
                fnet_dhcp::ClientProviderRequest::NewClient {
                    interface_id,
                    params,
                    request,
                    control_handle: _,
                } => {
                    assert_eq!(interface_id, INTERFACE_ID.get());
                    assert_eq!(params, dhcpv4::new_client_params());
                    request.into_stream()
                }
                fnet_dhcp::ClientProviderRequest::CheckPresence { responder: _ } => {
                    unreachable!("only called at startup")
                }
            };

            let responder = expect_watch_dhcpv4_configuration(&mut client_req_stream);

            (client_req_stream, responder)
        };

        // Make sure the DHCPv4 client is created on interface up.
        let ((mut client_stream, responder), ()) =
            futures::join!(handle_interface_watcher_event_fut, handle_route_set_fut);

        let check_route = |fnet_routes::RouteV4 { destination, action, properties },
                           routers: &mut HashSet<_>| {
            assert_eq!(destination, fnet_dhcp_ext::DEFAULT_ADDR_PREFIX);
            let (outbound_interface, next_hop) = assert_matches!(action, fnet_routes::RouteActionV4::Forward(
                fnet_routes::RouteTargetV4 {
                    outbound_interface,
                    next_hop
                }
            ) => (outbound_interface, next_hop));
            assert_eq!(outbound_interface, INTERFACE_ID.get());

            assert!(routers.insert(*next_hop.expect("specified next hop")));

            assert_matches!(
                properties,
                fnet_routes::RoutePropertiesV4 {
                    specified_properties: Some(fnet_routes::SpecifiedRouteProperties {
                        metric: Some(fnet_routes::SpecifiedMetric::InheritedFromInterface(
                            fnet_routes::Empty
                        )),
                        ..
                    }),
                    ..
                }
            );
        };

        let (expected_routers, route_set_request_stream) = {
            let dns_servers = vec![fidl_ip_v4!("192.0.2.1"), fidl_ip_v4!("192.0.2.2")];
            let routers = vec![fidl_ip_v4!("192.0.2.3"), fidl_ip_v4!("192.0.2.4")];

            let (_asp_client, asp_server) = fidl::endpoints::create_proxy();

            responder
                .send(fnet_dhcp::ClientWatchConfigurationResponse {
                    address: Some(fnet_dhcp::Address {
                        address: Some(DHCP_ADDRESS),
                        address_parameters: Some(dhcp_address_parameters()),
                        address_state_provider: Some(asp_server),
                        ..Default::default()
                    }),
                    dns_servers: Some(dns_servers.clone()),
                    routers: Some(routers.clone()),
                    ..Default::default()
                })
                .expect("send configuration update");

            let (got_interface_id, got_response) = netcfg
                .dhcpv4_configuration_streams
                .next()
                .await
                .expect("DHCPv4 configuration streams should never be exhausted");
            assert_eq!(got_interface_id, INTERFACE_ID);

            let dns_servers = dns_servers
                .into_iter()
                .map(|address| {
                    fnet::SocketAddress::Ipv4(fnet::Ipv4SocketAddress {
                        address,
                        port: DEFAULT_DNS_PORT,
                    })
                })
                .collect::<Vec<_>>();

            let expect_add_default_routers = futures::stream::repeat(()).take(routers.len()).fold(
                (HashSet::new(), route_set_request_stream),
                |(mut routers, mut route_set), ()| async move {
                    let (route, responder) = assert_matches!(
                        route_set.next().await.expect("route set request stream should not be exhausted"),
                        Ok(fnet_routes_admin::RouteSetV4Request::AddRoute { route, responder}) => {
                            (route, responder)
                        }
                    );
                    check_route(route, &mut routers);
                    responder.send(Ok(true)).expect("send add route response");
                    (routers, route_set)
                },
            );

            let expect_add_address_called = async {
                assert_matches!(
                    control.next().await.expect("control request stream should not be exhausted"),
                    Ok(fnet_interfaces_admin::ControlRequest::AddAddress {
                        address,
                        parameters,
                        address_state_provider: _,
                        control_handle: _,
                    })  => {
                        assert_eq!(address, fnet::Subnet::from_ext(DHCP_ADDRESS));
                        assert_eq!(parameters, dhcp_address_parameters());
                    }
                );
            };

            let (dhcpv4_result, (), (added_routers, route_set_request_stream), ()) = future::join4(
                netcfg.handle_dhcpv4_configuration(got_interface_id, got_response),
                run_lookup_admin_once(&mut lookup_admin, &dns_servers),
                expect_add_default_routers,
                expect_add_address_called,
            )
            .await;
            assert_eq!(netcfg.dns_servers.consolidated(), dns_servers);
            assert_matches!(dhcpv4_result, Dhcpv4ConfigurationHandlerResult::ContinueOperation);
            let expected_routers = routers.iter().cloned().collect::<HashSet<_>>();
            assert_eq!(added_routers, expected_routers);
            (expected_routers, route_set_request_stream)
        };

        // Netcfg always keeps the server hydrated with a WatchConfiguration
        // request.
        let _responder: fnet_dhcp::ClientWatchConfigurationResponder =
            expect_watch_dhcpv4_configuration(&mut client_stream);

        let expect_delete_default_routers = futures::stream::repeat(())
            .take(expected_routers.len())
            .fold((HashSet::new(), route_set_request_stream), |(mut routers, mut route_set), ()| async move {
                let (route, responder) = assert_matches!(
                    route_set.next().await.expect("route set request stream should not be exhausted"),
                    Ok(fnet_routes_admin::RouteSetV4Request::RemoveRoute { route, responder }) => {
                        (route, responder)
                    }
                );
                check_route(route, &mut routers);
                responder.send(Ok(true)).expect("send del fwd entry response");
                (routers, route_set)
            });

        // Make sure the DHCPv4 client is shutdown on interface disable/removal.
        let ((), (), (), (deleted_routers, mut route_set_request_stream)) = future::join4(
            async {
                if remove_interface {
                    let _removed_interface_id: Option<InterfaceId> = netcfg
                        .handle_interface_watcher_event(
                            fnet_interfaces::Event::Removed(INTERFACE_ID.get()).into(),
                            &mut dns_watchers,
                            &mut virtualization::Stub,
                        )
                        .await
                        .expect("error handling interface removed event");
                } else {
                    handle_update(
                        &mut netcfg,
                        Some(false), /* online */
                        None,        /* addresses */
                        &mut dns_watchers,
                    )
                    .await
                }
            },
            async {
                match client_stream
                    .try_next()
                    .await
                    .expect("wait for next shutdown request")
                    .expect("netcfg should send shutdown request before closing client")
                {
                    req @ fnet_dhcp::ClientRequest::WatchConfiguration { responder: _ } => {
                        panic!("unexpected request = {:?}", req);
                    }
                    fnet_dhcp::ClientRequest::Shutdown { control_handle } => control_handle
                        .send_on_exit(fnet_dhcp::ClientExitReason::GracefulShutdown)
                        .expect("send client exit reason"),
                }
            },
            run_lookup_admin_once(&mut lookup_admin, &Vec::new()),
            expect_delete_default_routers,
        )
        .await;
        assert_eq!(netcfg.dns_servers.consolidated(), []);
        assert_eq!(deleted_routers, expected_routers);

        // No more requests sent by NetCfg.
        assert_matches!(lookup_admin.next().now_or_never(), None);
        assert_matches!(route_set_request_stream.next().now_or_never(), None);
        let control_next = control.next().now_or_never();
        if remove_interface {
            assert_matches!(control_next, Some(None));
        } else {
            assert_matches!(control_next, None);
        }
    }

    #[test_case(fnet_interfaces_admin::AddressParameters {
            perform_dad: Some(false),
            ..dhcp_address_parameters()
        }; "perform_dad is false")]
    #[test_case(fnet_interfaces_admin::AddressParameters {
            add_subnet_route: Some(false),
            ..dhcp_address_parameters()
        }; "add_subnet_route is false")]
    #[test_case(fnet_interfaces_admin::AddressParameters {
            perform_dad: None,
            ..dhcp_address_parameters()
        }; "perform_dad is None")]
    #[test_case(fnet_interfaces_admin::AddressParameters {
            add_subnet_route: None,
            ..dhcp_address_parameters()
        }; "add_subnet_route is None")]
    #[fuchsia::test]
    async fn assert_address_parameters_ignored(
        parameters: fnet_interfaces_admin::AddressParameters,
    ) {
        let (
            mut netcfg,
            ServerEnds {
                mut lookup_admin,
                dhcpv4_client_provider: _,
                dhcpv6_client_provider: _,
                route_set_v4_provider: _,
                dhcpv4_server: _,
            },
        ) = test_netcfg(NetcfgTestArgs {
            with_dhcpv4_client_provider: false,
            enable_socket_proxy: false,
        })
        .expect("error creating test netcfg");

        let (control, control_server_end) =
            fidl_fuchsia_net_interfaces_ext::admin::Control::create_endpoints().unwrap();
        let mut control_stream = control_server_end.into_stream();

        let (route_set_proxy, route_set_server_end) =
            fidl::endpoints::create_proxy::<fnet_routes_admin::RouteSetV4Marker>();
        let mut route_set_request_stream = route_set_server_end.into_stream();

        let (shutdown_sender, _shutdown_receiver) = futures::channel::oneshot::channel();
        let auth_grant = fnet_resources::GrantForInterfaceAuthorization {
            interface_id: INTERFACE_ID.get(),
            token: zx::Event::create(),
        };

        assert_matches!(
            netcfg.interface_states.insert(
                INTERFACE_ID,
                InterfaceState {
                    control,
                    device_class: DeviceClass::Virtual,
                    config: InterfaceConfigState::Host(HostInterfaceState {
                        dhcpv4_client: Dhcpv4ClientState::Running(dhcpv4::ClientState {
                            routers: HashSet::new(),
                            route_set: route_set_proxy,
                            shutdown_sender,
                        }),
                        dhcpv6_client_state: None,
                        dhcpv6_pd_config: None,
                        interface_admin_auth: auth_grant,
                        interface_naming_id: test_interface_naming_id(),
                    }),
                    provisioning: interface::ProvisioningType::Local,
                },
            ),
            None
        );

        let (_asp_client, asp_server) = fidl::endpoints::create_proxy();
        let configuration = fnet_dhcp_ext::Configuration {
            address: Some(fnet_dhcp_ext::Address {
                address: DHCP_ADDRESS,
                address_parameters: parameters,
                address_state_provider: asp_server,
            }),
            dns_servers: Vec::new(),
            routers: Vec::new(),
        };

        let res = netcfg
            .handle_dhcpv4_configuration(INTERFACE_ID, Ok(configuration))
            .now_or_never()
            .expect("configuration should be ignored immediately");
        assert_eq!(res, Dhcpv4ConfigurationHandlerResult::ContinueOperation);

        // Assert no requests were sent.
        assert_matches!(lookup_admin.next().now_or_never(), None);
        assert_matches!(control_stream.next().now_or_never(), None);
        assert_matches!(route_set_request_stream.next().now_or_never(), None);
    }

    #[fuchsia::test]
    async fn test_dhcpv4_ignores_address_change() {
        let (
            mut netcfg,
            ServerEnds {
                lookup_admin: _,
                mut dhcpv4_client_provider,
                dhcpv6_client_provider: _,
                route_set_v4_provider,
                dhcpv4_server: _,
            },
        ) = test_netcfg(NetcfgTestArgs {
            with_dhcpv4_client_provider: true,
            enable_socket_proxy: false,
        })
        .expect("error creating test netcfg");
        let mut dns_watchers = DnsServerWatchers::empty();

        let _noop_route_sets_task = fasync::Task::local(
            fnet_routes_ext::testutil::admin::serve_noop_route_sets::<Ipv4>(route_set_v4_provider),
        );

        // Mock a new interface being discovered by NetCfg (we only need to make NetCfg aware of a
        // NIC with ID `INTERFACE_ID` to test DHCPv4).
        let (control, control_server_end) =
            fidl_fuchsia_net_interfaces_ext::admin::Control::create_endpoints()
                .expect("create endpoints");
        let port_class = fidl_fuchsia_hardware_network::PortClass::Virtual;

        let new_host_fut = InterfaceState::new_host(
            test_interface_naming_id(),
            control,
            port_class.try_into().unwrap(),
            None,
            interface::ProvisioningType::Local,
        );
        let mut control = control_server_end.into_stream();
        let (new_host_result, ()) =
            futures::join!(new_host_fut, expect_get_interface_auth(&mut control));

        assert_matches::assert_matches!(
            netcfg
                .interface_states
                .insert(INTERFACE_ID, new_host_result.expect("new_host should succeed")),
            None
        );

        // Make sure the DHCPv4 client is created on interface up.
        let _removed_interface_id: Option<InterfaceId> = netcfg
            .handle_interface_watcher_event(
                fnet_interfaces::Event::Added(fnet_interfaces::Properties {
                    id: Some(INTERFACE_ID.get()),
                    name: Some("testif01".to_string()),
                    port_class: Some(fnet_interfaces::PortClass::Device(
                        fidl_fuchsia_hardware_network::PortClass::Virtual,
                    )),
                    online: Some(true),
                    addresses: Some(Vec::new()),
                    has_default_ipv4_route: Some(false),
                    has_default_ipv6_route: Some(false),
                    ..Default::default()
                })
                .into(),
                &mut dns_watchers,
                &mut virtualization::Stub,
            )
            .await
            .expect("error handling interface added event");

        let mut client_req_stream = match dhcpv4_client_provider
            .try_next()
            .await
            .expect("get next dhcpv4 client provider event")
            .expect("dhcpv4 client provider request")
        {
            fnet_dhcp::ClientProviderRequest::NewClient {
                interface_id,
                params,
                request,
                control_handle: _,
            } => {
                assert_eq!(interface_id, INTERFACE_ID.get());
                assert_eq!(params, dhcpv4::new_client_params());
                request.into_stream()
            }
            fnet_dhcp::ClientProviderRequest::CheckPresence { responder: _ } => {
                unreachable!("only called at startup")
            }
        };

        let responder = expect_watch_dhcpv4_configuration(&mut client_req_stream);
        let (_asp_client, asp_server) = fidl::endpoints::create_proxy();
        responder
            .send(fnet_dhcp::ClientWatchConfigurationResponse {
                address: Some(fnet_dhcp::Address {
                    address: Some(DHCP_ADDRESS),
                    address_parameters: Some(dhcp_address_parameters()),
                    address_state_provider: Some(asp_server),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .expect("send configuration update");

        // A DHCP client should only be started or stopped when the interface is
        // enabled or disabled. If we change the addresses seen on the
        // interface, we shouldn't see requests to create a new DHCP client, and
        // the existing one should remain alive (netcfg shouldn't have sent a
        // shutdown request for it).
        for addresses in [
            vec![fidl_subnet!("192.2.2.2/28")],
            vec![],
            vec![fidl_subnet!("192.2.2.2/28"), fidl_subnet!("fe80::1234/64")],
        ] {
            handle_update(
                &mut netcfg,
                None,
                Some(addresses.into_iter().map(test_addr).collect()),
                &mut dns_watchers,
            )
            .await;
        }
        assert_matches!(dhcpv4_client_provider.next().now_or_never(), None);
        assert_matches!(client_req_stream.next().now_or_never(), None);
    }

    /// Waits for a `SetDnsServers` request with the specified servers.
    async fn run_lookup_admin_once(
        server: &mut fnet_name::LookupAdminRequestStream,
        expected_servers: &Vec<fnet::SocketAddress>,
    ) {
        let req = server
            .try_next()
            .await
            .expect("get next lookup admin request")
            .expect("lookup admin request stream should not be exhausted");
        let (servers, responder) = assert_matches!(
            req,
            fnet_name::LookupAdminRequest::SetDnsServers { servers, responder } => {
                (servers, responder)
            }
        );

        assert_eq!(expected_servers, &servers);
        responder.send(Ok(())).expect("send set dns servers response");
    }

    #[test_case(Some(fnet_dhcp::ClientExitReason::UnableToOpenSocket), AllowClientRestart::Yes)]
    #[test_case(Some(fnet_dhcp::ClientExitReason::NetworkUnreachable), AllowClientRestart::Yes)]
    #[test_case(None, AllowClientRestart::Yes)]
    #[test_case(Some(fnet_dhcp::ClientExitReason::AddressRemovedByUser), AllowClientRestart::No)]
    #[fuchsia::test]
    async fn dhcpv4_handles_client_exit(
        exit_reason: Option<fnet_dhcp::ClientExitReason>,
        expected_allow_restart: AllowClientRestart,
    ) {
        let (
            mut netcfg,
            ServerEnds {
                lookup_admin: _,
                mut dhcpv4_client_provider,
                dhcpv6_client_provider: _,
                route_set_v4_provider,
                dhcpv4_server: _,
            },
        ) = test_netcfg(NetcfgTestArgs {
            with_dhcpv4_client_provider: true,
            enable_socket_proxy: false,
        })
        .expect("error creating test netcfg");
        let mut dns_watchers = DnsServerWatchers::empty();

        let _noop_route_sets_task = fasync::Task::local(
            fnet_routes_ext::testutil::admin::serve_noop_route_sets::<Ipv4>(route_set_v4_provider),
        );

        // Mock a new interface being discovered by NetCfg (we only need to make NetCfg aware of a
        // NIC with ID `INTERFACE_ID` to test DHCPv4).
        let (control, control_server_end) =
            fidl_fuchsia_net_interfaces_ext::admin::Control::create_endpoints()
                .expect("create endpoints");
        let port_class = fidl_fuchsia_hardware_network::PortClass::Virtual;
        let new_host_fut = InterfaceState::new_host(
            test_interface_naming_id(),
            control,
            port_class.try_into().unwrap(),
            None,
            interface::ProvisioningType::Local,
        );
        let mut control = control_server_end.into_stream();
        let (new_host_result, ()) =
            futures::join!(new_host_fut, expect_get_interface_auth(&mut control));
        assert_matches::assert_matches!(
            netcfg
                .interface_states
                .insert(INTERFACE_ID, new_host_result.expect("new_host should succeed")),
            None
        );

        // Make sure the DHCPv4 client is created on interface up.
        let (client_stream, control_handle, responder) = {
            let _removed_interface_id: Option<InterfaceId> = netcfg
                .handle_interface_watcher_event(
                    fnet_interfaces::Event::Added(fnet_interfaces::Properties {
                        id: Some(INTERFACE_ID.get()),
                        name: Some("testif01".to_string()),
                        port_class: Some(fnet_interfaces::PortClass::Device(
                            fidl_fuchsia_hardware_network::PortClass::Virtual,
                        )),
                        online: Some(true),
                        addresses: Some(Vec::new()),
                        has_default_ipv4_route: Some(false),
                        has_default_ipv6_route: Some(false),
                        ..Default::default()
                    })
                    .into(),
                    &mut dns_watchers,
                    &mut virtualization::Stub,
                )
                .await
                .expect("error handling interface added event");

            let (mut client_req_stream, control_handle) = match dhcpv4_client_provider
                .try_next()
                .await
                .expect("get next dhcpv4 client provider event")
                .expect("dhcpv4 client provider request")
            {
                fnet_dhcp::ClientProviderRequest::NewClient {
                    interface_id,
                    params,
                    request,
                    control_handle: _,
                } => {
                    assert_eq!(interface_id, INTERFACE_ID.get());
                    assert_eq!(params, dhcpv4::new_client_params());
                    request.into_stream_and_control_handle()
                }
                fnet_dhcp::ClientProviderRequest::CheckPresence { responder: _ } => {
                    unreachable!("only called at startup")
                }
            };

            let responder = expect_watch_dhcpv4_configuration(&mut client_req_stream);
            (client_req_stream, control_handle, responder)
        };

        // Simulate the client exiting with an error.
        match exit_reason {
            Some(reason) => {
                control_handle.send_on_exit(reason).expect("sending OnExit should succeed");
            }
            None => {}
        }

        drop((client_stream, control_handle, responder));

        let (got_interface_id, got_response) = netcfg
            .dhcpv4_configuration_streams
            .next()
            .await
            .expect("DHCPv4 configuration streams should never be exhausted");
        assert_eq!(got_interface_id, INTERFACE_ID);

        let dhcpv4_result =
            netcfg.handle_dhcpv4_configuration(got_interface_id, got_response).await;
        assert_eq!(
            dhcpv4_result,
            Dhcpv4ConfigurationHandlerResult::ClientStopped(expected_allow_restart)
        );
    }

    #[fuchsia::test]
    async fn test_dhcpv6() {
        let (mut netcfg, mut servers) = test_netcfg(NetcfgTestArgs {
            with_dhcpv4_client_provider: false,
            enable_socket_proxy: false,
        })
        .expect("error creating test netcfg");
        let mut dns_watchers = DnsServerWatchers::empty();

        // Mock a fake DNS update from the DHCPv6 client.
        async fn update_dns(netcfg: &mut NetCfg<'static>, servers: &mut ServerEnds) {
            let ((), ()) = future::join(
                netcfg.update_dns_servers(
                    DHCPV6_DNS_SOURCE,
                    vec![fnet_name::DnsServer_ {
                        address: Some(DNS_SERVER2),
                        source: Some(fnet_name::DnsServerSource::Dhcpv6(
                            fnet_name::Dhcpv6DnsServerSource {
                                source_interface: Some(INTERFACE_ID.get()),
                                ..Default::default()
                            },
                        )),
                        ..Default::default()
                    }],
                ),
                run_lookup_admin_once(&mut servers.lookup_admin, &vec![DNS_SERVER2, DNS_SERVER1]),
            )
            .await;
        }

        // Mock a fake DNS update from the netstack.
        let netstack_servers = vec![DNS_SERVER1];
        let ((), ()) = future::join(
            netcfg.update_dns_servers(
                DnsServersUpdateSource::Netstack,
                vec![fnet_name::DnsServer_ {
                    address: Some(DNS_SERVER1),
                    source: Some(fnet_name::DnsServerSource::StaticSource(
                        fnet_name::StaticDnsServerSource::default(),
                    )),
                    ..Default::default()
                }],
            ),
            run_lookup_admin_once(&mut servers.lookup_admin, &netstack_servers),
        )
        .await;

        // Mock a new interface being discovered by NetCfg (we only need to make NetCfg aware of a
        // NIC with ID `INTERFACE_ID` to test DHCPv6).
        let (control, control_server_end) =
            fidl_fuchsia_net_interfaces_ext::admin::Control::create_endpoints()
                .expect("create endpoints");
        let mut control_request_stream = control_server_end.into_stream();
        let port_class = fidl_fuchsia_hardware_network::PortClass::Virtual;
        let (new_host_result, ()) = futures::join!(
            InterfaceState::new_host(
                test_interface_naming_id(),
                control,
                port_class.try_into().unwrap(),
                None,
                interface::ProvisioningType::Local,
            ),
            expect_get_interface_auth(&mut control_request_stream)
        );
        assert_matches::assert_matches!(
            netcfg
                .interface_states
                .insert(INTERFACE_ID, new_host_result.expect("new_host should succeed")),
            None
        );

        // Should start the DHCPv6 client when we get an interface changed event that shows the
        // interface as up with an link-local address.
        let _removed_interface_id: Option<InterfaceId> = netcfg
            .handle_interface_watcher_event(
                fnet_interfaces::Event::Added(fnet_interfaces::Properties {
                    id: Some(INTERFACE_ID.get()),
                    name: Some("testif01".to_string()),
                    port_class: Some(fnet_interfaces::PortClass::Device(port_class)),
                    online: Some(true),
                    addresses: Some(ipv6addrs(Some(LINK_LOCAL_SOCKADDR1))),
                    has_default_ipv4_route: Some(false),
                    has_default_ipv6_route: Some(false),
                    ..Default::default()
                })
                .into(),
                &mut dns_watchers,
                &mut virtualization::Stub,
            )
            .await
            .expect("error handling interface added event with interface up and sockaddr1");
        let mut client_server = check_new_dhcpv6_client(
            &mut servers.dhcpv6_client_provider,
            INTERFACE_ID,
            LINK_LOCAL_SOCKADDR1,
            None,
            &mut dns_watchers,
        )
        .await
        .expect("error checking for new client with sockaddr1");
        update_dns(&mut netcfg, &mut servers).await;

        // Not having any more link local IPv6 addresses should terminate the client.
        let ((), ()) = future::join(
            handle_interface_changed_event(
                &mut netcfg,
                &mut dns_watchers,
                None,
                Some(ipv6addrs(None)),
            )
            .map(|r| r.expect("error handling interface changed event with sockaddr1 removed")),
            run_lookup_admin_once(&mut servers.lookup_admin, &netstack_servers),
        )
        .await;
        assert!(!dns_watchers.contains_key(&DHCPV6_DNS_SOURCE), "should not have a watcher");
        assert_matches::assert_matches!(client_server.try_next().await, Ok(None));

        // Should start a new DHCPv6 client when we get an interface changed event that shows the
        // interface as up with an link-local address.
        handle_interface_changed_event(
            &mut netcfg,
            &mut dns_watchers,
            None,
            Some(ipv6addrs(Some(LINK_LOCAL_SOCKADDR2))),
        )
        .await
        .expect("error handling netstack event with sockaddr2 added");
        let mut client_server = check_new_dhcpv6_client(
            &mut servers.dhcpv6_client_provider,
            INTERFACE_ID,
            LINK_LOCAL_SOCKADDR2,
            None,
            &mut dns_watchers,
        )
        .await
        .expect("error checking for new client with sockaddr2");
        update_dns(&mut netcfg, &mut servers).await;

        // Interface being down should terminate the client.
        let ((), ()) = future::join(
            handle_interface_changed_event(
                &mut netcfg,
                &mut dns_watchers,
                Some(false), /* down */
                None,
            )
            .map(|r| r.expect("error handling interface changed event with interface down")),
            run_lookup_admin_once(&mut servers.lookup_admin, &netstack_servers),
        )
        .await;
        assert!(!dns_watchers.contains_key(&DHCPV6_DNS_SOURCE), "should not have a watcher");
        assert_matches::assert_matches!(client_server.try_next().await, Ok(None));

        // Should start a new DHCPv6 client when we get an interface changed event that shows the
        // interface as up with an link-local address.
        handle_interface_changed_event(
            &mut netcfg,
            &mut dns_watchers,
            Some(true), /* up */
            None,
        )
        .await
        .expect("error handling interface up event");
        let mut client_server = check_new_dhcpv6_client(
            &mut servers.dhcpv6_client_provider,
            INTERFACE_ID,
            LINK_LOCAL_SOCKADDR2,
            None,
            &mut dns_watchers,
        )
        .await
        .expect("error checking for new client with sockaddr2 after interface up again");
        update_dns(&mut netcfg, &mut servers).await;

        // Should start a new DHCPv6 client when we get an interface changed event that shows the
        // interface as up with a new link-local address.
        let ((), ()) = future::join(
            handle_interface_changed_event(
                &mut netcfg,
                &mut dns_watchers,
                None,
                Some(ipv6addrs(Some(LINK_LOCAL_SOCKADDR1))),
            )
            .map(|r| {
                r.expect("error handling interface change event with sockaddr1 replacing sockaddr2")
            }),
            run_lookup_admin_once(&mut servers.lookup_admin, &netstack_servers),
        )
        .await;
        assert_matches::assert_matches!(client_server.try_next().await, Ok(None));
        let _client_server: fnet_dhcpv6::ClientRequestStream = check_new_dhcpv6_client(
            &mut servers.dhcpv6_client_provider,
            INTERFACE_ID,
            LINK_LOCAL_SOCKADDR1,
            None,
            &mut dns_watchers,
        )
        .await
        .expect("error checking for new client with sockaddr1 after address change");
        update_dns(&mut netcfg, &mut servers).await;

        // Complete the DNS server watcher then start a new one.
        let ((), ()) = future::join(
            netcfg
                .handle_dns_server_watcher_done(DHCPV6_DNS_SOURCE, &mut dns_watchers)
                .map(|r| r.expect("error handling completion of dns server watcher")),
            run_lookup_admin_once(&mut servers.lookup_admin, &netstack_servers),
        )
        .await;
        assert!(!dns_watchers.contains_key(&DHCPV6_DNS_SOURCE), "should not have a watcher");
        handle_interface_changed_event(
            &mut netcfg,
            &mut dns_watchers,
            None,
            Some(ipv6addrs(Some(LINK_LOCAL_SOCKADDR2))),
        )
        .await
        .expect("error handling interface change event with sockaddr2 replacing sockaddr1");
        let mut client_server = check_new_dhcpv6_client(
            &mut servers.dhcpv6_client_provider,
            INTERFACE_ID,
            LINK_LOCAL_SOCKADDR2,
            None,
            &mut dns_watchers,
        )
        .await
        .expect("error checking for new client with sockaddr2 after completing dns watcher");
        update_dns(&mut netcfg, &mut servers).await;

        // An event that indicates the interface is removed should stop the client.
        let ((), ()) = future::join(
            netcfg
                .handle_interface_watcher_event(
                    fnet_interfaces::Event::Removed(INTERFACE_ID.get()).into(),
                    &mut dns_watchers,
                    &mut virtualization::Stub,
                )
                .map(|r| {
                    let _removed_interface_id: Option<InterfaceId> =
                        r.expect("error handling interface removed event");
                }),
            run_lookup_admin_once(&mut servers.lookup_admin, &netstack_servers),
        )
        .await;
        assert!(!dns_watchers.contains_key(&DHCPV6_DNS_SOURCE), "should not have a watcher");
        assert_matches::assert_matches!(client_server.try_next().await, Ok(None));
        assert!(!netcfg.interface_states.contains_key(&INTERFACE_ID));
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    enum InterfaceKind {
        Unowned,
        NonHost,
        Host { upstream: bool },
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    struct InterfaceConfig {
        id: InterfaceId,
        kind: InterfaceKind,
    }

    const UPSTREAM_INTERFACE_CONFIG: InterfaceConfig = InterfaceConfig {
        id: InterfaceId::new(1).unwrap(),
        kind: InterfaceKind::Host { upstream: true },
    };

    const ALLOWED_UPSTREAM_DEVICE_CLASS: DeviceClass = DeviceClass::Ethernet;
    const DISALLOWED_UPSTREAM_DEVICE_CLASS: DeviceClass = DeviceClass::Virtual;

    fn dhcpv6_sockaddr(interface_id: InterfaceId) -> fnet::Ipv6SocketAddress {
        let mut address = fidl_ip_v6!("fe80::");
        let interface_id: u8 =
            interface_id.get().try_into().expect("interface ID should fit into u8");
        *address.addr.last_mut().expect("IPv6 address is empty") = interface_id;
        fnet::Ipv6SocketAddress {
            address: address,
            port: fnet_dhcpv6::DEFAULT_CLIENT_PORT,
            zone_index: interface_id.into(),
        }
    }

    #[test_case(
        Some(UPSTREAM_INTERFACE_CONFIG.id),
        None; "specific_interface_no_preferred_prefix_len")]
    #[test_case(
        None,
        None; "all_upstreams_no_preferred_prefix_len")]
    #[test_case(
        Some(UPSTREAM_INTERFACE_CONFIG.id),
        Some(2); "specific_interface_preferred_prefix_len")]
    #[test_case(
        None,
        Some(1); "all_upstreams_preferred_prefix_len")]
    #[fuchsia::test]
    async fn test_dhcpv6_acquire_prefix(
        interface_id: Option<InterfaceId>,
        preferred_prefix_len: Option<u8>,
    ) {
        const INTERFACE_CONFIGS: [InterfaceConfig; 5] = [
            UPSTREAM_INTERFACE_CONFIG,
            InterfaceConfig {
                id: InterfaceId::new(2).unwrap(),
                kind: InterfaceKind::Host { upstream: true },
            },
            InterfaceConfig {
                id: InterfaceId::new(3).unwrap(),
                kind: InterfaceKind::Host { upstream: false },
            },
            InterfaceConfig { id: InterfaceId::new(4).unwrap(), kind: InterfaceKind::Unowned },
            InterfaceConfig { id: InterfaceId::new(5).unwrap(), kind: InterfaceKind::NonHost },
        ];

        let (
            mut netcfg,
            ServerEnds {
                lookup_admin: lookup_admin_request_stream,
                dhcpv4_client_provider: _,
                dhcpv6_client_provider: mut dhcpv6_client_provider_request_stream,
                route_set_v4_provider: _,
                dhcpv4_server: _,
            },
        ) = test_netcfg(NetcfgTestArgs {
            with_dhcpv4_client_provider: false,
            enable_socket_proxy: false,
        })
        .expect("error creating test netcfg");
        let allowed_upstream_device_classes = HashSet::from([ALLOWED_UPSTREAM_DEVICE_CLASS]);
        netcfg.allowed_upstream_device_classes = &allowed_upstream_device_classes;
        let mut dns_watchers = DnsServerWatchers::empty();

        struct TestInterfaceState {
            _control_server_end:
                Option<fidl::endpoints::ServerEnd<fnet_interfaces_admin::ControlMarker>>,
            dhcpv6_client_request_stream: Option<fnet_dhcpv6::ClientRequestStream>,
            kind: InterfaceKind,
        }
        let mut interface_states = HashMap::new();
        for InterfaceConfig { id, kind } in INTERFACE_CONFIGS.into_iter() {
            // Mock new interfaces being discovered by NetCfg as needed.
            let (device_class, control_server_end) = match kind {
                InterfaceKind::Unowned => (DISALLOWED_UPSTREAM_DEVICE_CLASS, None),
                InterfaceKind::NonHost => {
                    let (control, control_server_end) =
                        fidl_fuchsia_net_interfaces_ext::admin::Control::create_endpoints()
                            .expect("create endpoints");
                    let device_class = DeviceClass::WlanAp;
                    assert_matches::assert_matches!(
                        netcfg.interface_states.insert(
                            id.try_into().expect("interface ID should be nonzero"),
                            InterfaceState::new_wlan_ap(
                                test_interface_naming_id(),
                                control,
                                device_class,
                                interface::ProvisioningType::Local
                            )
                        ),
                        None
                    );
                    (device_class, Some(control_server_end))
                }
                InterfaceKind::Host { upstream } => {
                    let (control, control_server_end) =
                        fidl_fuchsia_net_interfaces_ext::admin::Control::create_endpoints()
                            .expect("create endpoints");
                    let device_class = if upstream {
                        ALLOWED_UPSTREAM_DEVICE_CLASS
                    } else {
                        DISALLOWED_UPSTREAM_DEVICE_CLASS
                    };

                    let mut control_request_stream = control_server_end.into_stream();

                    let (new_host_result, ()) = futures::join!(
                        InterfaceState::new_host(
                            test_interface_naming_id(),
                            control,
                            device_class,
                            None,
                            interface::ProvisioningType::Local,
                        ),
                        expect_get_interface_auth(&mut control_request_stream)
                    );

                    assert_matches::assert_matches!(
                        netcfg.interface_states.insert(
                            id.try_into().expect("interface ID should be nonzero"),
                            new_host_result.expect("new_host should succeed")
                        ),
                        None
                    );

                    let (control_inner, _is_terminated) = control_request_stream.into_inner();
                    let control_inner = std::sync::Arc::try_unwrap(control_inner)
                        .expect("recover original server end");

                    (
                        device_class,
                        Some(
                            fidl::endpoints::ServerEnd
                                ::<fnet_interfaces_admin::ControlMarker>
                                ::from(control_inner.into_channel().into_zx_channel()),
                        ),
                    )
                }
            };

            let sockaddr = dhcpv6_sockaddr(id);

            // Fake an interface added event.
            let _removed_interface_id: Option<InterfaceId> = netcfg
                .handle_interface_watcher_event(
                    fnet_interfaces::Event::Added(fnet_interfaces::Properties {
                        id: Some(id.get()),
                        name: Some(format!("testif{}", id)),
                        port_class: Some(fnet_interfaces::PortClass::Device(
                            device_class.into(),
                        )),
                        online: Some(true),
                        addresses: Some(ipv6addrs(Some(sockaddr))),
                        has_default_ipv4_route: Some(false),
                        has_default_ipv6_route: Some(false),
                        ..Default::default()
                    }).into(),
                    &mut dns_watchers,
                    &mut virtualization::Stub,
                )
                .await
                .unwrap_or_else(|e| panic!("error handling interface added event for {} with interface up and link-local addr: {}", id, e));

            // Expect DHCPv6 client to have started on host interfaces.
            let dhcpv6_client_request_stream = match kind {
                InterfaceKind::Unowned | InterfaceKind::NonHost => None,
                InterfaceKind::Host { upstream: _ } => {
                    let request_stream = check_new_dhcpv6_client(
                        &mut dhcpv6_client_provider_request_stream,
                        id,
                        sockaddr,
                        None,
                        &mut dns_watchers,
                    )
                    .await
                    .unwrap_or_else(|e| {
                        panic!("error checking for new DHCPv6 client on interface {}: {}", id, e)
                    });
                    Some(request_stream)
                }
            };
            assert!(
                interface_states
                    .insert(
                        id,
                        TestInterfaceState {
                            _control_server_end: control_server_end,
                            dhcpv6_client_request_stream,
                            kind,
                        }
                    )
                    .is_none()
            );
        }

        async fn assert_dhcpv6_clients_stopped(
            interface_states: &mut HashMap<InterfaceId, TestInterfaceState>,
            interface_id: Option<InterfaceId>,
        ) -> HashSet<InterfaceId> {
            futures::stream::iter(
                interface_states.iter_mut(),
            ).filter_map(|(
                id,
                TestInterfaceState { _control_server_end, dhcpv6_client_request_stream, kind },
            )| async move {
                // Expect DHCPv6 to be restarted iff the interface matches the
                // interface used to acquire prefix on, or is an upstream
                // capable host if no interface was specified to acquire
                // prefixes from.
                let expect_restart = interface_id.map_or_else(
                    || *kind == InterfaceKind::Host { upstream: true },
                    |want_id| want_id == *id,
                );
                if expect_restart {
                    let res = dhcpv6_client_request_stream
                        .take()
                        .unwrap_or_else(|| panic!("interface {} DHCPv6 client provider request stream missing when expecting restart", id))
                        .try_next().await;
                    assert_matches!(res, Ok(None));
                    Some(*id)
                } else {
                    if let Some(req_stream) = dhcpv6_client_request_stream.as_mut() {
                        // We do not expect DHCPv6 to restart and want to make
                        // sure that the stream has not ended. We can't `.await`
                        // because that will result in us blocking forever on
                        // the next event (since the DHCPv6 client did not close
                        // so the stream is still open and blocked).
                        assert_matches!(req_stream.try_next().now_or_never(), None, "interface_id={:?}, kind={:?}, id={:?}", interface_id, kind, id);
                        assert!(!req_stream.is_terminated());
                    }
                    None
                }
            })
            .collect().await
        }

        async fn assert_dhcpv6_clients_started(
            count: usize,
            dhcpv6_client_provider_request_stream: &mut fnet_dhcpv6::ClientProviderRequestStream,
            interface_states: &mut HashMap<InterfaceId, TestInterfaceState>,
            want_pd_config: Option<fnet_dhcpv6::PrefixDelegationConfig>,
        ) -> HashSet<InterfaceId> {
            dhcpv6_client_provider_request_stream
                .map(|res| res.expect("DHCPv6 ClientProvider request stream error"))
                .take(count)
                .then(
                    |fnet_dhcpv6::ClientProviderRequest::NewClient {
                         params:
                             fnet_dhcpv6::NewClientParams { interface_id, address: _, config, .. },
                         request,
                         control_handle: _,
                     }| async move {
                        let mut new_stream = request.into_stream();
                        // NetCfg always keeps the server hydrated with a pending hanging-get.
                        expect_watch_prefixes(&mut new_stream).await;
                        (interface_id, config, new_stream)
                    },
                )
                .map(|(interface_id, config, new_stream)| {
                    let interface_id = interface_id
                        .expect("interface ID missing in new DHCPv6 client request")
                        .try_into()
                        .expect("interface ID should be nonzero");
                    let TestInterfaceState {
                        _control_server_end,
                        dhcpv6_client_request_stream,
                        kind: _,
                    } = interface_states.get_mut(&interface_id).unwrap_or_else(|| {
                        panic!("interface {} must be present in map", interface_id)
                    });
                    assert!(
                        std::mem::replace(dhcpv6_client_request_stream, Some(new_stream)).is_none()
                    );

                    let config = fnet_dhcpv6_ext::ClientConfig::try_from(
                        config.expect("ClientConfig must be present"),
                    )
                    .expect("ClientConfig should pass FIDL table validation");
                    assert_eq!(
                        config,
                        fnet_dhcpv6_ext::ClientConfig {
                            information_config: fnet_dhcpv6_ext::InformationConfig {
                                dns_servers: true,
                            },
                            non_temporary_address_config: Default::default(),
                            prefix_delegation_config: want_pd_config.clone(),
                        }
                    );
                    interface_id
                })
                .collect()
                .await
        }

        async fn handle_watch_prefix_with_fake(
            netcfg: &mut NetCfg<'_>,
            dns_watchers: &mut DnsServerWatchers<'_>,
            interface_id: InterfaceId,
            fake_prefixes: Vec<fnet_dhcpv6::Prefix>,
            // Used to test handling PrefixControl.WatchPrefix & Client.WatchPrefixes
            // events in different orders.
            handle_dhcpv6_prefixes_before_watch_prefix: bool,
        ) {
            let responder = netcfg
                .dhcpv6_prefix_provider_handler
                .as_mut()
                .map(dhcpv6::PrefixProviderHandler::try_next_prefix_control_request)
                .expect("DHCPv6 prefix provider handler must be present")
                .await
                .expect("prefix provider request")
                .expect("PrefixProvider request stream exhausted")
                .into_watch_prefix()
                .expect("request not WatchPrefix");

            async fn handle_dhcpv6_prefixes(
                netcfg: &mut NetCfg<'_>,
                dns_watchers: &mut DnsServerWatchers<'_>,
                interface_id: InterfaceId,
                fake_prefixes: Vec<fnet_dhcpv6::Prefix>,
            ) {
                netcfg
                    .handle_dhcpv6_prefixes(interface_id, Ok(fake_prefixes), dns_watchers)
                    .await
                    .expect("handle DHCPv6 prefixes")
            }

            async fn handle_watch_prefix(
                netcfg: &mut NetCfg<'_>,
                dns_watchers: &mut DnsServerWatchers<'_>,
                responder: fnet_dhcpv6::PrefixControlWatchPrefixResponder,
            ) {
                netcfg
                    .handle_watch_prefix(responder, dns_watchers)
                    .await
                    .expect("handle watch prefix")
            }

            if handle_dhcpv6_prefixes_before_watch_prefix {
                handle_dhcpv6_prefixes(netcfg, dns_watchers, interface_id, fake_prefixes).await;
                handle_watch_prefix(netcfg, dns_watchers, responder).await;
            } else {
                handle_watch_prefix(netcfg, dns_watchers, responder).await;
                handle_dhcpv6_prefixes(netcfg, dns_watchers, interface_id, fake_prefixes).await;
            }
        }

        // Making an AcquirePrefix call should trigger restarting DHCPv6 w/ PD.
        let (prefix_control, server_end) =
            fidl::endpoints::create_proxy::<fnet_dhcpv6::PrefixControlMarker>();
        let mut lookup_admin_fut = lookup_admin_request_stream
            .try_for_each(|req| {
                let (_, responder): (Vec<fnet::SocketAddress>, _) =
                    req.into_set_dns_servers().expect("request must be SetDnsServers");
                responder.send(Ok(())).expect("send SetDnsServers response");
                futures::future::ok(())
            })
            .fuse();

        {
            let acquire_prefix_fut = netcfg.handle_dhcpv6_acquire_prefix(
                fnet_dhcpv6::AcquirePrefixConfig {
                    interface_id: interface_id.map(InterfaceId::get),
                    preferred_prefix_len,
                    ..Default::default()
                },
                server_end,
                &mut dns_watchers,
            );
            futures::select! {
                res = acquire_prefix_fut.fuse() => {
                    res.expect("acquire DHCPv6 prefix")
                }
                res = lookup_admin_fut => {
                    panic!("fuchsia.net.name/LookupAdmin request stream exhausted unexpectedly: {:?}", res)
                },
            };
            // Expect DHCPv6 client to have been restarted on the appropriate
            // interfaces with PD configured.
            let stopped = assert_dhcpv6_clients_stopped(&mut interface_states, interface_id).await;
            let started = assert_dhcpv6_clients_started(
                stopped.len(),
                dhcpv6_client_provider_request_stream.by_ref(),
                &mut interface_states,
                Some(preferred_prefix_len.map_or(
                    fnet_dhcpv6::PrefixDelegationConfig::Empty(fnet_dhcpv6::Empty),
                    fnet_dhcpv6::PrefixDelegationConfig::PrefixLength,
                )),
            )
            .await;
            assert_eq!(started, stopped);

            // Yield a prefix to the PrefixControl.
            let prefix = fnet_dhcpv6::Prefix {
                prefix: fidl_ip_v6_with_prefix!("abcd::/64"),
                lifetimes: fnet_dhcpv6::Lifetimes { valid_until: 123, preferred_until: 456 },
            };
            let any_eligible_interface = *started.iter().next().expect(
                "must have configured DHCPv6 client to perform PD on at least one interface",
            );
            let (watch_prefix_res, ()) = futures::future::join(
                prefix_control.watch_prefix(),
                handle_watch_prefix_with_fake(
                    &mut netcfg,
                    &mut dns_watchers,
                    any_eligible_interface,
                    vec![prefix.clone()],
                    true, /* handle_dhcpv6_prefixes_before_watch_prefix */
                ),
            )
            .await;
            assert_matches!(watch_prefix_res, Ok(fnet_dhcpv6::PrefixEvent::Assigned(got_prefix)) => {
                assert_eq!(got_prefix, prefix);
            });

            // Yield a different prefix from what was yielded before.
            let renewed_prefix = fnet_dhcpv6::Prefix {
                lifetimes: fnet_dhcpv6::Lifetimes {
                    valid_until: 123_123,
                    preferred_until: 456_456,
                },
                ..prefix
            };
            let (watch_prefix_res, ()) = futures::future::join(
                prefix_control.watch_prefix(),
                handle_watch_prefix_with_fake(
                    &mut netcfg,
                    &mut dns_watchers,
                    any_eligible_interface,
                    vec![renewed_prefix.clone()],
                    false, /* handle_dhcpv6_prefixes_before_watch_prefix */
                ),
            )
            .await;
            assert_matches!(watch_prefix_res, Ok(fnet_dhcpv6::PrefixEvent::Assigned(
                got_prefix,
            )) => {
                assert_eq!(got_prefix, renewed_prefix);
            });
            let (watch_prefix_res, ()) = futures::future::join(
                prefix_control.watch_prefix(),
                handle_watch_prefix_with_fake(
                    &mut netcfg,
                    &mut dns_watchers,
                    any_eligible_interface,
                    vec![],
                    true, /* handle_dhcpv6_prefixes_before_watch_prefix */
                ),
            )
            .await;
            assert_matches!(
                watch_prefix_res,
                Ok(fnet_dhcpv6::PrefixEvent::Unassigned(fnet_dhcpv6::Empty))
            );
        }

        // Closing the PrefixControl should trigger clients running DHCPv6-PD to
        // be restarted.
        {
            futures::select! {
                () = netcfg.on_dhcpv6_prefix_control_close(&mut dns_watchers).fuse() => {},
                res = lookup_admin_fut => {
                    panic!(
                        "fuchsia.net.name/LookupAdmin request stream exhausted unexpectedly: {:?}",
                        res,
                    )
                },
            }
            let stopped = assert_dhcpv6_clients_stopped(&mut interface_states, interface_id).await;
            let started = assert_dhcpv6_clients_started(
                stopped.len(),
                dhcpv6_client_provider_request_stream.by_ref(),
                &mut interface_states,
                None,
            )
            .await;
            assert_eq!(started, stopped);
        }
    }

    // Tests that DHCPv6 clients are configured to perform PD on eligible
    // upstream-providing interfaces while a `PrefixControl` channel is open.
    #[fuchsia::test]
    async fn test_dhcpv6_pd_on_added_upstream() {
        let (
            mut netcfg,
            ServerEnds {
                lookup_admin: _,
                dhcpv4_client_provider: _,
                dhcpv6_client_provider: mut dhcpv6_client_provider_request_stream,
                route_set_v4_provider: _,
                dhcpv4_server: _,
            },
        ) = test_netcfg(NetcfgTestArgs {
            with_dhcpv4_client_provider: false,
            enable_socket_proxy: false,
        })
        .expect("error creating test netcfg");
        let allowed_upstream_device_classes = HashSet::from([ALLOWED_UPSTREAM_DEVICE_CLASS]);
        netcfg.allowed_upstream_device_classes = &allowed_upstream_device_classes;
        let mut dns_watchers = DnsServerWatchers::empty();

        let (_prefix_control, server_end) =
            fidl::endpoints::create_proxy::<fnet_dhcpv6::PrefixControlMarker>();
        netcfg
            .handle_dhcpv6_acquire_prefix(
                fnet_dhcpv6::AcquirePrefixConfig::default(),
                server_end,
                &mut dns_watchers,
            )
            .await
            .expect("handle DHCPv6 acquire prefix");

        for (id, upstream) in
            [(InterfaceId::new(1).unwrap(), true), (InterfaceId::new(2).unwrap(), false)]
        {
            // Mock interface being discovered by NetCfg.
            let (control, control_server_end) =
                fidl_fuchsia_net_interfaces_ext::admin::Control::create_endpoints()
                    .expect("create endpoints");
            let device_class = if upstream {
                ALLOWED_UPSTREAM_DEVICE_CLASS
            } else {
                DISALLOWED_UPSTREAM_DEVICE_CLASS
            };
            let mut control_request_stream = control_server_end.into_stream();

            let (new_host_result, ()) = futures::join!(
                InterfaceState::new_host(
                    test_interface_naming_id(),
                    control,
                    device_class,
                    upstream
                        .then_some(fnet_dhcpv6::PrefixDelegationConfig::Empty(fnet_dhcpv6::Empty)),
                    interface::ProvisioningType::Local,
                ),
                expect_get_interface_auth(&mut control_request_stream)
            );

            assert_matches::assert_matches!(
                netcfg
                    .interface_states
                    .insert(id, new_host_result.expect("new_host should succeed")),
                None
            );

            let sockaddr = dhcpv6_sockaddr(id);

            // Fake an interface added event.
            let _removed_interface_id: Option<InterfaceId> = netcfg
                .handle_interface_watcher_event(
                    fnet_interfaces::Event::Added(fnet_interfaces::Properties {
                        id: Some(id.get()),
                        name: Some(format!("testif{}", id)),
                        port_class: Some(fnet_interfaces::PortClass::Device(
                            device_class.into(),
                        )),
                        online: Some(true),
                        addresses: Some(ipv6addrs(Some(sockaddr))),
                        has_default_ipv4_route: Some(false),
                        has_default_ipv6_route: Some(false),
                        ..Default::default()
                    }).into(),
                    &mut dns_watchers,
                    &mut virtualization::Stub,
                )
                .await
                .unwrap_or_else(|e| panic!("error handling interface added event for {} with interface up and link-local addr: {:?}", id, e));

            // Expect DHCPv6 client to have started with PD configuration.
            let _: fnet_dhcpv6::ClientRequestStream = check_new_dhcpv6_client(
                &mut dhcpv6_client_provider_request_stream,
                id,
                sockaddr,
                upstream.then_some(fnet_dhcpv6::PrefixDelegationConfig::Empty(fnet_dhcpv6::Empty)),
                &mut dns_watchers,
            )
            .await
            .unwrap_or_else(|e| {
                panic!("error checking for new DHCPv6 client on interface {}: {:?}", id, e)
            });
        }
    }

    struct InterfacePropertiesHelper {
        id: InterfaceId,
        online: Option<bool>,
        has_default_ipv4_route: Option<bool>,
        has_default_ipv6_route: Option<bool>,
        addresses: Option<Vec<fnet_interfaces::Address>>,
    }

    // Create a `fnet_interfaces::Properties` object using
    // a small subset of fields that influence whether an
    // interface is shared with the socketproxy.
    fn create_properties(
        InterfacePropertiesHelper {
            id,
            online,
            has_default_ipv4_route,
            has_default_ipv6_route,
            addresses,
        }: InterfacePropertiesHelper,
    ) -> fnet_interfaces::Properties {
        fnet_interfaces::Properties {
            id: Some(id.get()),
            name: Some(format!("testif{}", id)),
            port_class: Some(fnet_interfaces::PortClass::Device(
                ALLOWED_UPSTREAM_DEVICE_CLASS.into(),
            )),
            online,
            has_default_ipv4_route,
            has_default_ipv6_route,
            addresses,
            ..Default::default()
        }
    }

    // Given a list of interface ids and provisioning actions,
    // get the nth interface with `Local` provisioning.
    //
    // Returns None if there is no nth matching interface.
    fn get_nth_interface_id_locally_provisioned(
        interfaces: &[(InterfaceId, interface::ProvisioningType)],
        nth: usize,
    ) -> Option<InterfaceId> {
        interfaces.iter().filter_map(|(id, action)| (*action == Local).then_some(*id)).nth(nth)
    }

    const INTERFACE_ID2: InterfaceId = InterfaceId::new(2).unwrap();
    const INTERFACE_ID3: InterfaceId = InterfaceId::new(3).unwrap();

    async fn default_network_fallback_helper(
        interfaces: &[(InterfaceId, interface::ProvisioningType)],
        final_event: fnet_interfaces::Event,
    ) {
        // The provided interface id list must be non-empty.
        assert!(interfaces.len() > 0);

        let (mut netcfg, _server_ends) = test_netcfg(NetcfgTestArgs {
            with_dhcpv4_client_provider: false,
            enable_socket_proxy: true,
        })
        .expect("error creating test netcfg");
        let mut dns_watchers = DnsServerWatchers::empty();

        assert_eq!(netcfg.netpol_networks_service.default_network(), None);

        // The initial default interface is the first one that is
        // locally provisioned. There must be at least one interface
        // that has ProvisioningAction::Local set.
        let initial_default_interface =
            get_nth_interface_id_locally_provisioned(interfaces, 0).unwrap();

        // Mock interfaces being discovered by NetCfg.
        for (id, provisioning_action) in interfaces.iter() {
            let (control, control_server_end) =
                fidl_fuchsia_net_interfaces_ext::admin::Control::create_endpoints()
                    .expect("create endpoints");
            let mut control_request_stream = control_server_end.into_stream();

            let (new_host_result, ()) = futures::join!(
                InterfaceState::new_host(
                    test_interface_naming_id(),
                    control,
                    ALLOWED_UPSTREAM_DEVICE_CLASS,
                    None,
                    *provisioning_action,
                ),
                expect_get_interface_auth(&mut control_request_stream)
            );

            assert_matches::assert_matches!(
                netcfg
                    .interface_states
                    .insert(*id, new_host_result.expect("new_host should succeed")),
                None
            );
            // Fake an interface added event. As `has_default_ipv4_route` and
            // `has_default_ipv6_route` are true, the network should be added to
            // the networks service and made default.
            let watcher_result = netcfg
                .handle_interface_watcher_event(
                    fnet_interfaces::Event::Added(create_properties(InterfacePropertiesHelper {
                        id: *id,
                        online: Some(true),
                        has_default_ipv4_route: Some(true),
                        has_default_ipv6_route: Some(true),
                        addresses: Some(ipv6addrs(None)),
                    }))
                    .into(),
                    &mut dns_watchers,
                    &mut virtualization::Stub,
                )
                .await;
            assert_matches::assert_matches!(watcher_result, Ok(None));
        }

        // Verify Fuchsia networks are successfully registered in the networks service.
        for (id, provisioning_action) in interfaces.iter() {
            if *provisioning_action == Local {
                assert!(
                    netcfg.netpol_networks_service.has_network(network::NetworkId::fuchsia(*id))
                );
            }
        }
        // Verify the default network through the networks service.
        assert_eq!(
            netcfg.netpol_networks_service.default_network(),
            Some(network::NetworkId::fuchsia(initial_default_interface))
        );

        // Send an interface changed event with `has_default_ipv4_route`
        // becoming false. Default network remains unchanged due to one of
        // the default routes still being present.
        assert_matches::assert_matches!(
            netcfg
                .handle_interface_watcher_event(
                    fnet_interfaces::Event::Changed(create_properties(InterfacePropertiesHelper {
                        id: initial_default_interface,
                        online: None,
                        has_default_ipv4_route: Some(false),
                        has_default_ipv6_route: None,
                        addresses: None,
                    }))
                    .into(),
                    &mut dns_watchers,
                    &mut virtualization::Stub,
                )
                .await,
            Ok(None)
        );

        // Send an interface changed event with an address update
        // that does not impact the network being a valid candidate.
        assert_matches::assert_matches!(
            netcfg
                .handle_interface_watcher_event(
                    fnet_interfaces::Event::Changed(create_properties(InterfacePropertiesHelper {
                        id: initial_default_interface,
                        online: None,
                        has_default_ipv4_route: None,
                        has_default_ipv6_route: None,
                        addresses: Some(vec![fnet_interfaces::Address {
                            addr: Some(fidl_subnet!("192.0.2.0/24")),
                            assignment_state: Some(
                                fnet_interfaces::AddressAssignmentState::Assigned
                            ),
                            ..Default::default()
                        }]),
                    }))
                    .into(),
                    &mut dns_watchers,
                    &mut virtualization::Stub,
                )
                .await,
            Ok(None)
        );

        // Send an interface changed event that should cause the network
        // to be removed from the registry.
        let expected_result = match &final_event {
            fnet_interfaces::Event::Removed(id) => Some(InterfaceId::new(*id).unwrap()),
            _ => None,
        };
        let watcher_result = netcfg
            .handle_interface_watcher_event(
                final_event.into(),
                &mut dns_watchers,
                &mut virtualization::Stub,
            )
            .await;
        let watcher_result = watcher_result.expect("watcher_result error");
        assert_eq!(watcher_result, expected_result);

        // Verify the central networks service fallback is correctly computed.
        assert_eq!(
            netcfg.netpol_networks_service.default_network(),
            get_nth_interface_id_locally_provisioned(interfaces, 1)
                .map(network::NetworkId::fuchsia)
        );
    }

    // Test the default network fallback behavior when there is another
    // valid network and when no alternative network exists.
    #[test_case(
        &[INTERFACE_ID],
            fnet_interfaces::Event::Changed(create_properties(InterfacePropertiesHelper {
                id: INTERFACE_ID,
                online: None,
                has_default_ipv4_route: None,
                has_default_ipv6_route: Some(false),
                addresses: None,
        }))
    ; "one_iface_update_lost_candidacy_v6")]
    #[test_case(
        &[INTERFACE_ID],
            fnet_interfaces::Event::Changed(create_properties(InterfacePropertiesHelper {
                id: INTERFACE_ID,
                online: Some(false),
                has_default_ipv4_route: None,
                has_default_ipv6_route: None,
                addresses: None,
        }))
    ; "one_iface_update_lost_candidacy_offline")]
    #[test_case(
        &[INTERFACE_ID],
        fnet_interfaces::Event::Removed(INTERFACE_ID.get())
    ; "one_iface_remove_default")]
    #[fuchsia::test]
    async fn test_default_network_fallback_all_local(
        interface_ids: &[InterfaceId],
        final_event: fnet_interfaces::Event,
    ) {
        let interfaces: Vec<(InterfaceId, interface::ProvisioningType)> =
            interface_ids.iter().map(|id| (*id, Local)).collect();
        default_network_fallback_helper(&interfaces, final_event).await
    }

    // Test the default network fallback behavior when there are delegated
    // interfaces present.
    #[test_case(
        &[(INTERFACE_ID, Local), (INTERFACE_ID2, Delegated)],
        fnet_interfaces::Event::Removed(INTERFACE_ID.get())
    ; "one_local_iface_first_iface")]
    #[test_case(
        &[(INTERFACE_ID, Delegated), (INTERFACE_ID2, Delegated), (INTERFACE_ID3, Local)],
        fnet_interfaces::Event::Removed(INTERFACE_ID3.get())
    ; "one_local_iface_not_first_iface")]
    #[fuchsia::test]
    async fn test_default_network_fallback_mixed_provisioning(
        interfaces: &[(InterfaceId, interface::ProvisioningType)],
        final_event: fnet_interfaces::Event,
    ) {
        default_network_fallback_helper(&interfaces, final_event).await
    }

    #[test]
    fn test_config() {
        let config_str = r#"
{
  "dns_config": {
    "servers": ["8.8.8.8"]
  },
  "filter_config": {
    "rules": [],
    "nat_rules": [],
    "rdr_rules": []
  },
  "filter_enabled_interface_types": ["wlanclient", "wlanap"],
  "interface_metrics": {
    "wlan_metric": 100,
    "eth_metric": 10,
    "blackhole_metric": 123
  },
  "allowed_upstream_device_classes": ["ethernet", "wlanclient"],
  "allowed_bridge_upstream_device_classes": ["ethernet"],
  "enable_dhcpv6": true,
  "forwarded_device_classes": { "ipv4": [ "ethernet" ], "ipv6": [ "wlanclient" ] },
  "interface_naming_policy": [ { "matchers": [
        {"bus_types": ["usb", "pci", "sdio"]},
        {"device_classes": ["ethernet", "wlanclient", "wlanap"]},
        {"topological_path": "abcde"},
        {"any": true}
    ], "naming_scheme": [
        { "type": "dynamic", "rule": "device_class" },
        { "type": "static", "value": "x" },
        { "type": "dynamic", "rule": "normalized_mac" },
        { "type": "dynamic", "rule": "bus_type" },
        { "type": "dynamic", "rule": "bus_path" },
        { "type": "default" }
    ] } ],
    "interface_provisioning_policy": [ {
        "matchers": [ {"any": false } ],
        "provisioning": "delegated",
        "netstack_managed_routes_designation": "interface_local"
    }, {
        "matchers": [ {"interface_name": "xyz" } ],
        "provisioning": "local"
    } ],
   "blackhole_interfaces": [ "ifb0" ],
   "enable_socket_proxy": true
}
"#;

        let Config {
            dns_config: DnsConfig { servers },
            filter_config,
            filter_enabled_interface_types,
            interface_metrics,
            allowed_upstream_device_classes,
            allowed_bridge_upstream_device_classes,
            enable_dhcpv6,
            forwarded_device_classes,
            interface_naming_policy,
            interface_provisioning_policy,
            blackhole_interfaces,
            enable_socket_proxy,
        } = Config::load_str(config_str).unwrap();

        assert_eq!(vec!["8.8.8.8".parse::<std::net::IpAddr>().unwrap()], servers);
        let FilterConfig { rules, nat_rules, rdr_rules } = filter_config;
        assert_eq!(Vec::<String>::new(), rules);
        assert_eq!(Vec::<String>::new(), nat_rules);
        assert_eq!(Vec::<String>::new(), rdr_rules);

        assert_eq!(
            HashSet::from([InterfaceType::WlanClient, InterfaceType::WlanAp]),
            filter_enabled_interface_types
        );

        let expected_metrics = InterfaceMetrics {
            wlan_metric: Metric(100),
            eth_metric: Metric(10),
            blackhole_metric: Metric(123),
        };
        assert_eq!(interface_metrics, expected_metrics);

        assert_eq!(
            AllowedDeviceClasses(HashSet::from([DeviceClass::Ethernet, DeviceClass::WlanClient])),
            allowed_upstream_device_classes
        );
        assert_eq!(
            AllowedDeviceClasses(HashSet::from([DeviceClass::Ethernet])),
            allowed_bridge_upstream_device_classes
        );

        assert_eq!(enable_dhcpv6, true);

        let expected_classes = ForwardedDeviceClasses {
            ipv4: HashSet::from([DeviceClass::Ethernet]),
            ipv6: HashSet::from([DeviceClass::WlanClient]),
        };
        assert_eq!(forwarded_device_classes, expected_classes);

        let expected_naming_policy = Vec::from([interface::NamingRule {
            matchers: HashSet::from([
                interface::MatchingRule::BusTypes(vec![
                    interface::BusType::USB,
                    interface::BusType::PCI,
                    interface::BusType::SDIO,
                ]),
                interface::MatchingRule::DeviceClasses(vec![
                    DeviceClass::Ethernet,
                    DeviceClass::WlanClient,
                    DeviceClass::WlanAp,
                ]),
                interface::MatchingRule::TopologicalPath(glob::Pattern::new("abcde").unwrap()),
                interface::MatchingRule::Any(true),
            ]),
            naming_scheme: vec![
                interface::NameCompositionRule::Dynamic {
                    rule: interface::DynamicNameCompositionRule::DeviceClass,
                },
                interface::NameCompositionRule::Static { value: "x".to_owned() },
                interface::NameCompositionRule::Dynamic {
                    rule: interface::DynamicNameCompositionRule::NormalizedMac,
                },
                interface::NameCompositionRule::Dynamic {
                    rule: interface::DynamicNameCompositionRule::BusType,
                },
                interface::NameCompositionRule::Dynamic {
                    rule: interface::DynamicNameCompositionRule::BusPath,
                },
                interface::NameCompositionRule::Default,
            ],
        }]);
        assert_eq!(interface_naming_policy, expected_naming_policy);

        let expected_provisioning_policy = Vec::from([
            interface::ProvisioningRule {
                matchers: HashSet::from([interface::ProvisioningMatchingRule::Common(
                    interface::MatchingRule::Any(false),
                )]),
                action: interface::ProvisioningAction {
                    provisioning: interface::ProvisioningType::Delegated,
                    netstack_managed_routes_designation: Some(
                        interface::NetstackManagedRoutesDesignation::InterfaceLocal,
                    ),
                },
            },
            interface::ProvisioningRule {
                matchers: HashSet::from([interface::ProvisioningMatchingRule::InterfaceName {
                    pattern: glob::Pattern::new("xyz").unwrap(),
                }]),
                action: interface::ProvisioningAction {
                    provisioning: interface::ProvisioningType::Local,
                    netstack_managed_routes_designation: None,
                },
            },
        ]);
        assert_eq!(interface_provisioning_policy, expected_provisioning_policy);

        assert_eq!(blackhole_interfaces, vec!["ifb0".to_string()]);

        assert_eq!(enable_socket_proxy, true)
    }

    #[test]
    fn test_config_defaults() {
        let config_str = r#"
{
  "dns_config": { "servers": [] },
  "filter_config": {
    "rules": [],
    "nat_rules": [],
    "rdr_rules": []
  },
  "filter_enabled_interface_types": []
}
"#;

        let Config {
            dns_config: _,
            filter_config: _,
            filter_enabled_interface_types: _,
            allowed_upstream_device_classes,
            allowed_bridge_upstream_device_classes,
            interface_metrics,
            enable_dhcpv6,
            forwarded_device_classes: _,
            interface_naming_policy,
            interface_provisioning_policy,
            blackhole_interfaces,
            enable_socket_proxy,
        } = Config::load_str(config_str).unwrap();

        assert_eq!(allowed_upstream_device_classes, Default::default());
        assert_eq!(allowed_bridge_upstream_device_classes, Default::default());
        assert_eq!(interface_metrics, Default::default());
        assert_eq!(enable_dhcpv6, true);
        assert_eq!(interface_naming_policy.len(), 0);
        assert_eq!(interface_provisioning_policy.len(), 0);
        assert_eq!(blackhole_interfaces, Vec::<String>::new());
        assert_eq!(enable_socket_proxy, false);
    }

    #[test_case(
        "eth_metric", Default::default(), Metric(1), Metric(1);
        "wlan assumes default metric when unspecified")]
    #[test_case("wlan_metric", Metric(1), Default::default(), Metric(1);
        "eth assumes default metric when unspecified")]
    fn test_config_metric_individual_defaults(
        metric_name: &'static str,
        wlan_metric: Metric,
        eth_metric: Metric,
        expect_metric: Metric,
    ) {
        let config_str = format!(
            r#"
{{
  "dns_config": {{ "servers": [] }},
  "filter_config": {{
    "rules": [],
    "nat_rules": [],
    "rdr_rules": []
  }},
  "filter_enabled_interface_types": [],
  "interface_metrics": {{ "{}": {} }}
}}
"#,
            metric_name, expect_metric
        );

        let Config {
            dns_config: _,
            filter_config: _,
            filter_enabled_interface_types: _,
            allowed_upstream_device_classes: _,
            allowed_bridge_upstream_device_classes: _,
            enable_dhcpv6: _,
            interface_metrics,
            forwarded_device_classes: _,
            interface_naming_policy: _,
            interface_provisioning_policy: _,
            blackhole_interfaces: _,
            enable_socket_proxy: _,
        } = Config::load_str(&config_str).unwrap();

        let expected_metrics =
            InterfaceMetrics { wlan_metric, eth_metric, blackhole_metric: Default::default() };
        assert_eq!(interface_metrics, expected_metrics);
    }

    #[test]
    fn test_config_denies_unknown_fields() {
        let config_str = r#"{
            "filter_enabled_interface_types": ["wlanclient"],
            "foobar": "baz"
        }"#;

        let err = Config::load_str(config_str).expect_err("config shouldn't accept unknown fields");
        let err = err.downcast::<serde_json5::Error>().expect("downcast error");
        assert_matches!(
            err,
            serde_json5::Error::Message {
                location: Some(serde_json5::Location { line: 3, .. }),
                ..
            }
        );
        // Ensure the error is complaining about unknown field.
        assert!(format!("{:?}", err).contains("foobar"));
    }

    #[test]
    fn test_config_denies_unknown_fields_nested() {
        let bad_configs = vec![
            r#"
{
  "dns_config": { "speling": [] },
  "filter_config": {
    "rules": [],
    "nat_rules": [],
    "rdr_rules": []
  },
  "filter_enabled_interface_types": []
}
"#,
            r#"
{
  "dns_config": { "servers": [] },
  "filter_config": {
    "speling": [],
    "nat_rules": [],
    "rdr_rules": []
  },
  "filter_enabled_interface_types": []
}
"#,
            r#"
{
  "dns_config": { "servers": [] },
  "filter_config": {
    "rules": [],
    "nat_rules": [],
    "rdr_rules": []
  },
  "filter_enabled_interface_types": ["speling"]
}
"#,
            r#"
{
  "dns_config": { "servers": [] },
  "filter_config": {
    "rules": [],
    "nat_rules": [],
    "rdr_rules": []
  },
  "interface_metrics": {
    "eth_metric": 1,
    "wlan_metric": 2,
    "speling": 3,
  },
  "filter_enabled_interface_types": []
}
"#,
            r#"
{
  "dns_config": { "servers": [] },
  "filter_config": {
    "rules": [],
    "nat_rules": [],
    "rdr_rules": []
  },
  "filter_enabled_interface_types": ["speling"]
}
"#,
            r#"
{
  "dns_config": { "servers": [] },
  "filter_config": {
    "rules": [],
    "nat_rules": [],
    "rdr_rules": []
  },
  "filter_enabled_interface_types": [],
  "allowed_upstream_device_classes": ["speling"]
}
"#,
            r#"
{
  "dns_config": { "servers": [] },
  "filter_config": {
    "rules": [],
    "nat_rules": [],
    "rdr_rules": []
  },
  "filter_enabled_interface_types": [],
  "allowed_bridge_upstream_device_classes": ["speling"]
}
"#,
            r#"
{
  "dns_config": { "servers": [] },
  "filter_config": {
    "rules": [],
    "nat_rules": [],
    "rdr_rules": []
  },
  "filter_enabled_interface_types": [],
  "allowed_upstream_device_classes": [],
  "forwarded_device_classes": { "ipv4": [], "ipv6": [], "speling": [] }
}
"#,
            r#"
{
  "dns_config": { "servers": [] },
  "filter_config": {
    "rules": [],
    "nat_rules": [],
    "rdr_rules": []
  },
  "filter_enabled_interface_types": [],
  "allowed_upstream_device_classes": [],
  "forwarded_device_classes": { "ipv4": [], "ipv6": [] },
  "interface_naming_policy": [{
    "matchers": [],
    "naming_scheme": [],
    "speling": []
  }]
}
"#,
            r#"
{
  "dns_config": { "servers": [] },
  "filter_config": {
    "rules": [],
    "nat_rules": [],
    "rdr_rules": []
  },
  "filter_enabled_interface_types": [],
  "allowed_upstream_device_classes": [],
  "forwarded_device_classes": { "ipv4": [], "ipv6": [] },
  "interface_naming_policy": [{
    "matchers": [ { "speling": [] } ],
    "naming_scheme": []
  }]
}
"#,
            r#"
{
  "dns_config": { "servers": [] },
  "filter_config": {
    "rules": [],
    "nat_rules": [],
    "rdr_rules": []
  },
  "filter_enabled_interface_types": [],
  "allowed_upstream_device_classes": [],
  "forwarded_device_classes": { "ipv4": [], "ipv6": [] },
  "interface_naming_policy": [{
    "matchers": [ { "bus_types": ["speling"] } ],
    "naming_scheme": []
  }]
}
"#,
            r#"
{
  "dns_config": { "servers": [] },
  "filter_config": {
    "rules": [],
    "nat_rules": [],
    "rdr_rules": []
  },
  "filter_enabled_interface_types": [],
  "allowed_upstream_device_classes": [],
  "forwarded_device_classes": { "ipv4": [], "ipv6": [] },
  "interface_naming_policy": [{
    "matchers": [ { "device_classes": ["speling"] } ],
    "naming_scheme": []
  }]
}
"#,
            r#"
{
  "dns_config": { "servers": [] },
  "filter_config": {
    "rules": [],
    "nat_rules": [],
    "rdr_rules": []
  },
  "filter_enabled_interface_types": [],
  "allowed_upstream_device_classes": [],
  "forwarded_device_classes": { "ipv4": [], "ipv6": [] },
  "interface_naming_policy": [{
    "matchers": [ { "any": "speling" } ],
    "naming_scheme": []
  }]
}
"#,
            r#"
{
  "dns_config": { "servers": [] },
  "filter_config": {
    "rules": [],
    "nat_rules": [],
    "rdr_rules": []
  },
  "filter_enabled_interface_types": [],
  "allowed_upstream_device_classes": [],
  "forwarded_device_classes": { "ipv4": [], "ipv6": [] },
  "interface_naming_policy": [{
    "matchers": [ { "any": true } ],
    "naming_scheme": [ { "type": "speling" } ]
  }]
}
"#,
            r#"
{
  "dns_config": { "servers": [] },
  "filter_config": {
    "rules": [],
    "nat_rules": [],
    "rdr_rules": []
  },
  "filter_enabled_interface_types": [],
  "allowed_upstream_device_classes": [],
  "forwarded_device_classes": { "ipv4": [], "ipv6": [] },
  "interface_naming_policy": [{
    "matchers": [ { "any": true } ],
    "naming_scheme": [ { "type": "dynamic", "rule": "speling" } ]
  }]
}
"#,
            r#"
{
  "dns_config": { "servers": [] },
  "filter_config": {
    "rules": [],
    "nat_rules": [],
    "rdr_rules": []
  },
  "filter_enabled_interface_types": [],
  "allowed_upstream_device_classes": [],
  "forwarded_device_classes": { "ipv4": [], "ipv6": [] },
  "interface_provisioning_policy": [{
    "matchers": [ { "any": true } ],
    "speling": ""
  }]
}
"#,
            r#"
{
  "dns_config": { "servers": [] },
  "filter_config": {
    "rules": [],
    "nat_rules": [],
    "rdr_rules": []
  },
  "filter_enabled_interface_types": [],
  "allowed_upstream_device_classes": [],
  "forwarded_device_classes": { "ipv4": [], "ipv6": [] },
  "interface_provisioning_policy": [{
    "matchers": [ { "any": true } ],
    "provisioning": "speling"
  }]
}
"#,
            r#"
{
  "dns_config": { "servers": [] },
  "filter_config": {
    "rules": [],
    "nat_rules": [],
    "rdr_rules": []
  },
  "filter_enabled_interface_types": [],
  "allowed_upstream_device_classes": [],
  "forwarded_device_classes": { "ipv4": [], "ipv6": [] },
  "interface_provisioning_policy": [{
    "matchers": [ { "any": true } ],
    "provisioning": "delegated",
    "netstack_managed_routes_designation": "speling"
  }]
}
"#,
        ];

        for config_str in bad_configs {
            let err =
                Config::load_str(config_str).expect_err("config shouldn't accept unknown fields");
            let err = err.downcast::<serde_json5::Error>().expect("downcast error");
            let err_str = format!("{:?}", err);
            // Ensure the error is complaining about unknown field, or complaining
            // about the missing field.
            assert!(err_str.contains("speling") || err_str.contains("missing field"));
        }
    }

    #[test_case(
        r#"
{
  "dns_config": { "servers": [] },
  "filter_config": {
    "rules": [],
    "nat_rules": [],
    "rdr_rules": []
  },
  "filter_enabled_interface_types": [],
  "allowed_upstream_device_classes": [],
  "forwarded_device_classes": { "ipv4": [], "ipv6": [] },
  "interface_naming_policy": [{
    "matchers": [ { "topological_path": "[speling" } ],
    "naming_scheme": []
  }]
}
"#,
        "invalid range";
        "topological_path"
    )]
    #[test_case(
        r#"
{
  "dns_config": { "servers": [] },
  "filter_config": {
    "rules": [],
    "nat_rules": [],
    "rdr_rules": []
  },
  "filter_enabled_interface_types": [],
  "allowed_upstream_device_classes": [],
  "forwarded_device_classes": { "ipv4": [], "ipv6": [] },
  "interface_provisioning_policy": [{
    "matchers": [ { "interface_name": "[speling" } ],
    "provisioning": "delegated"
  }]
}
"#,
        "did not match any variant";
        "interface_name"
    )]
    fn test_config_denies_invalid_glob(bad_config: &'static str, err_text: &'static str) {
        // Should fail on improper glob: square braces not closed.
        let err =
            Config::load_str(bad_config).expect_err("config shouldn't accept invalid pattern");
        let err = err.downcast::<serde_json5::Error>().expect("downcast error");
        // Ensure the error is complaining about invalid glob.
        assert!(format!("{:?}", err).contains(err_text));
    }

    #[test]
    fn test_config_legacy_wlan_name() {
        let config_str = r#"
{
  "dns_config": { "servers": [] },
  "filter_config": {
    "rules": [],
    "nat_rules": [],
    "rdr_rules": []
  },
  "filter_enabled_interface_types": ["wlan", "ap"],
  "allowed_upstream_device_classes": ["wlan"]
}
"#;
        let Config { filter_enabled_interface_types, allowed_upstream_device_classes, .. } =
            Config::load_str(config_str).unwrap();
        assert_eq!(
            HashSet::from([InterfaceType::WlanClient, InterfaceType::WlanAp]),
            filter_enabled_interface_types
        );
        assert_eq!(
            AllowedDeviceClasses(HashSet::from([DeviceClass::WlanClient])),
            allowed_upstream_device_classes
        );
    }

    #[test]
    fn test_config_supports_json5() {
        let config_str = r#"{
            "dns_config": { "servers": [] },
            // A comment on the config
            "filter_config": {
                'rules': [], // Single quoted string
                "nat_rules": [],
                "rdr_rules": [], // Trailing comma
            },
            "filter_enabled_interface_types": [],
            "allowed_upstream_device_classes": []
        }"#;
        let _ = Config::load_str(config_str).unwrap();
    }

    #[fuchsia::test]
    async fn test_ndp_dns_servers_removed_per_interface_id() {
        let (
            mut netcfg,
            ServerEnds {
                lookup_admin: _,
                dhcpv4_client_provider: _,
                dhcpv6_client_provider: _,
                route_set_v4_provider: _,
                dhcpv4_server: _,
            },
        ) = test_netcfg(NetcfgTestArgs {
            with_dhcpv4_client_provider: false,
            enable_socket_proxy: false,
        })
        .expect("error creating test netcfg");
        let mut dns_watchers = DnsServerWatchers::empty();

        const IFACE_1: InterfaceId = InterfaceId::new(1).unwrap();
        const IFACE_2: InterfaceId = InterfaceId::new(2).unwrap();
        let router_addr_1: [u8; 16] = [1; 16];
        let router_addr_2: [u8; 16] = [2; 16];

        add_test_host_interface(&mut netcfg, &mut dns_watchers, IFACE_1, "testif01").await;
        add_test_host_interface(&mut netcfg, &mut dns_watchers, IFACE_2, "testif02").await;

        let dns_server1 = test_ndp_dns_server(IFACE_1, fidl_ip_v6!("2001:db8::1"));
        let dns_server2 = test_ndp_dns_server(IFACE_2, fidl_ip_v6!("2001:db8::2"));

        // Directly insert entries into ndp_dns_servers at the beginning of the test.
        assert!(
            netcfg
                .ndp_dns_servers
                .insert((IFACE_1, router_addr_1.into()), vec![dns_server1])
                .is_none()
        );
        assert!(
            netcfg
                .ndp_dns_servers
                .insert((IFACE_2, router_addr_2.into()), vec![dns_server2])
                .is_none()
        );
        assert_eq!(netcfg.ndp_dns_servers.len(), 2);

        // Show that a removal event for IFACE_2 does not affect the IFACE_1 entry in the hashmap.
        assert_matches!(netcfg
            .handle_interface_watcher_event(
                fnet_interfaces::Event::Removed(IFACE_2.get()).into(),
                &mut dns_watchers,
                &mut virtualization::Stub,
            )
            .await,
            Ok(Some(id)) => assert_eq!(id, IFACE_2)
        );

        assert_eq!(netcfg.ndp_dns_servers.len(), 1);
        assert!(netcfg.ndp_dns_servers.contains_key(&(IFACE_1, router_addr_1.into())));
        assert!(!netcfg.ndp_dns_servers.contains_key(&(IFACE_2, router_addr_2.into())));

        // Finally show that the removal event for IFACE_1 causes the removal of the IFACE_1 entry.
        assert_matches!(
            netcfg
                .handle_interface_watcher_event(
                    fnet_interfaces::Event::Removed(IFACE_1.get()).into(),
                    &mut dns_watchers,
                    &mut virtualization::Stub,
                )
                .await,
            Ok(Some(IFACE_1))
        );

        assert!(netcfg.ndp_dns_servers.is_empty());
    }

    #[fuchsia::test]
    async fn test_ndp_dns_servers_interface_removal_purges() {
        let (
            mut netcfg,
            ServerEnds {
                lookup_admin: _,
                dhcpv4_client_provider: _,
                dhcpv6_client_provider: _,
                route_set_v4_provider: _,
                dhcpv4_server: _,
            },
        ) = test_netcfg(NetcfgTestArgs {
            with_dhcpv4_client_provider: false,
            enable_socket_proxy: false,
        })
        .expect("error creating test netcfg");
        let mut dns_watchers = DnsServerWatchers::empty();

        const IFACE_ID: InterfaceId = InterfaceId::new(1).unwrap();
        let router_a: [u8; 16] = [10; 16];
        let router_b: [u8; 16] = [20; 16];

        add_test_host_interface(&mut netcfg, &mut dns_watchers, IFACE_ID, "testif01").await;

        let dns_server_a = test_ndp_dns_server(IFACE_ID, fidl_ip_v6!("2001:db8::1"));
        let dns_server_b = test_ndp_dns_server(IFACE_ID, fidl_ip_v6!("2001:db8::2"));

        // Populate multiple router entries for IFACE_ID in ndp_dns_servers.
        let _: Option<Vec<fnet_name::DnsServer_>> =
            netcfg.ndp_dns_servers.insert((IFACE_ID, router_a.into()), vec![dns_server_a]);
        let _: Option<Vec<fnet_name::DnsServer_>> =
            netcfg.ndp_dns_servers.insert((IFACE_ID, router_b.into()), vec![dns_server_b]);
        assert_eq!(netcfg.ndp_dns_servers.len(), 2);

        // Remove IFACE_ID.
        assert_matches!(
            netcfg
                .handle_interface_watcher_event(
                    fnet_interfaces::Event::Removed(IFACE_ID.get()).into(),
                    &mut dns_watchers,
                    &mut virtualization::Stub,
                )
                .await,
            Ok(Some(IFACE_ID))
        );

        // Verify all per-router entries for IFACE_ID are purged from ndp_dns_servers.
        assert!(netcfg.ndp_dns_servers.is_empty());
    }

    async fn add_test_host_interface(
        netcfg: &mut NetCfg<'_>,
        dns_watchers: &mut DnsServerWatchers<'_>,
        interface_id: InterfaceId,
        name: &str,
    ) {
        let (control, _control_server_end) =
            fidl_fuchsia_net_interfaces_ext::admin::Control::create_endpoints()
                .expect("create endpoints");
        let auth_grant = fnet_resources::GrantForInterfaceAuthorization {
            interface_id: interface_id.get(),
            token: zx::Event::create(),
        };

        assert_matches!(
            netcfg.interface_states.insert(
                interface_id,
                InterfaceState {
                    control,
                    device_class: DeviceClass::Virtual,
                    config: InterfaceConfigState::Host(HostInterfaceState {
                        dhcpv4_client: Dhcpv4ClientState::NotRunning,
                        dhcpv6_client_state: None,
                        dhcpv6_pd_config: None,
                        interface_admin_auth: auth_grant,
                        interface_naming_id: test_interface_naming_id(),
                    }),
                    provisioning: interface::ProvisioningType::Local,
                },
            ),
            None
        );

        assert_matches!(
            netcfg
                .handle_interface_watcher_event(
                    fnet_interfaces::Event::Added(fnet_interfaces::Properties {
                        id: Some(interface_id.get()),
                        name: Some(name.to_string()),
                        port_class: Some(fnet_interfaces::PortClass::Device(
                            fidl_fuchsia_hardware_network::PortClass::Virtual,
                        )),
                        online: Some(true),
                        addresses: Some(Vec::new()),
                        has_default_ipv4_route: Some(false),
                        has_default_ipv6_route: Some(false),
                        ..Default::default()
                    })
                    .into(),
                    dns_watchers,
                    &mut virtualization::Stub,
                )
                .await,
            Ok(None)
        );
    }

    fn test_ndp_dns_server(
        interface_id: InterfaceId,
        address: fnet::Ipv6Address,
    ) -> fnet_name::DnsServer_ {
        fnet_name::DnsServer_ {
            address: Some(fnet::SocketAddress::Ipv6(fnet::Ipv6SocketAddress {
                address,
                port: dns::DNS_PORT,
                zone_index: interface_id.get(),
            })),
            source: Some(fnet_name::DnsServerSource::Ndp(fnet_name::NdpDnsServerSource {
                source_interface: Some(interface_id.get()),
                ..Default::default()
            })),
            ..Default::default()
        }
    }

    #[test]
    fn test_ndp_dns_expired_event_removes_target_router() {
        let mut exec = fuchsia_async::TestExecutor::new();

        let (
            mut netcfg,
            ServerEnds {
                lookup_admin: _,
                dhcpv4_client_provider: _,
                dhcpv6_client_provider: _,
                route_set_v4_provider: _,
                dhcpv4_server: _,
            },
        ) = test_netcfg(NetcfgTestArgs {
            with_dhcpv4_client_provider: false,
            enable_socket_proxy: false,
        })
        .expect("error creating test netcfg");
        const IFACE_ID: InterfaceId = InterfaceId::new(1).unwrap();
        let router_a: net_types::ip::Ipv6Addr = [10; 16].into();
        let router_b: net_types::ip::Ipv6Addr = [20; 16].into();

        let dns_server_a = test_ndp_dns_server(IFACE_ID, fidl_ip_v6!("2001:db8::1"));
        let dns_server_b = test_ndp_dns_server(IFACE_ID, fidl_ip_v6!("2001:db8::2"));

        // Insert entries for both Router A and Router B on IFACE_ID.
        assert!(netcfg.ndp_dns_servers.insert((IFACE_ID, router_a), vec![dns_server_a]).is_none());
        assert!(netcfg.ndp_dns_servers.insert((IFACE_ID, router_b), vec![dns_server_b]).is_none());
        assert_eq!(netcfg.ndp_dns_servers.len(), 2);

        // Call handle_ndp_dns_expired for Router A directly.
        {
            let fut = netcfg.handle_ndp_dns_expired(IFACE_ID, router_a);
            futures::pin_mut!(fut);
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));
        }

        // Assert Router A is removed from ndp_dns_servers, while Router B remains.
        assert_eq!(netcfg.ndp_dns_servers.len(), 1);
        assert_eq!(netcfg.ndp_dns_servers.get(&(IFACE_ID, router_a)), None);
        assert!(netcfg.ndp_dns_servers.contains_key(&(IFACE_ID, router_b)));
    }
}
