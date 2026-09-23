// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Provides utilities for test realms.

use std::borrow::Cow;
use std::collections::HashMap;
use std::future::Future;

use cm_rust::NativeIntoFidl as _;
use fidl::endpoints::{DiscoverableProtocolMarker as _, ServiceMarker};
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_hardware_network as fhwnet;
use fidl_fuchsia_net_debug as fnet_debug;
use fidl_fuchsia_net_dhcp as fnet_dhcp;
use fidl_fuchsia_net_dhcpv6 as fnet_dhcpv6;
use fidl_fuchsia_net_filter as fnet_filter;
use fidl_fuchsia_net_filter_deprecated as fnet_filter_deprecated;
use fidl_fuchsia_net_interfaces as fnet_interfaces;
use fidl_fuchsia_net_interfaces_admin as fnet_interfaces_admin;
use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
use fidl_fuchsia_net_masquerade as fnet_masquerade;
use fidl_fuchsia_net_multicast_admin as fnet_multicast_admin;
use fidl_fuchsia_net_name as fnet_name;
use fidl_fuchsia_net_ndp as fnet_ndp;
use fidl_fuchsia_net_neighbor as fnet_neighbor;
use fidl_fuchsia_net_policy_properties as fnp_properties;
use fidl_fuchsia_net_policy_socketproxy as fnp_socketproxy;
use fidl_fuchsia_net_power as fnet_power;
use fidl_fuchsia_net_reachability as fnet_reachability;
use fidl_fuchsia_net_root as fnet_root;
use fidl_fuchsia_net_routes as fnet_routes;
use fidl_fuchsia_net_routes_admin as fnet_routes_admin;
use fidl_fuchsia_net_settings as fnet_settings;
use fidl_fuchsia_net_sockets as fnet_sockets;
use fidl_fuchsia_net_stack as fnet_stack;
use fidl_fuchsia_net_test_realm as fntr;
use fidl_fuchsia_net_virtualization as fnet_virtualization;
use fidl_fuchsia_netemul as fnetemul;
use fidl_fuchsia_posix_socket as fposix_socket;
use fidl_fuchsia_posix_socket_packet as fposix_socket_packet;
use fidl_fuchsia_posix_socket_raw as fposix_socket_raw;
use fidl_fuchsia_scheduler as fscheduler;
use fidl_fuchsia_stash as fstash;
use fidl_fuchsia_update_verify as fupdate_verify;

use anyhow::Context as _;

use crate::Result;

/// The Netstack version. Used to specify which Netstack version to use in a
/// [`KnownServiceProvider::Netstack`].
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[allow(missing_docs)]
pub enum NetstackVersion {
    Netstack2 { tracing: bool, fast_udp: bool },
    Netstack3,
    ProdNetstack2,
    ProdNetstack3,
}

impl NetstackVersion {
    /// Gets the Fuchsia URL for this Netstack component.
    pub fn get_url(&self) -> &'static str {
        match self {
            NetstackVersion::Netstack2 { tracing, fast_udp } => match (tracing, fast_udp) {
                (false, false) => "#meta/netstack-debug.cm",
                (false, true) => "#meta/netstack-with-fast-udp-debug.cm",
                (true, false) => "#meta/netstack-with-tracing.cm",
                (true, true) => "#meta/netstack-with-fast-udp-tracing.cm",
            },
            NetstackVersion::Netstack3 => "#meta/netstack3-debug.cm",
            NetstackVersion::ProdNetstack2 => "#meta/netstack.cm",
            NetstackVersion::ProdNetstack3 => "#meta/netstack3.cm",
        }
    }

    /// Gets the services exposed by this Netstack component.
    pub fn get_services(&self) -> &[&'static str] {
        macro_rules! common_services_and {
            ($($name:expr),*) => {[
                fnet_debug::InterfacesMarker::PROTOCOL_NAME,
                fnet_interfaces_admin::InstallerMarker::PROTOCOL_NAME,
                fnet_interfaces::StateMarker::PROTOCOL_NAME,
                fnet_multicast_admin::Ipv4RoutingTableControllerMarker::PROTOCOL_NAME,
                fnet_multicast_admin::Ipv6RoutingTableControllerMarker::PROTOCOL_NAME,
                fnet_name::DnsServerWatcherMarker::PROTOCOL_NAME,
                fnet_neighbor::ControllerMarker::PROTOCOL_NAME,
                fnet_neighbor::ViewMarker::PROTOCOL_NAME,
                fnet_root::InterfacesMarker::PROTOCOL_NAME,
                fnet_root::RoutesV4Marker::PROTOCOL_NAME,
                fnet_root::RoutesV6Marker::PROTOCOL_NAME,
                fnet_routes::StateMarker::PROTOCOL_NAME,
                fnet_routes::StateV4Marker::PROTOCOL_NAME,
                fnet_routes::StateV6Marker::PROTOCOL_NAME,
                fnet_routes_admin::RouteTableProviderV4Marker::PROTOCOL_NAME,
                fnet_routes_admin::RouteTableProviderV6Marker::PROTOCOL_NAME,
                fnet_routes_admin::RouteTableV4Marker::PROTOCOL_NAME,
                fnet_routes_admin::RouteTableV6Marker::PROTOCOL_NAME,
                fnet_routes_admin::RuleTableV4Marker::PROTOCOL_NAME,
                fnet_routes_admin::RuleTableV6Marker::PROTOCOL_NAME,
                fnet_stack::StackMarker::PROTOCOL_NAME,
                fposix_socket_packet::ProviderMarker::PROTOCOL_NAME,
                fposix_socket_raw::ProviderMarker::PROTOCOL_NAME,
                fposix_socket::ProviderMarker::PROTOCOL_NAME,
                fnet_debug::DiagnosticsMarker::PROTOCOL_NAME,
                fnet_debug::PacketCaptureProviderMarker::PROTOCOL_NAME,

                fupdate_verify::ComponentOtaHealthCheckMarker::PROTOCOL_NAME,
                $($name),*
            ]};
            // Strip trailing comma.
            ($($name:expr),*,) => {common_services_and!($($name),*)}
        }
        match self {
            NetstackVersion::Netstack2 { tracing: _, fast_udp: _ }
            | NetstackVersion::ProdNetstack2 => &common_services_and!(
                fnet_filter_deprecated::FilterMarker::PROTOCOL_NAME,
                fnet_stack::LogMarker::PROTOCOL_NAME,
            ),
            NetstackVersion::Netstack3 | NetstackVersion::ProdNetstack3 => &common_services_and!(
                fnet_filter::ControlMarker::PROTOCOL_NAME,
                fnet_filter::StateMarker::PROTOCOL_NAME,
                fnet_ndp::RouterAdvertisementOptionWatcherProviderMarker::PROTOCOL_NAME,
                fnet_power::WakeGroupProviderMarker::PROTOCOL_NAME,
                fnet_root::FilterMarker::PROTOCOL_NAME,
                fnet_settings::StateMarker::PROTOCOL_NAME,
                fnet_settings::ControlMarker::PROTOCOL_NAME,
                fnet_sockets::DiagnosticsMarker::PROTOCOL_NAME,
                fnet_sockets::ControlMarker::PROTOCOL_NAME,
            ),
        }
    }

    /// Returns true if this is a netstack3 version.
    pub const fn is_netstack3(&self) -> bool {
        match self {
            Self::Netstack3 | Self::ProdNetstack3 => true,
            Self::Netstack2 { .. } | Self::ProdNetstack2 => false,
        }
    }
}

