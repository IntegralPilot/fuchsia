// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{
    Address, EventWithInterest, FieldInterests, PortClass, Properties, PropertiesAndState, Update,
    UpdateResult, WatcherOperationError,
};

use flex_fuchsia_net as fnet;
use flex_fuchsia_net_interfaces as fnet_interfaces;
use futures::{Stream, TryStreamExt};
use net_types::{LinkLocalAddress as _, ScopeableAddress as _};
use std::collections::{HashMap, HashSet};
use thiserror::Error;

/// Returns true iff the supplied [`Properties`] (expected to be fully populated)
/// appears to provide network connectivity, i.e. is not loopback, is online, and has a default
/// route and a globally routable address for either IPv4 or IPv6. An IPv4 address is assumed to be
/// globally routable if it's not link-local. An IPv6 address is assumed to be globally routable if
/// it has global scope.
pub fn is_globally_routable<I: FieldInterests>(
    &Properties {
        ref port_class,
        online,
        ref addresses,
        has_default_ipv4_route,
        has_default_ipv6_route,
        ..
    }: &Properties<I>,
) -> bool {
    match port_class {
        // TODO(https://fxbug.dev/389732915): In the presence of particular TPROXY/NAT configs
        // early-returning here might be incorrect, as we could potentially be providing upstream
        // connectivity over loopback or blackhole interfaces.
        PortClass::Loopback | PortClass::Blackhole => return false,
        PortClass::Virtual
        | PortClass::Ethernet
        | PortClass::WlanClient
        | PortClass::WlanAp
        | PortClass::Ppp
        | PortClass::Bridge
        | PortClass::Lowpan => {}
    }
    if !online {
        return false;
    }
    if !has_default_ipv4_route && !has_default_ipv6_route {
        return false;
    }
    addresses.iter().any(
        |Address {
             addr: fnet::Subnet { addr, prefix_len: _ },
             valid_until: _,
             preferred_lifetime_info: _,
             assignment_state,
         }| {
            let assigned = match assignment_state {
                fnet_interfaces::AddressAssignmentState::Assigned => true,
                fnet_interfaces::AddressAssignmentState::Tentative
                | fnet_interfaces::AddressAssignmentState::Unavailable => false,
            };
            assigned
                && match addr {
                    fnet::IpAddress::Ipv4(fnet::Ipv4Address { addr }) => {
                        has_default_ipv4_route
                            && !net_types::ip::Ipv4Addr::new(*addr).is_link_local()
                    }
                    fnet::IpAddress::Ipv6(fnet::Ipv6Address { addr }) => {
                        has_default_ipv6_route
                            && net_types::ip::Ipv6Addr::from_bytes(*addr).scope()
                                == net_types::ip::Ipv6Scope::Global
                    }
                }
        },
    )
}

/// Wraps `event_stream` and returns a stream which yields the reachability
/// status as a bool (true iff there exists an interface with properties that
/// satisfy [`is_globally_routable`]) whenever it changes. The first item the
/// returned stream yields is the reachability status of the first interface
/// discovered through an `Added` or `Existing` event on `event_stream`.
///
/// Note that `event_stream` must be created from a watcher with interest in the
/// appropriate fields, such as one created from
/// [`crate::event_stream_from_state`].
pub fn to_reachability_stream<I: FieldInterests>(
    event_stream: impl Stream<Item = Result<EventWithInterest<I>, fidl::Error>>,
) -> impl Stream<Item = Result<bool, WatcherOperationError<(), HashMap<u64, PropertiesAndState<(), I>>>>>
{
    let mut if_map = HashMap::<u64, _>::new();
    let mut reachable = None;
    let mut reachable_ids = HashSet::new();
    event_stream.map_err(WatcherOperationError::EventStream).try_filter_map(move |event| {
        futures::future::ready(if_map.update(event).map_err(WatcherOperationError::Update).map(
            |changed: UpdateResult<'_, (), _>| {
                let reachable_ids_changed = match changed {
                    UpdateResult::Existing { properties, state: _ }
                    | UpdateResult::Added { properties, state: _ }
                    | UpdateResult::Changed { previous: _, current: properties, state: _ }
                        if is_globally_routable(properties) =>
                    {
                        reachable_ids.insert(properties.id)
                    }
                    UpdateResult::Existing { .. } | UpdateResult::Added { .. } => false,
                    UpdateResult::Changed { previous: _, current: properties, state: _ } => {
                        reachable_ids.remove(&properties.id)
                    }
                    UpdateResult::Removed(PropertiesAndState { properties, state: _ }) => {
                        reachable_ids.remove(&properties.id)
                    }
                    UpdateResult::NoChange => return None,
                };
                // If the stream hasn't yielded anything yet, do so even if the set of reachable
                // interfaces hasn't changed.
                if reachable.is_none() {
                    reachable = Some(!reachable_ids.is_empty());
                    return reachable;
                } else if reachable_ids_changed {
                    let new_reachable = Some(!reachable_ids.is_empty());
                    if reachable != new_reachable {
                        reachable = new_reachable;
                        return reachable;
                    }
                }
                None
            },
        ))
    })
}

