// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![warn(missing_docs)]

//! Provides utilities for using `fuchsia.net.interfaces` and
//! `fuchsia.net.interfaces.admin` in Netstack integration tests.

use super::Result;

use anyhow::Context as _;
use fuchsia_async::{DurationExt as _, TimeoutExt as _};

use futures::future::{FusedFuture, Future, FutureExt as _, TryFutureExt as _};
use std::collections::{HashMap, HashSet};
use std::pin::pin;

/// Waits for a non-loopback interface to come up with an ID not in `exclude_ids`.
///
/// Useful when waiting for an interface to be discovered and brought up by a
/// network manager.
///
/// Returns the interface's ID and name.
pub async fn wait_for_non_loopback_interface_up<
    F: Unpin + FusedFuture + Future<Output = Result<component_events::events::Stopped>>,
>(
    interface_state: &fidl_fuchsia_net_interfaces::StateProxy,
    mut wait_for_netmgr: &mut F,
    exclude_ids: Option<&HashSet<u64>>,
    timeout: zx::MonotonicDuration,
) -> Result<(u64, String)> {
    let mut if_map =
        HashMap::<u64, fidl_fuchsia_net_interfaces_ext::PropertiesAndState<(), _>>::new();
    let mut wait_for_interface = pin!(
        fidl_fuchsia_net_interfaces_ext::wait_interface(
            fidl_fuchsia_net_interfaces_ext::event_stream_from_state::<
                fidl_fuchsia_net_interfaces_ext::DefaultInterest,
            >(interface_state, Default::default(),)?,
            &mut if_map,
            |if_map| {
                if_map.iter().find_map(
                    |(
                        id,
                        fidl_fuchsia_net_interfaces_ext::PropertiesAndState {
                            properties:
                                fidl_fuchsia_net_interfaces_ext::Properties {
                                    name,
                                    port_class,
                                    online,
                                    ..
                                },
                            state: _,
                        },
                    )| {
                        (*port_class != fidl_fuchsia_net_interfaces_ext::PortClass::Loopback
                            && *online
                            && exclude_ids.map_or(true, |ids| !ids.contains(id)))
                        .then(|| (*id, name.clone()))
                    },
                )
            },
        )
        .map_err(anyhow::Error::from)
        .on_timeout(timeout.after_now(), || Err(anyhow::anyhow!("timed out")))
        .map(|r| r.context("failed to wait for non-loopback interface up"))
        .fuse()
    );
    futures::select! {
        wait_for_interface_res = wait_for_interface => {
            wait_for_interface_res
        }
        stopped_event = wait_for_netmgr => {
            Err(anyhow::anyhow!("the network manager unexpectedly stopped with event = {:?}", stopped_event))
        }
    }
}

/// Add an address, returning once the assignment state is `Assigned`.
pub async fn add_address_wait_assigned(
    control: &fidl_fuchsia_net_interfaces_ext::admin::Control,
    address: fidl_fuchsia_net::Subnet,
    address_parameters: fidl_fuchsia_net_interfaces_admin::AddressParameters,
) -> std::result::Result<
    fidl_fuchsia_net_interfaces_admin::AddressStateProviderProxy,
    fidl_fuchsia_net_interfaces_ext::admin::AddressStateProviderError,
> {
    let (address_state_provider, server) = fidl::endpoints::create_proxy::<
        fidl_fuchsia_net_interfaces_admin::AddressStateProviderMarker,
    >();
    control
        .add_address(&address, &address_parameters, server)
        .expect("Control.AddAddress FIDL error");

    fidl_fuchsia_net_interfaces_ext::admin::wait_for_address_added_event(
        &mut address_state_provider.take_event_stream(),
    )
    .await?;

    {
        let mut state_stream =
            pin!(fidl_fuchsia_net_interfaces_ext::admin::assignment_state_stream(
                address_state_provider.clone(),
            ));
        fidl_fuchsia_net_interfaces_ext::admin::wait_assignment_state(
            &mut state_stream,
            fidl_fuchsia_net_interfaces::AddressAssignmentState::Assigned,
        )
        .await?;
    }
    Ok(address_state_provider)
}

/// Remove a subnet address and route, returning true if the address was removed.
pub async fn remove_subnet_address_and_route<'a>(
    iface: &'a netemul::TestInterface<'a>,
    subnet: fidl_fuchsia_net::Subnet,
) -> Result<bool> {
    iface.del_address_and_subnet_route(subnet).await
}