/// An extension trait for [`Netstack`].
pub trait NetstackExt {
    /// Whether to use the out of stack DHCP client for the given Netstack.
    const USE_OUT_OF_STACK_DHCP_CLIENT: bool;
}

impl<N: Netstack> NetstackExt for N {
    const USE_OUT_OF_STACK_DHCP_CLIENT: bool = match Self::VERSION {
        NetstackVersion::Netstack3 | NetstackVersion::ProdNetstack3 => true,
        NetstackVersion::Netstack2 { .. } | NetstackVersion::ProdNetstack2 => false,
    };
}

/// The NetCfg version.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum NetCfgVersion {
    /// The basic NetCfg version.
    Basic,
    /// The advanced NetCfg version.
    Advanced,
}

/// The network manager to use in a [`KnownServiceProvider::Manager`].
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum ManagementAgent {
    /// A version of netcfg.
    NetCfg(NetCfgVersion),
}

impl ManagementAgent {
    /// Gets the URL for this network manager component.
    pub fn get_url(&self) -> &'static str {
        match self {
            Self::NetCfg(NetCfgVersion::Basic) => constants::netcfg::basic::COMPONENT_URL,
            Self::NetCfg(NetCfgVersion::Advanced) => constants::netcfg::advanced::COMPONENT_URL,
        }
    }

    /// Default arguments that should be passed to the component when run in a
    /// test realm.
    pub fn get_program_args(&self) -> &[&'static str] {
        match self {
            Self::NetCfg(NetCfgVersion::Basic) | Self::NetCfg(NetCfgVersion::Advanced) => {
                &["--min-severity", "DEBUG"]
            }
        }
    }

    /// Gets the services exposed by this management agent.
    pub fn get_services(&self) -> &[&'static str] {
        match self {
            Self::NetCfg(NetCfgVersion::Basic) => &[
                fnet_dhcpv6::PrefixProviderMarker::PROTOCOL_NAME,
                fnet_masquerade::FactoryMarker::PROTOCOL_NAME,
                fnet_name::DnsServerWatcherMarker::PROTOCOL_NAME,
                fnp_properties::NetworksMarker::PROTOCOL_NAME,
                fnp_socketproxy::NetworkRegistryMarker::PROTOCOL_NAME,
                fnp_properties::NetworkTokenResolverMarker::PROTOCOL_NAME,
                fnet_reachability::MonitorMarker::PROTOCOL_NAME,
            ],
            Self::NetCfg(NetCfgVersion::Advanced) => &[
                fnet_dhcpv6::PrefixProviderMarker::PROTOCOL_NAME,
                fnet_masquerade::FactoryMarker::PROTOCOL_NAME,
                fnet_name::DnsServerWatcherMarker::PROTOCOL_NAME,
                fnet_virtualization::ControlMarker::PROTOCOL_NAME,
                fnp_properties::NetworksMarker::PROTOCOL_NAME,
                fnp_socketproxy::NetworkRegistryMarker::PROTOCOL_NAME,
                fnp_properties::NetworkTokenResolverMarker::PROTOCOL_NAME,
                fnet_reachability::MonitorMarker::PROTOCOL_NAME,
            ],
        }
    }
}

/// Available configurations for a Manager.
#[derive(Clone, Eq, PartialEq, Debug)]
#[allow(missing_docs)]
pub enum ManagerConfig {
    Empty,
    Dhcpv6,
    Forwarding,
    AllDelegated,
    IfacePrefix,
    DuplicateNames,
    EnableSocketProxy,
    EnableSocketProxyAllDelegated,
    PacketFilterEthernet,
    PacketFilterWlan,
    WithBlackhole,
    AllInterfaceLocalDelegated,
}

impl ManagerConfig {
    fn as_str(&self) -> &'static str {
        match self {
            ManagerConfig::Empty => "/pkg/netcfg/empty.json",
            ManagerConfig::Dhcpv6 => "/pkg/netcfg/dhcpv6.json",
            ManagerConfig::Forwarding => "/pkg/netcfg/forwarding.json",
            ManagerConfig::AllDelegated => "/pkg/netcfg/all_delegated.json",
            ManagerConfig::IfacePrefix => "/pkg/netcfg/iface_prefix.json",
            ManagerConfig::DuplicateNames => "/pkg/netcfg/duplicate_names.json",
            ManagerConfig::EnableSocketProxy => "/pkg/netcfg/enable_socket_proxy.json",
            ManagerConfig::EnableSocketProxyAllDelegated => {
                "/pkg/netcfg/enable_socket_proxy_all_delegated.json"
            }
            ManagerConfig::PacketFilterEthernet => "/pkg/netcfg/packet_filter_ethernet.json",
            ManagerConfig::PacketFilterWlan => "/pkg/netcfg/packet_filter_wlan.json",
            ManagerConfig::WithBlackhole => "/pkg/netcfg/with_blackhole.json",
            ManagerConfig::AllInterfaceLocalDelegated => {
                "/pkg/netcfg/all_interface_local_delegated.json"
            }
        }
    }
}

