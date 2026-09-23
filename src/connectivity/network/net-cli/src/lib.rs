// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod capture;
mod filter;
mod opts;
mod ser;

use anyhow::{Context as _, Error, anyhow};
use fidl_fuchsia_net_ext as fnet_ext;
#[cfg(not(feature = "fdomain"))]
use fidl_fuchsia_net_interfaces_ext as finterfaces_ext;
#[cfg(feature = "fdomain")]
use fidl_fuchsia_net_interfaces_ext_fdomain as finterfaces_ext;
#[cfg(not(feature = "fdomain"))]
use fidl_fuchsia_net_matchers_ext as fnet_matchers_ext;
#[cfg(feature = "fdomain")]
use fidl_fuchsia_net_matchers_ext_fdomain as fnet_matchers_ext;
#[cfg(not(feature = "fdomain"))]
use fidl_fuchsia_net_neighbor_ext as fneighbor_ext;
#[cfg(feature = "fdomain")]
use fidl_fuchsia_net_neighbor_ext_fdomain as fneighbor_ext;
#[cfg(not(feature = "fdomain"))]
use fidl_fuchsia_net_routes_ext as froutes_ext;
#[cfg(feature = "fdomain")]
use fidl_fuchsia_net_routes_ext_fdomain as froutes_ext;
use fidl_fuchsia_net_stack_ext::{self as fstack_ext, FidlReturn as _};
use flex_client::ProxyHasDomain;
use flex_fuchsia_net as fnet;
use flex_fuchsia_net_debug as fdebug;
use flex_fuchsia_net_dhcp as fdhcp;
use flex_fuchsia_net_filter as fnet_filter;
use flex_fuchsia_net_filter_deprecated as ffilter_deprecated;
use flex_fuchsia_net_interfaces as finterfaces;
use flex_fuchsia_net_interfaces_admin as finterfaces_admin;
use flex_fuchsia_net_name as fname;
use flex_fuchsia_net_neighbor as fneighbor;
use flex_fuchsia_net_resources as fnet_resources;
use flex_fuchsia_net_root as froot;
use flex_fuchsia_net_routes as froutes;
use flex_fuchsia_net_stack as fstack;
use flex_fuchsia_net_stackmigrationdeprecated as fnet_migration;
use futures::{FutureExt as _, StreamExt as _, TryFutureExt as _, TryStreamExt as _};
use itertools::Itertools as _;
use log::{info, warn};
use net_types::SpecifiedAddr;
use net_types::ip::{Ip, IpAddress as _, Ipv4, Ipv4Addr, Ipv6, Ipv6Addr, Subnet};
use netfilter::FidlReturn as _;
use prettytable::{Row, Table, format, row};
use ser::AddressAssignmentState;
use serde_json::json;
use serde_json::value::Value;
use std::borrow::Cow;
use std::collections::hash_map::HashMap;
use std::convert::TryFrom as _;
use std::iter::FromIterator as _;
use std::num::NonZeroU64;
use std::ops::Deref;
use std::pin::pin;
use std::str::FromStr as _;
use writer::ToolIO as _;
use zx_status as zx;

pub use capture::DEFAULT_CAPTURE_NAME;
pub use opts::{
    Command, CommandEnum, UserFacingError, underlying_user_facing_error, user_facing_error,
};

macro_rules! filter_fidl {
    ($method:expr, $context:expr) => {
        $method.await.transform_result().context($context)
    };
}

fn add_row(t: &mut Table, row: Row) {
    let _: &mut Row = t.add_row(row);
}

/// An interface for acquiring a proxy to a FIDL service.
#[async_trait::async_trait]
pub trait ServiceConnector<S: flex_client::fidl::ProtocolMarker> {
    /// Acquires a proxy to the parameterized FIDL interface.
    async fn connect(&self) -> Result<S::Proxy, Error>;
}

pub trait CaptureDeps {
    type OutputWriter: std::io::Write + Send + 'static;
    fn create_output_writer(
        &self,
        path: &std::path::Path,
    ) -> Result<Self::OutputWriter, anyhow::Error>;
}

/// An interface for acquiring all system dependencies required by net-cli.
///
/// FIDL dependencies are specified as supertraits. These supertraits are a complete enumeration of
/// all FIDL dependencies required by net-cli.
pub trait NetCliDepsConnector:
    ServiceConnector<fdebug::InterfacesMarker>
    + ServiceConnector<froot::InterfacesMarker>
    + ServiceConnector<froot::RoutesV4Marker>
    + ServiceConnector<froot::RoutesV6Marker>
    + ServiceConnector<froot::FilterMarker>
    + ServiceConnector<fdhcp::Server_Marker>
    + ServiceConnector<ffilter_deprecated::FilterMarker>
    + ServiceConnector<finterfaces::StateMarker>
    + ServiceConnector<finterfaces_admin::InstallerMarker>
    + ServiceConnector<fneighbor::ControllerMarker>
    + ServiceConnector<fneighbor::ViewMarker>
    + ServiceConnector<fstack::LogMarker>
    + ServiceConnector<fstack::StackMarker>
    + ServiceConnector<froutes::StateV4Marker>
    + ServiceConnector<froutes::StateV6Marker>
    + ServiceConnector<fname::LookupMarker>
    + ServiceConnector<fnet_migration::ControlMarker>
    + ServiceConnector<fnet_migration::StateMarker>
    + ServiceConnector<fnet_filter::StateMarker>
    + ServiceConnector<fdebug::PacketCaptureProviderMarker>
{
}

impl<O> NetCliDepsConnector for O where
    O: ServiceConnector<fdebug::InterfacesMarker>
        + ServiceConnector<froot::InterfacesMarker>
        + ServiceConnector<froot::RoutesV4Marker>
        + ServiceConnector<froot::RoutesV6Marker>
        + ServiceConnector<froot::FilterMarker>
        + ServiceConnector<fdhcp::Server_Marker>
        + ServiceConnector<ffilter_deprecated::FilterMarker>
        + ServiceConnector<finterfaces::StateMarker>
        + ServiceConnector<finterfaces_admin::InstallerMarker>
        + ServiceConnector<fneighbor::ControllerMarker>
        + ServiceConnector<fneighbor::ViewMarker>
        + ServiceConnector<fstack::LogMarker>
        + ServiceConnector<fstack::StackMarker>
        + ServiceConnector<froutes::StateV4Marker>
        + ServiceConnector<froutes::StateV6Marker>
        + ServiceConnector<fname::LookupMarker>
        + ServiceConnector<fnet_migration::ControlMarker>
        + ServiceConnector<fnet_migration::StateMarker>
        + ServiceConnector<fnet_filter::StateMarker>
        + ServiceConnector<fdebug::PacketCaptureProviderMarker>
{
}

pub async fn do_root<C, D>(
    mut out: writer::JsonWriter<serde_json::Value>,
    Command { cmd }: Command,
    connector: &C,
    deps: &D,
) -> Result<(), Error>
where
    C: NetCliDepsConnector,
    D: CaptureDeps,
{
    match cmd {
        CommandEnum::Capture(cmd) => capture::do_capture(out, cmd, connector, deps)
            .await
            .context("failed during capture command"),
        CommandEnum::If(opts::If { if_cmd: cmd }) => {
            do_if(&mut out, cmd, connector).await.context("failed during if command")
        }
        CommandEnum::Route(opts::Route { route_cmd: cmd }) => {
            do_route(&mut out, cmd, connector).await.context("failed during route command")
        }
        CommandEnum::Rule(opts::Rule { rule_cmd: cmd }) => {
            do_rule(&mut out, cmd, connector).await.context("failed during rule command")
        }
        CommandEnum::FilterDeprecated(opts::FilterDeprecated { filter_cmd: cmd }) => {
            do_filter_deprecated(out, cmd, connector)
                .await
                .context("failed during filter-deprecated command")
        }
        CommandEnum::Filter(opts::filter::Filter { filter_cmd: cmd }) => {
            filter::do_filter(out, cmd, connector).await.context("failed during filter command")
        }
        CommandEnum::Log(opts::Log { log_cmd: cmd }) => {
            do_log(cmd, connector).await.context("failed during log command")
        }
        CommandEnum::Dhcp(opts::Dhcp { dhcp_cmd: cmd }) => {
            do_dhcp(cmd, connector).await.context("failed during dhcp command")
        }
        CommandEnum::Dhcpd(opts::dhcpd::Dhcpd { dhcpd_cmd: cmd }) => {
            do_dhcpd(cmd, connector).await.context("failed during dhcpd command")
        }
        CommandEnum::Neigh(opts::Neigh { neigh_cmd: cmd }) => {
            do_neigh(out, cmd, connector).await.context("failed during neigh command")
        }
        CommandEnum::Dns(opts::dns::Dns { dns_cmd: cmd }) => {
            do_dns(out, cmd, connector).await.context("failed during dns command")
        }
        CommandEnum::NetstackMigration(opts::NetstackMigration { cmd }) => {
            do_netstack_migration(out, cmd, connector)
                .await
                .context("failed during migration command")
        }
    }
}

fn shortlist_interfaces(
    name_pattern: &str,
    interfaces: &mut HashMap<
        u64,
        finterfaces_ext::PropertiesAndState<(), finterfaces_ext::AllInterest>,
    >,
) {
    interfaces.retain(|_: &u64, properties_and_state| {
        properties_and_state.properties.name.contains(name_pattern)
    })
}

fn write_tabulated_interfaces_info<
    W: std::io::Write,
    I: IntoIterator<Item = ser::InterfaceView>,
>(
    mut out: W,
    interfaces: I,
) -> Result<(), Error> {
    let mut t = Table::new();
    t.set_format(format::FormatBuilder::new().padding(2, 2).build());
    for (
        i,
        ser::InterfaceView {
            nicid,
            name,
            device_class,
            online,
            addresses,
            mac,
            has_default_ipv4_route,
            has_default_ipv6_route,
            port_identity_koid,
        },
    ) in interfaces.into_iter().enumerate()
    {
        if i > 0 {
            add_row(&mut t, row![]);
        }

        add_row(&mut t, row!["nicid", nicid]);
        add_row(&mut t, row!["name", name]);
        add_row(
            &mut t,
            row![
                "device class",
                match device_class {
                    ser::DeviceClass::Loopback => "loopback",
                    ser::DeviceClass::Blackhole => "blackhole",
                    ser::DeviceClass::Virtual => "virtual",
                    ser::DeviceClass::Ethernet => "ethernet",
                    ser::DeviceClass::WlanClient => "wlan-client",
                    ser::DeviceClass::Ppp => "ppp",
                    ser::DeviceClass::Bridge => "bridge",
                    ser::DeviceClass::WlanAp => "wlan-ap",
                    ser::DeviceClass::Lowpan => "lowpan",
                }
            ],
        );
        add_row(&mut t, row!["online", online]);

        let default_routes: std::borrow::Cow<'_, _> =
            if has_default_ipv4_route || has_default_ipv6_route {
                itertools::Itertools::intersperse(
                    has_default_ipv4_route
                        .then_some("IPv4")
                        .into_iter()
                        .chain(has_default_ipv6_route.then_some("IPv6")),
                    ",",
                )
                .collect::<String>()
                .into()
            } else {
                "-".into()
            };
        add_row(&mut t, row!["default routes", default_routes]);

        for ser::Address {
            subnet: ser::Subnet { addr, prefix_len },
            valid_until,
            assignment_state,
        } in addresses.all_addresses()
        {
            let valid_until = valid_until.map(|v| {
                let v = std::time::Duration::from_nanos(v.try_into().unwrap_or(0)).as_secs_f32();
                std::borrow::Cow::Owned(format!("valid until [{v}s]"))
            });
            let assignment_state: Option<std::borrow::Cow<'_, _>> = match assignment_state {
                AddressAssignmentState::Assigned => None,
                AddressAssignmentState::Tentative => Some("TENTATIVE".into()),
                AddressAssignmentState::Unavailable => Some("UNAVAILABLE".into()),
            };
            let extra_bits = itertools::Itertools::intersperse(
                assignment_state.into_iter().chain(valid_until),
                " ".into(),
            )
            .collect::<String>();

            add_row(&mut t, row!["addr", format!("{addr}/{prefix_len}"), extra_bits]);
        }
        match mac {
            None => add_row(&mut t, row!["mac", "-"]),
            Some(mac) => add_row(&mut t, row!["mac", mac]),
        }
        match port_identity_koid {
            None => add_row(&mut t, row!["port_identity_koid", "-"]),
            Some(port_identity_koid) => {
                add_row(&mut t, row!["port_identity_koid", port_identity_koid])
            }
        }
    }
    writeln!(out, "{}", t)?;
    Ok(())
}

pub(crate) async fn connect_with_context<S, C>(connector: &C) -> Result<S::Proxy, Error>
where
    C: ServiceConnector<S>,
    S: flex_client::fidl::ProtocolMarker,
{
    connector.connect().await.with_context(|| format!("failed to connect to {}", S::DEBUG_NAME))
}

async fn get_control<C>(connector: &C, id: u64) -> Result<finterfaces_ext::admin::Control, Error>
where
    C: ServiceConnector<froot::InterfacesMarker>,
{
    let root_interfaces = connect_with_context::<froot::InterfacesMarker, _>(connector).await?;
    let (proxy, server_end) =
        root_interfaces.domain().create_proxy::<finterfaces_admin::ControlMarker>();
    let control = finterfaces_ext::admin::Control::new(proxy);
    root_interfaces.get_admin(id, server_end).context("send get admin request")?;
    Ok(control)
}

fn configuration_with_ip_forwarding_set(
    ip_version: fnet::IpVersion,
    forwarding: bool,
) -> finterfaces_admin::Configuration {
    match ip_version {
        fnet::IpVersion::V4 => finterfaces_admin::Configuration {
            ipv4: Some(finterfaces_admin::Ipv4Configuration {
                unicast_forwarding: Some(forwarding),
                ..Default::default()
            }),
            ..Default::default()
        },
        fnet::IpVersion::V6 => finterfaces_admin::Configuration {
            ipv6: Some(finterfaces_admin::Ipv6Configuration {
                unicast_forwarding: Some(forwarding),
                ..Default::default()
            }),
            ..Default::default()
        },
    }
}

fn extract_ip_forwarding(
    finterfaces_admin::Configuration {
        ipv4: ipv4_config, ipv6: ipv6_config, ..
    }: finterfaces_admin::Configuration,
    ip_version: fnet::IpVersion,
) -> Result<bool, Error> {
    match ip_version {
        fnet::IpVersion::V4 => {
            let finterfaces_admin::Ipv4Configuration { unicast_forwarding, .. } =
                ipv4_config.context("get IPv4 configuration")?;
            unicast_forwarding.context("get IPv4 forwarding configuration")
        }
        fnet::IpVersion::V6 => {
            let finterfaces_admin::Ipv6Configuration { unicast_forwarding, .. } =
                ipv6_config.context("get IPv6 configuration")?;
            unicast_forwarding.context("get IPv6 forwarding configuration")
        }
    }
}

fn extract_igmp_version(
    finterfaces_admin::Configuration { ipv4: ipv4_config, .. }: finterfaces_admin::Configuration,
) -> Result<Option<finterfaces_admin::IgmpVersion>, Error> {
    let finterfaces_admin::Ipv4Configuration { igmp, .. } =
        ipv4_config.context("get IPv4 configuration")?;
    let finterfaces_admin::IgmpConfiguration { version: igmp_version, .. } =
        igmp.context("get IGMP configuration")?;
    Ok(igmp_version)
}

fn extract_mld_version(
    finterfaces_admin::Configuration { ipv6: ipv6_config, .. }: finterfaces_admin::Configuration,
) -> Result<Option<finterfaces_admin::MldVersion>, Error> {
    let finterfaces_admin::Ipv6Configuration { mld, .. } =
        ipv6_config.context("get IPv6 configuration")?;
    let finterfaces_admin::MldConfiguration { version: mld_version, .. } =
        mld.context("get MLD configuration")?;
    Ok(mld_version)
}

fn extract_nud_config(
    finterfaces_admin::Configuration { ipv4, ipv6, .. }: finterfaces_admin::Configuration,
    ip_version: fnet::IpVersion,
) -> Result<finterfaces_admin::NudConfiguration, Error> {
    match ip_version {
        fnet::IpVersion::V4 => {
            let finterfaces_admin::Ipv4Configuration { arp, .. } =
                ipv4.context("get IPv4 configuration")?;
            let finterfaces_admin::ArpConfiguration { nud, .. } =
                arp.context("get ARP configuration")?;
            nud.context("get NUD configuration")
        }
        fnet::IpVersion::V6 => {
            let finterfaces_admin::Ipv6Configuration { ndp, .. } =
                ipv6.context("get IPv6 configuration")?;
            let finterfaces_admin::NdpConfiguration { nud, .. } =
                ndp.context("get NDP configuration")?;
            nud.context("get NUD configuration")
        }
    }
}