/// Wait until there is an IPv4 and an IPv6 link-local address assigned to the
/// interface identified by `id`.
///
/// If there are multiple IPv4 or multiple IPv6 link-local addresses assigned,
/// the choice of which particular address to return is arbitrary and should
/// not be relied upon.
///
/// Note that if a `netemul::TestInterface` is available, helpers on said type
/// should be preferred over using this function.
pub async fn wait_for_v4_and_v6_ll(
    interfaces_state: &fidl_fuchsia_net_interfaces::StateProxy,
    id: u64,
) -> Result<(net_types::ip::Ipv4Addr, net_types::ip::Ipv6Addr)> {
    wait_for_addresses(interfaces_state, id, |addresses| {
        let (v4, v6) = addresses.iter().fold(
            (None, None),
            |(v4, v6),
             &fidl_fuchsia_net_interfaces_ext::Address {
                 addr: fidl_fuchsia_net::Subnet { addr, prefix_len: _ },
                 valid_until: _,
                 preferred_lifetime_info: _,
                 assignment_state,
             }| {
                assert_eq!(
                    assignment_state,
                    fidl_fuchsia_net_interfaces::AddressAssignmentState::Assigned
                );
                match addr {
                    fidl_fuchsia_net::IpAddress::Ipv4(fidl_fuchsia_net::Ipv4Address { addr }) => {
                        (Some(net_types::ip::Ipv4Addr::from(addr)), v6)
                    }
                    fidl_fuchsia_net::IpAddress::Ipv6(fidl_fuchsia_net::Ipv6Address { addr }) => {
                        let v6_addr = net_types::ip::Ipv6Addr::from_bytes(addr);
                        (v4, if v6_addr.is_unicast_link_local() { Some(v6_addr) } else { v6 })
                    }
                }
            },
        );
        match (v4, v6) {
            (Some(v4), Some(v6)) => Some((v4, v6)),
            _ => None,
        }
    })
    .await
    .context("wait for addresses")
}

/// Wait until there is an IPv6 link-local address assigned to the interface
/// identified by `id`.
///
/// If there are multiple IPv6 link-local addresses assigned, the choice
/// of which particular address to return is arbitrary and should not be
/// relied upon.
///
/// Note that if a `netemul::TestInterface` is available, helpers on said type
/// should be preferred over using this function.
pub async fn wait_for_v6_ll(
    interfaces_state: &fidl_fuchsia_net_interfaces::StateProxy,
    id: u64,
) -> Result<net_types::ip::Ipv6Addr> {
    wait_for_addresses(interfaces_state, id, |addresses| {
        addresses.iter().find_map(
            |&fidl_fuchsia_net_interfaces_ext::Address {
                 addr: fidl_fuchsia_net::Subnet { addr, prefix_len: _ },
                 valid_until: _,
                 preferred_lifetime_info: _,
                 assignment_state,
             }| {
                assert_eq!(
                    assignment_state,
                    fidl_fuchsia_net_interfaces::AddressAssignmentState::Assigned
                );
                match addr {
                    fidl_fuchsia_net::IpAddress::Ipv4(fidl_fuchsia_net::Ipv4Address {
                        addr: _,
                    }) => None,
                    fidl_fuchsia_net::IpAddress::Ipv6(fidl_fuchsia_net::Ipv6Address { addr }) => {
                        let v6_addr = net_types::ip::Ipv6Addr::from_bytes(addr);
                        v6_addr.is_unicast_link_local().then(|| v6_addr)
                    }
                }
            },
        )
    })
    .await
    .context("wait for IPv6 link-local address")
}

/// Wait until the given interface has a set of assigned addresses that matches
/// the given predicate.
pub async fn wait_for_addresses<T, F>(
    interfaces_state: &fidl_fuchsia_net_interfaces::StateProxy,
    id: u64,
    mut predicate: F,
) -> Result<T>
where
    F: FnMut(
        &[fidl_fuchsia_net_interfaces_ext::Address<fidl_fuchsia_net_interfaces_ext::AllInterest>],
    ) -> Option<T>,
{
    let mut state =
        fidl_fuchsia_net_interfaces_ext::InterfaceState::<(), _>::Unknown(u64::from(id));
    fidl_fuchsia_net_interfaces_ext::wait_interface_with_id(
        fidl_fuchsia_net_interfaces_ext::event_stream_from_state::<
            fidl_fuchsia_net_interfaces_ext::AllInterest,
        >(&interfaces_state, Default::default())
        .context("get interface event stream")?,
        &mut state,
        |properties_and_state| predicate(&properties_and_state.properties.addresses),
    )
    .await
    .context("wait for address")
}

/// Wait until the interface's online property matches `want_online`.
pub async fn wait_for_online(
    interfaces_state: &fidl_fuchsia_net_interfaces::StateProxy,
    id: u64,
    want_online: bool,
) -> Result<()> {
    let mut state =
        fidl_fuchsia_net_interfaces_ext::InterfaceState::<(), _>::Unknown(u64::from(id));
    fidl_fuchsia_net_interfaces_ext::wait_interface_with_id(
        fidl_fuchsia_net_interfaces_ext::event_stream_from_state::<
            fidl_fuchsia_net_interfaces_ext::DefaultInterest,
        >(&interfaces_state, Default::default())
        .context("get interface event stream")?,
        &mut state,
        |properties_and_state| {
            (properties_and_state.properties.online == want_online).then_some(())
        },
    )
    .await
    .with_context(|| format!("wait for online {}", want_online))
}

/// Helpers for `netemul::TestInterface`.
pub trait TestInterfaceExt {
    /// Calls [`crate::nud::apply_nud_flake_workaround`] for this interface.
    fn apply_nud_flake_workaround(&self) -> impl Future<Output = Result>;
}

impl<'a> TestInterfaceExt for netemul::TestInterface<'a> {
    async fn apply_nud_flake_workaround(&self) -> Result {
        crate::nud::apply_nud_flake_workaround(self.control()).await
    }
}