#[derive(Copy, Clone, Default, Eq, PartialEq, Debug)]
/// The type of `socket-proxy` implementation to use.
pub enum SocketProxyType {
    #[default]
    /// No socket proxy is present.
    None,
    /// Use the real socket proxy implementation.
    Real,
    /// Use the fake socket proxy implementation to allow mocking behavior.
    Fake,
}

impl SocketProxyType {
    /// Returns the appropriate `KnownServiceProvider` variant for this type.
    pub fn known_service_provider(&self) -> Option<KnownServiceProvider> {
        match self {
            SocketProxyType::None => None,
            SocketProxyType::Real => Some(KnownServiceProvider::SocketProxy),
            SocketProxyType::Fake => Some(KnownServiceProvider::FakeSocketProxy),
        }
    }
}

/// Components that provide known services used in tests.
#[derive(Clone, Eq, PartialEq, Debug)]
#[allow(missing_docs)]
pub enum KnownServiceProvider {
    Netstack(NetstackVersion),
    Manager {
        agent: ManagementAgent,
        config: ManagerConfig,
        use_dhcp_server: bool,
        use_out_of_stack_dhcp_client: bool,
        socket_proxy_type: SocketProxyType,
    },
    SecureStash,
    DhcpServer {
        persistent: bool,
    },
    DhcpClient,
    Dhcpv6Client,
    DnsResolver,
    Reachability {
        eager: bool,
    },
    SocketProxy,
    NetworkTestRealm {
        require_outer_netstack: bool,
    },
    FakeClock,
    FakeSocketProxy,
    FakeNetcfg,
}

/// Constant properties of components used in networking integration tests, such
/// as monikers and URLs.
#[allow(missing_docs)]
pub mod constants {
    pub mod netstack {
        pub const COMPONENT_NAME: &str = "netstack";
    }
    pub mod netcfg {
        pub const COMPONENT_NAME: &str = "netcfg";
        pub mod basic {
            pub const COMPONENT_URL: &str = "#meta/netcfg-basic.cm";
        }
        pub mod advanced {
            pub const COMPONENT_URL: &str = "#meta/netcfg-advanced.cm";
        }
        pub mod fake {
            pub const COMPONENT_URL: &str = "#meta/fake_netcfg.cm";
        }
        // These capability names and filepaths should match the devfs capabilities used by netcfg
        // in its component manifest, i.e. netcfg.cml.
        pub const DEV_CLASS_NETWORK: &str = "dev-class-network";
        pub const CLASS_NETWORK_PATH: &str = "class/network";
    }
    pub mod socket_proxy {
        pub const COMPONENT_NAME: &str = "network-socket-proxy";
        pub const COMPONENT_URL: &str = "#meta/network-socket-proxy.cm";
    }
    pub mod secure_stash {
        pub const COMPONENT_NAME: &str = "stash_secure";
        pub const COMPONENT_URL: &str = "#meta/stash_secure.cm";
    }
    pub mod dhcp_server {
        pub const COMPONENT_NAME: &str = "dhcpd";
        pub const COMPONENT_URL: &str = "#meta/dhcpv4_server.cm";
    }
    pub mod dhcp_client {
        pub const COMPONENT_NAME: &str = "dhcp-client";
        pub const COMPONENT_URL: &str = "#meta/dhcp-client.cm";
    }
    pub mod dhcpv6_client {
        pub const COMPONENT_NAME: &str = "dhcpv6-client";
        pub const COMPONENT_URL: &str = "#meta/dhcpv6-client.cm";
    }
    pub mod dns_resolver {
        pub const COMPONENT_NAME: &str = "dns_resolver";
        pub const COMPONENT_URL: &str = "#meta/dns_resolver_with_fake_time.cm";
    }
    pub mod reachability {
        pub const COMPONENT_NAME: &str = "reachability";
        pub const COMPONENT_URL: &str = "#meta/reachability_with_fake_time.cm";
    }
    pub mod network_test_realm {
        pub const COMPONENT_NAME: &str = "controller";
        pub const COMPONENT_URL: &str = "#meta/controller.cm";
    }
    pub mod fake_clock {
        pub const COMPONENT_NAME: &str = "fake_clock";
        pub const COMPONENT_URL: &str = "#meta/fake_clock.cm";
    }
    pub mod fake_socket_proxy {
        pub const COMPONENT_NAME: &str = "fake_socket_proxy";
        pub const COMPONENT_URL: &str = "#meta/fake_socket_proxy.cm";
    }
}