async fn do_if<C: NetCliDepsConnector>(
    out: &mut writer::JsonWriter<serde_json::Value>,
    cmd: opts::IfEnum,
    connector: &C,
) -> Result<(), Error> {
    match cmd {
        opts::IfEnum::List(opts::IfList { name_pattern }) => {
            let root_interfaces =
                connect_with_context::<froot::InterfacesMarker, _>(connector).await?;
            let interface_state =
                connect_with_context::<finterfaces::StateMarker, _>(connector).await?;
            let stream = finterfaces_ext::event_stream_from_state::<finterfaces_ext::AllInterest>(
                &interface_state,
                Default::default(),
            )?;
            let mut response = finterfaces_ext::existing(
                stream,
                HashMap::<u64, finterfaces_ext::PropertiesAndState<(), _>>::new(),
            )
            .await?;
            if let Some(name_pattern) = name_pattern {
                shortlist_interfaces(&name_pattern, &mut response);
            }
            let response = response.into_values().map(
                |finterfaces_ext::PropertiesAndState { properties, state: () }| async {
                    let mac = root_interfaces
                        .get_mac(properties.id.get())
                        .await
                        .context("call get_mac")?;
                    Ok::<_, Error>((properties, mac))
                },
            );
            let response = futures::future::try_join_all(response).await?;
            let mut response: Vec<_> = response
                .into_iter()
                .filter_map(|(properties, mac)| match mac {
                    Err(froot::InterfacesGetMacError::NotFound) => None,
                    Ok(mac) => {
                        let mac = mac.map(|box_| *box_);
                        Some((properties, mac).into())
                    }
                })
                .collect();
            response.sort_by_key(|ser::InterfaceView { nicid, .. }| *nicid);
            if out.is_machine() {
                out.machine(&serde_json::to_value(&response)?)?;
            } else {
                write_tabulated_interfaces_info(out, response.into_iter())
                    .context("error tabulating interface info")?;
            }
        }
        opts::IfEnum::Get(opts::IfGet { interface }) => {
            let id = interface.find_nicid(connector).await?.get();
            let root_interfaces =
                connect_with_context::<froot::InterfacesMarker, _>(connector).await?;
            let interface_state =
                connect_with_context::<finterfaces::StateMarker, _>(connector).await?;
            let stream = finterfaces_ext::event_stream_from_state::<finterfaces_ext::AllInterest>(
                &interface_state,
                Default::default(),
            )?;
            let response = finterfaces_ext::existing(
                stream,
                finterfaces_ext::InterfaceState::<(), _>::Unknown(id),
            )
            .await?;
            match response {
                finterfaces_ext::InterfaceState::Unknown(id) => {
                    return Err(user_facing_error(format!("interface with id={} not found", id)));
                }
                finterfaces_ext::InterfaceState::Known(finterfaces_ext::PropertiesAndState {
                    properties,
                    state: _,
                }) => {
                    let finterfaces_ext::Properties { id, .. } = &properties;
                    let mac = root_interfaces.get_mac(id.get()).await.context("call get_mac")?;
                    match mac {
                        Err(froot::InterfacesGetMacError::NotFound) => {
                            return Err(user_facing_error(format!(
                                "interface with id={} not found",
                                id
                            )));
                        }
                        Ok(mac) => {
                            let mac = mac.map(|box_| *box_);
                            let view = (properties, mac).into();
                            if out.is_machine() {
                                out.machine(&serde_json::to_value(&view)?)?;
                            } else {
                                write_tabulated_interfaces_info(out, std::iter::once(view))
                                    .context("error tabulating interface info")?;
                            }
                        }
                    };
                }
            }
        }
        opts::IfEnum::Igmp(opts::IfIgmp { cmd }) => match cmd {
            opts::IfIgmpEnum::Get(opts::IfIgmpGet { interface }) => {
                let id = interface.find_nicid(connector).await.context("find nicid")?.get();
                let control = get_control(connector, id).await.context("get control")?;
                let configuration = control
                    .get_configuration()
                    .await
                    .map_err(anyhow::Error::new)
                    .and_then(|res| {
                        res.map_err(|e: finterfaces_admin::ControlGetConfigurationError| {
                            anyhow!("{:?}", e)
                        })
                    })
                    .context("get configuration")?;

                out.line(format!("IGMP configuration on interface {}:", id))?;
                out.line(format!(
                    "    Version: {:?}",
                    extract_igmp_version(configuration).context("get IGMP version")?
                ))?;
            }
            opts::IfIgmpEnum::Set(opts::IfIgmpSet { interface, version }) => {
                let id = interface.find_nicid(connector).await.context("find nicid")?.get();
                let control = get_control(connector, id).await.context("get control")?;
                let prev_config = control
                    .set_configuration(&finterfaces_admin::Configuration {
                        ipv4: Some(finterfaces_admin::Ipv4Configuration {
                            igmp: Some(finterfaces_admin::IgmpConfiguration {
                                version,
                                ..Default::default()
                            }),
                            ..Default::default()
                        }),
                        ..Default::default()
                    })
                    .await
                    .map_err(anyhow::Error::new)
                    .and_then(|res| {
                        res.map_err(|e: finterfaces_admin::ControlSetConfigurationError| {
                            anyhow!("{:?}", e)
                        })
                    })
                    .context("set configuration")?;

                info!(
                    "IGMP version set to {:?} on interface {}; previously set to {:?}",
                    version,
                    id,
                    extract_igmp_version(prev_config).context("set IGMP version")?,
                );
            }
        },
        opts::IfEnum::Mld(opts::IfMld { cmd }) => match cmd {
            opts::IfMldEnum::Get(opts::IfMldGet { interface }) => {
                let id = interface.find_nicid(connector).await.context("find nicid")?.get();
                let control = get_control(connector, id).await.context("get control")?;
                let configuration = control
                    .get_configuration()
                    .await
                    .map_err(anyhow::Error::new)
                    .and_then(|res| {
                        res.map_err(|e: finterfaces_admin::ControlGetConfigurationError| {
                            anyhow!("{:?}", e)
                        })
                    })
                    .context("get configuration")?;

                out.line(format!("MLD configuration on interface {}:", id))?;
                out.line(format!(
                    "    Version: {:?}",
                    extract_mld_version(configuration).context("get MLD version")?
                ))?;
            }
            opts::IfMldEnum::Set(opts::IfMldSet { interface, version }) => {
                let id = interface.find_nicid(connector).await.context("find nicid")?.get();
                let control = get_control(connector, id).await.context("get control")?;
                let prev_config = control
                    .set_configuration(&finterfaces_admin::Configuration {
                        ipv6: Some(finterfaces_admin::Ipv6Configuration {
                            mld: Some(finterfaces_admin::MldConfiguration {
                                version,
                                ..Default::default()
                            }),
                            ..Default::default()
                        }),
                        ..Default::default()
                    })
                    .await
                    .map_err(anyhow::Error::new)
                    .and_then(|res| {
                        res.map_err(|e: finterfaces_admin::ControlSetConfigurationError| {
                            anyhow!("{:?}", e)
                        })
                    })
                    .context("set configuration")?;

                info!(
                    "MLD version set to {:?} on interface {}; previously set to {:?}",
                    version,
                    id,
                    extract_mld_version(prev_config).context("set MLD version")?,
                );
            }
        },
        opts::IfEnum::IpForward(opts::IfIpForward { cmd }) => match cmd {
            opts::IfIpForwardEnum::Get(opts::IfIpForwardGet { interface, ip_version }) => {
                let id = interface.find_nicid(connector).await.context("find nicid")?.get();
                let control = get_control(connector, id).await.context("get control")?;
                let configuration = control
                    .get_configuration()
                    .await
                    .map_err(anyhow::Error::new)
                    .and_then(|res| {
                        res.map_err(|e: finterfaces_admin::ControlGetConfigurationError| {
                            anyhow!("{:?}", e)
                        })
                    })
                    .context("get configuration")?;

                out.line(format!(
                    "IP forwarding for {:?} is {} on interface {}",
                    ip_version,
                    extract_ip_forwarding(configuration, ip_version)
                        .context("extract IP forwarding configuration")?,
                    id
                ))?;
            }
            opts::IfIpForwardEnum::Set(opts::IfIpForwardSet { interface, ip_version, enable }) => {
                let id = interface.find_nicid(connector).await.context("find nicid")?.get();
                let control = get_control(connector, id).await.context("get control")?;
                let prev_config = control
                    .set_configuration(&configuration_with_ip_forwarding_set(ip_version, enable))
                    .await
                    .map_err(anyhow::Error::new)
                    .and_then(|res| {
                        res.map_err(|e: finterfaces_admin::ControlSetConfigurationError| {
                            anyhow!("{:?}", e)
                        })
                    })
                    .context("set configuration")?;
                info!(
                    "IP forwarding for {:?} set to {} on interface {}; previously set to {}",
                    ip_version,
                    enable,
                    id,
                    extract_ip_forwarding(prev_config, ip_version)
                        .context("set IP forwarding configuration")?
                );
            }
        },
        opts::IfEnum::Enable(opts::IfEnable { interface }) => {
            let id = interface.find_nicid(connector).await?.get();
            let control = get_control(connector, id).await?;
            let did_enable = control
                .enable()
                .await
                .map_err(anyhow::Error::new)
                .and_then(|res| {
                    res.map_err(|e: finterfaces_admin::ControlEnableError| anyhow!("{:?}", e))
                })
                .context("error enabling interface")?;
            if did_enable {
                info!("Interface {} enabled", id);
            } else {
                info!("Interface {} already enabled", id);
            }
        }
        opts::IfEnum::Disable(opts::IfDisable { interface }) => {
            let id = interface.find_nicid(connector).await?.get();
            let control = get_control(connector, id).await?;
            let did_disable = control
                .disable()
                .await
                .map_err(anyhow::Error::new)
                .and_then(|res| {
                    res.map_err(|e: finterfaces_admin::ControlDisableError| anyhow!("{:?}", e))
                })
                .context("error disabling interface")?;
            if did_disable {
                info!("Interface {} disabled", id);
            } else {
                info!("Interface {} already disabled", id);
            }
        }
        opts::IfEnum::Addr(opts::IfAddr { addr_cmd }) => match addr_cmd {
            opts::IfAddrEnum::Add(opts::IfAddrAdd { interface, addr, prefix, no_subnet_route }) => {
                let id = interface.find_nicid(connector).await?.get();
                let control = get_control(connector, id).await?;
                let addr = fnet_ext::IpAddress::from_str(&addr)?.into();
                let subnet = fnet_ext::Subnet { addr, prefix_len: prefix };
                let root_interfaces =
                    connect_with_context::<froot::InterfacesMarker, _>(connector).await?;
                let (address_state_provider, server_end) =
                    root_interfaces
                        .domain()
                        .create_proxy::<finterfaces_admin::AddressStateProviderMarker>();
                control
                    .add_address(
                        &subnet.into(),
                        &finterfaces_admin::AddressParameters {
                            add_subnet_route: Some(!no_subnet_route),
                            ..Default::default()
                        },
                        server_end,
                    )
                    .context("call add address")?;

                address_state_provider.detach().context("detach address lifetime")?;
                let state_stream =
                    finterfaces_ext::admin::assignment_state_stream(address_state_provider);

                state_stream
                    .try_filter_map(|state| {
                        futures::future::ok(match state {
                            finterfaces::AddressAssignmentState::Tentative => None,
                            finterfaces::AddressAssignmentState::Assigned => Some(()),
                            finterfaces::AddressAssignmentState::Unavailable => Some(()),
                        })
                    })
                    .try_next()
                    .await
                    .context("error after adding address")?
                    .ok_or_else(|| {
                        anyhow!(
                            "Address assignment state stream unexpectedly ended \
                                 before reaching Assigned or Unavailable state. \
                                 This is probably a bug."
                        )
                    })?;

                info!("Address {}/{} added to interface {}", addr, prefix, id);
            }
            opts::IfAddrEnum::Del(opts::IfAddrDel { interface, addr, prefix }) => {
                let id = interface.find_nicid(connector).await?.get();
                let control = get_control(connector, id).await?;
                let addr = fnet_ext::IpAddress::from_str(&addr)?;
                let did_remove = {
                    let addr = addr.into();
                    let subnet = fnet::Subnet {
                        addr,
                        prefix_len: prefix.unwrap_or_else(|| {
                            8 * u8::try_from(match addr {
                                fnet::IpAddress::Ipv4(fnet::Ipv4Address { addr }) => addr.len(),
                                fnet::IpAddress::Ipv6(fnet::Ipv6Address { addr }) => addr.len(),
                            })
                            .expect("prefix length doesn't fit u8")
                        }),
                    };
                    control
                        .remove_address(&subnet)
                        .await
                        .map_err(anyhow::Error::new)
                        .and_then(|res| {
                            res.map_err(|e: finterfaces_admin::ControlRemoveAddressError| {
                                anyhow!("{:?}", e)
                            })
                        })
                        .context("call remove address")?
                };
                if !did_remove {
                    return Err(user_facing_error(format!(
                        "Address {} not found on interface {}",
                        addr, id
                    )));
                }
                info!("Address {} deleted from interface {}", addr, id);
            }
            opts::IfAddrEnum::Wait(opts::IfAddrWait { interface, ipv6 }) => {
                let id = interface.find_nicid(connector).await?.get();
                let interfaces_state =
                    connect_with_context::<finterfaces::StateMarker, _>(connector).await?;
                let mut state = finterfaces_ext::InterfaceState::<(), _>::Unknown(id);

                let assigned_addr = finterfaces_ext::wait_interface_with_id(
                    finterfaces_ext::event_stream_from_state::<finterfaces_ext::AllInterest>(
                        &interfaces_state,
                        Default::default(),
                    )?,
                    &mut state,
                    |finterfaces_ext::PropertiesAndState { properties, state: _ }| {
                        let finterfaces_ext::Properties { addresses, .. } = properties;
                        let addr = if ipv6 {
                            addresses.iter().find_map(
                                |finterfaces_ext::Address {
                                     addr: fnet::Subnet { addr, .. },
                                     ..
                                 }| {
                                    match addr {
                                        fnet::IpAddress::Ipv4(_) => None,
                                        fnet::IpAddress::Ipv6(_) => Some(addr),
                                    }
                                },
                            )
                        } else {
                            addresses.first().map(
                                |finterfaces_ext::Address {
                                     addr: fnet::Subnet { addr, .. },
                                     ..
                                 }| addr,
                            )
                        };
                        addr.map(|addr| {
                            let fnet_ext::IpAddress(addr) = (*addr).into();
                            addr
                        })
                    },
                )
                .await
                .context("wait for assigned address")?;

                out.line(format!("{assigned_addr}"))?;
                info!("Address {} assigned on interface {}", assigned_addr, id);
            }
        },
        opts::IfEnum::Bridge(opts::IfBridge { interfaces }) => {
            let stack = connect_with_context::<fstack::StackMarker, _>(connector).await?;
            let build_name_to_id_map = || async {
                let interface_state =
                    connect_with_context::<finterfaces::StateMarker, _>(connector).await?;
                let stream = finterfaces_ext::event_stream_from_state::<
                    finterfaces_ext::AllInterest,
                >(&interface_state, Default::default())?;
                let response = finterfaces_ext::existing(stream, HashMap::new()).await?;
                Ok::<HashMap<String, u64>, Error>(
                    response
                        .into_iter()
                        .map(
                            |(
                                id,
                                finterfaces_ext::PropertiesAndState {
                                    properties: finterfaces_ext::Properties { name, .. },
                                    state: (),
                                },
                            )| (name, id),
                        )
                        .collect(),
                )
            };

            let num_interfaces = interfaces.len();

            let (_name_to_id, ids): (Option<HashMap<String, u64>>, Vec<u64>) =
                futures::stream::iter(interfaces)
                    .map(Ok::<_, Error>)
                    .try_fold(
                        (None, Vec::with_capacity(num_interfaces)),
                        |(name_to_id, mut ids), interface| async move {
                            let (name_to_id, id) = match interface {
                                opts::InterfaceIdentifier::Id(id) => (name_to_id, id.get()),
                                opts::InterfaceIdentifier::Name(name) => {
                                    let name_to_id = match name_to_id {
                                        Some(name_to_id) => name_to_id,
                                        None => build_name_to_id_map().await?,
                                    };
                                    let id = name_to_id.get(&name).copied().ok_or_else(|| {
                                        user_facing_error(format!("no interface named {}", name))
                                    })?;
                                    (Some(name_to_id), id)
                                }
                            };
                            ids.push(id);
                            Ok((name_to_id, ids))
                        },
                    )
                    .await?;

            let (bridge, server_end) = stack.domain().create_proxy();
            stack.bridge_interfaces(&ids, server_end).context("bridge interfaces")?;
            let bridge_id = bridge.get_id().await.context("get bridge id")?;
            // Detach the channel so it won't cause bridge destruction on exit.
            bridge.detach().context("detach bridge")?;
            info!("network bridge created with id {}", bridge_id);
        }
        opts::IfEnum::Config(opts::IfConfig { interface, cmd }) => {
            let id = interface.find_nicid(connector).await.context("find nicid")?.get();
            let control = get_control(connector, id).await.context("get control")?;

            match cmd {
                opts::IfConfigEnum::Set(opts::IfConfigSet { options }) => {
                    do_if_config_set(control, options).await?;
                }
                opts::IfConfigEnum::Get(opts::IfConfigGet {}) => {
                    let configuration = control
                        .get_configuration()
                        .await
                        .map_err(anyhow::Error::new)
                        .and_then(|res| {
                            res.map_err(|e: finterfaces_admin::ControlGetConfigurationError| {
                                anyhow::anyhow!("{:?}", e)
                            })
                        })
                        .context("get configuration")?;
                    // TODO(https://fxbug.dev/368806554): Print these with the same names used when
                    // setting each property.
                    out.line(format!("{:#?}", configuration))?;
                }
            }
        }
        opts::IfEnum::Add(opts::IfAdd {
            cmd: opts::IfAddEnum::Blackhole(opts::IfBlackholeAdd { name }),
        }) => {
            let installer =
                ServiceConnector::<finterfaces_admin::InstallerMarker>::connect(connector)
                    .await
                    .expect("connect should succeed");

            let (proxy, server_end) =
                installer.domain().create_proxy::<finterfaces_admin::ControlMarker>();
            let control = finterfaces_ext::admin::Control::new(proxy);
            installer
                .install_blackhole_interface(
                    server_end,
                    finterfaces_admin::Options { name: Some(name), ..Default::default() },
                )
                .expect("install blackhole interface should succeed");
            control.detach().expect("detach should succeed");
        }
        opts::IfEnum::Remove(opts::IfRemove { interface }) => {
            let id = interface.find_nicid(connector).await.context("find nicid")?.get();
            let control = get_control(connector, id).await.context("get control")?;
            control
                .remove()
                .await
                .expect("should not get FIDL error")
                .expect("remove should succeed");
        }
    }
    Ok(())
}

async fn do_if_config_set(
    control: finterfaces_ext::admin::Control,
    options: Vec<String>,
) -> Result<(), Error> {
    if options.len() % 2 != 0 {
        return Err(user_facing_error(format!(
            "if config set expects property value pairs and thus an even number of arguments"
        )));
    }
    let config = options.iter().tuples().try_fold(
        finterfaces_admin::Configuration::default(),
        |mut config, (property, value)| {
            match property.as_str() {
                "ipv6.ndp.slaac.temporary_address_enabled" => {
                    let enabled = value.parse::<bool>().map_err(|e| {
                        user_facing_error(format!("failed to parse {value} as bool: {e}"))
                    })?;
                    config
                        .ipv6
                        .get_or_insert_default()
                        .ndp
                        .get_or_insert_default()
                        .slaac
                        .get_or_insert_default()
                        .temporary_address = Some(enabled);
                }
                "ipv6.ndp.dad.transmits" => {
                    let transmits = value.parse::<u16>().map_err(|e| {
                        user_facing_error(format!("failed to parse {value} as u16: {e}"))
                    })?;
                    config
                        .ipv6
                        .get_or_insert_default()
                        .ndp
                        .get_or_insert_default()
                        .dad
                        .get_or_insert_default()
                        .transmits = Some(transmits);
                }
                "ipv6.ndp.router_solicitations.max" => {
                    let max_router_solicitations = value.parse::<u8>().map_err(|e| {
                        user_facing_error(format!("failed to parse {value} as u8: {e}"))
                    })?;
                    config
                        .ipv6
                        .get_or_insert_default()
                        .ndp
                        .get_or_insert_default()
                        .router_solicitations
                        .get_or_insert_default()
                        .max = Some(max_router_solicitations);
                }
                unknown_property => {
                    return Err(user_facing_error(format!(
                        "unknown configuration parameter: {unknown_property}"
                    )));
                }
            }
            Ok(config)
        },
    )?;

    // TODO(https://fxbug.dev/368806554): Print the returned configuration
    // struct to give feedback to user about which parameters changed.
    let _: finterfaces_admin::Configuration = control
        .set_configuration(&config)
        .await
        .map_err(anyhow::Error::new)
        .and_then(|res| {
            res.map_err(|e: finterfaces_admin::ControlSetConfigurationError| {
                anyhow::anyhow!("{:?}", e)
            })
        })
        .context("set configuration")?;

    Ok(())
}

enum IpSubnetAndGateway {
    V4(Subnet<Ipv4Addr>, Option<Ipv4Addr>),
    V6(Subnet<Ipv6Addr>, Option<Ipv6Addr>),
}

fn try_into_matched_subnet_and_gateway(
    destination: std::net::IpAddr,
    prefix_len: u8,
    gateway: Option<std::net::IpAddr>,
) -> Result<IpSubnetAndGateway, Error> {
    match (destination, gateway) {
        (std::net::IpAddr::V4(addr), Some(std::net::IpAddr::V4(g))) => Ok(IpSubnetAndGateway::V4(
            Subnet::new(Ipv4Addr::from(addr).mask(prefix_len), prefix_len)
                .map_err(|e| anyhow!("invalid subnet: {:?}", e))?,
            Some(g.into()),
        )),
        (std::net::IpAddr::V4(addr), None) => Ok(IpSubnetAndGateway::V4(
            Subnet::new(Ipv4Addr::from(addr).mask(prefix_len), prefix_len)
                .map_err(|e| anyhow!("invalid subnet: {:?}", e))?,
            None,
        )),
        (std::net::IpAddr::V6(addr), Some(std::net::IpAddr::V6(g))) => Ok(IpSubnetAndGateway::V6(
            Subnet::new(Ipv6Addr::from(addr).mask(prefix_len), prefix_len)
                .map_err(|e| anyhow!("invalid subnet: {:?}", e))?,
            Some(g.into()),
        )),
        (std::net::IpAddr::V6(addr), None) => Ok(IpSubnetAndGateway::V6(
            Subnet::new(Ipv6Addr::from(addr).mask(prefix_len), prefix_len)
                .map_err(|e| anyhow!("invalid subnet: {:?}", e))?,
            None,
        )),
        (std::net::IpAddr::V4(_), Some(std::net::IpAddr::V6(_)))
        | (std::net::IpAddr::V6(_), Some(std::net::IpAddr::V4(_))) => {
            Err(anyhow!("Gateway IP version mismatch"))
        }
    }
}

async fn get_interface_authorization_proof<C: NetCliDepsConnector>(
    interface: &opts::InterfaceIdentifier,
    connector: &C,
) -> Result<(NonZeroU64, fnet_resources::ProofOfInterfaceAuthorization), Error> {
    let nicid = interface.find_nicid(connector).await?;

    let control = get_control(connector, nicid.get()).await?;

    let grant = control
        .get_authorization_for_interface()
        .await
        .context("failed to get authorization for interface")?;
    let proof = fnet_resources::ProofOfInterfaceAuthorization {
        interface_id: grant.interface_id,
        token: grant.token,
    };

    Ok((nicid, proof))
}

async fn do_route<C: NetCliDepsConnector>(
    out: &mut writer::JsonWriter<serde_json::Value>,
    cmd: opts::RouteEnum,
    connector: &C,
) -> Result<(), Error> {
    match cmd {
        opts::RouteEnum::List(opts::RouteList {}) => do_route_list(out, connector).await?,
        opts::RouteEnum::Add(route) => do_route_add(route, connector).await?,
        opts::RouteEnum::Del(route) => do_route_del(route, connector).await?,
    }
    Ok(())
}