/// Reachability status stream operational errors.
#[derive(Error, Debug)]
pub enum OperationError<S: std::fmt::Debug, B: Update<S> + std::fmt::Debug> {
    #[error("watcher operation error: {0}")]
    Watcher(WatcherOperationError<S, B>),
    #[error("reachability status stream ended unexpectedly")]
    UnexpectedEnd,
}

/// Returns a future which resolves when any network interface observed through `event_stream`
/// has properties which satisfy [`is_globally_routable`].
pub async fn wait_for_reachability<I: FieldInterests>(
    event_stream: impl Stream<Item = Result<EventWithInterest<I>, fidl::Error>>,
) -> Result<(), OperationError<(), HashMap<u64, PropertiesAndState<(), I>>>> {
    futures::pin_mut!(event_stream);
    let rtn = to_reachability_stream(event_stream)
        .map_err(OperationError::Watcher)
        .try_filter_map(|reachable| futures::future::ok(if reachable { Some(()) } else { None }))
        .try_next()
        .await
        .and_then(|item| item.ok_or_else(|| OperationError::UnexpectedEnd));
    rtn
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::{AllInterest, PositiveMonotonicInstant, PreferredLifetimeInfo};

    use anyhow::Context as _;
    use flex_fuchsia_hardware_network as fnetwork;
    use futures::FutureExt as _;
    use net_declare::fidl_subnet;
    use std::convert::TryInto as _;
    use zx_types as zx;

    const IPV4_LINK_LOCAL: fnet::Subnet = fidl_subnet!("169.254.0.1/16");
    const IPV6_LINK_LOCAL: fnet::Subnet = fidl_subnet!("fe80::1/64");
    const IPV4_GLOBAL: fnet::Subnet = fidl_subnet!("192.168.0.1/16");
    const IPV6_GLOBAL: fnet::Subnet = fidl_subnet!("100::1/64");

    fn valid_interface(id: u64) -> fnet_interfaces::Properties {
        fnet_interfaces::Properties {
            id: Some(id),
            name: Some("test1".to_string()),
            port_class: Some(fnet_interfaces::PortClass::Device(fnetwork::PortClass::Ethernet)),
            online: Some(true),
            addresses: Some(vec![
                fnet_interfaces::Address {
                    addr: Some(IPV4_GLOBAL),
                    valid_until: Some(zx::ZX_TIME_INFINITE),
                    assignment_state: Some(fnet_interfaces::AddressAssignmentState::Assigned),
                    preferred_lifetime_info: Some(
                        PreferredLifetimeInfo::preferred_forever().into(),
                    ),
                    __source_breaking: Default::default(),
                },
                fnet_interfaces::Address {
                    addr: Some(IPV4_LINK_LOCAL),
                    valid_until: Some(zx::ZX_TIME_INFINITE),
                    assignment_state: Some(fnet_interfaces::AddressAssignmentState::Assigned),
                    preferred_lifetime_info: Some(
                        PreferredLifetimeInfo::preferred_forever().into(),
                    ),
                    __source_breaking: Default::default(),
                },
                fnet_interfaces::Address {
                    addr: Some(IPV6_GLOBAL),
                    valid_until: Some(zx::ZX_TIME_INFINITE),
                    assignment_state: Some(fnet_interfaces::AddressAssignmentState::Assigned),
                    preferred_lifetime_info: Some(
                        PreferredLifetimeInfo::preferred_forever().into(),
                    ),
                    __source_breaking: Default::default(),
                },
                fnet_interfaces::Address {
                    addr: Some(IPV6_LINK_LOCAL),
                    valid_until: Some(zx::ZX_TIME_INFINITE),
                    assignment_state: Some(fnet_interfaces::AddressAssignmentState::Assigned),
                    preferred_lifetime_info: Some(
                        PreferredLifetimeInfo::preferred_forever().into(),
                    ),
                    __source_breaking: Default::default(),
                },
            ]),
            has_default_ipv4_route: Some(true),
            has_default_ipv6_route: Some(true),
            ..Default::default()
        }
    }

    #[test]
    fn test_is_globally_routable() -> Result<(), anyhow::Error> {
        const ID: u64 = 1;
        const ASSIGNED_ADDR: Address<AllInterest> = Address {
            addr: IPV4_GLOBAL,
            valid_until: PositiveMonotonicInstant::INFINITE_FUTURE,
            preferred_lifetime_info: PreferredLifetimeInfo::preferred_forever(),
            assignment_state: fnet_interfaces::AddressAssignmentState::Assigned,
        };
        // These combinations are not globally routable.
        assert!(!is_globally_routable(&Properties::<AllInterest> {
            port_class: PortClass::Loopback,
            ..valid_interface(ID).try_into()?
        }));
        assert!(!is_globally_routable(&Properties::<AllInterest> {
            online: false,
            ..valid_interface(ID).try_into()?
        }));
        assert!(!is_globally_routable(&Properties::<AllInterest> {
            addresses: vec![],
            ..valid_interface(ID).try_into()?
        }));
        assert!(!is_globally_routable(&Properties::<AllInterest> {
            has_default_ipv4_route: false,
            has_default_ipv6_route: false,
            ..valid_interface(ID).try_into()?
        }));
        assert!(!is_globally_routable(&Properties::<AllInterest> {
            addresses: vec![Address { addr: IPV4_GLOBAL, ..ASSIGNED_ADDR }],
            has_default_ipv4_route: false,
            ..valid_interface(ID).try_into()?
        }));
        assert!(!is_globally_routable(&Properties::<AllInterest> {
            addresses: vec![Address { addr: IPV6_GLOBAL, ..ASSIGNED_ADDR }],
            has_default_ipv6_route: false,
            ..valid_interface(ID).try_into()?
        }));
        assert!(!is_globally_routable(&Properties::<AllInterest> {
            addresses: vec![Address { addr: IPV6_LINK_LOCAL, ..ASSIGNED_ADDR }],
            has_default_ipv6_route: true,
            ..valid_interface(ID).try_into()?
        }));
        assert!(!is_globally_routable(&Properties::<AllInterest> {
            addresses: vec![Address { addr: IPV4_LINK_LOCAL, ..ASSIGNED_ADDR }],
            has_default_ipv4_route: true,
            ..valid_interface(ID).try_into()?
        }));

        // These combinations are globally routable.
        assert!(is_globally_routable::<AllInterest>(&valid_interface(ID).try_into()?));
        assert!(is_globally_routable::<AllInterest>(&Properties {
            addresses: vec![Address { addr: IPV4_GLOBAL, ..ASSIGNED_ADDR }],
            has_default_ipv4_route: true,
            has_default_ipv6_route: false,
            ..valid_interface(ID).try_into()?
        }));
        assert!(is_globally_routable::<AllInterest>(&Properties {
            addresses: vec![Address { addr: IPV6_GLOBAL, ..ASSIGNED_ADDR }],
            has_default_ipv4_route: false,
            has_default_ipv6_route: true,
            ..valid_interface(ID).try_into()?
        }));
        Ok(())
    }

    #[test]
    fn test_to_reachability_stream() -> Result<(), anyhow::Error> {
        let (sender, receiver) = futures::channel::mpsc::unbounded();
        let mut reachability_stream = to_reachability_stream::<AllInterest>(receiver);
        for (event, want) in vec![
            (fnet_interfaces::Event::Idle(fnet_interfaces::Empty {}), None),
            // Added events
            (
                fnet_interfaces::Event::Added(fnet_interfaces::Properties {
                    online: Some(false),
                    ..valid_interface(1)
                }),
                Some(false),
            ),
            (fnet_interfaces::Event::Added(valid_interface(2)), Some(true)),
            (
                fnet_interfaces::Event::Added(fnet_interfaces::Properties {
                    online: Some(false),
                    ..valid_interface(3)
                }),
                None,
            ),
            // Changed events
            (
                fnet_interfaces::Event::Changed(fnet_interfaces::Properties {
                    id: Some(2),
                    online: Some(false),
                    ..Default::default()
                }),
                Some(false),
            ),
            (
                fnet_interfaces::Event::Changed(fnet_interfaces::Properties {
                    id: Some(1),
                    online: Some(true),
                    ..Default::default()
                }),
                Some(true),
            ),
            (
                fnet_interfaces::Event::Changed(fnet_interfaces::Properties {
                    id: Some(3),
                    online: Some(true),
                    ..Default::default()
                }),
                None,
            ),
            // Removed events
            (fnet_interfaces::Event::Removed(1), None),
            (fnet_interfaces::Event::Removed(3), Some(false)),
            (fnet_interfaces::Event::Removed(2), None),
        ] {
            let () =
                sender.unbounded_send(Ok(event.clone().into())).context("failed to send event")?;
            let got = reachability_stream.try_next().now_or_never();
            if let Some(want_reachable) = want {
                let r = got.ok_or_else(|| {
                    anyhow::anyhow!("reachability status stream unexpectedly yielded nothing")
                })?;
                let item = r.context("reachability status stream error")?;
                let got_reachable = item.ok_or_else(|| {
                    anyhow::anyhow!("reachability status stream ended unexpectedly")
                })?;
                assert_eq!(got_reachable, want_reachable);
            } else {
                if got.is_some() {
                    panic!(
                        "got {:?} from reachability stream after event {:?}, \
                        want None as reachability status should not have changed",
                        got, event
                    );
                }
            }
        }
        Ok(())
    }
}