fn protocol_dep<P>(component_name: &'static str) -> fnetemul::ChildDep
where
    P: fidl::endpoints::DiscoverableProtocolMarker,
{
    fnetemul::ChildDep {
        name: Some(component_name.into()),
        capability: Some(fnetemul::ExposedCapability::Protocol(P::PROTOCOL_NAME.to_string())),
        ..Default::default()
    }
}

fn or_void_protocol_dep<P>(
    component_name: &'static str,
    is_child_present: bool,
) -> fnetemul::ChildDep
where
    P: fidl::endpoints::DiscoverableProtocolMarker,
{
    if is_child_present { protocol_dep::<P>(component_name) } else { void_protocol_dep::<P>() }
}

fn void_protocol_dep<P>() -> fnetemul::ChildDep
where
    P: fidl::endpoints::DiscoverableProtocolMarker,
{
    fnetemul::ChildDep {
        name: None,
        capability: Some(fnetemul::ExposedCapability::Protocol(P::PROTOCOL_NAME.to_string())),
        ..Default::default()
    }
}

impl From<KnownServiceProvider> for fnetemul::ChildDef {
    fn from(s: KnownServiceProvider) -> Self {
        (&s).into()
    }
}

impl<'a> From<&'a KnownServiceProvider> for fnetemul::ChildDef {
    fn from(s: &'a KnownServiceProvider) -> Self {
        match s {
            KnownServiceProvider::Netstack(version) => fnetemul::ChildDef {
                name: Some(constants::netstack::COMPONENT_NAME.to_string()),
                source: Some(fnetemul::ChildSource::Component(version.get_url().to_string())),
                exposes: Some(
                    version.get_services().iter().map(|service| service.to_string()).collect(),
                ),
                uses: {
                    let mut uses = vec![fnetemul::Capability::LogSink(fnetemul::Empty {})];
                    match version {
                        // NB: intentionally do not route SecureStore; it is
                        // intentionally not available in all tests to
                        // ensure that its absence is handled gracefully.
                        // Note also that netstack-debug does not have a use
                        // declaration for this protocol for the same
                        // reason.
                        NetstackVersion::Netstack2 { tracing: false, fast_udp: _ } => {}
                        NetstackVersion::Netstack2 { tracing: true, fast_udp: _ } => {
                            uses.push(fnetemul::Capability::TracingProvider(fnetemul::Empty));
                        }
                        NetstackVersion::ProdNetstack2 => {
                            uses.push(fnetemul::Capability::ChildDep(protocol_dep::<
                                fstash::SecureStoreMarker,
                            >(
                                constants::secure_stash::COMPONENT_NAME,
                            )));
                        }
                        NetstackVersion::Netstack3 | NetstackVersion::ProdNetstack3 => {
                            uses.push(fnetemul::Capability::TracingProvider(fnetemul::Empty));
                            uses.push(fnetemul::Capability::StorageDep(fnetemul::StorageDep {
                                variant: Some(fnetemul::StorageVariant::Data),
                                path: Some("/data".to_string()),
                                ..Default::default()
                            }));
                        }
                    }
                    Some(fnetemul::ChildUses::Capabilities(uses))
                },
                ..Default::default()
            },
            KnownServiceProvider::Manager {
                agent,
                use_dhcp_server,
                config,
                use_out_of_stack_dhcp_client,
                socket_proxy_type: _,
            } => {
                let enable_dhcpv6 = match config {
                    ManagerConfig::Dhcpv6 => true,
                    ManagerConfig::Forwarding
                    | ManagerConfig::Empty
                    | ManagerConfig::AllDelegated
                    | ManagerConfig::IfacePrefix
                    | ManagerConfig::DuplicateNames
                    | ManagerConfig::EnableSocketProxy
                    | ManagerConfig::EnableSocketProxyAllDelegated
                    | ManagerConfig::PacketFilterEthernet
                    | ManagerConfig::PacketFilterWlan
                    | ManagerConfig::WithBlackhole
                    | ManagerConfig::AllInterfaceLocalDelegated => false,
                };

                fnetemul::ChildDef {
                    name: Some(constants::netcfg::COMPONENT_NAME.to_string()),
                    source: Some(fnetemul::ChildSource::Component(agent.get_url().to_string())),
                    program_args: Some(
                        agent
                            .get_program_args()
                            .iter()
                            .cloned()
                            .chain(std::iter::once("--config-data"))
                            .chain(std::iter::once(config.as_str()))
                            .map(Into::into)
                            .collect(),
                    ),
                    exposes: Some(
                        agent.get_services().iter().map(|service| service.to_string()).collect(),
                    ),
                    uses: Some(fnetemul::ChildUses::Capabilities(
                        std::iter::once(fnetemul::Capability::ChildDep(or_void_protocol_dep::<
                            fnet_dhcp::Server_Marker,
                        >(
                            constants::dhcp_server::COMPONENT_NAME,
                            *use_dhcp_server,
                        )))
                        .chain(std::iter::once(fnetemul::Capability::ChildDep(
                            or_void_protocol_dep::<fnet_dhcpv6::ClientProviderMarker>(
                                constants::dhcpv6_client::COMPONENT_NAME,
                                enable_dhcpv6,
                            ),
                        )))
                        .chain(std::iter::once(fnetemul::Capability::ChildDep(
                            or_void_protocol_dep::<fnet_dhcp::ClientProviderMarker>(
                                constants::dhcp_client::COMPONENT_NAME,
                                *use_out_of_stack_dhcp_client,
                            ),
                        )))
                        .chain(
                            [
                                fnetemul::Capability::LogSink(fnetemul::Empty {}),
                                fnetemul::Capability::ChildDep(fnetemul::ChildDep {
                                    dynamically_offer_from_void: Some(true),
                                    ..protocol_dep::<fnet_filter::ControlMarker>(
                                        constants::netstack::COMPONENT_NAME,
                                    )
                                }),
                                fnetemul::Capability::ChildDep(fnetemul::ChildDep {
                                    dynamically_offer_from_void: Some(true),
                                    ..protocol_dep::<fnet_filter_deprecated::FilterMarker>(
                                        constants::netstack::COMPONENT_NAME,
                                    )
                                }),
                                fnetemul::Capability::ChildDep(protocol_dep::<
                                    fnet_interfaces::StateMarker,
                                >(
                                    constants::netstack::COMPONENT_NAME,
                                )),
                                fnetemul::Capability::ChildDep(protocol_dep::<
                                    fnet_interfaces_admin::InstallerMarker,
                                >(
                                    constants::netstack::COMPONENT_NAME,
                                )),
                                fnetemul::Capability::ChildDep(protocol_dep::<
                                    fnet_stack::StackMarker,
                                >(
                                    constants::netstack::COMPONENT_NAME,
                                )),
                                fnetemul::Capability::ChildDep(protocol_dep::<
                                    fnet_routes_admin::RouteTableV4Marker,
                                >(
                                    constants::netstack::COMPONENT_NAME,
                                )),
                                fnetemul::Capability::ChildDep(protocol_dep::<
                                    fnet_routes_admin::RouteTableV6Marker,
                                >(
                                    constants::netstack::COMPONENT_NAME,
                                )),
                                fnetemul::Capability::ChildDep(protocol_dep::<
                                    fnet_routes_admin::RuleTableV4Marker,
                                >(
                                    constants::netstack::COMPONENT_NAME,
                                )),
                                fnetemul::Capability::ChildDep(protocol_dep::<
                                    fnet_routes_admin::RuleTableV6Marker,
                                >(
                                    constants::netstack::COMPONENT_NAME,
                                )),
                                fnetemul::Capability::ChildDep(protocol_dep::<
                                    fnet_name::DnsServerWatcherMarker,
                                >(
                                    constants::netstack::COMPONENT_NAME,
                                )),
                                fnetemul::Capability::ChildDep(protocol_dep::<
                                    fnet_name::LookupAdminMarker,
                                >(
                                    constants::dns_resolver::COMPONENT_NAME,
                                )),
                                fnetemul::Capability::ChildDep(protocol_dep::<
                                    fnet_ndp::RouterAdvertisementOptionWatcherProviderMarker,
                                >(
                                    constants::netstack::COMPONENT_NAME,
                                )),
                                fnetemul::Capability::ChildDep(fnetemul::ChildDep {
                                    name: Some(
                                        fnetemul::NETEMUL_SERVICES_COMPONENT_NAME.to_string(),
                                    ),
                                    capability: Some(fnetemul::ExposedCapability::Service(
                                        fhwnet::ServiceMarker::SERVICE_NAME.to_string(),
                                    )),
                                    ..Default::default()
                                }),
                                fnetemul::Capability::StorageDep(fnetemul::StorageDep {
                                    variant: Some(fnetemul::StorageVariant::Data),
                                    path: Some("/data".to_string()),
                                    ..Default::default()
                                }),
                            ]
                            .into_iter(),
                        )
                        .collect(),
                    )),
                    eager: Some(true),
                    ..Default::default()
                }
            }
            KnownServiceProvider::SecureStash => fnetemul::ChildDef {
                name: Some(constants::secure_stash::COMPONENT_NAME.to_string()),
                source: Some(fnetemul::ChildSource::Component(
                    constants::secure_stash::COMPONENT_URL.to_string(),
                )),
                exposes: Some(vec![fstash::SecureStoreMarker::PROTOCOL_NAME.to_string()]),
                uses: Some(fnetemul::ChildUses::Capabilities(vec![
                    fnetemul::Capability::LogSink(fnetemul::Empty {}),
                    fnetemul::Capability::StorageDep(fnetemul::StorageDep {
                        variant: Some(fnetemul::StorageVariant::Data),
                        path: Some("/data".to_string()),
                        ..Default::default()
                    }),
                ])),
                ..Default::default()
            },
            KnownServiceProvider::DhcpServer { persistent } => fnetemul::ChildDef {
                name: Some(constants::dhcp_server::COMPONENT_NAME.to_string()),
                source: Some(fnetemul::ChildSource::Component(
                    constants::dhcp_server::COMPONENT_URL.to_string(),
                )),
                exposes: Some(vec![fnet_dhcp::Server_Marker::PROTOCOL_NAME.to_string()]),
                uses: Some(fnetemul::ChildUses::Capabilities(
                    [
                        fnetemul::Capability::LogSink(fnetemul::Empty {}),
                        fnetemul::Capability::ChildDep(protocol_dep::<
                            fnet_neighbor::ControllerMarker,
                        >(
                            constants::netstack::COMPONENT_NAME
                        )),
                        fnetemul::Capability::ChildDep(
                            protocol_dep::<fposix_socket::ProviderMarker>(
                                constants::netstack::COMPONENT_NAME,
                            ),
                        ),
                        fnetemul::Capability::ChildDep(protocol_dep::<
                            fposix_socket_packet::ProviderMarker,
                        >(
                            constants::netstack::COMPONENT_NAME
                        )),
                    ]
                    .into_iter()
                    .chain(persistent.then_some(fnetemul::Capability::ChildDep(protocol_dep::<
                        fstash::SecureStoreMarker,
                    >(
                        constants::secure_stash::COMPONENT_NAME,
                    ))))
                    .collect(),
                )),
                program_args: if *persistent {
                    Some(vec![String::from("--persistent")])
                } else {
                    None
                },
                ..Default::default()
            },
            KnownServiceProvider::DhcpClient => fnetemul::ChildDef {
                name: Some(constants::dhcp_client::COMPONENT_NAME.to_string()),
                source: Some(fnetemul::ChildSource::Component(
                    constants::dhcp_client::COMPONENT_URL.to_string(),
                )),
                exposes: Some(vec![fnet_dhcp::ClientProviderMarker::PROTOCOL_NAME.to_string()]),
                uses: Some(fnetemul::ChildUses::Capabilities(vec![
                    fnetemul::Capability::LogSink(fnetemul::Empty {}),
                    fnetemul::Capability::ChildDep(protocol_dep::<fposix_socket::ProviderMarker>(
                        constants::netstack::COMPONENT_NAME,
                    )),
                    fnetemul::Capability::ChildDep(protocol_dep::<
                        fposix_socket_packet::ProviderMarker,
                    >(
                        constants::netstack::COMPONENT_NAME
                    )),
                ])),
                program_args: None,
                ..Default::default()
            },
            KnownServiceProvider::Dhcpv6Client => fnetemul::ChildDef {
                name: Some(constants::dhcpv6_client::COMPONENT_NAME.to_string()),
                source: Some(fnetemul::ChildSource::Component(
                    constants::dhcpv6_client::COMPONENT_URL.to_string(),
                )),
                exposes: Some(vec![fnet_dhcpv6::ClientProviderMarker::PROTOCOL_NAME.to_string()]),
                uses: Some(fnetemul::ChildUses::Capabilities(vec![
                    fnetemul::Capability::LogSink(fnetemul::Empty {}),
                    fnetemul::Capability::ChildDep(protocol_dep::<fposix_socket::ProviderMarker>(
                        constants::netstack::COMPONENT_NAME,
                    )),
                ])),
                ..Default::default()
            },
            KnownServiceProvider::DnsResolver => fnetemul::ChildDef {
                name: Some(constants::dns_resolver::COMPONENT_NAME.to_string()),
                source: Some(fnetemul::ChildSource::Component(
                    constants::dns_resolver::COMPONENT_URL.to_string(),
                )),
                exposes: Some(vec![
                    fnet_name::LookupAdminMarker::PROTOCOL_NAME.to_string(),
                    fnet_name::LookupMarker::PROTOCOL_NAME.to_string(),
                ]),
                uses: Some(fnetemul::ChildUses::Capabilities(vec![
                    fnetemul::Capability::LogSink(fnetemul::Empty {}),
                    fnetemul::Capability::ChildDep(protocol_dep::<fnet_routes::StateMarker>(
                        constants::netstack::COMPONENT_NAME,
                    )),
                    fnetemul::Capability::ChildDep(protocol_dep::<fposix_socket::ProviderMarker>(
                        constants::netstack::COMPONENT_NAME,
                    )),
                    fnetemul::Capability::ChildDep(protocol_dep::<
                        fidl_fuchsia_testing::FakeClockMarker,
                    >(
                        constants::fake_clock::COMPONENT_NAME
                    )),
                    fnetemul::Capability::ChildDep(void_protocol_dep::<
                        fscheduler::RoleManagerMarker,
                    >()),
                ])),
                ..Default::default()
            },
            KnownServiceProvider::Reachability { eager } => fnetemul::ChildDef {
                name: Some(constants::reachability::COMPONENT_NAME.to_string()),
                source: Some(fnetemul::ChildSource::Component(
                    constants::reachability::COMPONENT_URL.to_string(),
                )),
                exposes: Some(vec![fnet_reachability::MonitorMarker::PROTOCOL_NAME.to_string()]),
                uses: Some(fnetemul::ChildUses::Capabilities(vec![
                    fnetemul::Capability::LogSink(fnetemul::Empty {}),
                    fnetemul::Capability::ChildDep(protocol_dep::<fnet_interfaces::StateMarker>(
                        constants::netstack::COMPONENT_NAME,
                    )),
                    fnetemul::Capability::ChildDep(protocol_dep::<fposix_socket::ProviderMarker>(
                        constants::netstack::COMPONENT_NAME,
                    )),
                    fnetemul::Capability::ChildDep(protocol_dep::<fnet_name::LookupMarker>(
                        constants::dns_resolver::COMPONENT_NAME,
                    )),
                    fnetemul::Capability::ChildDep(protocol_dep::<fnet_neighbor::ViewMarker>(
                        constants::netstack::COMPONENT_NAME,
                    )),
                    fnetemul::Capability::ChildDep(protocol_dep::<fnet_debug::InterfacesMarker>(
                        constants::netstack::COMPONENT_NAME,
                    )),
                    fnetemul::Capability::ChildDep(protocol_dep::<fnet_root::InterfacesMarker>(
                        constants::netstack::COMPONENT_NAME,
                    )),
                    fnetemul::Capability::ChildDep(protocol_dep::<fnet_routes::StateV4Marker>(
                        constants::netstack::COMPONENT_NAME,
                    )),
                    fnetemul::Capability::ChildDep(protocol_dep::<fnet_routes::StateV6Marker>(
                        constants::netstack::COMPONENT_NAME,
                    )),
                    fnetemul::Capability::ChildDep(protocol_dep::<fnet_debug::DiagnosticsMarker>(
                        constants::netstack::COMPONENT_NAME,
                    )),
                    fnetemul::Capability::ChildDep(protocol_dep::<
                        fidl_fuchsia_testing::FakeClockMarker,
                    >(
                        constants::fake_clock::COMPONENT_NAME
                    )),
                ])),
                eager: Some(*eager),
                ..Default::default()
            },
            KnownServiceProvider::SocketProxy => fnetemul::ChildDef {
                name: Some(constants::socket_proxy::COMPONENT_NAME.to_string()),
                source: Some(fnetemul::ChildSource::Component(
                    constants::socket_proxy::COMPONENT_URL.to_string(),
                )),
                exposes: Some(vec![
                    fposix_socket::ProviderMarker::PROTOCOL_NAME.to_string(),
                    fposix_socket_raw::ProviderMarker::PROTOCOL_NAME.to_string(),
                    fnp_socketproxy::StarnixNetworksMarker::PROTOCOL_NAME.to_string(),
                ]),
                uses: Some(fnetemul::ChildUses::Capabilities(vec![
                    fnetemul::Capability::ChildDep(protocol_dep::<fposix_socket::ProviderMarker>(
                        constants::netstack::COMPONENT_NAME,
                    )),
                    fnetemul::Capability::ChildDep(
                        protocol_dep::<fposix_socket_raw::ProviderMarker>(
                            constants::netstack::COMPONENT_NAME,
                        ),
                    ),
                    fnetemul::Capability::ChildDep(fnetemul::ChildDep {
                        is_weak: Some(true),
                        ..protocol_dep::<fnp_properties::NetworksMarker>(
                            constants::netcfg::COMPONENT_NAME,
                        )
                    }),
                    fnetemul::Capability::ChildDep(fnetemul::ChildDep {
                        is_weak: Some(true),
                        ..protocol_dep::<fnp_socketproxy::NetworkRegistryMarker>(
                            constants::netcfg::COMPONENT_NAME,
                        )
                    }),
                ])),
                ..Default::default()
            },
            KnownServiceProvider::NetworkTestRealm { require_outer_netstack } => {
                fnetemul::ChildDef {
                    name: Some(constants::network_test_realm::COMPONENT_NAME.to_string()),
                    source: Some(fnetemul::ChildSource::Component(
                        constants::network_test_realm::COMPONENT_URL.to_string(),
                    )),
                    exposes: Some(vec![
                        fntr::ControllerMarker::PROTOCOL_NAME.to_string(),
                        fcomponent::RealmMarker::PROTOCOL_NAME.to_string(),
                    ]),
                    uses: Some(fnetemul::ChildUses::Capabilities(
                        [
                            fnetemul::Capability::LogSink(fnetemul::Empty {}),
                            fnetemul::Capability::ChildDep(fnetemul::ChildDep {
                                name: Some(fnetemul::NETEMUL_SERVICES_COMPONENT_NAME.to_string()),
                                capability: Some(fnetemul::ExposedCapability::Service(
                                    fhwnet::ServiceMarker::SERVICE_NAME.to_string(),
                                )),
                                ..Default::default()
                            }),
                        ]
                        .into_iter()
                        .chain(
                            require_outer_netstack
                                .then_some([
                                    fnetemul::Capability::ChildDep(protocol_dep::<
                                        fnet_stack::StackMarker,
                                    >(
                                        constants::netstack::COMPONENT_NAME,
                                    )),
                                    fnetemul::Capability::ChildDep(protocol_dep::<
                                        fnet_debug::InterfacesMarker,
                                    >(
                                        constants::netstack::COMPONENT_NAME,
                                    )),
                                    fnetemul::Capability::ChildDep(protocol_dep::<
                                        fnet_root::InterfacesMarker,
                                    >(
                                        constants::netstack::COMPONENT_NAME,
                                    )),
                                    fnetemul::Capability::ChildDep(protocol_dep::<
                                        fnet_interfaces::StateMarker,
                                    >(
                                        constants::netstack::COMPONENT_NAME,
                                    )),
                                ])
                                .into_iter()
                                .flatten(),
                        )
                        .collect::<Vec<_>>(),
                    )),
                    ..Default::default()
                }
            }
            KnownServiceProvider::FakeClock => fnetemul::ChildDef {
                name: Some(constants::fake_clock::COMPONENT_NAME.to_string()),
                source: Some(fnetemul::ChildSource::Component(
                    constants::fake_clock::COMPONENT_URL.to_string(),
                )),
                exposes: Some(vec![
                    fidl_fuchsia_testing::FakeClockMarker::PROTOCOL_NAME.to_string(),
                    fidl_fuchsia_testing::FakeClockControlMarker::PROTOCOL_NAME.to_string(),
                ]),
                uses: Some(fnetemul::ChildUses::Capabilities(vec![fnetemul::Capability::LogSink(
                    fnetemul::Empty {},
                )])),
                ..Default::default()
            },
            KnownServiceProvider::FakeSocketProxy => fnetemul::ChildDef {
                name: Some(constants::fake_socket_proxy::COMPONENT_NAME.to_string()),
                source: Some(fnetemul::ChildSource::Component(
                    constants::fake_socket_proxy::COMPONENT_URL.to_string(),
                )),
                exposes: Some(vec![
                    fnp_socketproxy::NetworkRegistryMarker::PROTOCOL_NAME.to_string(),
                ]),
                uses: Some(fnetemul::ChildUses::Capabilities(vec![
                    fnetemul::Capability::ChildDep(fnetemul::ChildDep {
                        is_weak: Some(true),
                        ..protocol_dep::<fnp_socketproxy::NetworkRegistryMarker>(
                            constants::netcfg::COMPONENT_NAME,
                        )
                    }),
                ])),
                ..Default::default()
            },
            KnownServiceProvider::FakeNetcfg => fnetemul::ChildDef {
                name: Some(constants::netcfg::COMPONENT_NAME.to_string()),
                source: Some(fnetemul::ChildSource::Component(
                    constants::netcfg::fake::COMPONENT_URL.to_string(),
                )),
                exposes: Some(vec![
                    fnp_properties::NetworksMarker::PROTOCOL_NAME.to_string(),
                    fnp_socketproxy::NetworkRegistryMarker::PROTOCOL_NAME.to_string(),
                ]),
                ..Default::default()
            },
        }
    }
}

/// Set the `opaque_iids` structured configuration value for Netstack3.
pub fn set_netstack3_opaque_iids(netstack: &mut fnetemul::ChildDef, value: bool) {
    const KEY: &str = "opaque_iids";
    set_structured_config_value(netstack, KEY.to_owned(), cm_rust::ConfigValue::from(value));
}

/// Set the `suspend_enabled` structured configuration value for Netstack3.
pub fn set_netstack3_suspend_enabled(netstack: &mut fnetemul::ChildDef, value: bool) {
    const KEY: &str = "suspend_enabled";
    set_structured_config_value(netstack, KEY.to_owned(), cm_rust::ConfigValue::from(value));
}

/// Set a structured configuration value for the provided component.
fn set_structured_config_value(
    component: &mut fnetemul::ChildDef,
    key: String,
    value: cm_rust::ConfigValue,
) {
    component
        .config_values
        .get_or_insert_default()
        .push(fnetemul::ChildConfigValue { key, value: value.native_into_fidl() });
}

/// Abstraction for a Fuchsia component which offers network stack services.
pub trait Netstack: Copy + Clone {
    /// The Netstack version.
    const VERSION: NetstackVersion;
}

/// Uninstantiable type that represents Netstack2's implementation of a
/// network stack.
#[derive(Copy, Clone)]
pub enum Netstack2 {}

impl Netstack for Netstack2 {
    const VERSION: NetstackVersion = NetstackVersion::Netstack2 { tracing: false, fast_udp: false };
}

/// Uninstantiable type that represents Netstack2's production implementation of
/// a network stack.
#[derive(Copy, Clone)]
pub enum ProdNetstack2 {}

impl Netstack for ProdNetstack2 {
    const VERSION: NetstackVersion = NetstackVersion::ProdNetstack2;
}

/// Uninstantiable type that represents Netstack3's implementation of a
/// network stack.
#[derive(Copy, Clone)]
pub enum Netstack3 {}

impl Netstack for Netstack3 {
    const VERSION: NetstackVersion = NetstackVersion::Netstack3;
}

/// Uninstantiable type that represents Netstack3's production implementation of
/// a network stack.
#[derive(Copy, Clone)]
pub enum ProdNetstack3 {}

impl Netstack for ProdNetstack3 {
    const VERSION: NetstackVersion = NetstackVersion::ProdNetstack3;
}

/// Abstraction for a Fuchsia component which offers network configuration services.
pub trait Manager: Copy + Clone {
    /// The management agent to be used.
    const MANAGEMENT_AGENT: ManagementAgent;
}

/// Uninstantiable type that represents netcfg_basic's implementation of a network manager.
#[derive(Copy, Clone)]
pub enum NetCfgBasic {}

impl Manager for NetCfgBasic {
    const MANAGEMENT_AGENT: ManagementAgent = ManagementAgent::NetCfg(NetCfgVersion::Basic);
}

/// Uninstantiable type that represents netcfg_advanced's implementation of a
/// network manager.
#[derive(Copy, Clone)]
pub enum NetCfgAdvanced {}

impl Manager for NetCfgAdvanced {
    const MANAGEMENT_AGENT: ManagementAgent = ManagementAgent::NetCfg(NetCfgVersion::Advanced);
}

pub use netemul::{DhcpClient, DhcpClientVersion, InStack, OutOfStack};

/// A combination of Netstack and DhcpClient guaranteed to be compatible with
/// each other.
pub trait NetstackAndDhcpClient: Copy + Clone {
    /// The netstack to be used.
    type Netstack: Netstack;
    /// The DHCP client to be used.
    type DhcpClient: DhcpClient;
}

/// Netstack2 with the in-stack DHCP client.
#[derive(Copy, Clone)]
pub enum Netstack2AndInStackDhcpClient {}

impl NetstackAndDhcpClient for Netstack2AndInStackDhcpClient {
    type Netstack = Netstack2;
    type DhcpClient = InStack;
}

/// Netstack2 with the out-of-stack DHCP client.
#[derive(Copy, Clone)]
pub enum Netstack2AndOutOfStackDhcpClient {}

impl NetstackAndDhcpClient for Netstack2AndOutOfStackDhcpClient {
    type Netstack = Netstack2;
    type DhcpClient = OutOfStack;
}

/// Netstack3 with the out-of-stack DHCP client.
#[derive(Copy, Clone)]
pub enum Netstack3AndOutOfStackDhcpClient {}

impl NetstackAndDhcpClient for Netstack3AndOutOfStackDhcpClient {
    type Netstack = Netstack3;
    type DhcpClient = OutOfStack;
}

/// Helpers for `netemul::TestSandbox`.
pub trait TestSandboxExt {
    /// Creates a realm with Netstack services.
    fn create_netstack_realm<'a, N, S>(&'a self, name: S) -> Result<netemul::TestRealm<'a>>
    where
        N: Netstack,
        S: Into<Cow<'a, str>>;

    /// Creates a realm with the base Netstack services plus additional ones in
    /// `children`.
    fn create_netstack_realm_with<'a, N, S, I>(
        &'a self,
        name: S,
        children: I,
    ) -> Result<netemul::TestRealm<'a>>
    where
        S: Into<Cow<'a, str>>,
        N: Netstack,
        I: IntoIterator,
        I::Item: Into<fnetemul::ChildDef>;
}