async fn do_route_add<C: NetCliDepsConnector>(
    route: opts::RouteAdd,
    connector: &C,
) -> Result<(), Error> {
    let interface = route.interface;
    let (nicid, proof) = get_interface_authorization_proof(&interface, connector).await?;

    match try_into_matched_subnet_and_gateway(route.destination, route.prefix_len, route.gateway)? {
        IpSubnetAndGateway::V4(dest, g) => {
            let gateway = match g {
                Some(g) => Some(
                    SpecifiedAddr::new(g.into())
                        .ok_or_else(|| anyhow!("gateway must be a specified address"))?,
                ),
                None => None,
            };
            do_route_add_inner::<Ipv4, C>(dest, gateway, nicid, route.metric, proof, connector)
                .await?;
        }
        IpSubnetAndGateway::V6(dest, g) => {
            let gateway = match g {
                Some(g) => Some(
                    SpecifiedAddr::new(g.into())
                        .ok_or_else(|| anyhow!("gateway must be a specified address"))?,
                ),
                None => None,
            };
            do_route_add_inner::<Ipv6, C>(dest, gateway, nicid, route.metric, proof, connector)
                .await?;
        }
    }
    Ok(())
}

fn new_route<I: Ip>(
    destination: net_types::ip::Subnet<I::Addr>,
    nicid: u64,
    gateway: Option<SpecifiedAddr<I::Addr>>,
    metric: Option<u32>,
) -> froutes_ext::Route<I> {
    match metric {
        Some(metric) => froutes_ext::Route::<I>::new_forward_with_explicit_metric(
            destination,
            nicid,
            gateway,
            metric,
        ),
        None => {
            froutes_ext::Route::<I>::new_forward_with_inherited_metric(destination, nicid, gateway)
        }
    }
}

async fn do_route_add_inner<I, C>(
    destination: net_types::ip::Subnet<I::Addr>,
    gateway: Option<SpecifiedAddr<I::Addr>>,
    nicid: NonZeroU64,
    metric: Option<u32>,
    proof: fnet_resources::ProofOfInterfaceAuthorization,
    connector: &C,
) -> Result<(), Error>
where
    I: Ip + froutes_ext::admin::FidlRouteAdminIpExt + froutes_ext::FidlRouteIpExt,
    C: NetCliDepsConnector + ServiceConnector<I::GlobalRouteTableMarker>,
{
    let routes_proxy = connect_with_context::<I::GlobalRouteTableMarker, _>(connector).await?;
    let route_set_proxy = froutes_ext::admin::new_global_route_set::<I>(&routes_proxy)
        .context("failed to create global route set")?;

    froutes_ext::admin::authenticate_for_interface::<I>(&route_set_proxy, proof)
        .await
        .context("authenticate_for_interface failed")?
        .map_err(|e| anyhow!("authenticate failed: {:?}", e))?;

    let route = new_route::<I>(destination, nicid.get(), gateway, metric);
    let fidl_route =
        route.try_into().map_err(|e| anyhow!("failed to convert route to FIDL: {:?}", e))?;

    let did_add = froutes_ext::admin::add_route::<I>(&route_set_proxy, &fidl_route)
        .await
        .context("add_route failed")?
        .map_err(|e| anyhow!("add_route failed: {:?}", e))?;
    if !did_add {
        warn!("Route already exists, did not add.");
    }
    Ok(())
}

async fn do_route_del<C: NetCliDepsConnector>(
    route: opts::RouteDel,
    connector: &C,
) -> Result<(), Error> {
    let interface = route.interface;
    let (nicid, proof) = get_interface_authorization_proof(&interface, connector).await?;

    match try_into_matched_subnet_and_gateway(route.destination, route.prefix_len, route.gateway)? {
        IpSubnetAndGateway::V4(dest, g) => {
            let gateway = match g {
                Some(g) => Some(
                    SpecifiedAddr::new(g.into())
                        .ok_or_else(|| anyhow!("gateway must be a specified address"))?,
                ),
                None => None,
            };
            do_route_del_inner::<Ipv4, C>(dest, gateway, nicid, route.metric, proof, connector)
                .await?;
        }
        IpSubnetAndGateway::V6(dest, g) => {
            let gateway = match g {
                Some(g) => Some(
                    SpecifiedAddr::new(g.into())
                        .ok_or_else(|| anyhow!("gateway must be a specified address"))?,
                ),
                None => None,
            };
            do_route_del_inner::<Ipv6, C>(dest, gateway, nicid, route.metric, proof, connector)
                .await?;
        }
    }
    Ok(())
}

async fn do_route_del_inner<I, C>(
    destination: net_types::ip::Subnet<I::Addr>,
    gateway: Option<SpecifiedAddr<I::Addr>>,
    nicid: NonZeroU64,
    metric: Option<u32>,
    proof: fnet_resources::ProofOfInterfaceAuthorization,
    connector: &C,
) -> Result<(), Error>
where
    I: Ip + froutes_ext::admin::FidlRouteAdminIpExt + froutes_ext::FidlRouteIpExt,
    C: NetCliDepsConnector + ServiceConnector<I::GlobalRouteTableMarker>,
{
    let routes_proxy = connect_with_context::<I::GlobalRouteTableMarker, _>(connector).await?;
    let route_set_proxy = froutes_ext::admin::new_global_route_set::<I>(&routes_proxy)
        .context("failed to create global route set")?;

    froutes_ext::admin::authenticate_for_interface::<I>(&route_set_proxy, proof)
        .await
        .context("authenticate_for_interface failed")?
        .map_err(|e| anyhow!("authenticate failed: {:?}", e))?;

    let route = new_route::<I>(destination, nicid.get(), gateway, metric);
    let fidl_route =
        route.try_into().map_err(|e| anyhow!("failed to convert route to FIDL: {:?}", e))?;

    let did_remove = froutes_ext::admin::remove_route::<I>(&route_set_proxy, &fidl_route)
        .await
        .context("remove_route failed")?
        .map_err(|e| anyhow!("remove_route failed: {:?}", e))?;
    if !did_remove {
        warn!("Route was not found, did not remove.");
    }
    Ok(())
}

async fn do_route_list<C: NetCliDepsConnector>(
    out: &mut writer::JsonWriter<serde_json::Value>,
    connector: &C,
) -> Result<(), Error> {
    let ipv4_route_event_stream = pin!({
        let state_v4 = connect_with_context::<froutes::StateV4Marker, _>(connector)
            .await
            .context("failed to connect to fuchsia.net.routes/StateV4")?;
        froutes_ext::event_stream_from_state::<Ipv4>(&state_v4)
            .context("failed to initialize a `WatcherV4` client")?
            .fuse()
    });
    let ipv6_route_event_stream = pin!({
        let state_v6 = connect_with_context::<froutes::StateV6Marker, _>(connector)
            .await
            .context("failed to connect to fuchsia.net.routes/StateV6")?;
        froutes_ext::event_stream_from_state::<Ipv6>(&state_v6)
            .context("failed to initialize a `WatcherV6` client")?
            .fuse()
    });
    let (v4_routes, v6_routes) = futures::future::join(
        froutes_ext::collect_routes_until_idle::<_, Vec<_>>(ipv4_route_event_stream),
        froutes_ext::collect_routes_until_idle::<_, Vec<_>>(ipv6_route_event_stream),
    )
    .await;
    let mut v4_routes = v4_routes.context("failed to collect all existing IPv4 routes")?;
    let mut v6_routes = v6_routes.context("failed to collect all existing IPv6 routes")?;

    fn group_by_table_id_and_sort<I: net_types::ip::Ip>(
        routes: &mut Vec<froutes_ext::InstalledRoute<I>>,
    ) {
        routes.sort_unstable_by_key(|r| r.table_id);
        for chunk in routes.chunk_by_mut(|a, b| a.table_id == b.table_id) {
            chunk.sort();
        }
    }
    group_by_table_id_and_sort(&mut v4_routes);
    group_by_table_id_and_sort(&mut v6_routes);

    if out.is_machine() {
        fn to_ser<I: net_types::ip::Ip>(
            route: froutes_ext::InstalledRoute<I>,
        ) -> Option<ser::ForwardingEntry> {
            route.try_into().map_err(|e| warn!("failed to convert route: {:?}", e)).ok()
        }
        let routes = v4_routes
            .into_iter()
            .filter_map(to_ser)
            .chain(v6_routes.into_iter().filter_map(to_ser))
            .collect::<Vec<_>>();
        out.machine(&serde_json::to_value(routes)?).context("serialize")?;
    } else {
        let mut t = Table::new();
        t.set_format(format::FormatBuilder::new().padding(2, 2).build());

        // TODO(https://fxbug.dev/342413894): Populate the table name.
        t.set_titles(row!["Destination", "Gateway", "NICID", "Metric", "TableId"]);
        fn write_route<I: net_types::ip::Ip>(t: &mut Table, route: froutes_ext::InstalledRoute<I>) {
            let froutes_ext::InstalledRoute {
                route: froutes_ext::Route { destination, action, properties: _ },
                effective_properties: froutes_ext::EffectiveRouteProperties { metric },
                table_id,
            } = route;
            let (device_id, next_hop) = match action {
                froutes_ext::RouteAction::Forward(froutes_ext::RouteTarget {
                    outbound_interface,
                    next_hop,
                }) => (outbound_interface, next_hop),
                froutes_ext::RouteAction::Unknown => {
                    warn!("observed route with unknown RouteAction.");
                    return;
                }
            };
            let next_hop = next_hop.map(|next_hop| next_hop.to_string());
            let next_hop = next_hop.as_ref().map_or("-", |s| s.as_str());
            add_row(t, row![destination, next_hop, device_id, metric, table_id]);
        }

        for route in v4_routes {
            write_route(&mut t, route);
        }
        for route in v6_routes {
            write_route(&mut t, route);
        }

        let _lines_printed: usize = t.print(out)?;
        out.line("")?;
    }
    Ok(())
}

async fn do_rule<C: NetCliDepsConnector>(
    out: &mut writer::JsonWriter<serde_json::Value>,
    cmd: opts::RuleEnum,
    connector: &C,
) -> Result<(), Error> {
    match cmd {
        opts::RuleEnum::List(opts::RuleList {}) => do_rule_list(out, connector).await,
    }
}

async fn do_rule_list<C: NetCliDepsConnector>(
    out: &mut writer::JsonWriter<serde_json::Value>,
    connector: &C,
) -> Result<(), Error> {
    let ipv4_rule_event_stream = pin!({
        let state_v4 = connect_with_context::<froutes::StateV4Marker, _>(connector)
            .await
            .context("failed to connect to fuchsia.net.routes/StateV4")?;
        froutes_ext::rules::rule_event_stream_from_state::<Ipv4>(&state_v4)
            .context("failed to initialize a `RuleWatcherV4` client")?
            .fuse()
    });
    let ipv6_rule_event_stream = pin!({
        let state_v6 = connect_with_context::<froutes::StateV6Marker, _>(connector)
            .await
            .context("failed to connect to fuchsia.net.routes/StateV6")?;
        froutes_ext::rules::rule_event_stream_from_state::<Ipv6>(&state_v6)
            .context("failed to initialize a `RuleWatcherV6` client")?
            .fuse()
    });
    let (v4_rules, v6_rules) = futures::future::join(
        froutes_ext::rules::collect_rules_until_idle::<Ipv4, Vec<_>>(ipv4_rule_event_stream),
        froutes_ext::rules::collect_rules_until_idle::<Ipv6, Vec<_>>(ipv6_rule_event_stream),
    )
    .await;
    let mut v4_rules = v4_rules.context("failed to collect all existing IPv4 rules")?;
    let mut v6_rules = v6_rules.context("failed to collect all existing IPv6 rules")?;

    v4_rules.sort_by_key(|r| (r.priority, r.index));
    v6_rules.sort_by_key(|r| (r.priority, r.index));

    fn format_matcher(matcher: fnet_matchers_ext::Mark) -> Cow<'static, str> {
        match matcher {
            fnet_matchers_ext::Mark::Unmarked => Cow::Borrowed("unmarked"),
            fnet_matchers_ext::Mark::Marked { mask, between, invert: _ } => {
                format!("{mask:#010x}:{:#010x}..{:#010x}", between.start(), between.end()).into()
            }
        }
    }

    struct FormatRule {
        rule_set_priority: u32,
        index: u32,
        from: Option<String>,
        locally_generated: Option<String>,
        bound_device: Option<String>,
        mark_1: Option<Cow<'static, str>>,
        mark_2: Option<Cow<'static, str>>,
        action: Cow<'static, str>,
    }

    impl FormatRule {
        fn from<I: Ip>(rule: froutes_ext::rules::InstalledRule<I>) -> Self {
            let froutes_ext::rules::InstalledRule {
                priority: rule_set_priority,
                index,
                matcher:
                    froutes_ext::rules::RuleMatcher {
                        from,
                        locally_generated,
                        bound_device,
                        mark_1,
                        mark_2,
                    },
                action,
            } = rule;

            let rule_set_priority = u32::from(rule_set_priority);
            let index = u32::from(index);
            let from = from.map(|from| from.to_string());
            let locally_generated = locally_generated.map(|x| x.to_string());
            let bound_device = bound_device.map(|matcher| match matcher {
                fnet_matchers_ext::BoundInterface::Bound(fnet_matchers_ext::Interface::Name(
                    name,
                )) => format!("name:{name}"),
                fnet_matchers_ext::BoundInterface::Bound(fnet_matchers_ext::Interface::Id(id)) => {
                    format!("id:{id}")
                }
                fnet_matchers_ext::BoundInterface::Bound(
                    fnet_matchers_ext::Interface::PortClass(class),
                ) => format!("class:{class:?}"),
                fnet_matchers_ext::BoundInterface::Unbound => "unbound".into(),
            });
            let mark_1 = mark_1.map(format_matcher);
            let mark_2 = mark_2.map(format_matcher);
            let action = match action {
                froutes_ext::rules::RuleAction::Unreachable => Cow::Borrowed("unreachable"),
                froutes_ext::rules::RuleAction::Lookup(table_id) => {
                    format!("lookup {table_id}").into()
                }
            };

            FormatRule {
                rule_set_priority,
                index,
                from,
                locally_generated,
                bound_device,
                mark_1,
                mark_2,
                action,
            }
        }
    }

    if out.is_machine() {
        fn rule_to_json<I: Ip>(rule: froutes_ext::rules::InstalledRule<I>) -> serde_json::Value {
            let FormatRule {
                rule_set_priority,
                index,
                from,
                locally_generated,
                bound_device,
                mark_1,
                mark_2,
                action,
            } = FormatRule::from(rule);

            serde_json::json!({
                "rule_set_priority": rule_set_priority,
                "index": index,
                "from": from,
                "locally_generated": locally_generated,
                "bound_device": bound_device,
                "mark_1": mark_1,
                "mark_2": mark_2,
                "action": action,
            })
        }

        let rules = v4_rules
            .into_iter()
            .map(rule_to_json)
            .chain(v6_rules.into_iter().map(rule_to_json))
            .collect::<Vec<_>>();
        out.machine(&serde_json::Value::Array(rules)).context("serialize")?;
    } else {
        let mut t = Table::new();
        t.set_format(format::FormatBuilder::new().padding(2, 2).build());
        t.set_titles(row![
            "RuleSetPriority",
            "RuleIndex",
            "From",
            "LocallyGenerated",
            "BoundDevice",
            "Mark1Matcher",
            "Mark2Matcher",
            "Action"
        ]);

        fn option<D: Deref<Target = str>>(string: &Option<D>) -> &str {
            string.as_ref().map_or("-", |s| s.deref())
        }

        fn write_rule<I: Ip>(t: &mut Table, rule: froutes_ext::rules::InstalledRule<I>) {
            let FormatRule {
                rule_set_priority,
                index,
                from,
                locally_generated,
                bound_device,
                mark_1,
                mark_2,
                action,
            } = FormatRule::from(rule);

            add_row(
                t,
                row![
                    rule_set_priority,
                    index,
                    option(&from),
                    option(&locally_generated),
                    option(&bound_device),
                    option(&mark_1),
                    option(&mark_2),
                    action,
                ],
            );
        }

        for rule in v4_rules {
            write_rule(&mut t, rule);
        }

        for rule in v6_rules {
            write_rule(&mut t, rule);
        }

        let _lines_printed: usize = t.print(out)?;
        out.line("")?;
    }
    Ok(())
}

async fn do_filter_deprecated<C: NetCliDepsConnector, W: std::io::Write>(
    mut out: W,
    cmd: opts::FilterDeprecatedEnum,
    connector: &C,
) -> Result<(), Error> {
    let filter = connect_with_context::<ffilter_deprecated::FilterMarker, _>(connector).await?;
    match cmd {
        opts::FilterDeprecatedEnum::GetRules(opts::FilterGetRules {}) => {
            let (rules, generation): (Vec<ffilter_deprecated::Rule>, u32) =
                filter.get_rules().await?;
            writeln!(out, "{:?} (generation {})", rules, generation)?;
        }
        opts::FilterDeprecatedEnum::SetRules(opts::FilterSetRules { rules }) => {
            let (_cur_rules, generation) = filter.get_rules().await?;
            let rules = netfilter::parser_deprecated::parse_str_to_rules(&rules)?;
            filter_fidl!(filter.update_rules(&rules, generation), "error setting filter rules")?;
            info!("successfully set filter rules");
        }
        opts::FilterDeprecatedEnum::GetNatRules(opts::FilterGetNatRules {}) => {
            let (rules, generation): (Vec<ffilter_deprecated::Nat>, u32) =
                filter.get_nat_rules().await?;
            writeln!(out, "{:?} (generation {})", rules, generation)?;
        }
        opts::FilterDeprecatedEnum::SetNatRules(opts::FilterSetNatRules { rules }) => {
            let (_cur_rules, generation) = filter.get_nat_rules().await?;
            let rules = netfilter::parser_deprecated::parse_str_to_nat_rules(&rules)?;
            filter_fidl!(filter.update_nat_rules(&rules, generation), "error setting NAT rules")?;
            info!("successfully set NAT rules");
        }
        opts::FilterDeprecatedEnum::GetRdrRules(opts::FilterGetRdrRules {}) => {
            let (rules, generation): (Vec<ffilter_deprecated::Rdr>, u32) =
                filter.get_rdr_rules().await?;
            writeln!(out, "{:?} (generation {})", rules, generation)?;
        }
        opts::FilterDeprecatedEnum::SetRdrRules(opts::FilterSetRdrRules { rules }) => {
            let (_cur_rules, generation) = filter.get_rdr_rules().await?;
            let rules = netfilter::parser_deprecated::parse_str_to_rdr_rules(&rules)?;
            filter_fidl!(filter.update_rdr_rules(&rules, generation), "error setting RDR rules")?;
            info!("successfully set RDR rules");
        }
    }
    Ok(())
}

async fn do_log<C: NetCliDepsConnector>(cmd: opts::LogEnum, connector: &C) -> Result<(), Error> {
    let log = connect_with_context::<fstack::LogMarker, _>(connector).await?;
    match cmd {
        opts::LogEnum::SetPackets(opts::LogSetPackets { enabled }) => {
            log.set_log_packets(enabled).await.context("error setting log packets")?;
            info!("log packets set to {:?}", enabled);
        }
    }
    Ok(())
}

async fn do_dhcp<C: NetCliDepsConnector>(cmd: opts::DhcpEnum, connector: &C) -> Result<(), Error> {
    let stack = connect_with_context::<fstack::StackMarker, _>(connector).await?;
    match cmd {
        opts::DhcpEnum::Start(opts::DhcpStart { interface }) => {
            let id = interface.find_nicid(connector).await?.get();
            fstack_ext::exec_fidl!(
                stack.set_dhcp_client_enabled(id, true),
                "error stopping DHCP client"
            )?;
            info!("dhcp client started on interface {}", id);
        }
        opts::DhcpEnum::Stop(opts::DhcpStop { interface }) => {
            let id = interface.find_nicid(connector).await?.get();
            fstack_ext::exec_fidl!(
                stack.set_dhcp_client_enabled(id, false),
                "error stopping DHCP client"
            )?;
            info!("dhcp client stopped on interface {}", id);
        }
    }
    Ok(())
}

async fn do_dhcpd<C: NetCliDepsConnector>(
    cmd: opts::dhcpd::DhcpdEnum,
    connector: &C,
) -> Result<(), Error> {
    let dhcpd_server = connect_with_context::<fdhcp::Server_Marker, _>(connector).await?;
    match cmd {
        opts::dhcpd::DhcpdEnum::Start(opts::dhcpd::Start {}) => {
            Ok(do_dhcpd_start(dhcpd_server).await?)
        }
        opts::dhcpd::DhcpdEnum::Stop(opts::dhcpd::Stop {}) => {
            Ok(do_dhcpd_stop(dhcpd_server).await?)
        }
        opts::dhcpd::DhcpdEnum::Get(get_arg) => Ok(do_dhcpd_get(get_arg, dhcpd_server).await?),
        opts::dhcpd::DhcpdEnum::Set(set_arg) => Ok(do_dhcpd_set(set_arg, dhcpd_server).await?),
        opts::dhcpd::DhcpdEnum::List(list_arg) => Ok(do_dhcpd_list(list_arg, dhcpd_server).await?),
        opts::dhcpd::DhcpdEnum::Reset(reset_arg) => {
            Ok(do_dhcpd_reset(reset_arg, dhcpd_server).await?)
        }
        opts::dhcpd::DhcpdEnum::ClearLeases(opts::dhcpd::ClearLeases {}) => {
            Ok(do_dhcpd_clear_leases(dhcpd_server).await?)
        }
    }
}