impl TestSandboxExt for netemul::TestSandbox {
    fn create_netstack_realm<'a, N, S>(&'a self, name: S) -> Result<netemul::TestRealm<'a>>
    where
        N: Netstack,
        S: Into<Cow<'a, str>>,
    {
        self.create_netstack_realm_with::<N, _, _>(name, std::iter::empty::<fnetemul::ChildDef>())
    }

    fn create_netstack_realm_with<'a, N, S, I>(
        &'a self,
        name: S,
        children: I,
    ) -> Result<netemul::TestRealm<'a>>
    where
        S: Into<Cow<'a, str>>,
        N: Netstack,
        I: IntoIterator,
        I::Item: Into<fnetemul::ChildDef>,
    {
        self.create_realm(
            name,
            [KnownServiceProvider::Netstack(N::VERSION)]
                .iter()
                .map(fnetemul::ChildDef::from)
                .chain(children.into_iter().map(Into::into)),
        )
    }
}

/// Helpers for `netemul::TestRealm`.
pub trait TestRealmExt {
    /// Returns the properties of the loopback interface, or `None` if there is no
    /// loopback interface.
    fn loopback_properties(
        &self,
    ) -> impl Future<
        Output = Result<Option<fnet_interfaces_ext::Properties<fnet_interfaces_ext::AllInterest>>>,
    >;