async fn do_neigh<C: NetCliDepsConnector>(
    out: writer::JsonWriter<serde_json::Value>,
    cmd: opts::NeighEnum,
    connector: &C,
) -> Result<(), Error> {
    match cmd {
        opts::NeighEnum::Add(opts::NeighAdd { interface, ip, mac }) => {
            let interface = interface.find_nicid(connector).await?.get();
            let controller =
                connect_with_context::<fneighbor::ControllerMarker, _>(connector).await?;
            do_neigh_add(interface, ip.into(), mac.into(), controller)
                .await
                .context("failed during neigh add command")?;
            info!("Added entry ({}, {}) for interface {}", ip, mac, interface);
        }
        opts::NeighEnum::Clear(opts::NeighClear { interface, ip_version }) => {
            let interface = interface.find_nicid(connector).await?.get();
            let controller =
                connect_with_context::<fneighbor::ControllerMarker, _>(connector).await?;
            do_neigh_clear(interface, ip_version, controller)
                .await
                .context("failed during neigh clear command")?;
            info!("Cleared entries for interface {}", interface);
        }
        opts::NeighEnum::Del(opts::NeighDel { interface, ip }) => {
            let interface = interface.find_nicid(connector).await?.get();
            let controller =
                connect_with_context::<fneighbor::ControllerMarker, _>(connector).await?;
            do_neigh_del(interface, ip.into(), controller)
                .await
                .context("failed during neigh del command")?;
            info!("Deleted entry {} for interface {}", ip, interface);
        }
        opts::NeighEnum::List(opts::NeighList {}) => {
            let view = connect_with_context::<fneighbor::ViewMarker, _>(connector).await?;
            print_neigh_entries(out, false /* watch_for_changes */, view)
                .await
                .context("error listing neighbor entries")?;
        }
        opts::NeighEnum::Watch(opts::NeighWatch {}) => {
            let view = connect_with_context::<fneighbor::ViewMarker, _>(connector).await?;
            print_neigh_entries(out, true /* watch_for_changes */, view)
                .await
                .context("error watching for changes to the neighbor table")?;
        }
        opts::NeighEnum::Config(opts::NeighConfig { neigh_config_cmd }) => match neigh_config_cmd {
            opts::NeighConfigEnum::Get(opts::NeighGetConfig { interface, ip_version }) => {
                let interface = interface.find_nicid(connector).await?.get();
                let control = get_control(connector, interface).await.context("get control")?;
                let configuration = control
                    .get_configuration()
                    .await
                    .map_err(anyhow::Error::new)
                    .and_then(|res| {
                        res.map_err(|e: finterfaces_admin::ControlGetConfigurationError| {
                            anyhow!("{:?}", e)
                        })
                    })
                    .context("get configuration")?;
                let nud = extract_nud_config(configuration, ip_version)?;
                println!("{:#?}", nud);
            }
            opts::NeighConfigEnum::Update(opts::NeighUpdateConfig {
                interface,
                ip_version,
                base_reachable_time,
            }) => {
                let interface = interface.find_nicid(connector).await?.get();
                let control = get_control(connector, interface).await.context("get control")?;
                let nud_config = finterfaces_admin::NudConfiguration {
                    base_reachable_time,
                    ..Default::default()
                };
                let config = match ip_version {
                    fnet::IpVersion::V4 => finterfaces_admin::Configuration {
                        ipv4: Some(finterfaces_admin::Ipv4Configuration {
                            arp: Some(finterfaces_admin::ArpConfiguration {
                                nud: Some(nud_config),
                                ..Default::default()
                            }),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    fnet::IpVersion::V6 => finterfaces_admin::Configuration {
                        ipv6: Some(finterfaces_admin::Ipv6Configuration {
                            ndp: Some(finterfaces_admin::NdpConfiguration {
                                nud: Some(nud_config),
                                ..Default::default()
                            }),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                };
                let prev_config = control
                    .set_configuration(&config)
                    .await
                    .map_err(anyhow::Error::new)
                    .and_then(|res| {
                        res.map_err(|e: finterfaces_admin::ControlSetConfigurationError| {
                            anyhow!("{:?}", e)
                        })
                    })
                    .context("set configuration")?;
                let prev_nud = extract_nud_config(prev_config, ip_version)?;
                info!("Updated config for interface {}; previously was: {:?}", interface, prev_nud);
            }
        },
    }
    Ok(())
}

async fn do_neigh_add(
    interface: u64,
    neighbor: fnet::IpAddress,
    mac: fnet::MacAddress,
    controller: fneighbor::ControllerProxy,
) -> Result<(), Error> {
    controller
        .add_entry(interface, &neighbor.into(), &mac.into())
        .await
        .context("FIDL error adding neighbor entry")?
        .map_err(|e| anyhow::anyhow!("error adding neighbor entry: {e:?}"))
}

async fn do_neigh_clear(
    interface: u64,
    ip_version: fnet::IpVersion,
    controller: fneighbor::ControllerProxy,
) -> Result<(), Error> {
    controller
        .clear_entries(interface, ip_version)
        .await
        .context("FIDL error clearing neighbor table")?
        .map_err(|e| anyhow::anyhow!("error clearing neighbor table: {e:?}"))
}

async fn do_neigh_del(
    interface: u64,
    neighbor: fnet::IpAddress,
    controller: fneighbor::ControllerProxy,
) -> Result<(), Error> {
    controller
        .remove_entry(interface, &neighbor.into())
        .await
        .context("FIDL error removing neighbor entry")?
        .map_err(|e| anyhow::anyhow!("error removing neighbor entry: {e:?}"))
}

fn unpack_neigh_iter_item(
    item: fneighbor::EntryIteratorItem,
) -> Result<(&'static str, Option<fneighbor_ext::Entry>), Error> {
    let displayed_state_change_status = ser::DISPLAYED_NEIGH_ENTRY_VARIANTS.select(&item);

    Ok((
        displayed_state_change_status,
        match item {
            fneighbor::EntryIteratorItem::Existing(entry)
            | fneighbor::EntryIteratorItem::Added(entry)
            | fneighbor::EntryIteratorItem::Changed(entry)
            | fneighbor::EntryIteratorItem::Removed(entry) => {
                Some(fneighbor_ext::Entry::try_from(entry)?)
            }
            fneighbor::EntryIteratorItem::Idle(fneighbor::IdleEvent) => None,
        },
    ))
}

fn jsonify_neigh_iter_item(
    item: fneighbor::EntryIteratorItem,
    include_entry_state: bool,
) -> Result<Value, Error> {
    let (state_change_status, entry) = unpack_neigh_iter_item(item)?;
    let entry_json = entry
        .map(ser::NeighborTableEntry::from)
        .map(serde_json::to_value)
        .map(|res| res.map_err(Error::new))
        .unwrap_or_else(|| Err(anyhow!("failed to jsonify NeighborTableEntry")))?;
    if include_entry_state {
        Ok(json!({
            "state_change_status": state_change_status,
            "entry": entry_json,
        }))
    } else {
        Ok(entry_json)
    }
}

async fn print_neigh_entries(
    mut out: writer::JsonWriter<serde_json::Value>,
    watch_for_changes: bool,
    view: fneighbor::ViewProxy,
) -> Result<(), Error> {
    let (it, it_server) = view.domain().create_proxy::<fneighbor::EntryIteratorMarker>();

    view.open_entry_iterator(it_server, &fneighbor::EntryIteratorOptions::default())
        .context("error opening a connection to the entry iterator")?;

    let out_ref = &mut out;
    if watch_for_changes {
        neigh_entry_stream(it, watch_for_changes)
            .map_ok(|item| {
                write_neigh_entry(out_ref, item, /* include_entry_state= */ watch_for_changes)
                    .context("error writing entry")
            })
            .try_fold((), |(), r| futures::future::ready(r))
            .await?;
    } else {
        let results: Vec<Result<fneighbor::EntryIteratorItem, _>> =
            neigh_entry_stream(it, watch_for_changes).collect().await;
        if out.is_machine() {
            let jsonified_items: Value =
                itertools::process_results(results.into_iter(), |items| {
                    itertools::process_results(
                        items.map(|item| {
                            jsonify_neigh_iter_item(
                                item,
                                /* include_entry_state= */ watch_for_changes,
                            )
                        }),
                        |json_values| Value::from_iter(json_values),
                    )
                })??;
            out.machine(&jsonified_items)?;
        } else {
            itertools::process_results(results.into_iter(), |mut items| {
                items.try_for_each(|item| {
                    write_tabular_neigh_entry(
                        &mut out,
                        item,
                        /* include_entry_state= */ watch_for_changes,
                    )
                })
            })??;
        }
    }

    Ok(())
}

fn neigh_entry_stream(
    iterator: fneighbor::EntryIteratorProxy,
    watch_for_changes: bool,
) -> impl futures::Stream<Item = Result<fneighbor::EntryIteratorItem, Error>> {
    futures::stream::try_unfold(iterator, |iterator| {
        iterator
            .get_next()
            .map_ok(|items| Some((items, iterator)))
            .map(|r| r.context("error getting items from iterator"))
    })
    .map_ok(|items| futures::stream::iter(items.into_iter().map(Ok)))
    .try_flatten()
    .take_while(move |item| {
        futures::future::ready(item.as_ref().is_ok_and(|item| {
            if let fneighbor::EntryIteratorItem::Idle(fneighbor::IdleEvent {}) = item {
                watch_for_changes
            } else {
                true
            }
        }))
    })
}

fn write_tabular_neigh_entry<W: std::io::Write>(
    mut f: W,
    item: fneighbor::EntryIteratorItem,
    include_entry_state: bool,
) -> Result<(), Error> {
    let (state_change_status, entry) = unpack_neigh_iter_item(item)?;
    match entry {
        Some(entry) => {
            if include_entry_state {
                writeln!(
                    &mut f,
                    "{:width$} | {}",
                    state_change_status,
                    entry,
                    width = ser::DISPLAYED_NEIGH_ENTRY_VARIANTS
                        .into_iter()
                        .map(|s| s.len())
                        .max()
                        .unwrap_or(0),
                )?
            } else {
                writeln!(&mut f, "{}", entry)?
            }
        }
        None => writeln!(&mut f, "{}", state_change_status)?,
    }
    Ok(())
}

fn write_neigh_entry(
    f: &mut writer::JsonWriter<serde_json::Value>,
    item: fneighbor::EntryIteratorItem,
    include_entry_state: bool,
) -> Result<(), Error> {
    if f.is_machine() {
        let entry = jsonify_neigh_iter_item(item, include_entry_state)?;
        f.machine(&entry)?;
    } else {
        write_tabular_neigh_entry(f, item, include_entry_state)?
    }
    Ok(())
}

async fn do_dhcpd_start(server: fdhcp::Server_Proxy) -> Result<(), Error> {
    server
        .start_serving()
        .await?
        .map_err(zx::Status::err_from_raw)
        .context("failed to start server")
}

async fn do_dhcpd_stop(server: fdhcp::Server_Proxy) -> Result<(), Error> {
    server.stop_serving().await.context("failed to stop server")
}

async fn do_dhcpd_get(get_arg: opts::dhcpd::Get, server: fdhcp::Server_Proxy) -> Result<(), Error> {
    match get_arg.arg {
        opts::dhcpd::GetArg::Option(opts::dhcpd::OptionArg { name }) => {
            let res = server
                .get_option(name.clone().into())
                .await?
                .map_err(zx::Status::err_from_raw)
                .with_context(|| format!("get_option({:?}) failed", name))?;
            println!("{:#?}", res);
        }
        opts::dhcpd::GetArg::Parameter(opts::dhcpd::ParameterArg { name }) => {
            let res = server
                .get_parameter(name.clone().into())
                .await?
                .map_err(zx::Status::err_from_raw)
                .with_context(|| format!("get_parameter({:?}) failed", name))?;
            println!("{:#?}", res);
        }
    };
    Ok(())
}

async fn do_dhcpd_set(set_arg: opts::dhcpd::Set, server: fdhcp::Server_Proxy) -> Result<(), Error> {
    match set_arg.arg {
        opts::dhcpd::SetArg::Option(opts::dhcpd::OptionArg { name }) => {
            server
                .set_option(&name.clone().into())
                .await?
                .map_err(zx::Status::err_from_raw)
                .with_context(|| format!("set_option({:?}) failed", name))?;
        }
        opts::dhcpd::SetArg::Parameter(opts::dhcpd::ParameterArg { name }) => {
            server
                .set_parameter(&name.clone().into())
                .await?
                .map_err(zx::Status::err_from_raw)
                .with_context(|| format!("set_parameter({:?}) failed", name))?;
        }
    };
    Ok(())
}

async fn do_dhcpd_list(
    list_arg: opts::dhcpd::List,
    server: fdhcp::Server_Proxy,
) -> Result<(), Error> {
    match list_arg.arg {
        opts::dhcpd::ListArg::Option(opts::dhcpd::OptionToken {}) => {
            let res = server
                .list_options()
                .await?
                .map_err(zx::Status::err_from_raw)
                .context("list_options() failed")?;

            println!("{:#?}", res);
        }
        opts::dhcpd::ListArg::Parameter(opts::dhcpd::ParameterToken {}) => {
            let res = server
                .list_parameters()
                .await?
                .map_err(zx::Status::err_from_raw)
                .context("list_parameters() failed")?;
            println!("{:#?}", res);
        }
    };
    Ok(())
}

async fn do_dhcpd_reset(
    reset_arg: opts::dhcpd::Reset,
    server: fdhcp::Server_Proxy,
) -> Result<(), Error> {
    match reset_arg.arg {
        opts::dhcpd::ResetArg::Option(opts::dhcpd::OptionToken {}) => {
            server
                .reset_options()
                .await?
                .map_err(zx::Status::err_from_raw)
                .context("reset_options() failed")?;
        }
        opts::dhcpd::ResetArg::Parameter(opts::dhcpd::ParameterToken {}) => {
            server
                .reset_parameters()
                .await?
                .map_err(zx::Status::err_from_raw)
                .context("reset_parameters() failed")?;
        }
    };
    Ok(())
}

async fn do_dhcpd_clear_leases(server: fdhcp::Server_Proxy) -> Result<(), Error> {
    server.clear_leases().await?.map_err(zx::Status::err_from_raw).context("clear_leases() failed")
}

async fn do_dns<W: std::io::Write, C: NetCliDepsConnector>(
    mut out: W,
    cmd: opts::dns::DnsEnum,
    connector: &C,
) -> Result<(), Error> {
    let lookup = connect_with_context::<fname::LookupMarker, _>(connector).await?;
    let opts::dns::DnsEnum::Lookup(opts::dns::Lookup { hostname, ipv4, ipv6, sort }) = cmd;
    let result = lookup
        .lookup_ip(
            &hostname,
            &fname::LookupIpOptions {
                ipv4_lookup: Some(ipv4),
                ipv6_lookup: Some(ipv6),
                sort_addresses: Some(sort),
                ..Default::default()
            },
        )
        .await?
        .map_err(|e| anyhow!("DNS lookup failed: {:?}", e))?;
    let fname::LookupResult { addresses, .. } = result;
    let addrs = addresses.context("`addresses` not set in response from DNS resolver")?;
    for addr in addrs {
        writeln!(out, "{}", fnet_ext::IpAddress::from(addr))?;
    }
    Ok(())
}

async fn do_netstack_migration<W: std::io::Write, C: NetCliDepsConnector>(
    mut out: W,
    cmd: opts::NetstackMigrationEnum,
    connector: &C,
) -> Result<(), Error> {
    match cmd {
        opts::NetstackMigrationEnum::Set(opts::NetstackMigrationSet { version }) => {
            let control =
                connect_with_context::<fnet_migration::ControlMarker, _>(connector).await?;
            control
                .set_user_netstack_version(Some(&fnet_migration::VersionSetting { version }))
                .await
                .context("failed to set stack version")
        }
        opts::NetstackMigrationEnum::Clear(opts::NetstackMigrationClear {}) => {
            let control =
                connect_with_context::<fnet_migration::ControlMarker, _>(connector).await?;
            control.set_user_netstack_version(None).await.context("failed to set stack version")
        }
        opts::NetstackMigrationEnum::Get(opts::NetstackMigrationGet {}) => {
            let state = connect_with_context::<fnet_migration::StateMarker, _>(connector).await?;
            let fnet_migration::InEffectVersion { current_boot, user, automated, .. } =
                state.get_netstack_version().await.context("failed to get stack version")?;
            writeln!(out, "current_boot = {current_boot:?}")?;
            writeln!(out, "user = {user:?}")?;
            writeln!(out, "automated = {automated:?}")?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod testutil {
    use flex_client::fidl::ProtocolMarker;

    use super::*;

    #[derive(Default)]
    pub(crate) struct TestConnector {
        pub debug_interfaces: Option<fdebug::InterfacesProxy>,
        pub dhcpd: Option<fdhcp::Server_Proxy>,
        pub interfaces_state: Option<finterfaces::StateProxy>,
        pub stack: Option<fstack::StackProxy>,
        pub root_interfaces: Option<froot::InterfacesProxy>,
        pub root_filter: Option<froot::FilterProxy>,
        pub root_routes_v4: Option<froot::RoutesV4Proxy>,
        pub root_routes_v6: Option<froot::RoutesV6Proxy>,
        pub routes_v4: Option<froutes::StateV4Proxy>,
        pub routes_v6: Option<froutes::StateV6Proxy>,
        pub name_lookup: Option<fname::LookupProxy>,
        pub filter: Option<fnet_filter::StateProxy>,
        pub installer: Option<finterfaces_admin::InstallerProxy>,
        pub packet_capture_provider: Option<fdebug::PacketCaptureProviderProxy>,
    }

    impl CaptureDeps for TestConnector {
        type OutputWriter = std::io::Sink;
        fn create_output_writer(
            &self,
            _path: &std::path::Path,
        ) -> Result<Self::OutputWriter, anyhow::Error> {
            Ok(std::io::sink())
        }
    }

    #[async_trait::async_trait]
    impl ServiceConnector<fdebug::InterfacesMarker> for TestConnector {
        async fn connect(
            &self,
        ) -> Result<<fdebug::InterfacesMarker as ProtocolMarker>::Proxy, Error> {
            self.debug_interfaces
                .as_ref()
                .cloned()
                .ok_or_else(|| anyhow!("connector has no dhcp server instance"))
        }
    }

    #[async_trait::async_trait]
    impl ServiceConnector<froot::InterfacesMarker> for TestConnector {
        async fn connect(
            &self,
        ) -> Result<<froot::InterfacesMarker as ProtocolMarker>::Proxy, Error> {
            self.root_interfaces
                .as_ref()
                .cloned()
                .ok_or_else(|| anyhow!("connector has no root interfaces instance"))
        }
    }

    #[async_trait::async_trait]
    impl ServiceConnector<froot::FilterMarker> for TestConnector {
        async fn connect(&self) -> Result<<froot::FilterMarker as ProtocolMarker>::Proxy, Error> {
            self.root_filter
                .as_ref()
                .cloned()
                .ok_or_else(|| anyhow!("connector has no root filter instance"))
        }
    }

    #[async_trait::async_trait]
    impl ServiceConnector<fdhcp::Server_Marker> for TestConnector {
        async fn connect(&self) -> Result<<fdhcp::Server_Marker as ProtocolMarker>::Proxy, Error> {
            self.dhcpd
                .as_ref()
                .cloned()
                .ok_or_else(|| anyhow!("connector has no dhcp server instance"))
        }
    }

    #[async_trait::async_trait]
    impl ServiceConnector<ffilter_deprecated::FilterMarker> for TestConnector {
        async fn connect(
            &self,
        ) -> Result<<ffilter_deprecated::FilterMarker as ProtocolMarker>::Proxy, Error> {
            Err(anyhow!("connect filter_deprecated unimplemented for test connector"))
        }
    }

    #[async_trait::async_trait]
    impl ServiceConnector<finterfaces::StateMarker> for TestConnector {
        async fn connect(
            &self,
        ) -> Result<<finterfaces::StateMarker as ProtocolMarker>::Proxy, Error> {
            self.interfaces_state
                .as_ref()
                .cloned()
                .ok_or_else(|| anyhow!("connector has no interfaces state instance"))
        }
    }

    #[async_trait::async_trait]
    impl ServiceConnector<finterfaces_admin::InstallerMarker> for TestConnector {
        async fn connect(
            &self,
        ) -> Result<<finterfaces_admin::InstallerMarker as ProtocolMarker>::Proxy, Error> {
            self.installer
                .as_ref()
                .cloned()
                .ok_or_else(|| anyhow!("connector has no fuchsia.net.interfaces.admin.Installer"))
        }
    }

    #[async_trait::async_trait]
    impl ServiceConnector<fdebug::PacketCaptureProviderMarker> for TestConnector {
        async fn connect(
            &self,
        ) -> Result<<fdebug::PacketCaptureProviderMarker as ProtocolMarker>::Proxy, Error> {
            self.packet_capture_provider
                .as_ref()
                .cloned()
                .ok_or_else(|| anyhow!("connector has no packet capture provider instance"))
        }
    }

    #[async_trait::async_trait]
    impl ServiceConnector<fneighbor::ControllerMarker> for TestConnector {
        async fn connect(
            &self,
        ) -> Result<<fneighbor::ControllerMarker as ProtocolMarker>::Proxy, Error> {
            Err(anyhow!("connect neighbor controller unimplemented for test connector"))
        }
    }

    #[async_trait::async_trait]
    impl ServiceConnector<fneighbor::ViewMarker> for TestConnector {
        async fn connect(&self) -> Result<<fneighbor::ViewMarker as ProtocolMarker>::Proxy, Error> {
            Err(anyhow!("connect neighbor view unimplemented for test connector"))
        }
    }

    #[async_trait::async_trait]
    impl ServiceConnector<fstack::LogMarker> for TestConnector {
        async fn connect(&self) -> Result<<fstack::LogMarker as ProtocolMarker>::Proxy, Error> {
            Err(anyhow!("connect log unimplemented for test connector"))
        }
    }

    #[async_trait::async_trait]
    impl ServiceConnector<fstack::StackMarker> for TestConnector {
        async fn connect(&self) -> Result<<fstack::StackMarker as ProtocolMarker>::Proxy, Error> {
            self.stack.as_ref().cloned().ok_or_else(|| anyhow!("connector has no stack instance"))
        }
    }

    #[async_trait::async_trait]
    impl ServiceConnector<froutes::StateV4Marker> for TestConnector {
        async fn connect(
            &self,
        ) -> Result<<froutes::StateV4Marker as ProtocolMarker>::Proxy, Error> {
            self.routes_v4
                .as_ref()
                .cloned()
                .ok_or_else(|| anyhow!("connector has no routes_v4 instance"))
        }
    }

    #[async_trait::async_trait]
    impl ServiceConnector<froutes::StateV6Marker> for TestConnector {
        async fn connect(
            &self,
        ) -> Result<<froutes::StateV6Marker as ProtocolMarker>::Proxy, Error> {
            self.routes_v6
                .as_ref()
                .cloned()
                .ok_or_else(|| anyhow!("connector has no routes_v6 instance"))
        }
    }

    #[async_trait::async_trait]
    impl ServiceConnector<froot::RoutesV4Marker> for TestConnector {
        async fn connect(&self) -> Result<<froot::RoutesV4Marker as ProtocolMarker>::Proxy, Error> {
            self.root_routes_v4
                .as_ref()
                .cloned()
                .ok_or_else(|| anyhow!("connector has no root_routes_v4 instance"))
        }
    }

    #[async_trait::async_trait]
    impl ServiceConnector<froot::RoutesV6Marker> for TestConnector {
        async fn connect(&self) -> Result<<froot::RoutesV6Marker as ProtocolMarker>::Proxy, Error> {
            self.root_routes_v6
                .as_ref()
                .cloned()
                .ok_or_else(|| anyhow!("connector has no root_routes_v6 instance"))
        }
    }

    #[async_trait::async_trait]
    impl ServiceConnector<fname::LookupMarker> for TestConnector {
        async fn connect(&self) -> Result<<fname::LookupMarker as ProtocolMarker>::Proxy, Error> {
            self.name_lookup
                .as_ref()
                .cloned()
                .ok_or_else(|| anyhow!("connector has no name lookup instance"))
        }
    }

    #[async_trait::async_trait]
    impl ServiceConnector<fnet_migration::ControlMarker> for TestConnector {
        async fn connect(
            &self,
        ) -> Result<<fnet_migration::ControlMarker as ProtocolMarker>::Proxy, Error> {
            unimplemented!("stack migration not supported");
        }
    }

    #[async_trait::async_trait]
    impl ServiceConnector<fnet_migration::StateMarker> for TestConnector {
        async fn connect(
            &self,
        ) -> Result<<fnet_migration::StateMarker as ProtocolMarker>::Proxy, Error> {
            unimplemented!("stack migration not supported");
        }
    }

    #[async_trait::async_trait]
    impl ServiceConnector<fnet_filter::StateMarker> for TestConnector {
        async fn connect(
            &self,
        ) -> Result<<fnet_filter::StateMarker as ProtocolMarker>::Proxy, Error> {
            self.filter.as_ref().cloned().ok_or_else(|| anyhow!("connector has no filter instance"))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::convert::TryInto as _;
    use std::fmt::Debug;
    use std::num::NonZeroU64;

    use assert_matches::assert_matches;
    #[cfg(not(feature = "fdomain"))]
    use fidl_fuchsia_net_routes_ext as froutes_ext;
    #[cfg(feature = "fdomain")]
    use fidl_fuchsia_net_routes_ext_fdomain as froutes_ext;
    use flex_fuchsia_net_routes as froutes;
    use fuchsia_async::TimeoutExt as _;
    use net_declare::{fidl_ip, fidl_ip_v4, fidl_mac, fidl_subnet};
    use test_case::test_case;

    use super::testutil::TestConnector;
    use super::*;

    const IF_ADDR_V4: fnet::Subnet = fidl_subnet!("192.168.0.1/32");
    const IF_ADDR_V6: fnet::Subnet = fidl_subnet!("fd00::1/64");

    const MAC_1: fnet::MacAddress = fidl_mac!("01:02:03:04:05:06");
    const MAC_2: fnet::MacAddress = fidl_mac!("02:03:04:05:06:07");

    fn trim_whitespace_for_comparison(s: &str) -> String {
        s.trim().lines().map(|s| s.trim()).collect::<Vec<&str>>().join("\n")
    }

    fn get_fake_interface(
        id: u64,
        name: &'static str,
        port_class: finterfaces_ext::PortClass,
        octets: Option<[u8; 6]>,
    ) -> (finterfaces_ext::Properties<finterfaces_ext::AllInterest>, Option<fnet::MacAddress>) {
        let port_identity_koid = match port_class {
            finterfaces_ext::PortClass::Loopback => None,
            finterfaces_ext::PortClass::Virtual
            | finterfaces_ext::PortClass::Ethernet
            | finterfaces_ext::PortClass::WlanClient
            | finterfaces_ext::PortClass::WlanAp
            | finterfaces_ext::PortClass::Ppp
            | finterfaces_ext::PortClass::Bridge
            | finterfaces_ext::PortClass::Lowpan
            | finterfaces_ext::PortClass::Blackhole => {
                Some(finterfaces_ext::PortIdentityKoid::from_raw(id))
            }
        };
        (
            finterfaces_ext::Properties {
                id: id.try_into().unwrap(),
                name: name.to_string(),
                port_class,
                online: true,
                addresses: Vec::new(),
                has_default_ipv4_route: false,
                has_default_ipv6_route: false,
                port_identity_koid,
            },
            octets.map(|octets| fnet::MacAddress { octets }),
        )
    }

    fn shortlist_interfaces_by_nicid(name_pattern: &str) -> Vec<u64> {
        let mut interfaces = [
            get_fake_interface(1, "lo", finterfaces_ext::PortClass::Loopback, None),
            get_fake_interface(
                10,
                "eth001",
                finterfaces_ext::PortClass::Ethernet,
                Some([1, 2, 3, 4, 5, 6]),
            ),
            get_fake_interface(
                20,
                "eth002",
                finterfaces_ext::PortClass::Ethernet,
                Some([1, 2, 3, 4, 5, 7]),
            ),
            get_fake_interface(
                30,
                "eth003",
                finterfaces_ext::PortClass::Ethernet,
                Some([1, 2, 3, 4, 5, 8]),
            ),
            get_fake_interface(
                100,
                "wlan001",
                finterfaces_ext::PortClass::WlanClient,
                Some([2, 2, 3, 4, 5, 6]),
            ),
            get_fake_interface(
                200,
                "wlan002",
                finterfaces_ext::PortClass::WlanClient,
                Some([2, 2, 3, 4, 5, 7]),
            ),
            get_fake_interface(
                300,
                "wlan003",
                finterfaces_ext::PortClass::WlanClient,
                Some([2, 2, 3, 4, 5, 8]),
            ),
        ]
        .into_iter()
        .map(|(properties, _): (_, Option<fnet::MacAddress>)| {
            let finterfaces_ext::Properties { id, .. } = &properties;
            (id.get(), finterfaces_ext::PropertiesAndState { properties, state: () })
        })
        .collect();
        shortlist_interfaces(name_pattern, &mut interfaces);
        let mut interfaces: Vec<_> = interfaces.into_keys().collect();
        interfaces.sort();
        interfaces
    }

    #[test]
    fn test_shortlist_interfaces() {
        assert_eq!(vec![1, 10, 20, 30, 100, 200, 300], shortlist_interfaces_by_nicid(""));
        assert_eq!(vec![0_u64; 0], shortlist_interfaces_by_nicid("no such thing"));

        assert_eq!(vec![1], shortlist_interfaces_by_nicid("lo"));
        assert_eq!(vec![10, 20, 30], shortlist_interfaces_by_nicid("eth"));
        assert_eq!(vec![10, 20, 30], shortlist_interfaces_by_nicid("th"));
        assert_eq!(vec![100, 200, 300], shortlist_interfaces_by_nicid("wlan"));
        assert_eq!(vec![10, 100], shortlist_interfaces_by_nicid("001"));
    }

    #[test_case(fnet::IpVersion::V4, true ; "IPv4 enable routing")]
    #[test_case(fnet::IpVersion::V4, false ; "IPv4 disable routing")]
    #[test_case(fnet::IpVersion::V6, true ; "IPv6 enable routing")]
    #[test_case(fnet::IpVersion::V6, false ; "IPv6 disable routing")]
    #[fuchsia::test(logging = false)]
    async fn if_ip_forward(ip_version: fnet::IpVersion, enable: bool) {
        let client = flex_local::local_client_empty();

        let interface1 = TestInterface { nicid: 1, name: "interface1" };
        let (root_interfaces, mut requests) =
            client.create_proxy_and_stream::<froot::InterfacesMarker>();
        let connector =
            TestConnector { root_interfaces: Some(root_interfaces), ..Default::default() };

        let requests_fut = set_configuration_request(
            &mut requests,
            interface1.nicid,
            |c| extract_ip_forwarding(c, ip_version).expect("extract IP forwarding configuration"),
            enable,
        );
        let buf = writer::TestBuffers::default();
        let mut out = writer::JsonWriter::new_test(None, &buf);
        let do_if_fut = do_if(
            &mut out,
            opts::IfEnum::IpForward(opts::IfIpForward {
                cmd: opts::IfIpForwardEnum::Set(opts::IfIpForwardSet {
                    interface: interface1.identifier(false /* use_ifname */),
                    ip_version,
                    enable,
                }),
            }),
            &connector,
        );
        let ((), ()) = futures::future::try_join(do_if_fut, requests_fut.map(Ok))
            .await
            .expect("setting interface ip forwarding should succeed");

        let requests_fut = get_configuration_request(
            &mut requests,
            interface1.nicid,
            configuration_with_ip_forwarding_set(ip_version, enable),
        );
        let buf = writer::TestBuffers::default();
        let mut out = writer::JsonWriter::new_test(None, &buf);
        let do_if_fut = do_if(
            &mut out,
            opts::IfEnum::IpForward(opts::IfIpForward {
                cmd: opts::IfIpForwardEnum::Get(opts::IfIpForwardGet {
                    interface: interface1.identifier(false /* use_ifname */),
                    ip_version,
                }),
            }),
            &connector,
        );
        let ((), ()) = futures::future::try_join(do_if_fut, requests_fut.map(Ok))
            .await
            .expect("getting interface ip forwarding should succeed");
        let got_output = buf.into_stdout_str();
        pretty_assertions::assert_eq!(
            trim_whitespace_for_comparison(&got_output),
            trim_whitespace_for_comparison(&format!(
                "IP forwarding for {:?} is {} on interface {}",
                ip_version, enable, interface1.nicid
            )),
        )
    }

    async fn set_configuration_request<
        O: Debug + PartialEq,
        F: FnOnce(finterfaces_admin::Configuration) -> O,
    >(
        requests: &mut froot::InterfacesRequestStream,
        expected_nicid: u64,
        extract_config: F,
        expected_config: O,
    ) {
        let (id, control, _control_handle) = requests
            .next()
            .await
            .expect("root request stream not ended")
            .expect("root request stream not error")
            .into_get_admin()
            .expect("get admin request");
        assert_eq!(id, expected_nicid);

        let mut control: finterfaces_admin::ControlRequestStream = control.into_stream();
        let (configuration, responder) = control
            .next()
            .await
            .expect("control request stream not ended")
            .expect("control request stream not error")
            .into_set_configuration()
            .expect("set configuration request");
        assert_eq!(extract_config(configuration), expected_config);
        // net-cli does not check the returned configuration so we do not
        // return a populated one.
        responder.send(Ok(&Default::default())).expect("responder.send should succeed");
    }

    async fn get_configuration_request(
        requests: &mut froot::InterfacesRequestStream,
        expected_nicid: u64,
        config: finterfaces_admin::Configuration,
    ) {
        let (id, control, _control_handle) = requests
            .next()
            .await
            .expect("root request stream not ended")
            .expect("root request stream not error")
            .into_get_admin()
            .expect("get admin request");
        assert_eq!(id, expected_nicid);

        let mut control: finterfaces_admin::ControlRequestStream = control.into_stream();
        let responder = control
            .next()
            .await
            .expect("control request stream not ended")
            .expect("control request stream not error")
            .into_get_configuration()
            .expect("get configuration request");
        responder.send(Ok(&config)).expect("responder.send should succeed");
    }

    #[test_case(finterfaces_admin::IgmpVersion::V1)]
    #[test_case(finterfaces_admin::IgmpVersion::V2)]
    #[test_case(finterfaces_admin::IgmpVersion::V3)]
    #[fuchsia::test(logging = false)]
    async fn if_igmp(igmp_version: finterfaces_admin::IgmpVersion) {
        let client = flex_local::local_client_empty();

        let interface1 = TestInterface { nicid: 1, name: "interface1" };
        let (root_interfaces, mut requests) =
            client.create_proxy_and_stream::<froot::InterfacesMarker>();
        let connector =
            TestConnector { root_interfaces: Some(root_interfaces), ..Default::default() };

        let requests_fut = set_configuration_request(
            &mut requests,
            interface1.nicid,
            |c| extract_igmp_version(c).unwrap(),
            Some(igmp_version),
        );
        let buffers = writer::TestBuffers::default();
        let mut out = writer::JsonWriter::new_test(None, &buffers);
        let do_if_fut = do_if(
            &mut out,
            opts::IfEnum::Igmp(opts::IfIgmp {
                cmd: opts::IfIgmpEnum::Set(opts::IfIgmpSet {
                    interface: interface1.identifier(false /* use_ifname */),
                    version: Some(igmp_version),
                }),
            }),
            &connector,
        );
        let ((), ()) = futures::future::try_join(do_if_fut, requests_fut.map(Ok))
            .await
            .expect("setting interface IGMP configuration should succeed");

        let requests_fut = get_configuration_request(
            &mut requests,
            interface1.nicid,
            finterfaces_admin::Configuration {
                ipv4: Some(finterfaces_admin::Ipv4Configuration {
                    igmp: Some(finterfaces_admin::IgmpConfiguration {
                        version: Some(igmp_version),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        let buffers = writer::TestBuffers::default();
        let mut output_buf = writer::JsonWriter::new_test(None, &buffers);
        let do_if_fut = do_if(
            &mut output_buf,
            opts::IfEnum::Igmp(opts::IfIgmp {
                cmd: opts::IfIgmpEnum::Get(opts::IfIgmpGet {
                    interface: interface1.identifier(false /* use_ifname */),
                }),
            }),
            &connector,
        );
        let ((), ()) = futures::future::try_join(do_if_fut, requests_fut.map(Ok))
            .await
            .expect("getting interface IGMP configuration should succeed");
        let got_output = buffers.into_stdout_str();
        pretty_assertions::assert_eq!(
            trim_whitespace_for_comparison(&got_output),
            trim_whitespace_for_comparison(&format!(
                "IGMP configuration on interface {}:\n    Version: {:?}",
                interface1.nicid,
                Some(igmp_version),
            )),
        )
    }

    #[test_case(finterfaces_admin::MldVersion::V1)]
    #[test_case(finterfaces_admin::MldVersion::V2)]
    #[fuchsia::test(logging = false)]
    async fn if_mld(mld_version: finterfaces_admin::MldVersion) {
        let client = flex_local::local_client_empty();

        let interface1 = TestInterface { nicid: 1, name: "interface1" };
        let (root_interfaces, mut requests) =
            client.create_proxy_and_stream::<froot::InterfacesMarker>();
        let connector =
            TestConnector { root_interfaces: Some(root_interfaces), ..Default::default() };

        let requests_fut = set_configuration_request(
            &mut requests,
            interface1.nicid,
            |c| extract_mld_version(c).unwrap(),
            Some(mld_version),
        );
        let buffers = writer::TestBuffers::default();
        let mut out = writer::JsonWriter::new_test(None, &buffers);
        let do_if_fut = do_if(
            &mut out,
            opts::IfEnum::Mld(opts::IfMld {
                cmd: opts::IfMldEnum::Set(opts::IfMldSet {
                    interface: interface1.identifier(false /* use_ifname */),
                    version: Some(mld_version),
                }),
            }),
            &connector,
        );
        let ((), ()) = futures::future::try_join(do_if_fut, requests_fut.map(Ok))
            .await
            .expect("setting interface MLD configuration should succeed");

        let requests_fut = get_configuration_request(
            &mut requests,
            interface1.nicid,
            finterfaces_admin::Configuration {
                ipv6: Some(finterfaces_admin::Ipv6Configuration {
                    mld: Some(finterfaces_admin::MldConfiguration {
                        version: Some(mld_version),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        let buffers = writer::TestBuffers::default();
        let mut output_buf = writer::JsonWriter::new_test(None, &buffers);
        let do_if_fut = do_if(
            &mut output_buf,
            opts::IfEnum::Mld(opts::IfMld {
                cmd: opts::IfMldEnum::Get(opts::IfMldGet {
                    interface: interface1.identifier(false /* use_ifname */),
                }),
            }),
            &connector,
        );
        let ((), ()) = futures::future::try_join(do_if_fut, requests_fut.map(Ok))
            .await
            .expect("getting interface MLD configuration should succeed");
        let got_output = buffers.into_stdout_str();
        pretty_assertions::assert_eq!(
            trim_whitespace_for_comparison(&got_output),
            trim_whitespace_for_comparison(&format!(
                "MLD configuration on interface {}:\n    Version: {:?}",
                interface1.nicid,
                Some(mld_version),
            )),
        )
    }

    async fn always_answer_with_interfaces(
        interfaces_state_requests: finterfaces::StateRequestStream,
        interfaces: Vec<finterfaces::Properties>,
    ) {
        interfaces_state_requests
            .try_for_each(|request| {
                let interfaces = interfaces.clone();
                async move {
                    let (finterfaces::WatcherOptions { .. }, server_end, _): (
                        _,
                        _,
                        finterfaces::StateControlHandle,
                    ) = request.into_get_watcher().expect("request type should be GetWatcher");

                    let mut watcher_request_stream: finterfaces::WatcherRequestStream =
                        server_end.into_stream();

                    for event in interfaces
                        .into_iter()
                        .map(finterfaces::Event::Existing)
                        .chain(std::iter::once(finterfaces::Event::Idle(finterfaces::Empty)))
                    {
                        watcher_request_stream
                            .try_next()
                            .await
                            .expect("watcher watch FIDL error")
                            .expect("watcher request stream should not have ended")
                            .into_watch()
                            .expect("request should be of type Watch")
                            .send(&event)
                            .expect("responder.send should succeed");
                    }

                    assert_matches!(
                        watcher_request_stream.try_next().await.expect("watcher watch FIDL error"),
                        None,
                        "remaining watcher request stream should be empty"
                    );
                    Ok(())
                }
            })
            .await
            .expect("interfaces state FIDL error")
    }

    #[derive(Clone)]
    struct TestInterface {
        nicid: u64,
        name: &'static str,
    }

    impl TestInterface {
        fn identifier(&self, use_ifname: bool) -> opts::InterfaceIdentifier {
            let Self { nicid, name } = self;
            if use_ifname {
                opts::InterfaceIdentifier::Name(name.to_string())
            } else {
                opts::InterfaceIdentifier::Id(NonZeroU64::new(*nicid).unwrap())
            }
        }
    }

    #[test_case(true, false ; "when interface is up, and adding subnet route")]
    #[test_case(true, true ; "when interface is up, and not adding subnet route")]
    #[test_case(false, false ; "when interface is down, and adding subnet route")]
    #[test_case(false, true ; "when interface is down, and not adding subnet route")]
    #[fuchsia::test]
    async fn if_addr_add(interface_is_up: bool, no_subnet_route: bool) {
        let client = flex_local::local_client_empty();

        const TEST_PREFIX_LENGTH: u8 = 64;

        let interface1 = TestInterface { nicid: 1, name: "interface1" };
        let (root_interfaces, mut requests) =
            client.create_proxy_and_stream::<froot::InterfacesMarker>();

        let connector =
            TestConnector { root_interfaces: Some(root_interfaces), ..Default::default() };
        let buffers = writer::TestBuffers::default();
        let mut out = writer::JsonWriter::new_test(None, &buffers);
        let do_if_fut = do_if(
            &mut out,
            opts::IfEnum::Addr(opts::IfAddr {
                addr_cmd: opts::IfAddrEnum::Add(opts::IfAddrAdd {
                    interface: interface1.identifier(false /* use_ifname */),
                    addr: fnet_ext::IpAddress::from(IF_ADDR_V6.addr).to_string(),
                    prefix: TEST_PREFIX_LENGTH,
                    no_subnet_route,
                }),
            }),
            &connector,
        )
        .map(|res| res.expect("success"));

        let admin_fut = async {
            let (id, control, _control_handle) = requests
                .next()
                .await
                .expect("root request stream not ended")
                .expect("root request stream not error")
                .into_get_admin()
                .expect("get admin request");
            assert_eq!(id, interface1.nicid);

            let mut control: finterfaces_admin::ControlRequestStream = control.into_stream();
            let (
                addr,
                addr_params,
                address_state_provider_server_end,
                _admin_control_control_handle,
            ) = control
                .next()
                .await
                .expect("control request stream not ended")
                .expect("control request stream not error")
                .into_add_address()
                .expect("add address request");
            assert_eq!(addr, IF_ADDR_V6);
            assert_eq!(
                addr_params,
                finterfaces_admin::AddressParameters {
                    add_subnet_route: Some(!no_subnet_route),
                    ..Default::default()
                }
            );

            let mut address_state_provider_request_stream =
                address_state_provider_server_end.into_stream();
            async fn next_request(
                stream: &mut finterfaces_admin::AddressStateProviderRequestStream,
            ) -> finterfaces_admin::AddressStateProviderRequest {
                stream
                    .next()
                    .await
                    .expect("address state provider request stream not ended")
                    .expect("address state provider request stream not error")
            }

            let _address_state_provider_control_handle =
                next_request(&mut address_state_provider_request_stream)
                    .await
                    .into_detach()
                    .expect("detach request");

            for _ in 0..3 {
                next_request(&mut address_state_provider_request_stream)
                    .await
                    .into_watch_address_assignment_state()
                    .expect("watch address assignment state request")
                    .send(finterfaces::AddressAssignmentState::Tentative)
                    .expect("send address assignment state succeeds");
            }

            next_request(&mut address_state_provider_request_stream)
                .await
                .into_watch_address_assignment_state()
                .expect("watch address assignment state request")
                .send(if interface_is_up {
                    finterfaces::AddressAssignmentState::Assigned
                } else {
                    finterfaces::AddressAssignmentState::Unavailable
                })
                .expect("send address assignment state succeeds");
        };

        let ((), ()) = futures::join!(admin_fut, do_if_fut);
    }

    #[test_case(false ; "providing nicids")]
    #[test_case(true ; "providing interface names")]
    #[fuchsia::test]
    async fn if_del_addr(use_ifname: bool) {
        let client = flex_local::local_client_empty();

        let interface1 = TestInterface { nicid: 1, name: "interface1" };
        let interface2 = TestInterface { nicid: 2, name: "interface2" };

        let (root_interfaces, mut requests) =
            client.create_proxy_and_stream::<froot::InterfacesMarker>();
        let (interfaces_state, interfaces_requests) =
            client.create_proxy_and_stream::<finterfaces::StateMarker>();

        let (interface1_properties, _mac) = get_fake_interface(
            interface1.nicid,
            interface1.name,
            finterfaces_ext::PortClass::Ethernet,
            None,
        );

        let interfaces_fut =
            always_answer_with_interfaces(interfaces_requests, vec![interface1_properties.into()])
                .fuse();
        let mut interfaces_fut = pin!(interfaces_fut);

        let connector = TestConnector {
            root_interfaces: Some(root_interfaces),
            interfaces_state: Some(interfaces_state),
            ..Default::default()
        };

        let buffers = writer::TestBuffers::default();
        let mut out = writer::JsonWriter::new_test(None, &buffers);
        // Make the first request.
        let succeeds = do_if(
            &mut out,
            opts::IfEnum::Addr(opts::IfAddr {
                addr_cmd: opts::IfAddrEnum::Del(opts::IfAddrDel {
                    interface: interface1.identifier(use_ifname),
                    addr: fnet_ext::IpAddress::from(IF_ADDR_V4.addr).to_string(),
                    prefix: None, // The prefix should be set to the default of 32 for IPv4.
                }),
            }),
            &connector,
        )
        .map(|res| res.expect("success"));
        let handler_fut = async {
            let (id, control, _control_handle) = requests
                .next()
                .await
                .expect("root request stream not ended")
                .expect("root request stream not error")
                .into_get_admin()
                .expect("get admin request");
            assert_eq!(id, interface1.nicid);
            let mut control = control.into_stream();
            let (addr, responder) = control
                .next()
                .await
                .expect("control request stream not ended")
                .expect("control request stream not error")
                .into_remove_address()
                .expect("del address request");
            assert_eq!(addr, IF_ADDR_V4);
            responder.send(Ok(true)).expect("responder send");
        };

        futures::select! {
            () = interfaces_fut => panic!("interfaces_fut should never complete"),
            ((), ()) = futures::future::join(handler_fut, succeeds).fuse() => {},
        }

        let buffers = writer::TestBuffers::default();
        let mut out = writer::JsonWriter::new_test(None, &buffers);
        // Make the second request.
        let fails = do_if(
            &mut out,
            opts::IfEnum::Addr(opts::IfAddr {
                addr_cmd: opts::IfAddrEnum::Del(opts::IfAddrDel {
                    interface: interface2.identifier(use_ifname),
                    addr: fnet_ext::IpAddress::from(IF_ADDR_V6.addr).to_string(),
                    prefix: Some(IF_ADDR_V6.prefix_len),
                }),
            }),
            &connector,
        )
        .map(|res| res.expect_err("failure"));

        if use_ifname {
            // The caller will have failed to find an interface matching the name,
            // so we don't expect any requests to make it to us.
            futures::select! {
                () = interfaces_fut => panic!("interfaces_fut should never complete"),
                e = fails.fuse() => {
                    assert_eq!(e.to_string(), format!("No interface with name {}", interface2.name));
                },
            }
        } else {
            let handler_fut = async {
                let (id, control, _control_handle) = requests
                    .next()
                    .await
                    .expect("root request stream not ended")
                    .expect("root request stream not error")
                    .into_get_admin()
                    .expect("get admin request");
                assert_eq!(id, interface2.nicid);
                let mut control = control.into_stream();
                let (addr, responder) = control
                    .next()
                    .await
                    .expect("control request stream not ended")
                    .expect("control request stream not error")
                    .into_remove_address()
                    .expect("del address request");
                assert_eq!(addr, IF_ADDR_V6);
                responder.send(Ok(false)).expect("responder send");
            };
            futures::select! {
                () = interfaces_fut => panic!("interfaces_fut should never complete"),
                ((), e) = futures::future::join(handler_fut, fails).fuse() => {
                    let fnet_ext::IpAddress(addr) = IF_ADDR_V6.addr.into();
                    assert_eq!(e.to_string(), format!("Address {} not found on interface {}", addr, interface2.nicid));
                },
            }
        }
    }

    const INTERFACE_NAME: &str = "if1";

    fn interface_properties(
        addrs: Vec<(fnet::Subnet, finterfaces::AddressAssignmentState)>,
    ) -> finterfaces::Properties {
        finterfaces_ext::Properties {
            id: INTERFACE_ID.try_into().unwrap(),
            name: INTERFACE_NAME.to_string(),
            port_class: finterfaces_ext::PortClass::Ethernet,
            online: true,
            addresses: addrs
                .into_iter()
                .map(|(addr, assignment_state)| finterfaces_ext::Address::<
                    finterfaces_ext::AllInterest,
                > {
                    addr,
                    assignment_state,
                    valid_until: finterfaces_ext::PositiveMonotonicInstant::INFINITE_FUTURE,
                    preferred_lifetime_info:
                        finterfaces_ext::PreferredLifetimeInfo::preferred_forever(),
                })
                .collect(),
            has_default_ipv4_route: false,
            has_default_ipv6_route: false,
            port_identity_koid: None,
        }
        .into()
    }

    #[test_case(
        false,
        vec![
            finterfaces::Event::Existing(interface_properties(vec![])),
            finterfaces::Event::Idle(finterfaces::Empty),
            finterfaces::Event::Changed(interface_properties(vec![
                (fidl_subnet!("192.168.0.1/32"), finterfaces::AddressAssignmentState::Assigned)
            ])),
        ],
        "192.168.0.1";
        "wait for an address to be assigned"
    )]
    #[test_case(
        false,
        vec![
            finterfaces::Event::Existing(interface_properties(vec![
                (fidl_subnet!("192.168.0.1/32"), finterfaces::AddressAssignmentState::Assigned),
                (fidl_subnet!("fd00::1/64"), finterfaces::AddressAssignmentState::Assigned),
            ])),
        ],
        "192.168.0.1";
        "prefer first when any address requested"
    )]
    #[test_case(
        true,
        vec![
            finterfaces::Event::Existing(interface_properties(vec![
                (fidl_subnet!("192.168.0.1/32"), finterfaces::AddressAssignmentState::Assigned)
            ])),
            finterfaces::Event::Idle(finterfaces::Empty),
            finterfaces::Event::Changed(interface_properties(vec![
                (fidl_subnet!("fd00::1/64"), finterfaces::AddressAssignmentState::Assigned)
            ])),
        ],
        "fd00::1";
        "wait for IPv6 when IPv6 address requested"
    )]
    #[fuchsia::test]
    async fn if_addr_wait(ipv6: bool, events: Vec<finterfaces::Event>, expected_output: &str) {
        let client = flex_local::local_client_empty();

        let interface = TestInterface { nicid: INTERFACE_ID, name: INTERFACE_NAME };

        let (interfaces_state, mut request_stream) =
            client.create_proxy_and_stream::<finterfaces::StateMarker>();

        let interfaces_handler = async move {
            let (finterfaces::WatcherOptions { include_non_assigned_addresses, .. }, server_end, _) =
                request_stream
                    .next()
                    .await
                    .expect("should call state")
                    .expect("should succeed")
                    .into_get_watcher()
                    .expect("request should be GetWatcher");
            assert_eq!(include_non_assigned_addresses, Some(false));
            let mut request_stream: finterfaces::WatcherRequestStream = server_end.into_stream();
            for event in events {
                request_stream
                    .next()
                    .await
                    .expect("should call watcher")
                    .expect("should succeed")
                    .into_watch()
                    .expect("request should be Watch")
                    .send(&event)
                    .expect("send response");
            }
        };

        let connector =
            TestConnector { interfaces_state: Some(interfaces_state), ..Default::default() };
        let buffers = writer::TestBuffers::default();
        let mut out = writer::JsonWriter::new_test(None, &buffers);
        let run_command = do_if(
            &mut out,
            opts::IfEnum::Addr(opts::IfAddr {
                addr_cmd: opts::IfAddrEnum::Wait(opts::IfAddrWait {
                    interface: interface.identifier(false),
                    ipv6,
                }),
            }),
            &connector,
        )
        .map(|r| r.expect("command should succeed"));

        let ((), ()) = futures::future::join(interfaces_handler, run_command).await;

        let output = buffers.into_stdout_str();
        pretty_assertions::assert_eq!(
            trim_whitespace_for_comparison(&output),
            trim_whitespace_for_comparison(expected_output),
        );
    }

    fn wanted_net_if_list_json() -> String {
        json!([
            {
                "addresses": {
                    "ipv4": [],
                    "ipv6": [],
                },
                "device_class": "Loopback",
                "mac": "00:00:00:00:00:00",
                "name": "lo",
                "nicid": 1,
                "online": true,
                "has_default_ipv4_route": false,
                "has_default_ipv6_route": false,
                "port_identity_koid": null,
            },
            {
                "addresses": {
                    "ipv4": [],
                    "ipv6": [],
                },
                "device_class": "Ethernet",
                "mac": "01:02:03:04:05:06",
                "name": "eth001",
                "nicid": 10,
                "online": true,
                "has_default_ipv4_route": false,
                "has_default_ipv6_route": false,
                "port_identity_koid": 10,
            },
            {
                "addresses": {
                    "ipv4": [],
                    "ipv6": [],
                },
                "device_class": "Virtual",
                "mac": null,
                "name": "virt001",
                "nicid": 20,
                "online": true,
                "has_default_ipv4_route": false,
                "has_default_ipv6_route": false,
                "port_identity_koid": 20,
            },
            {
                "addresses": {
                    "ipv4": [
                        {
                            "addr": "192.168.0.1",
                            "assignment_state": "Tentative",
                            "prefix_len": 24,
                            "valid_until": 2500000000_u64,
                        }
                    ],
                    "ipv6": [],
                },
                "device_class": "Ethernet",
                "mac": null,
                "name": "eth002",
                "nicid": 30,
                "online": true,
                "has_default_ipv4_route": false,
                "has_default_ipv6_route": true,
                "port_identity_koid": null,
            },
            {
                "addresses": {
                    "ipv4": [],
                    "ipv6": [{
                        "addr": "2001:db8::1",
                        "assignment_state": "Unavailable",
                        "prefix_len": 64,
                        "valid_until": null,
                    }],
                },
                "device_class": "Ethernet",
                "mac": null,
                "name": "eth003",
                "nicid": 40,
                "online": true,
                "has_default_ipv4_route": true,
                "has_default_ipv6_route": true,
                "port_identity_koid": null,
            },
        ])
        .to_string()
    }

    fn wanted_net_if_list_tabular() -> String {
        String::from(
            r#"
nicid                 1
name                  lo
device class          loopback
online                true
default routes        -
mac                   00:00:00:00:00:00
port_identity_koid    -

nicid                 10
name                  eth001
device class          ethernet
online                true
default routes        -
mac                   01:02:03:04:05:06
port_identity_koid    10

nicid                 20
name                  virt001
device class          virtual
online                true
default routes        -
mac                   -
port_identity_koid    20

nicid                 30
name                  eth002
device class          ethernet
online                true
default routes        IPv6
addr                  192.168.0.1/24       TENTATIVE valid until [2.5s]
mac                   -
port_identity_koid    -

nicid                 40
name                  eth003
device class          ethernet
online                true
default routes        IPv4,IPv6
addr                  2001:db8::1/64       UNAVAILABLE
mac                   -
port_identity_koid    -
"#,
        )
    }

    #[test_case(true, wanted_net_if_list_json() ; "in json format")]
    #[test_case(false, wanted_net_if_list_tabular() ; "in tabular format")]
    #[fuchsia::test]
    async fn if_list(json: bool, wanted_output: String) {
        let client = flex_local::local_client_empty();

        let (root_interfaces, root_interfaces_stream) =
            client.create_proxy_and_stream::<froot::InterfacesMarker>();
        let (interfaces_state, interfaces_state_stream) =
            client.create_proxy_and_stream::<finterfaces::StateMarker>();

        let buffers = writer::TestBuffers::default();
        let mut output = if json {
            writer::JsonWriter::new_test(Some(writer::Format::Json), &buffers)
        } else {
            writer::JsonWriter::new_test(None, &buffers)
        };
        let output_ref = &mut output;

        let do_if_fut = async {
            let connector = TestConnector {
                root_interfaces: Some(root_interfaces),
                interfaces_state: Some(interfaces_state),
                ..Default::default()
            };
            do_if(output_ref, opts::IfEnum::List(opts::IfList { name_pattern: None }), &connector)
                .map(|res| res.expect("if list"))
                .await
        };
        let watcher_stream = interfaces_state_stream
            .and_then(|req| match req {
                finterfaces::StateRequest::GetWatcher {
                    options: _,
                    watcher,
                    control_handle: _,
                } => futures::future::ready(Ok(watcher.into_stream())),
            })
            .try_flatten()
            .map(|res| res.expect("watcher stream error"));
        let (interfaces, mac_addresses): (Vec<_>, HashMap<_, _>) = [
            get_fake_interface(
                1,
                "lo",
                finterfaces_ext::PortClass::Loopback,
                Some([0, 0, 0, 0, 0, 0]),
            ),
            get_fake_interface(
                10,
                "eth001",
                finterfaces_ext::PortClass::Ethernet,
                Some([1, 2, 3, 4, 5, 6]),
            ),
            get_fake_interface(20, "virt001", finterfaces_ext::PortClass::Virtual, None),
            (
                finterfaces_ext::Properties {
                    id: 30.try_into().unwrap(),
                    name: "eth002".to_string(),
                    port_class: finterfaces_ext::PortClass::Ethernet,
                    online: true,
                    addresses: vec![finterfaces_ext::Address {
                        addr: fidl_subnet!("192.168.0.1/24"),
                        valid_until: i64::try_from(
                            std::time::Duration::from_millis(2500).as_nanos(),
                        )
                        .unwrap()
                        .try_into()
                        .unwrap(),
                        assignment_state: finterfaces::AddressAssignmentState::Tentative,
                        preferred_lifetime_info:
                            finterfaces_ext::PreferredLifetimeInfo::preferred_forever(),
                    }],
                    has_default_ipv4_route: false,
                    has_default_ipv6_route: true,
                    port_identity_koid: None,
                },
                None,
            ),
            (
                finterfaces_ext::Properties {
                    id: 40.try_into().unwrap(),
                    name: "eth003".to_string(),
                    port_class: finterfaces_ext::PortClass::Ethernet,
                    online: true,
                    addresses: vec![finterfaces_ext::Address {
                        addr: fidl_subnet!("2001:db8::1/64"),
                        valid_until: finterfaces_ext::PositiveMonotonicInstant::INFINITE_FUTURE,
                        assignment_state: finterfaces::AddressAssignmentState::Unavailable,
                        preferred_lifetime_info:
                            finterfaces_ext::PreferredLifetimeInfo::preferred_forever(),
                    }],
                    has_default_ipv4_route: true,
                    has_default_ipv6_route: true,
                    port_identity_koid: None,
                },
                None,
            ),
        ]
        .into_iter()
        .map(|(properties, mac)| {
            let finterfaces_ext::Properties { id, .. } = &properties;
            let id = *id;
            (properties, (id, mac))
        })
        .unzip();
        let interfaces =
            futures::stream::iter(interfaces.into_iter().map(Some).chain(std::iter::once(None)));
        let watcher_fut = watcher_stream.zip(interfaces).for_each(|(req, properties)| match req {
            finterfaces::WatcherRequest::Watch { responder } => {
                let event = properties.map_or(
                    finterfaces::Event::Idle(finterfaces::Empty),
                    |finterfaces_ext::Properties {
                         id,
                         name,
                         port_class,
                         online,
                         addresses,
                         has_default_ipv4_route,
                         has_default_ipv6_route,
                         port_identity_koid,
                     }| {
                        finterfaces::Event::Existing(finterfaces::Properties {
                            id: Some(id.get()),
                            name: Some(name),
                            port_class: Some(port_class.into()),
                            online: Some(online),
                            addresses: Some(
                                addresses.into_iter().map(finterfaces::Address::from).collect(),
                            ),
                            has_default_ipv4_route: Some(has_default_ipv4_route),
                            has_default_ipv6_route: Some(has_default_ipv6_route),
                            port_identity_koid: port_identity_koid.map(|p| p.raw_koid()),
                            ..Default::default()
                        })
                    },
                );
                responder.send(&event).expect("send watcher event");
                futures::future::ready(())
            }
        });
        let root_fut = root_interfaces_stream
            .map(|res| res.expect("root interfaces stream error"))
            .for_each_concurrent(None, |req| {
                let (id, responder) = req.into_get_mac().expect("get_mac request");
                responder
                    .send(
                        mac_addresses
                            .get(&id.try_into().unwrap())
                            .map(Option::as_ref)
                            .ok_or(froot::InterfacesGetMacError::NotFound),
                    )
                    .expect("send get_mac response");
                futures::future::ready(())
            });
        let ((), (), ()) = futures::future::join3(do_if_fut, watcher_fut, root_fut).await;

        let got_output = buffers.into_stdout_str();

        if json {
            let got: Value = serde_json::from_str(&got_output).unwrap();
            let want: Value = serde_json::from_str(&wanted_output).unwrap();
            pretty_assertions::assert_eq!(got, want);
        } else {
            pretty_assertions::assert_eq!(
                trim_whitespace_for_comparison(&got_output),
                trim_whitespace_for_comparison(&wanted_output),
            );
        }
    }

    async fn test_do_dhcp(client: flex_client::ClientArg, cmd: opts::DhcpEnum) {
        let (stack, mut requests) = client.create_proxy_and_stream::<fstack::StackMarker>();
        let connector = TestConnector { stack: Some(stack), ..Default::default() };
        let op = do_dhcp(cmd.clone(), &connector);
        let op_succeeds = async move {
            let (expected_id, expected_enable) = match cmd {
                opts::DhcpEnum::Start(opts::DhcpStart { interface }) => (interface, true),
                opts::DhcpEnum::Stop(opts::DhcpStop { interface }) => (interface, false),
            };
            let request = requests
                .try_next()
                .await
                .expect("start FIDL error")
                .expect("request stream should not have ended");
            let (received_id, enable, responder) = request
                .into_set_dhcp_client_enabled()
                .expect("request should be of type StopDhcpClient");
            assert_eq!(
                opts::InterfaceIdentifier::Id(NonZeroU64::new(u64::from(received_id)).unwrap()),
                expected_id
            );
            assert_eq!(enable, expected_enable);
            responder.send(Ok(())).map_err(anyhow::Error::new)
        };
        let ((), ()) =
            futures::future::try_join(op, op_succeeds).await.expect("dhcp command should succeed");
    }

    #[fuchsia::test]
    async fn dhcp_start() {
        let client = flex_local::local_client_empty();

        test_do_dhcp(
            client,
            opts::DhcpEnum::Start(opts::DhcpStart {
                interface: NonZeroU64::new(1).unwrap().into(),
            }),
        )
        .await;
    }

    #[fuchsia::test]
    async fn dhcp_stop() {
        let client = flex_local::local_client_empty();

        test_do_dhcp(
            client,
            opts::DhcpEnum::Stop(opts::DhcpStop { interface: NonZeroU64::new(1).unwrap().into() }),
        )
        .await;
    }

    async fn test_modify_route(client: flex_client::ClientArg, cmd: opts::RouteEnum) {
        let event_token = client.create_event();
        let expected_interface = match &cmd {
            opts::RouteEnum::List(_) => panic!("test_modify_route should not take a List command"),
            opts::RouteEnum::Add(opts::RouteAdd { interface, .. }) => interface,
            opts::RouteEnum::Del(opts::RouteDel { interface, .. }) => interface,
        }
        .clone();
        let expected_id = match expected_interface {
            opts::InterfaceIdentifier::Id(ref id) => *id,
            opts::InterfaceIdentifier::Name(_) => {
                panic!("expected test to work only with ids")
            }
        };

        let (root_interfaces, mut interfaces_requests) =
            client.create_proxy_and_stream::<froot::InterfacesMarker>();
        let (routes_v4, mut routes_v4_requests) =
            client.create_proxy_and_stream::<froot::RoutesV4Marker>();
        let (routes_v6, _routes_v6_requests) =
            client.create_proxy_and_stream::<froot::RoutesV6Marker>();

        let connector = TestConnector {
            root_interfaces: Some(root_interfaces),
            root_routes_v4: Some(routes_v4),
            root_routes_v6: Some(routes_v6),
            ..Default::default()
        };
        let buffers = writer::TestBuffers::default();
        let mut out = writer::JsonWriter::new_test(None, &buffers);
        let op = do_route(&mut out, cmd.clone(), &connector);

        let op_succeeds = async move {
            // Handle get_interface_authorization_proof.
            let (nic_id, control_server, _get_admin_responder) = interfaces_requests
                .try_next()
                .await
                .expect("get_admin FIDL error")
                .expect("request stream should not have ended")
                .into_get_admin()
                .expect("request should be get_admin");
            assert_eq!(nic_id, expected_id.get());

            let mut control_stream = control_server.into_stream();
            let auth_responder = control_stream
                .try_next()
                .await
                .expect("get_authorization_for_interface FIDL error")
                .expect("request stream should not have ended")
                .into_get_authorization_for_interface()
                .expect("request should be get_authorization_for_interface");

            let grant = fnet_resources::GrantForInterfaceAuthorization {
                interface_id: nic_id,
                token: event_token,
            };
            auth_responder.send(grant).expect("failed to send grant");

            // Handle RouteSet API calls.
            match cmd {
                opts::RouteEnum::List(_) => {
                    panic!("test_modify_route should not take a List command")
                }
                opts::RouteEnum::Add(route) => match route.destination {
                    std::net::IpAddr::V4(addr) => {
                        let (route_set_server, _routes_responder) = routes_v4_requests
                            .try_next()
                            .await
                            .expect("global_route_set FIDL error")
                            .expect("request stream should not have ended")
                            .into_global_route_set()
                            .expect("request should be global_route_set");

                        let mut route_set_stream = route_set_server.into_stream();

                        let (proof, auth_responder) = route_set_stream
                            .try_next()
                            .await
                            .expect("authenticate FIDL error")
                            .expect("request stream should not have ended")
                            .into_authenticate_for_interface()
                            .expect("request should be authenticate");
                        assert_eq!(proof.interface_id, nic_id);
                        auth_responder.send(Ok(())).expect("failed to send authenticate response");

                        let (route_v4, add_responder) = route_set_stream
                            .try_next()
                            .await
                            .expect("add_route FIDL error")
                            .expect("request stream should not have ended")
                            .into_add_route()
                            .expect("request should be add_route");
                        assert_eq!(route_v4.destination.addr.addr, addr.octets());
                        assert_eq!(route_v4.destination.prefix_len, route.prefix_len);
                        assert_eq!(
                            route_v4.action,
                            froutes::RouteActionV4::Forward(froutes::RouteTargetV4 {
                                outbound_interface: nic_id,
                                next_hop: None,
                            })
                        );
                        add_responder.send(Ok(true)).expect("failed to send add_route response");
                    }
                    _ => panic!("test only mocks IPv4 currently"),
                },
                opts::RouteEnum::Del(route) => match route.destination {
                    std::net::IpAddr::V4(addr) => {
                        let (route_set_server, _routes_responder) = routes_v4_requests
                            .try_next()
                            .await
                            .expect("global_route_set FIDL error")
                            .expect("request stream should not have ended")
                            .into_global_route_set()
                            .expect("request should be global_route_set");

                        let mut route_set_stream = route_set_server.into_stream();

                        let (proof, auth_responder) = route_set_stream
                            .try_next()
                            .await
                            .expect("authenticate FIDL error")
                            .expect("request stream should not have ended")
                            .into_authenticate_for_interface()
                            .expect("request should be authenticate");
                        assert_eq!(proof.interface_id, nic_id);
                        auth_responder.send(Ok(())).expect("failed to send authenticate response");

                        let (route_v4, del_responder) = route_set_stream
                            .try_next()
                            .await
                            .expect("remove_route FIDL error")
                            .expect("request stream should not have ended")
                            .into_remove_route()
                            .expect("request should be remove_route");
                        assert_eq!(route_v4.destination.addr.addr, addr.octets());
                        assert_eq!(route_v4.destination.prefix_len, route.prefix_len);
                        assert_eq!(
                            route_v4.action,
                            froutes::RouteActionV4::Forward(froutes::RouteTargetV4 {
                                outbound_interface: nic_id,
                                next_hop: None,
                            })
                        );
                        del_responder.send(Ok(true)).expect("failed to send remove_route response");
                    }
                    _ => panic!("test only mocks IPv4 currently"),
                },
            }
            Ok(())
        };
        let ((), ()) =
            futures::future::try_join(op, op_succeeds).await.expect("dhcp command should succeed");
    }

    #[fuchsia::test]
    async fn route_add() {
        let client = flex_local::local_client_empty();

        // Test arguments have been arbitrarily selected.
        test_modify_route(
            client,
            opts::RouteEnum::Add(opts::RouteAdd {
                destination: std::net::IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 1, 0)),
                prefix_len: 24,
                gateway: None,
                interface: opts::InterfaceIdentifier::Id(NonZeroU64::new(2).unwrap()),
                metric: Some(100),
            }),
        )
        .await;
    }

    #[fuchsia::test]
    async fn route_del() {
        let client = flex_local::local_client_empty();

        // Test arguments have been arbitrarily selected.
        test_modify_route(
            client,
            opts::RouteEnum::Del(opts::RouteDel {
                destination: std::net::IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 1, 0)),
                prefix_len: 24,
                gateway: None,
                interface: opts::InterfaceIdentifier::Id(NonZeroU64::new(2).unwrap()),
                metric: Some(100),
            }),
        )
        .await;
    }

    fn wanted_route_list_json() -> String {
        json!([
            {
                "destination":{"addr":"0.0.0.0","prefix_len":0},
                "gateway":"127.0.0.1",
                "metric":4,
                "nicid":3,
                "table_id":0,
            },
            {
                "destination":{"addr":"1.1.1.0","prefix_len":24},
                "gateway":"1.1.1.2",
                "metric":4,
                "nicid":3,
                "table_id":0,
            },
            {
                "destination":{"addr":"10.10.10.0","prefix_len":24},
                "gateway":"10.10.10.20",
                "metric":40,
                "nicid":30,
                "table_id":1,
            },
            {
                "destination":{"addr":"11.11.11.0","prefix_len":28},
                "gateway":null,
                "metric":40,
                "nicid":30,
                "table_id":1,
            },
            {
                "destination":{"addr":"ff00::","prefix_len":8},
                "gateway":null,
                "metric":400,
                "nicid":300,
                "table_id":2,
            },
            {
                "destination":{"addr":"fe80::","prefix_len":64},
                "gateway":null,
                "metric":400,
                "nicid":300,
                "table_id":2,
            },
        ])
        .to_string()
    }

    fn wanted_route_list_tabular() -> String {
        "Destination      Gateway        NICID    Metric    TableId
         0.0.0.0/0        127.0.0.1      3        4         0
         1.1.1.0/24       1.1.1.2        3        4         0
         10.10.10.0/24    10.10.10.20    30       40        1
         11.11.11.0/28    -              30       40        1
         ff00::/8         -              300      400       2
         fe80::/64        -              300      400       2
         "
        .to_string()
    }

    #[test_case(true, wanted_route_list_json() ; "in json format")]
    #[test_case(false, wanted_route_list_tabular() ; "in tabular format")]
    #[fuchsia::test]
    async fn route_list(json: bool, wanted_output: String) {
        let client = flex_local::local_client_empty();

        let (routes_v4_controller, mut routes_v4_state_stream) =
            client.create_proxy_and_stream::<froutes::StateV4Marker>();
        let (routes_v6_controller, mut routes_v6_state_stream) =
            client.create_proxy_and_stream::<froutes::StateV6Marker>();
        let connector = TestConnector {
            routes_v4: Some(routes_v4_controller),
            routes_v6: Some(routes_v6_controller),
            ..Default::default()
        };

        let buffers = writer::TestBuffers::default();
        let mut output = if json {
            writer::JsonWriter::new_test(Some(writer::Format::Json), &buffers)
        } else {
            writer::JsonWriter::new_test(None, &buffers)
        };

        let do_route_fut =
            do_route(&mut output, opts::RouteEnum::List(opts::RouteList {}), &connector);

        let v4_route_events = vec![
            froutes::EventV4::Existing(froutes::InstalledRouteV4 {
                route: Some(froutes::RouteV4 {
                    destination: net_declare::fidl_ip_v4_with_prefix!("1.1.1.0/24"),
                    action: froutes::RouteActionV4::Forward(froutes::RouteTargetV4 {
                        outbound_interface: 3,
                        next_hop: Some(Box::new(net_declare::fidl_ip_v4!("1.1.1.2"))),
                    }),
                    properties: froutes::RoutePropertiesV4 {
                        specified_properties: Some(froutes::SpecifiedRouteProperties {
                            metric: Some(froutes::SpecifiedMetric::ExplicitMetric(4)),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                }),
                effective_properties: Some(froutes::EffectiveRouteProperties {
                    metric: Some(4),
                    ..Default::default()
                }),
                table_id: Some(0),
                ..Default::default()
            }),
            froutes::EventV4::Existing(froutes::InstalledRouteV4 {
                route: Some(froutes::RouteV4 {
                    destination: net_declare::fidl_ip_v4_with_prefix!("10.10.10.0/24"),
                    action: froutes::RouteActionV4::Forward(froutes::RouteTargetV4 {
                        outbound_interface: 30,
                        next_hop: Some(Box::new(net_declare::fidl_ip_v4!("10.10.10.20"))),
                    }),
                    properties: froutes::RoutePropertiesV4 {
                        specified_properties: Some(froutes::SpecifiedRouteProperties {
                            metric: Some(froutes::SpecifiedMetric::ExplicitMetric(40)),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                }),
                effective_properties: Some(froutes::EffectiveRouteProperties {
                    metric: Some(40),
                    ..Default::default()
                }),
                table_id: Some(1),
                ..Default::default()
            }),
            froutes::EventV4::Existing(froutes::InstalledRouteV4 {
                route: Some(froutes::RouteV4 {
                    destination: net_declare::fidl_ip_v4_with_prefix!("0.0.0.0/0"),
                    action: froutes::RouteActionV4::Forward(froutes::RouteTargetV4 {
                        outbound_interface: 3,
                        next_hop: Some(Box::new(net_declare::fidl_ip_v4!("127.0.0.1"))),
                    }),
                    properties: froutes::RoutePropertiesV4 {
                        specified_properties: Some(froutes::SpecifiedRouteProperties {
                            metric: Some(froutes::SpecifiedMetric::ExplicitMetric(4)),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                }),
                effective_properties: Some(froutes::EffectiveRouteProperties {
                    metric: Some(4),
                    ..Default::default()
                }),
                table_id: Some(0),
                ..Default::default()
            }),
            froutes::EventV4::Existing(froutes::InstalledRouteV4 {
                route: Some(froutes::RouteV4 {
                    destination: net_declare::fidl_ip_v4_with_prefix!("11.11.11.0/28"),
                    action: froutes::RouteActionV4::Forward(froutes::RouteTargetV4 {
                        outbound_interface: 30,
                        next_hop: None,
                    }),
                    properties: froutes::RoutePropertiesV4 {
                        specified_properties: Some(froutes::SpecifiedRouteProperties {
                            metric: Some(froutes::SpecifiedMetric::ExplicitMetric(40)),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                }),
                effective_properties: Some(froutes::EffectiveRouteProperties {
                    metric: Some(40),
                    ..Default::default()
                }),
                table_id: Some(1),
                ..Default::default()
            }),
            froutes::EventV4::Idle(froutes::Empty),
        ];
        let v6_route_events = vec![
            froutes::EventV6::Existing(froutes::InstalledRouteV6 {
                route: Some(froutes::RouteV6 {
                    destination: net_declare::fidl_ip_v6_with_prefix!("fe80::/64"),
                    action: froutes::RouteActionV6::Forward(froutes::RouteTargetV6 {
                        outbound_interface: 300,
                        next_hop: None,
                    }),
                    properties: froutes::RoutePropertiesV6 {
                        specified_properties: Some(froutes::SpecifiedRouteProperties {
                            metric: Some(froutes::SpecifiedMetric::ExplicitMetric(400)),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                }),
                effective_properties: Some(froutes::EffectiveRouteProperties {
                    metric: Some(400),
                    ..Default::default()
                }),
                table_id: Some(2),
                ..Default::default()
            }),
            froutes::EventV6::Existing(froutes::InstalledRouteV6 {
                route: Some(froutes::RouteV6 {
                    destination: net_declare::fidl_ip_v6_with_prefix!("ff00::/8"),
                    action: froutes::RouteActionV6::Forward(froutes::RouteTargetV6 {
                        outbound_interface: 300,
                        next_hop: None,
                    }),
                    properties: froutes::RoutePropertiesV6 {
                        specified_properties: Some(froutes::SpecifiedRouteProperties {
                            metric: Some(froutes::SpecifiedMetric::ExplicitMetric(400)),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                }),
                effective_properties: Some(froutes::EffectiveRouteProperties {
                    metric: Some(400),
                    ..Default::default()
                }),
                table_id: Some(2),
                ..Default::default()
            }),
            froutes::EventV6::Idle(froutes::Empty),
        ];

        let route_v4_fut = routes_v4_state_stream.select_next_some().then(|request| {
            froutes_ext::testutil::serve_state_request::<Ipv4>(
                request,
                futures::stream::once(futures::future::ready(v4_route_events)),
            )
        });
        let route_v6_fut = routes_v6_state_stream.select_next_some().then(|request| {
            froutes_ext::testutil::serve_state_request::<Ipv6>(
                request,
                futures::stream::once(futures::future::ready(v6_route_events)),
            )
        });

        let ((), (), ()) =
            futures::try_join!(do_route_fut, route_v4_fut.map(Ok), route_v6_fut.map(Ok))
                .expect("listing forwarding table entries should succeed");

        let got_output = buffers.into_stdout_str();

        if json {
            let got: Value = serde_json::from_str(&got_output).unwrap();
            let want: Value = serde_json::from_str(&wanted_output).unwrap();
            pretty_assertions::assert_eq!(got, want);
        } else {
            pretty_assertions::assert_eq!(
                trim_whitespace_for_comparison(&got_output),
                trim_whitespace_for_comparison(&wanted_output),
            );
        }
    }

    #[test_case(false ; "providing nicids")]
    #[test_case(true ; "providing interface names")]
    #[fuchsia::test]
    async fn bridge(use_ifname: bool) {
        let client = flex_local::local_client_empty();

        let (stack, mut stack_requests) = client.create_proxy_and_stream::<fstack::StackMarker>();
        let (interfaces_state, interfaces_state_requests) =
            client.create_proxy_and_stream::<finterfaces::StateMarker>();
        let connector = TestConnector {
            interfaces_state: Some(interfaces_state),
            stack: Some(stack),
            ..Default::default()
        };

        let bridge_ifs = vec![
            TestInterface { nicid: 1, name: "interface1" },
            TestInterface { nicid: 2, name: "interface2" },
            TestInterface { nicid: 3, name: "interface3" },
        ];

        let interface_fidls = bridge_ifs
            .iter()
            .map(|interface| {
                let (interface, _mac) = get_fake_interface(
                    interface.nicid,
                    interface.name,
                    finterfaces_ext::PortClass::Ethernet,
                    None,
                );
                interface.into()
            })
            .collect::<Vec<_>>();

        let interfaces_fut =
            always_answer_with_interfaces(interfaces_state_requests, interface_fidls);

        let bridge_id = 4;
        let buffers = writer::TestBuffers::default();
        let mut out = writer::JsonWriter::new_test(None, &buffers);
        let bridge = do_if(
            &mut out,
            opts::IfEnum::Bridge(opts::IfBridge {
                interfaces: bridge_ifs
                    .iter()
                    .map(|interface| interface.identifier(use_ifname))
                    .collect(),
            }),
            &connector,
        );

        let bridge_succeeds = async move {
            let (requested_ifs, bridge_server_end, _control_handle) = stack_requests
                .try_next()
                .await
                .expect("stack requests FIDL error")
                .expect("request stream should not have ended")
                .into_bridge_interfaces()
                .expect("request should be of type BridgeInterfaces");
            assert_eq!(
                requested_ifs,
                bridge_ifs.iter().map(|interface| interface.nicid).collect::<Vec<_>>()
            );
            let mut bridge_requests = bridge_server_end.into_stream();
            let responder = bridge_requests
                .try_next()
                .await
                .expect("bridge requests FIDL error")
                .expect("request stream should not have ended")
                .into_get_id()
                .expect("request should be get_id");
            responder.send(bridge_id).expect("responding with bridge ID should succeed");
            let _control_handle = bridge_requests
                .try_next()
                .await
                .expect("bridge requests FIDL error")
                .expect("request stream should not have ended")
                .into_detach()
                .expect("request should be detach");
            Ok(())
        };
        futures::select! {
            () = interfaces_fut.fuse() => panic!("interfaces_fut should never complete"),
            result = futures::future::try_join(bridge, bridge_succeeds).fuse() => {
                let ((), ()) = result.expect("if bridge should succeed");
            }
        }
    }

    async fn test_get_neigh_entries(
        watch_for_changes: bool,
        batches: Vec<Vec<fneighbor::EntryIteratorItem>>,
        want: String,
    ) {
        let client = flex_local::local_client_empty();

        let (it, mut requests) = client.create_proxy_and_stream::<fneighbor::EntryIteratorMarker>();

        let server = async {
            for items in batches {
                let responder = requests
                    .try_next()
                    .await
                    .expect("neigh FIDL error")
                    .expect("request stream should not have ended")
                    .into_get_next()
                    .expect("request should be of type GetNext");
                responder.send(&items).expect("responder.send should succeed");
            }
        }
        .on_timeout(std::time::Duration::from_secs(60), || panic!("server responder timed out"));

        let client = async {
            let mut stream = neigh_entry_stream(it, watch_for_changes);

            let item_to_string = |item| {
                let buffers = writer::TestBuffers::default();
                let mut buf = writer::JsonWriter::new_test(None, &buffers);
                write_neigh_entry(&mut buf, item, watch_for_changes)
                    .expect("write_neigh_entry should succeed");
                buffers.into_stdout_str()
            };

            // Check each string sent by get_neigh_entries
            for want_line in want.lines() {
                let got = stream
                    .next()
                    .await
                    .map(|item| item_to_string(item.expect("neigh_entry_stream should succeed")));
                assert_eq!(got, Some(format!("{}\n", want_line)));
            }

            // When listing entries, the sender should close after sending all existing entries.
            if !watch_for_changes {
                match stream.next().await {
                    Some(Ok(item)) => {
                        panic!("unexpected item from stream: {}", item_to_string(item))
                    }
                    Some(Err(err)) => panic!("unexpected error from stream: {}", err),
                    None => {}
                }
            }
        };

        let ((), ()) = futures::future::join(client, server).await;
    }

    async fn test_neigh_none(watch_for_changes: bool, want: String) {
        test_get_neigh_entries(
            watch_for_changes,
            vec![vec![fneighbor::EntryIteratorItem::Idle(fneighbor::IdleEvent {})]],
            want,
        )
        .await
    }

    #[fuchsia::test]
    async fn neigh_list_none() {
        test_neigh_none(false /* watch_for_changes */, "".to_string()).await
    }

    #[fuchsia::test]
    async fn neigh_watch_none() {
        test_neigh_none(true /* watch_for_changes */, "IDLE".to_string()).await
    }

    fn timestamp_60s_ago() -> i64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .expect("failed to get duration since epoch");
        let past = now - std::time::Duration::from_secs(60);
        i64::try_from(past.as_nanos()).expect("failed to convert duration to i64")
    }

    async fn test_neigh_one(watch_for_changes: bool, want: fn(fneighbor_ext::Entry) -> String) {
        fn new_entry(updated_at: i64) -> fneighbor::Entry {
            fneighbor::Entry {
                interface: Some(1),
                neighbor: Some(IF_ADDR_V4.addr),
                state: Some(fneighbor::EntryState::Reachable),
                mac: Some(MAC_1),
                updated_at: Some(updated_at),
                ..Default::default()
            }
        }

        let updated_at = timestamp_60s_ago();

        test_get_neigh_entries(
            watch_for_changes,
            vec![vec![
                fneighbor::EntryIteratorItem::Existing(new_entry(updated_at)),
                fneighbor::EntryIteratorItem::Idle(fneighbor::IdleEvent {}),
            ]],
            want(fneighbor_ext::Entry::try_from(new_entry(updated_at)).unwrap()),
        )
        .await
    }

    #[fuchsia::test]
    async fn neigh_list_one() {
        test_neigh_one(false /* watch_for_changes */, |entry| format!("{}\n", entry)).await
    }

    #[fuchsia::test]
    async fn neigh_watch_one() {
        test_neigh_one(true /* watch_for_changes */, |entry| {
            format!(
                "EXISTING | {}\n\
                 IDLE\n",
                entry
            )
        })
        .await
    }

    async fn test_neigh_many(
        watch_for_changes: bool,
        want: fn(fneighbor_ext::Entry, fneighbor_ext::Entry) -> String,
    ) {
        fn new_entry(
            ip: fnet::IpAddress,
            mac: fnet::MacAddress,
            updated_at: i64,
        ) -> fneighbor::Entry {
            fneighbor::Entry {
                interface: Some(1),
                neighbor: Some(ip),
                state: Some(fneighbor::EntryState::Reachable),
                mac: Some(mac),
                updated_at: Some(updated_at),
                ..Default::default()
            }
        }

        let updated_at = timestamp_60s_ago();
        let offset = i64::try_from(std::time::Duration::from_secs(60).as_nanos())
            .expect("failed to convert duration to i64");

        test_get_neigh_entries(
            watch_for_changes,
            vec![vec![
                fneighbor::EntryIteratorItem::Existing(new_entry(
                    IF_ADDR_V4.addr,
                    MAC_1,
                    updated_at,
                )),
                fneighbor::EntryIteratorItem::Existing(new_entry(
                    IF_ADDR_V6.addr,
                    MAC_2,
                    updated_at - offset,
                )),
                fneighbor::EntryIteratorItem::Idle(fneighbor::IdleEvent {}),
            ]],
            want(
                fneighbor_ext::Entry::try_from(new_entry(IF_ADDR_V4.addr, MAC_1, updated_at))
                    .unwrap(),
                fneighbor_ext::Entry::try_from(new_entry(
                    IF_ADDR_V6.addr,
                    MAC_2,
                    updated_at - offset,
                ))
                .unwrap(),
            ),
        )
        .await
    }

    #[fuchsia::test]
    async fn neigh_list_many() {
        test_neigh_many(false /* watch_for_changes */, |a, b| format!("{}\n{}\n", a, b)).await
    }

    #[fuchsia::test]
    async fn neigh_watch_many() {
        test_neigh_many(true /* watch_for_changes */, |a, b| {
            format!(
                "EXISTING | {}\n\
                 EXISTING | {}\n\
                 IDLE\n",
                a, b
            )
        })
        .await
    }

    fn wanted_neigh_list_json() -> String {
        json!({
            "interface": 1,
            "neighbor": "192.168.0.1",
            "state": "REACHABLE",
            "mac": "01:02:03:04:05:06",
        })
        .to_string()
    }

    fn wanted_neigh_watch_json() -> String {
        json!({
            "state_change_status": "EXISTING",
            "entry": {
                "interface": 1,
                "neighbor": "192.168.0.1",
                "state": "REACHABLE",
                "mac": "01:02:03:04:05:06",
            },
        })
        .to_string()
    }

    #[test_case(true, false, &wanted_neigh_list_json() ; "in json format, not including entry state")]
    #[test_case(false, false, "Interface 1 | IP 192.168.0.1 | MAC 01:02:03:04:05:06 | REACHABLE" ; "in tabular format, not including entry state")]
    #[test_case(true, true, &wanted_neigh_watch_json() ; "in json format, including entry state")]
    #[test_case(false, true, "EXISTING | Interface 1 | IP 192.168.0.1 | MAC 01:02:03:04:05:06 | REACHABLE" ; "in tabular format, including entry state")]
    fn neigh_write_entry(json: bool, include_entry_state: bool, wanted_output: &str) {
        let entry = fneighbor::EntryIteratorItem::Existing(fneighbor::Entry {
            interface: Some(1),
            neighbor: Some(IF_ADDR_V4.addr),
            state: Some(fneighbor::EntryState::Reachable),
            mac: Some(MAC_1),
            updated_at: Some(timestamp_60s_ago()),
            ..Default::default()
        });

        let buffers = writer::TestBuffers::default();
        let mut output = if json {
            writer::JsonWriter::new_test(Some(writer::Format::Json), &buffers)
        } else {
            writer::JsonWriter::new_test(None, &buffers)
        };
        write_neigh_entry(&mut output, entry, include_entry_state)
            .expect("write_neigh_entry should succeed");
        let got_output = buffers.into_stdout_str();
        pretty_assertions::assert_eq!(
            trim_whitespace_for_comparison(&got_output),
            trim_whitespace_for_comparison(wanted_output),
        );
    }

    const INTERFACE_ID: u64 = 1;
    const IP_VERSION: fnet::IpVersion = fnet::IpVersion::V4;

    #[fuchsia::test]
    async fn neigh_add() {
        let client = flex_local::local_client_empty();

        let (controller, mut requests) =
            client.create_proxy_and_stream::<fneighbor::ControllerMarker>();
        let neigh = do_neigh_add(INTERFACE_ID, IF_ADDR_V4.addr, MAC_1, controller);
        let neigh_succeeds = async {
            let (got_interface_id, got_ip_address, got_mac, responder) = requests
                .try_next()
                .await
                .expect("neigh FIDL error")
                .expect("request stream should not have ended")
                .into_add_entry()
                .expect("request should be of type AddEntry");
            assert_eq!(got_interface_id, INTERFACE_ID);
            assert_eq!(got_ip_address, IF_ADDR_V4.addr);
            assert_eq!(got_mac, MAC_1);
            responder.send(Ok(())).expect("responder.send should succeed");
            Ok(())
        };
        let ((), ()) = futures::future::try_join(neigh, neigh_succeeds)
            .await
            .expect("neigh add should succeed");
    }

    #[fuchsia::test]
    async fn neigh_clear() {
        let client = flex_local::local_client_empty();

        let (controller, mut requests) =
            client.create_proxy_and_stream::<fneighbor::ControllerMarker>();
        let neigh = do_neigh_clear(INTERFACE_ID, IP_VERSION, controller);
        let neigh_succeeds = async {
            let (got_interface_id, got_ip_version, responder) = requests
                .try_next()
                .await
                .expect("neigh FIDL error")
                .expect("request stream should not have ended")
                .into_clear_entries()
                .expect("request should be of type ClearEntries");
            assert_eq!(got_interface_id, INTERFACE_ID);
            assert_eq!(got_ip_version, IP_VERSION);
            responder.send(Ok(())).expect("responder.send should succeed");
            Ok(())
        };
        let ((), ()) = futures::future::try_join(neigh, neigh_succeeds)
            .await
            .expect("neigh clear should succeed");
    }

    #[fuchsia::test]
    async fn neigh_del() {
        let client = flex_local::local_client_empty();

        let (controller, mut requests) =
            client.create_proxy_and_stream::<fneighbor::ControllerMarker>();
        let neigh = do_neigh_del(INTERFACE_ID, IF_ADDR_V4.addr, controller);
        let neigh_succeeds = async {
            let (got_interface_id, got_ip_address, responder) = requests
                .try_next()
                .await
                .expect("neigh FIDL error")
                .expect("request stream should not have ended")
                .into_remove_entry()
                .expect("request should be of type RemoveEntry");
            assert_eq!(got_interface_id, INTERFACE_ID);
            assert_eq!(got_ip_address, IF_ADDR_V4.addr);
            responder.send(Ok(())).expect("responder.send should succeed");
            Ok(())
        };
        let ((), ()) = futures::future::try_join(neigh, neigh_succeeds)
            .await
            .expect("neigh remove should succeed");
    }

    #[test_case(opts::dhcpd::DhcpdEnum::Get(opts::dhcpd::Get {
            arg: opts::dhcpd::GetArg::Option(
                opts::dhcpd::OptionArg {
                    name: opts::dhcpd::Option_::SubnetMask(
                        opts::dhcpd::SubnetMask { mask: None }) }),
        }); "get option")]
    #[test_case(opts::dhcpd::DhcpdEnum::Get(opts::dhcpd::Get {
            arg: opts::dhcpd::GetArg::Parameter(opts::dhcpd::ParameterArg {
                name: opts::dhcpd::Parameter::LeaseLength(
                    opts::dhcpd::LeaseLength { default: None, max: None }),
            }),
        }); "get parameter")]
    #[test_case(opts::dhcpd::DhcpdEnum::Set(opts::dhcpd::Set {
            arg: opts::dhcpd::SetArg::Option(opts::dhcpd::OptionArg {
                name: opts::dhcpd::Option_::SubnetMask(opts::dhcpd::SubnetMask {
                    mask: Some(net_declare::std_ip_v4!("255.255.255.0")),
                }),
            }),
        }); "set option")]
    #[test_case(opts::dhcpd::DhcpdEnum::Set(opts::dhcpd::Set {
            arg: opts::dhcpd::SetArg::Parameter(opts::dhcpd::ParameterArg {
                name: opts::dhcpd::Parameter::LeaseLength(
                    opts::dhcpd::LeaseLength { max: Some(42), default: Some(42) }),
            }),
        }); "set parameter")]
    #[test_case(opts::dhcpd::DhcpdEnum::List(opts::dhcpd::List { arg:
        opts::dhcpd::ListArg::Option(opts::dhcpd::OptionToken {}) }); "list option")]
    #[test_case(opts::dhcpd::DhcpdEnum::List(
        opts::dhcpd::List { arg: opts::dhcpd::ListArg::Parameter(opts::dhcpd::ParameterToken {}) });
        "list parameter")]
    #[test_case(opts::dhcpd::DhcpdEnum::Reset(opts::dhcpd::Reset {
        arg: opts::dhcpd::ResetArg::Option(opts::dhcpd::OptionToken {}) }); "reset option")]
    #[test_case(opts::dhcpd::DhcpdEnum::Reset(
        opts::dhcpd::Reset {
            arg: opts::dhcpd::ResetArg::Parameter(opts::dhcpd::ParameterToken {}) });
        "reset parameter")]
    #[test_case(opts::dhcpd::DhcpdEnum::ClearLeases(opts::dhcpd::ClearLeases {}); "clear leases")]
    #[test_case(opts::dhcpd::DhcpdEnum::Start(opts::dhcpd::Start {}); "start")]
    #[test_case(opts::dhcpd::DhcpdEnum::Stop(opts::dhcpd::Stop {}); "stop")]
    #[fuchsia::test]
    async fn test_do_dhcpd(cmd: opts::dhcpd::DhcpdEnum) {
        let client = flex_local::local_client_empty();

        let (dhcpd, mut requests) = client.create_proxy_and_stream::<fdhcp::Server_Marker>();

        let connector = TestConnector { dhcpd: Some(dhcpd), ..Default::default() };
        let op = do_dhcpd(cmd.clone(), &connector);
        let op_succeeds = async move {
            let req = requests
                .try_next()
                .await
                .expect("receiving request")
                .expect("request stream should not have ended");
            match cmd {
                opts::dhcpd::DhcpdEnum::Get(opts::dhcpd::Get { arg }) => match arg {
                    opts::dhcpd::GetArg::Option(opts::dhcpd::OptionArg { name }) => {
                        let (code, responder) =
                            req.into_get_option().expect("request should be of type get option");
                        assert_eq!(
                            <opts::dhcpd::Option_ as Into<fdhcp::OptionCode>>::into(name),
                            code
                        );
                        // We don't care what the value is here, we just need something to give as
                        // an argument to responder.send().
                        let dummy_result = fdhcp::Option_::SubnetMask(fidl_ip_v4!("255.255.255.0"));
                        responder.send(Ok(&dummy_result)).expect("responder.send should succeed");
                        Ok(())
                    }
                    opts::dhcpd::GetArg::Parameter(opts::dhcpd::ParameterArg { name }) => {
                        let (param, responder) = req
                            .into_get_parameter()
                            .expect("request should be of type get parameter");
                        assert_eq!(
                            <opts::dhcpd::Parameter as Into<fdhcp::ParameterName>>::into(name),
                            param
                        );
                        // We don't care what the value is here, we just need something to give as
                        // an argument to responder.send().
                        let dummy_result = fdhcp::Parameter::Lease(fdhcp::LeaseLength::default());
                        responder.send(Ok(&dummy_result)).expect("responder.send should succeed");
                        Ok(())
                    }
                },
                opts::dhcpd::DhcpdEnum::Set(opts::dhcpd::Set { arg }) => match arg {
                    opts::dhcpd::SetArg::Option(opts::dhcpd::OptionArg { name }) => {
                        let (opt, responder) =
                            req.into_set_option().expect("request should be of type set option");
                        assert_eq!(<opts::dhcpd::Option_ as Into<fdhcp::Option_>>::into(name), opt);
                        responder.send(Ok(())).expect("responder.send should succeed");
                        Ok(())
                    }
                    opts::dhcpd::SetArg::Parameter(opts::dhcpd::ParameterArg { name }) => {
                        let (opt, responder) = req
                            .into_set_parameter()
                            .expect("request should be of type set parameter");
                        assert_eq!(
                            <opts::dhcpd::Parameter as Into<fdhcp::Parameter>>::into(name),
                            opt
                        );
                        responder.send(Ok(())).expect("responder.send should succeed");
                        Ok(())
                    }
                },
                opts::dhcpd::DhcpdEnum::List(opts::dhcpd::List { arg }) => match arg {
                    opts::dhcpd::ListArg::Option(opts::dhcpd::OptionToken {}) => {
                        let responder = req
                            .into_list_options()
                            .expect("request should be of type list options");
                        responder.send(Ok(&[])).expect("responder.send should succeed");
                        Ok(())
                    }
                    opts::dhcpd::ListArg::Parameter(opts::dhcpd::ParameterToken {}) => {
                        let responder = req
                            .into_list_parameters()
                            .expect("request should be of type list options");
                        responder.send(Ok(&[])).expect("responder.send should succeed");
                        Ok(())
                    }
                },
                opts::dhcpd::DhcpdEnum::Reset(opts::dhcpd::Reset { arg }) => match arg {
                    opts::dhcpd::ResetArg::Option(opts::dhcpd::OptionToken {}) => {
                        let responder = req
                            .into_reset_options()
                            .expect("request should be of type reset options");
                        responder.send(Ok(())).expect("responder.send should succeed");
                        Ok(())
                    }
                    opts::dhcpd::ResetArg::Parameter(opts::dhcpd::ParameterToken {}) => {
                        let responder = req
                            .into_reset_parameters()
                            .expect("request should be of type reset parameters");
                        responder.send(Ok(())).expect("responder.send should succeed");
                        Ok(())
                    }
                },
                opts::dhcpd::DhcpdEnum::ClearLeases(opts::dhcpd::ClearLeases {}) => {
                    let responder =
                        req.into_clear_leases().expect("request should be of type clear leases");
                    responder.send(Ok(())).expect("responder.send should succeed");
                    Ok(())
                }
                opts::dhcpd::DhcpdEnum::Start(opts::dhcpd::Start {}) => {
                    let responder =
                        req.into_start_serving().expect("request should be of type start serving");
                    responder.send(Ok(())).expect("responder.send should succeed");
                    Ok(())
                }
                opts::dhcpd::DhcpdEnum::Stop(opts::dhcpd::Stop {}) => {
                    let responder =
                        req.into_stop_serving().expect("request should be of type stop serving");
                    responder.send().expect("responder.send should succeed");
                    Ok(())
                }
            }
        };
        let ((), ()) = futures::future::try_join(op, op_succeeds)
            .await
            .expect("dhcp server command should succeed");
    }

    #[fuchsia::test]
    async fn dns_lookup() {
        let client = flex_local::local_client_empty();

        let (lookup, mut requests) = client.create_proxy_and_stream::<fname::LookupMarker>();
        let connector = TestConnector { name_lookup: Some(lookup), ..Default::default() };

        let cmd = opts::dns::DnsEnum::Lookup(opts::dns::Lookup {
            hostname: "example.com".to_string(),
            ipv4: true,
            ipv6: true,
            sort: true,
        });
        let mut output = Vec::new();
        let dns_command = do_dns(&mut output, cmd.clone(), &connector)
            .map(|result| result.expect("dns command should succeed"));

        let handle_request = async move {
            let (hostname, options, responder) = requests
                .try_next()
                .await
                .expect("FIDL error")
                .expect("request stream should not have ended")
                .into_lookup_ip()
                .expect("request should be of type LookupIp");
            let opts::dns::DnsEnum::Lookup(opts::dns::Lookup {
                hostname: want_hostname,
                ipv4,
                ipv6,
                sort,
            }) = cmd;
            let want_options = fname::LookupIpOptions {
                ipv4_lookup: Some(ipv4),
                ipv6_lookup: Some(ipv6),
                sort_addresses: Some(sort),
                ..Default::default()
            };
            assert_eq!(
                hostname, want_hostname,
                "received IP lookup request for unexpected hostname"
            );
            assert_eq!(options, want_options, "received unexpected IP lookup options");

            responder
                .send(Ok(&fname::LookupResult {
                    addresses: Some(vec![fidl_ip!("203.0.113.1"), fidl_ip!("2001:db8::1")]),
                    ..Default::default()
                }))
                .expect("send response");
        };
        let ((), ()) = futures::future::join(dns_command, handle_request).await;

        const WANT_OUTPUT: &str = "
203.0.113.1
2001:db8::1
";
        let got_output = std::str::from_utf8(&output).unwrap();
        pretty_assertions::assert_eq!(
            trim_whitespace_for_comparison(got_output),
            trim_whitespace_for_comparison(WANT_OUTPUT),
        );
    }
}