    /// Get a `fuchsia.net.interfaces.admin/Control` client proxy for the
    /// interface identified by [`id`] via `fuchsia.net.root`.
    ///
    /// Note that one should prefer to operate on a `TestInterface` if it is
    /// available; but this method exists in order to obtain a Control channel
    /// for interfaces such as loopback.
    fn interface_control(&self, id: u64) -> Result<fnet_interfaces_ext::admin::Control>;
}

impl TestRealmExt for netemul::TestRealm<'_> {
    async fn loopback_properties(
        &self,
    ) -> Result<Option<fnet_interfaces_ext::Properties<fnet_interfaces_ext::AllInterest>>> {
        let interface_state = self
            .connect_to_protocol::<fnet_interfaces::StateMarker>()
            .context("failed to connect to fuchsia.net.interfaces/State")?;

        let properties = fnet_interfaces_ext::existing(
            fnet_interfaces_ext::event_stream_from_state(&interface_state, Default::default())
                .expect("create watcher event stream"),
            HashMap::<u64, fnet_interfaces_ext::PropertiesAndState<(), _>>::new(),
        )
        .await
        .context("failed to get existing interface properties from watcher")?
        .into_iter()
        .find_map(|(_id, properties_and_state): (u64, _)| {
            let fnet_interfaces_ext::PropertiesAndState {
                properties: properties @ fnet_interfaces_ext::Properties { port_class, .. },
                state: (),
            } = properties_and_state;
            port_class.is_loopback().then_some(properties)
        });
        Ok(properties)
    }

    fn interface_control(&self, id: u64) -> Result<fnet_interfaces_ext::admin::Control> {
        let root_control = self
            .connect_to_protocol::<fnet_root::InterfacesMarker>()
            .context("connect to protocol")?;

        let (control, server) = fnet_interfaces_ext::admin::Control::create_endpoints()
            .context("create Control proxy")?;
        root_control.get_admin(id, server).context("get admin")?;
        Ok(control)
    }
}
