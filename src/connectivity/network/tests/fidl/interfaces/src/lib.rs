// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

mod error;

use anyhow::Context as _;
use fuchsia_async::{self as fasync, TimeoutExt as _};
use futures::{FutureExt as _, Stream, StreamExt as _, TryStreamExt as _};
use itertools::Itertools as _;
use net_declare::{fidl_ip, fidl_subnet, net_subnet_v4, net_subnet_v6, std_ip};
use net_types::ip::{Ip, IpVersion};
use netemul::{RealmTcpListener as _, RealmTcpStream as _, RealmUdpSocket as _};
use netstack_testing_common::realms::{Netstack3, TestSandboxExt as _};
use netstack_testing_common::{ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT, Result, interfaces};
use netstack_testing_macros::netstack_test;
use std::collections::{HashMap, HashSet};
use std::convert::TryInto as _;
use std::pin::pin;
use test_case::test_case;

use fidl_fuchsia_net_interfaces as fnet_interfaces;
use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
use fidl_fuchsia_net_routes as fnet_routes;
use fidl_fuchsia_net_routes_ext as fnet_routes_ext;
use fidl_fuchsia_net_routes_ext::FidlRouteIpExt;
use fidl_fuchsia_net_routes_ext::admin::FidlRouteAdminIpExt;

#[netstack_test]
async fn watcher_existing(name: &str) {
    // This test is limited to mostly IPv4 because IPv6 LL addresses are
    // subject to DAD and hard to test with Existing events.

    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum Expectation {
        Loopback(u64),
        Ethernet {
            id: u64,
            name: String,
            addr: fidl_fuchsia_net::Subnet,
            has_default_ipv4_route: bool,
            has_default_ipv6_route: bool,
            port_identity_koid: Option<zx::Koid>,
        },
    }

    impl
        PartialEq<
            fidl_fuchsia_net_interfaces_ext::Properties<
                fidl_fuchsia_net_interfaces_ext::AllInterest,
            >,
        > for Expectation
    {
        fn eq(
            &self,
            other: &fidl_fuchsia_net_interfaces_ext::Properties<
                fidl_fuchsia_net_interfaces_ext::AllInterest,
            >,
        ) -> bool {
            match self {
                Expectation::Loopback(id) => {
                    other
                        == &fidl_fuchsia_net_interfaces_ext::Properties {
                            id: (*id).try_into().expect("should be nonzero"),
                            name: "lo".to_owned(),
                            port_class: fidl_fuchsia_net_interfaces_ext::PortClass::Loopback,
                            online: true,
                            addresses: vec![
                                fidl_fuchsia_net_interfaces_ext::Address {
                                    addr: fidl_subnet!("127.0.0.1/8"),
                                    valid_until:
                                        fidl_fuchsia_net_interfaces_ext::PositiveMonotonicInstant::INFINITE_FUTURE,
                                    preferred_lifetime_info:
                                        fidl_fuchsia_net_interfaces_ext::PreferredLifetimeInfo::preferred_forever(),
                                    assignment_state:
                                        fnet_interfaces::AddressAssignmentState::Assigned,
                                },
                                fidl_fuchsia_net_interfaces_ext::Address {
                                    addr: fidl_subnet!("::1/128"),
                                    valid_until:
                                        fidl_fuchsia_net_interfaces_ext::PositiveMonotonicInstant::INFINITE_FUTURE,
                                    preferred_lifetime_info:
                                        fidl_fuchsia_net_interfaces_ext::PreferredLifetimeInfo::preferred_forever(),
                                    assignment_state:
                                        fnet_interfaces::AddressAssignmentState::Assigned,
                                },
                            ],
                            has_default_ipv4_route: false,
                            has_default_ipv6_route: false,
                            port_identity_koid: None,
                        }
                }
                Expectation::Ethernet {
                    id,
                    name,
                    addr,
                    has_default_ipv4_route,
                    has_default_ipv6_route,
                    port_identity_koid,
                } => {
                    let fidl_fuchsia_net_interfaces_ext::Properties {
                        id: rhs_id,
                        name: rhs_name,
                        port_class,
                        online,
                        addresses,
                        has_default_ipv4_route: rhs_ipv4_route,
                        has_default_ipv6_route: rhs_ipv6_route,
                        port_identity_koid: rhs_port_identity_koid,
                    } = other;

                    // We use contains here because netstack can generate
                    // link-local addresses that can't be predicted.
                    addresses.contains(&fidl_fuchsia_net_interfaces_ext::Address {
                        addr: *addr,
                        valid_until:
                            fidl_fuchsia_net_interfaces_ext::PositiveMonotonicInstant::INFINITE_FUTURE,
                        preferred_lifetime_info:
                            fidl_fuchsia_net_interfaces_ext::PreferredLifetimeInfo::preferred_forever(),
                        assignment_state: fnet_interfaces::AddressAssignmentState::Assigned,
                    }) && *online
                        && name == rhs_name
                        && id == &rhs_id.get()
                        && has_default_ipv4_route == rhs_ipv4_route
                        && has_default_ipv6_route == rhs_ipv6_route
                        && port_class == &fidl_fuchsia_net_interfaces_ext::PortClass::Virtual
                        && rhs_port_identity_koid == port_identity_koid
                }
            }
        }
    }

    let interfaces_state = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
        .expect("connect to protocol");

    let mut eps = Vec::new();
    let mut expectations = HashMap::<u64, _>::new();
    for (idx, (has_default_ipv4_route, has_default_ipv6_route)) in
        [true, false].into_iter().cartesian_product([true, false]).enumerate()
    {
        let if_name = format!("test-ep-{}", idx);

        let ep = sandbox.create_endpoint(if_name.clone()).await.expect("create endpoint");

        let iface = ep
            .into_interface_in_realm_with_name(
                &realm,
                netemul::InterfaceConfig {
                    name: Some(if_name.clone().into()),
                    ..Default::default()
                },
            )
            .await
            .expect("add device to stack");
        let id = iface.id();

        assert!(iface.control().enable().await.expect("send enable").expect("enable interface"));

        // Interface must be online for us to observe the address in the
        // assigned state later.
        iface.set_link_up(true).await.expect("bring device up");

        let port_identity_koid =
            Some(iface.endpoint().get_port_identity_koid().await.expect("get id event"));

        let addr = fidl_fuchsia_net::Subnet {
            addr: fidl_fuchsia_net::IpAddress::Ipv4(fidl_fuchsia_net::Ipv4Address {
                addr: [192, 168, idx.try_into().unwrap(), 1],
            }),
            prefix_len: 24,
        };
        let expected = Expectation::Ethernet {
            id,
            name: if_name,
            addr,
            has_default_ipv4_route,
            has_default_ipv6_route,
            port_identity_koid,
        };
        assert_eq!(expectations.insert(id, expected), None);
        let address_state_provider = interfaces::add_address_wait_assigned(
            iface.control(),
            addr,
            fidl_fuchsia_net_interfaces_admin::AddressParameters::default(),
        )
        .await
        .expect("add address");
        address_state_provider.detach().expect("detach address lifetime");

        if has_default_ipv4_route {
            iface
                .add_subnet_route(fidl_subnet!("0.0.0.0/0"))
                .await
                .expect("add default ipv4 route entry");
        }

        if has_default_ipv6_route {
            iface
                .add_subnet_route(fidl_subnet!("::/0"))
                .await
                .expect("add default ipv6 route entry");
        }
        eps.push(iface);
    }

    // The netstacks report the loopback interface as NIC 1.
    assert_eq!(expectations.insert(1, Expectation::Loopback(1)), None);

    // Guard against a newly created interface watcher observing a recently
    // added interface as offline by waiting for all interfaces to be online.
    fidl_fuchsia_net_interfaces_ext::wait_interface(
        fidl_fuchsia_net_interfaces_ext::event_stream_from_state::<
            fidl_fuchsia_net_interfaces_ext::AllInterest,
        >(&interfaces_state, Default::default())
        .expect("get interface event stream"),
        &mut HashMap::<u64, fidl_fuchsia_net_interfaces_ext::PropertiesAndState<(), _>>::new(),
        |properties_map| {
            (properties_map.iter().filter(|(_, iface)| iface.properties.online).count()
                == expectations.len())
            .then_some(())
        },
    )
    .await
    .expect("waiting for all interfaces to be online");

    let mut interfaces = fidl_fuchsia_net_interfaces_ext::existing(
        fidl_fuchsia_net_interfaces_ext::event_stream_from_state(
            &interfaces_state,
            Default::default(),
        )
        .expect("get interface event stream"),
        HashMap::<u64, fidl_fuchsia_net_interfaces_ext::PropertiesAndState<(), _>>::new(),
    )
    .await
    .expect("fetch existing interfaces");

    for (id, expected) in expectations.iter() {
        assert_eq!(
            expected,
            &interfaces.remove(id).unwrap_or_else(|| panic!("get interface {}", id)).properties
        );
    }

    assert_eq!(interfaces, HashMap::<u64, _>::new());
}

#[netstack_test]
async fn watcher_after_state_closed(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    // New scope so when we get back the WatcherProxy, the StateProxy is closed.
    let stream = {
        let interfaces_state = realm
            .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
            .expect("connect to protocol");
        let event_stream = fidl_fuchsia_net_interfaces_ext::event_stream_from_state::<
            fidl_fuchsia_net_interfaces_ext::AllInterest,
        >(&interfaces_state, Default::default())
        .expect("get interface event stream");
        event_stream
    };

    let interfaces = fidl_fuchsia_net_interfaces_ext::existing(
        stream,
        HashMap::<u64, fidl_fuchsia_net_interfaces_ext::PropertiesAndState<(), _>>::new(),
    )
    .await
    .expect("collect interfaces");
    let expected = std::iter::once((
        1,
        fidl_fuchsia_net_interfaces_ext::PropertiesAndState {
            properties: fidl_fuchsia_net_interfaces_ext::Properties {
                id: 1.try_into().expect("should be nonzero"),
                name: "lo".to_owned(),
                port_class: fidl_fuchsia_net_interfaces_ext::PortClass::Loopback,
                online: true,
                addresses: vec![
                    fidl_fuchsia_net_interfaces_ext::Address {
                        addr: fidl_subnet!("127.0.0.1/8"),
                        valid_until:
                            fidl_fuchsia_net_interfaces_ext::PositiveMonotonicInstant::INFINITE_FUTURE,
                        preferred_lifetime_info:
                            fidl_fuchsia_net_interfaces_ext::PreferredLifetimeInfo::preferred_forever(),
                        assignment_state: fnet_interfaces::AddressAssignmentState::Assigned,
                    },
                    fidl_fuchsia_net_interfaces_ext::Address {
                        addr: fidl_subnet!("::1/128"),
                        valid_until:
                            fidl_fuchsia_net_interfaces_ext::PositiveMonotonicInstant::INFINITE_FUTURE,
                        preferred_lifetime_info:
                            fidl_fuchsia_net_interfaces_ext::PreferredLifetimeInfo::preferred_forever(),
                        assignment_state: fnet_interfaces::AddressAssignmentState::Assigned,
                    },
                ],
                has_default_ipv4_route: false,
                has_default_ipv6_route: false,
                port_identity_koid: None,
            },
            state: (),
        },
    ))
    .collect();
    assert_eq!(interfaces, expected);
}

/// Tests that adding an interface causes an interface changed event.
#[netstack_test]
async fn test_add_remove_interface(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let device = sandbox.create_endpoint(name).await.expect("create endpoint");

    let iface = device.into_interface_in_realm(&realm).await.expect("add device");
    let id = iface.id();

    let interface_state = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
        .expect("connect to protocol");
    let event_stream = fidl_fuchsia_net_interfaces_ext::event_stream_from_state::<
        fidl_fuchsia_net_interfaces_ext::DefaultInterest,
    >(&interface_state, Default::default())
    .expect("get interface event stream");
    let mut event_stream = pin!(event_stream);

    let mut if_map =
        HashMap::<u64, fidl_fuchsia_net_interfaces_ext::PropertiesAndState<(), _>>::new();
    fidl_fuchsia_net_interfaces_ext::wait_interface(event_stream.by_ref(), &mut if_map, |if_map| {
        if_map.contains_key(&id).then_some(())
    })
    .await
    .expect("observe interface addition");

    let (_endpoint, _device_control) = iface.remove().await.expect("failed to remove interface");

    fidl_fuchsia_net_interfaces_ext::wait_interface(event_stream.by_ref(), &mut if_map, |if_map| {
        (!if_map.contains_key(&id)).then_some(())
    })
    .await
    .expect("observe interface deletion");
}

/// Tests that including all addresses includes temporary and unavailable
/// addresses.
#[netstack_test]
#[variant(I, Ip)]
async fn test_include_all_addresses<I: Ip>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    let interface_state = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
        .expect("connect to protocol");
    let event_stream = fidl_fuchsia_net_interfaces_ext::event_stream_from_state(
        &interface_state,
        fidl_fuchsia_net_interfaces_ext::WatchOptions {
            included_addresses: fidl_fuchsia_net_interfaces_ext::IncludedAddresses::All,
            ..Default::default()
        },
    )
    .expect("get interface event stream");
    let mut event_stream = pin!(event_stream);

    let device = sandbox.create_endpoint(name).await.expect("create endpoint");
    device.set_link_up(true).await.expect("send link up");
    let iface = device.into_interface_in_realm(&realm).await.expect("add device");
    let id = iface.id();

    let mut state = fidl_fuchsia_net_interfaces_ext::InterfaceState::<(), _>::Unknown(id);
    fidl_fuchsia_net_interfaces_ext::wait_interface_with_id(
        event_stream.by_ref(),
        &mut state,
        |_| Some(()),
    )
    .await
    .expect("observe interface addition");

    async fn wait_until_address_state(
        want_assignment_state: fidl_fuchsia_net_interfaces::AddressAssignmentState,
        want_addr: fidl_fuchsia_net::Subnet,
        state: &mut fidl_fuchsia_net_interfaces_ext::InterfaceState<
            (),
            fidl_fuchsia_net_interfaces_ext::AllInterest,
        >,
        event_stream: impl Stream<
            Item = std::result::Result<
                fidl_fuchsia_net_interfaces_ext::EventWithInterest<
                    fidl_fuchsia_net_interfaces_ext::AllInterest,
                >,
                fidl::Error,
            >,
        >,
    ) {
        fidl_fuchsia_net_interfaces_ext::wait_interface_with_id(event_stream, state, |iface| {
            iface
                .properties
                .addresses
                .iter()
                .any(
                    |fidl_fuchsia_net_interfaces_ext::Address {
                         addr, assignment_state, ..
                     }| {
                        *addr == want_addr && *assignment_state == want_assignment_state
                    },
                )
                .then_some(())
        })
        .await
        .expect("observe address change")
    }

    let (addr, addr_params) = match I::VERSION {
        IpVersion::V4 => (
            fidl_subnet!("192.0.2.1/24"),
            fidl_fuchsia_net_interfaces_admin::AddressParameters {
                perform_dad: Some(true),
                ..Default::default()
            },
        ),
        IpVersion::V6 => (
            fidl_subnet!("a::1/64"),
            // NB: For IPv6 DAD is performed by default, so there's no need to
            // opt in like for IPv4.
            fidl_fuchsia_net_interfaces_admin::AddressParameters::default(),
        ),
    };

    // Address should appear as unavailable when it is added on a disabled
    // interface.
    {
        let (address_state_provider, server) = fidl::endpoints::create_proxy::<
            fidl_fuchsia_net_interfaces_admin::AddressStateProviderMarker,
        >();
        address_state_provider.detach().expect("detach address lifetime");
        iface.control().add_address(&addr, &addr_params, server).expect("send add address request");
        // We need to wait for the `OnAddressAdded` before dropping the
        // ASP to make sure that the `Detach` request above is handled.
        // See https://bugs.fuchsia.dev/p/fuchsia/issues/detail?id=131322
        // for more details.
        fnet_interfaces_ext::admin::wait_for_address_added_event(
            &mut address_state_provider.take_event_stream(),
        )
        .await
        .expect("wait for address successfully added");
    }
    wait_until_address_state(
        fidl_fuchsia_net_interfaces::AddressAssignmentState::Unavailable,
        addr,
        &mut state,
        event_stream.by_ref(),
    )
    .await;

    // We should see the address transition from unavailable to tentative then
    // assigned when the interface is assigned.
    assert!(iface.control().enable().await.expect("send enable").expect("enable interface"));
    wait_until_address_state(
        fidl_fuchsia_net_interfaces::AddressAssignmentState::Tentative,
        addr,
        &mut state,
        event_stream.by_ref(),
    )
    .await;
    wait_until_address_state(
        fidl_fuchsia_net_interfaces::AddressAssignmentState::Assigned,
        addr,
        &mut state,
        event_stream.by_ref(),
    )
    .await;

    // We should see the address transition from assigned to unavailable when
    // the interface is disabled.
    assert!(iface.control().disable().await.expect("send enable").expect("enable interface"));
    wait_until_address_state(
        fidl_fuchsia_net_interfaces::AddressAssignmentState::Unavailable,
        addr,
        &mut state,
        event_stream.by_ref(),
    )
    .await;
}

/// Tests that adding/removing a default route causes an interface changed event.
#[netstack_test]
#[variant(I, Ip)]
async fn test_add_remove_default_route<I: net_types::ip::Ip>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    // Add an interface and watch for its addition.
    let device = sandbox.create_endpoint(name).await.expect("create endpoint");
    let iface = device.into_interface_in_realm(&realm).await.expect("add device");
    let id = iface.id();
    iface.set_link_up(true).await.expect("bring device up");
    assert!(iface.control().enable().await.expect("send enable").expect("enable interface"));
    let interface_state = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
        .expect("connect to protocol");
    let event_stream = fidl_fuchsia_net_interfaces_ext::event_stream_from_state::<
        fidl_fuchsia_net_interfaces_ext::DefaultInterest,
    >(&interface_state, Default::default())
    .expect("get interface event stream");
    let mut event_stream = pin!(event_stream);
    let mut if_map =
        HashMap::<u64, fidl_fuchsia_net_interfaces_ext::PropertiesAndState<(), _>>::new();
    fidl_fuchsia_net_interfaces_ext::wait_interface(event_stream.by_ref(), &mut if_map, |if_map| {
        if_map.contains_key(&id).then_some(())
    })
    .await
    .expect("observe interface addition");

    // Ip generic helper function to check for the presence of a default route.
    let has_default_route =
        |iface: &fnet_interfaces_ext::PropertiesAndState<(), _>| match I::VERSION {
            IpVersion::V4 => iface.properties.has_default_ipv4_route,
            IpVersion::V6 => iface.properties.has_default_ipv6_route,
        };

    // Add the default route and watch for its addition.
    let route = match I::VERSION {
        IpVersion::V4 => fidl_subnet!("0.0.0.0/0"),
        IpVersion::V6 => fidl_subnet!("::/0"),
    };
    iface.add_subnet_route(route).await.expect("add default route");

    fidl_fuchsia_net_interfaces_ext::wait_interface(event_stream.by_ref(), &mut if_map, |if_map| {
        if_map.get(&id).map(|properties| has_default_route(properties).then_some(())).flatten()
    })
    .await
    .expect("observe default route addition");

    // Remove the default route and watch for its removal.
    iface.del_subnet_route(route).await.expect("remove default route");

    fidl_fuchsia_net_interfaces_ext::wait_interface(event_stream.by_ref(), &mut if_map, |if_map| {
        if_map.get(&id).map(|properties| (!has_default_route(properties)).then_some(())).flatten()
    })
    .await
    .expect("observe default route removal");
}

/// Tests that interface default route state depends on all route tables.
#[netstack_test]
#[variant(I, Ip)]
async fn check_default_routes_in_all_tables<I: Ip + FidlRouteAdminIpExt + FidlRouteIpExt>(
    name: &str,
) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    // Add an interface and watch for its addition.
    let device = sandbox.create_endpoint(name).await.expect("create endpoint");
    let iface = device.into_interface_in_realm(&realm).await.expect("add device");
    let id = iface.id();
    iface.set_link_up(true).await.expect("bring device up");
    assert!(iface.control().enable().await.expect("send enable").expect("enable interface"));
    let interface_state = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
        .expect("connect to protocol");
    let event_stream = fidl_fuchsia_net_interfaces_ext::event_stream_from_state::<
        fidl_fuchsia_net_interfaces_ext::DefaultInterest,
    >(&interface_state, Default::default())
    .expect("get interface event stream");
    let mut event_stream = pin!(event_stream);
    let mut if_map =
        HashMap::<u64, fidl_fuchsia_net_interfaces_ext::PropertiesAndState<(), _>>::new();
    fidl_fuchsia_net_interfaces_ext::wait_interface(event_stream.by_ref(), &mut if_map, |if_map| {
        if_map.contains_key(&id).then_some(())
    })
    .await
    .expect("observe interface addition");

    let route_table_provider = realm
        .connect_to_protocol::<I::RouteTableProviderMarker>()
        .expect("connect to route table provider");
    let user_route_table =
        fnet_routes_ext::admin::new_route_table::<I>(&route_table_provider, None)
            .expect("create new user table");
    let user_table_id =
        fnet_routes_ext::admin::get_table_id::<I>(&user_route_table).await.expect("get table id");
    let user_route_set = fnet_routes_ext::admin::new_route_set::<I>(&user_route_table)
        .expect("failed to create a new user route set");
    let main_route_set = iface
        .create_authenticated_global_route_set::<I>()
        .await
        .expect("creating global route set for main table should succeed");
    let state = realm.connect_to_protocol::<I::StateMarker>().expect("connect to routes State");
    let mut routes_stream = pin!(
        fnet_routes_ext::event_stream_from_state_with_options::<I>(
            &state,
            fnet_routes_ext::WatcherOptions {
                table_interest: Some(fnet_routes::TableInterest::All(fnet_routes::All)),
            },
        )
        .expect("failed to watch the main table")
    );

    let existing_routes =
        fnet_routes_ext::collect_routes_until_idle::<I, Vec<_>>(&mut routes_stream)
            .await
            .expect("collect routes should succeed");

    assert_eq!(
        existing_routes.iter().find(|installed_route| installed_route.table_id == user_table_id),
        None
    );
    let grant = iface.get_authorization().await.expect("getting grant should succeed");
    let proof = fnet_interfaces_ext::admin::proof_from_grant(&grant);
    fnet_routes_ext::admin::authenticate_for_interface::<I>(&user_route_set, proof)
        .await
        .expect("no FIDL error")
        .expect("authentication should succeed");

    // Ip generic helper function to check for the presence of a default route.
    let has_default_route =
        |iface: &fnet_interfaces_ext::PropertiesAndState<(), _>| match I::VERSION {
            IpVersion::V4 => iface.properties.has_default_ipv4_route,
            IpVersion::V6 => iface.properties.has_default_ipv6_route,
        };

    // Add the default route to the user route table, and check that the
    // interface watcher doesn't yield anything.
    let default_route = fnet_routes_ext::Route {
        destination: I::map_ip_out(
            (),
            |()| net_subnet_v4!("0.0.0.0/0"),
            |()| net_subnet_v6!("::/0"),
        ),
        action: fnet_routes_ext::RouteAction::Forward(fnet_routes_ext::RouteTarget {
            outbound_interface: iface.id(),
            next_hop: None,
        }),
        properties: fnet_routes_ext::RouteProperties::from_explicit_metric(0),
    };

    assert!(
        fnet_routes_ext::admin::add_route::<I>(
            &user_route_set,
            &default_route.try_into().expect("convert to FIDL")
        )
        .await
        .expect("no FIDL error")
        .expect("add route should succeed")
    );

    // We should observe default route presence in the interface watcher.
    fidl_fuchsia_net_interfaces_ext::wait_interface(event_stream.by_ref(), &mut if_map, |if_map| {
        has_default_route(if_map.get(&id).expect("should have interface")).then_some(())
    })
    .await
    .expect("observe default route addition");

    // Add the default route to the main table and observe the default route is still there.
    assert!(
        fnet_routes_ext::admin::add_route::<I>(
            &main_route_set,
            &default_route.try_into().expect("convert to FIDL")
        )
        .await
        .expect("no FIDL error")
        .expect("add route should succeed")
    );
    fidl_fuchsia_net_interfaces_ext::wait_interface(event_stream.by_ref(), &mut if_map, |if_map| {
        (!has_default_route(if_map.get(&id).expect("should have interface"))).then_some(())
    })
    .map(|result| {
        result.expect("should not get error");
        panic!("should not observe default route removal")
    })
    .on_timeout(ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT, || ())
    .await;

    // Remove the default route from the user route table. We should not observe
    // any notification in the interface watcher.
    assert!(
        fnet_routes_ext::admin::remove_route::<I>(
            &user_route_set,
            &default_route.try_into().expect("convert to FIDL")
        )
        .await
        .expect("no FIDL error")
        .expect("remove route should succeed")
    );
    fidl_fuchsia_net_interfaces_ext::wait_interface(event_stream.by_ref(), &mut if_map, |if_map| {
        (!has_default_route(if_map.get(&id).expect("should have interface"))).then_some(())
    })
    .map(|result| {
        result.expect("should not get error");
        panic!("should not observe default route removal")
    })
    .on_timeout(ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT, || ())
    .await;

    // Now remove the default route from the main table. We should observe this
    // via the interfaces watcher because this is the last default route.
    assert!(
        fnet_routes_ext::admin::remove_route::<I>(
            &main_route_set,
            &default_route.try_into().expect("convert to FIDL")
        )
        .await
        .expect("no FIDL error")
        .expect("remove route should succeed")
    );
    fidl_fuchsia_net_interfaces_ext::wait_interface(event_stream.by_ref(), &mut if_map, |if_map| {
        (!has_default_route(if_map.get(&id).expect("should have interface"))).then_some(())
    })
    .await
    .expect("observe default route removal");
}

/// Tests that if a device closes (is removed from the system), the
/// corresponding Netstack interface is deleted.
/// if `enabled` is `true`, enables the interface before closing the device.
#[netstack_test]
#[test_case("disabled", false; "disabled")]
#[test_case("enabled", true ; "enabled")]
async fn test_close_interface(test_name: &str, sub_test_name: &str, enabled: bool) {
    let name = format!("{}_{}", test_name, sub_test_name);
    let name = name.as_str();

    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let device = sandbox.create_endpoint(name).await.expect("create endpoint");

    let iface = device.into_interface_in_realm(&realm).await.expect("add device");
    let id = iface.id();

    if enabled {
        let did_enable = iface.control().enable().await.expect("send enable").expect("enable");
        assert!(did_enable);
    }

    let interface_state = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
        .expect("connect to protocol");
    let event_stream = fidl_fuchsia_net_interfaces_ext::event_stream_from_state::<
        fidl_fuchsia_net_interfaces_ext::DefaultInterest,
    >(&interface_state, Default::default())
    .expect("get interface event stream");
    let mut event_stream = pin!(event_stream);
    let mut if_map =
        HashMap::<u64, fidl_fuchsia_net_interfaces_ext::PropertiesAndState<(), _>>::new();
    fidl_fuchsia_net_interfaces_ext::wait_interface(event_stream.by_ref(), &mut if_map, |if_map| {
        if_map.contains_key(&id).then_some(())
    })
    .await
    .expect("observe interface addition");

    let (_control, _device_control) = iface.remove_device();

    // Wait until we observe the removed interface is missing.
    fidl_fuchsia_net_interfaces_ext::wait_interface(event_stream.by_ref(), &mut if_map, |if_map| {
        (!if_map.contains_key(&id)).then_some(())
    })
    .await
    .expect("observe interface removal");
}

/// Tests races between device link down and close.
#[netstack_test]
async fn test_down_close_race(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create netstack realm");
    let interface_state = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
        .expect("connect to protocol");
    let event_stream = fidl_fuchsia_net_interfaces_ext::event_stream_from_state::<
        fidl_fuchsia_net_interfaces_ext::DefaultInterest,
    >(&interface_state, Default::default())
    .expect("event stream from state");
    let mut event_stream = pin!(event_stream);
    let mut if_map =
        HashMap::<u64, fidl_fuchsia_net_interfaces_ext::PropertiesAndState<(), _>>::new();

    for _ in 0..10u64 {
        let dev = sandbox
            .create_endpoint("ep")
            .await
            .expect("create endpoint")
            .into_interface_in_realm(&realm)
            .await
            .expect("add endpoint to Netstack");

        let did_enable = dev.control().enable().await.expect("send enable").expect("enable");
        assert!(did_enable);
        dev.set_link_up(true).await.expect("bring device up");

        let id = dev.id();
        // Wait until the interface is installed and the link state is up.
        fidl_fuchsia_net_interfaces_ext::wait_interface(
            event_stream.by_ref(),
            &mut if_map,
            |if_map| if_map.get(&id)?.properties.online.then_some(()),
        )
        .await
        .expect("observe interface online");

        // Here's where we cause the race. We bring the device's link down
        // and drop it right after; the two signals will race to reach
        // Netstack.
        dev.set_link_up(false).await.expect("bring link down");
        std::mem::drop(dev);

        // Wait until the interface is removed from Netstack cleanly.
        fidl_fuchsia_net_interfaces_ext::wait_interface(
            event_stream.by_ref(),
            &mut if_map,
            |if_map| (!if_map.contains_key(&id)).then_some(()),
        )
        .await
        .expect("observe interface removal");
    }
}

/// Tests races between data traffic and closing a device.
#[netstack_test]
async fn test_close_data_race(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let net = sandbox.create_network("net").await.expect("create network");
    let fake_ep = net.create_fake_endpoint().expect("create fake endpoint");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create netstack realm");

    // NOTE: We only run this test with IPv4 sockets since we only care about
    // exciting the tx path, the domain is irrelevant.
    const DEVICE_ADDRESS_IN_SUBNET: fidl_fuchsia_net::Subnet = fidl_subnet!("192.168.0.2/24");
    // We're going to send data over a UDP socket to a multicast address so we
    // skip ARP resolution.
    const MCAST_ADDR: std::net::IpAddr = std_ip!("224.0.0.1");

    let interface_state = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
        .expect("connect to protocol");
    let event_stream = fidl_fuchsia_net_interfaces_ext::event_stream_from_state::<
        fidl_fuchsia_net_interfaces_ext::DefaultInterest,
    >(&interface_state, Default::default())
    .expect("get interface event stream");
    let mut event_stream = pin!(event_stream);
    let mut if_map =
        HashMap::<u64, fidl_fuchsia_net_interfaces_ext::PropertiesAndState<(), _>>::new();
    for _ in 0..10u64 {
        let dev = net
            .create_endpoint("ep")
            .await
            .expect("create endpoint")
            .into_interface_in_realm(&realm)
            .await
            .expect("add endpoint to Netstack");

        assert!(dev.control().enable().await.expect("send enable").expect("enable"));
        dev.set_link_up(true).await.expect("bring device up");

        let address_state_provider = interfaces::add_address_wait_assigned(
            dev.control(),
            DEVICE_ADDRESS_IN_SUBNET,
            fidl_fuchsia_net_interfaces_admin::AddressParameters {
                add_subnet_route: Some(true),
                ..Default::default()
            },
        )
        .await
        .expect("add subnet address and route");
        address_state_provider.detach().expect("detach address lifetime");

        // Create a socket and start sending data on it nonstop.
        let fidl_fuchsia_net_ext::IpAddress(bind_addr) = DEVICE_ADDRESS_IN_SUBNET.addr.into();
        let sock = fuchsia_async::net::UdpSocket::bind_in_realm(
            &realm,
            std::net::SocketAddr::new(bind_addr, 0),
        )
        .await
        .expect("create socket");

        // Keep sending data until writing to the socket fails.
        let io_fut = async {
            let mut write_wait_interval = zx::MonotonicDuration::from_micros(10);
            loop {
                match sock
                    .send_to(&[1u8, 2, 3, 4], std::net::SocketAddr::new(MCAST_ADDR, 1234))
                    .await
                {
                    Ok(_sent) => {}
                    // We expect only "os errors" to happen, ideally we'd look
                    // only at specific errors (EPIPE, ENETUNREACH), but that
                    // made this test very flaky due to the branching error
                    // paths in gVisor when removing an interface.
                    Err(e) if e.raw_os_error().is_some() => break Result::Ok(()),
                    Err(e) => break Err(e).context("send_to error"),
                }

                // Enqueue some data on the rx path.
                fake_ep
                    // We don't care that it's a valid frame, only that it excites
                    // the rx path.
                    .write(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16])
                    .await
                    .expect("send frame on fake_ep");

                // Wait on a short timer with an exponential backoff to reduce log noise
                // when running the test (which can cause timeouts due to slowness in
                // the logging framework).
                fuchsia_async::Timer::new(fuchsia_async::MonotonicInstant::after(
                    write_wait_interval,
                ))
                .await;
                write_wait_interval = write_wait_interval * 2;
            }
        };

        let id = dev.id();
        let drop_fut = async move {
            fuchsia_async::Timer::new(fuchsia_async::MonotonicInstant::after(
                zx::MonotonicDuration::from_millis(3),
            ))
            .await;
            std::mem::drop(dev);
        };

        let iface_dropped = fidl_fuchsia_net_interfaces_ext::wait_interface(
            event_stream.by_ref(),
            &mut if_map,
            |if_map| (!if_map.contains_key(&id)).then_some(()),
        );

        let (io_result, iface_dropped, ()) =
            futures::future::join3(io_fut, iface_dropped, drop_fut).await;
        io_result.expect("unexpected error on io future");
        iface_dropped.expect("observe interface removal");
    }
}

/// Tests that when an interface is enabled and removed, no disable event is
/// observed before the remove event.
#[netstack_test]
async fn test_remove_enabled_interface(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create netstack realm");

    let interface_state = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
        .expect("connect to protocol");

    let ep = sandbox
        .create_endpoint("ep")
        .await
        .expect("create fixed ep")
        .into_interface_in_realm(&realm)
        .await
        .expect("install in realm");
    ep.set_link_up(true).await.expect("bring link up");

    let event_stream = fidl_fuchsia_net_interfaces_ext::event_stream_from_state::<
        fidl_fuchsia_net_interfaces_ext::DefaultInterest,
    >(&interface_state, Default::default())
    .expect("get interface event stream")
    .map(|r| r.expect("watcher error"))
    .fuse();
    let mut event_stream = pin!(event_stream);

    // Consume the watcher until we see the idle event.
    let mut existing = fidl_fuchsia_net_interfaces_ext::existing(
        event_stream.by_ref().map(std::result::Result::<_, fidl::Error>::Ok),
        HashMap::<u64, fidl_fuchsia_net_interfaces_ext::PropertiesAndState<(), _>>::new(),
    )
    .await
    .expect("existing");

    let iface_id = ep.id();
    let mut interface_state = fidl_fuchsia_net_interfaces_ext::InterfaceState::<(), _>::Known({
        let interface_state = existing.remove(&iface_id).unwrap();
        assert!(!interface_state.properties.online);
        interface_state
    });

    // Now enable the interface, then wait to see that it is enabled
    assert!(ep.control().enable().await.expect("send enable").expect("enable"));
    fidl_fuchsia_net_interfaces_ext::wait_interface_with_id(
        event_stream.by_ref().map(std::result::Result::<_, fidl::Error>::Ok),
        &mut interface_state,
        |s| s.properties.online.then_some(()),
    )
    .await
    .expect("is enabled");
    let mut interface_state = match interface_state {
        fidl_fuchsia_net_interfaces_ext::InterfaceState::Known(k) => k,
        fidl_fuchsia_net_interfaces_ext::InterfaceState::Unknown(_) => {
            unreachable!("interface is still known")
        }
    };

    #[derive(Debug)]
    struct InterfaceWasDisabled;

    // Disable the interface and wait for it to disappear (but not be disabled).
    drop(ep);
    assert_matches::assert_matches!(
        fidl_fuchsia_net_interfaces_ext::wait_interface(
            event_stream.by_ref().map(std::result::Result::<_, fidl::Error>::Ok),
            &mut interface_state,
            // Check that the interface does not go offline. Anything else can be
            // ignored.
            |s| (!s.properties.online).then_some(InterfaceWasDisabled)
        )
        .await,
        Err(fidl_fuchsia_net_interfaces_ext::WatcherOperationError::Update(
            fidl_fuchsia_net_interfaces_ext::UpdateError::Removed
        ))
    )
}

/// Tests that toggling interface enabled repeatedly results in every change
/// in the boolean value being observable.
#[netstack_test]
async fn test_watcher_online_edges(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create netstack realm");

    let interface_state = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
        .expect("connect to protocol");
    let event_stream = fidl_fuchsia_net_interfaces_ext::event_stream_from_state::<
        fidl_fuchsia_net_interfaces_ext::DefaultInterest,
    >(&interface_state, Default::default())
    .expect("get interface event stream")
    .map(|r| r.expect("watcher error"))
    .fuse();
    let mut event_stream = pin!(event_stream);

    // Consume the watcher until we see the idle event.
    let existing = fidl_fuchsia_net_interfaces_ext::existing(
        event_stream.by_ref().map(std::result::Result::<_, fidl::Error>::Ok),
        HashMap::<u64, fidl_fuchsia_net_interfaces_ext::PropertiesAndState<(), _>>::new(),
    )
    .await
    .expect("existing");
    // Only loopback should exist.
    assert_eq!(existing.len(), 1, "unexpected interfaces in existing: {:?}", existing);

    let ep = sandbox
        // We don't need to run variants for this test, all we care about is
        // the Netstack race. Use NetworkDevice because it's lighter weight.
        .create_endpoint("ep")
        .await
        .expect("create fixed ep")
        .into_interface_in_realm(&realm)
        .await
        .expect("install in realm");
    let iface_id = ep.id();
    assert_matches::assert_matches!(
        event_stream.select_next_some().await.into_inner(),
        fidl_fuchsia_net_interfaces::Event::Added(fidl_fuchsia_net_interfaces::Properties {
            id: Some(id),
            online: Some(false),
            ..
        }) => id == iface_id
    );
    // NB: Need to set link up and ensure that Netstack has observed link-up
    // (by also enabling the interface and observing that online changes
    // to true); otherwise enabling/disabling the interface may not change
    // interface online as we'd expect.
    ep.set_link_up(true).await.expect("bring link up");
    assert!(ep.control().enable().await.expect("send enable").expect("enable"));
    assert_matches::assert_matches!(
        event_stream.select_next_some().await.into_inner(),
        fidl_fuchsia_net_interfaces::Event::Changed(fidl_fuchsia_net_interfaces::Properties {
            id: Some(id),
            online: Some(true),
            ..
        }) => id == iface_id
    );

    // Future which concurrently enables and disables the interface a set
    // number of iterations.  Note that the raciness is intentional: the
    // interface may be enabled/disabled less than the number of iterations.
    let toggle_online_fut = {
        // The Netstack's event queue size is 128. Because events are
        // not observed (aka watched) as soon as they occur in this test, it's
        // possible to fill the event queues in the Netstack, causing it to drop
        // events. Keeping the number of iterations <= 50 prevents this (each
        // iteration yields two events: enable & disable).
        const ITERATIONS: usize = 50;
        let enable_fut = futures::stream::iter(std::iter::repeat(()).take(ITERATIONS)).fold(
            (ep, 0),
            |(ep, change_count), ()| async move {
                if ep.control().enable().await.expect("send enable").expect("enable") {
                    (ep, change_count + 1)
                } else {
                    (ep, change_count)
                }
            },
        );
        let disable_fut = {
            let root_interfaces = realm
                .connect_to_protocol::<fidl_fuchsia_net_root::InterfacesMarker>()
                .expect("connect to protocol");
            let (control, server) =
                fidl::endpoints::create_proxy::<fidl_fuchsia_net_interfaces_admin::ControlMarker>();
            root_interfaces.get_admin(iface_id, server).expect("send get_admin");
            futures::stream::iter(std::iter::repeat(()).take(ITERATIONS)).fold(
                0,
                move |change_count, ()| {
                    control.disable().map(move |r| {
                        change_count
                            + if r.expect("send disable").expect("disable") { 1 } else { 0 }
                    })
                },
            )
        };
        futures::future::join(enable_fut, disable_fut).map(|((ep, enable_count), disable_count)| {
            // Removes the interface.
            std::mem::drop(ep);
            (enable_count, disable_count)
        })
    };

    // Future which consumes interface watcher events and tallies number of
    // offline->online edges (and vice versa).
    let watcher_fut = event_stream
        .take_while(|e| {
            futures::future::ready(match e.inner() {
                fidl_fuchsia_net_interfaces::Event::Removed(removed_id) => *removed_id != iface_id,
                fidl_fuchsia_net_interfaces::Event::Added(_)
                | fidl_fuchsia_net_interfaces::Event::Existing(_)
                | fidl_fuchsia_net_interfaces::Event::Changed(_)
                | fidl_fuchsia_net_interfaces::Event::Idle(fidl_fuchsia_net_interfaces::Empty) => {
                    true
                }
            })
        })
        .fold((0, 0, true), |(enable_count, disable_count, online_prev), event| {
            let online_next = assert_matches::assert_matches!(
                event.into_inner(),
                fidl_fuchsia_net_interfaces::Event::Changed(
                    fidl_fuchsia_net_interfaces::Properties {
                        id: Some(id),
                        online,
                        ..
                    },
                ) if id == iface_id => online
            );
            futures::future::ready(match online_next {
                None => (enable_count, disable_count, online_prev),
                Some(online_next) => match (online_prev, online_next) {
                    (false, true) => (enable_count + 1, disable_count, online_next),
                    (true, false) => (enable_count, disable_count + 1, online_next),
                    (prev, next) => {
                        panic!(
                            "online changed event with no change: prev = {}, next = {}",
                            prev, next
                        )
                    }
                },
            })
        });

    let ((want_enable_count, want_disable_count), (got_enable_count, got_disable_count, online)) =
        futures::future::join(toggle_online_fut, watcher_fut).await;
    assert_eq!((got_enable_count, got_disable_count), (want_enable_count, want_disable_count));
    // Since we started with the interface being online, if we end on offline
    // then there must have been one more disable than enable.
    assert_eq!(got_disable_count, if !online { got_enable_count + 1 } else { got_enable_count });
}

/// Tests that competing interface change events are reported by
/// fuchsia.net.interfaces/Watcher in the correct order.
#[netstack_test]
async fn test_watcher_race(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create netstack realm");
    let interface_state = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
        .expect("connect to protocol");
    let root_interfaces = realm
        .connect_to_protocol::<fidl_fuchsia_net_root::InterfacesMarker>()
        .expect("connect to protocol");
    for _ in 0..100 {
        let (watcher, server) =
            fidl::endpoints::create_proxy::<fidl_fuchsia_net_interfaces::WatcherMarker>();
        interface_state
            .get_watcher(&fidl_fuchsia_net_interfaces::WatcherOptions::default(), server)
            .expect("initialize interface watcher");

        let ep = sandbox
            // We don't need to run variants for this test, all we care about is
            // the Netstack race. Use NetworkDevice because it's lighter weight.
            .create_endpoint("ep")
            .await
            .expect("create fixed ep")
            .into_interface_in_realm(&realm)
            .await
            .expect("install in realm");

        const ADDR: fidl_fuchsia_net::Subnet = fidl_subnet!("192.168.0.1/24");
        let control_add_address = {
            let (control, server_end) =
                fidl_fuchsia_net_interfaces_ext::admin::Control::create_endpoints()
                    .expect("create endpoints");
            root_interfaces.get_admin(ep.id(), server_end).expect("send get_admin");
            control
        };

        // Bring the link up, enable the interface, add an IP address,
        // and a default route for the address "non-sequentially" (as much
        // as possible) to cause races in Netstack when reporting events.
        let ((), (), (), ()) = futures::future::join4(
            ep.set_link_up(true).map(|r| r.expect("bring link up")),
            async {
                let did_enable = ep.control().enable().await.expect("send enable").expect("enable");
                assert!(did_enable);
            },
            async {
                let address_state_provider = interfaces::add_address_wait_assigned(
                    &control_add_address,
                    ADDR,
                    fidl_fuchsia_net_interfaces_admin::AddressParameters::default(),
                )
                .await
                .expect("add address");
                address_state_provider.detach().expect("detach address lifetime");
            },
            ep.add_subnet_route(fidl_subnet!("0.0.0.0/0")).map(|r| r.expect("add default route")),
        )
        .await;

        let id = ep.id();
        let () =
            futures::stream::unfold(
                (watcher, false, false, false, false),
                |(watcher, present, up, has_addr, has_default_ipv4_route)| async move {
                    let event = watcher.watch().await.expect("watch");

                    let (
                        mut new_present,
                        mut new_up,
                        mut new_has_addr,
                        mut new_has_default_ipv4_route,
                    ) = (present, up, has_addr, has_default_ipv4_route);
                    match event {
                        fidl_fuchsia_net_interfaces::Event::Added(properties)
                        | fidl_fuchsia_net_interfaces::Event::Existing(properties) => {
                            if properties.id == Some(id) {
                                assert!(!present, "duplicate added/existing event");
                                new_present = true;
                                new_up = properties
                                    .online
                                    .expect("added/existing event missing online property");
                                new_has_addr = properties
                                    .addresses
                                    .expect("added/existing event missing addresses property")
                                    .iter()
                                    .any(
                                        |fidl_fuchsia_net_interfaces::Address {
                                             addr,
                                             valid_until: _,
                                             ..
                                         }| {
                                            addr == &Some(ADDR)
                                        },
                                    );
                                new_has_default_ipv4_route = properties
                                    .has_default_ipv4_route
                                    .expect(
                                    "added/existing event missing has_default_ipv4_route property",
                                );
                            }
                        }
                        fidl_fuchsia_net_interfaces::Event::Changed(
                            fidl_fuchsia_net_interfaces::Properties {
                                id: changed_id,
                                online,
                                addresses,
                                has_default_ipv4_route,
                                ..
                            },
                        ) => {
                            if changed_id == Some(id) {
                                assert!(
                                    present,
                                    "property change event before added or existing event"
                                );
                                if let Some(online) = online {
                                    new_up = online;
                                }
                                if let Some(addresses) = addresses {
                                    new_has_addr = addresses.iter().any(
                                        |fidl_fuchsia_net_interfaces::Address {
                                             addr,
                                             valid_until: _,
                                             ..
                                         }| {
                                            addr == &Some(ADDR)
                                        },
                                    );
                                }
                                if let Some(has_default_ipv4_route) = has_default_ipv4_route {
                                    new_has_default_ipv4_route = has_default_ipv4_route;
                                }
                            }
                        }
                        fidl_fuchsia_net_interfaces::Event::Removed(removed_id) => {
                            if removed_id == id {
                                assert!(present, "removed event before added or existing");
                                new_present = false;
                            }
                        }
                        _ => {}
                    }
                    println!(
                        "Observed interfaces, previous = ({}, {}, {}, {}), new = ({}, {}, {}, {})",
                        present,
                        up,
                        has_addr,
                        has_default_ipv4_route,
                        new_present,
                        new_up,
                        new_has_addr,
                        new_has_default_ipv4_route
                    );

                    // Verify that none of the observed states can be seen as
                    // "undone" by bad event ordering in Netstack. We don't care
                    // about the order in which we see the events since we're
                    // intentionally racing some things, only that nothing tracks
                    // back.

                    // Device should not disappear.
                    assert!(!present || new_present, "out of order events, device disappeared");
                    // Device should not go offline.
                    assert!(!up || new_up, "out of order events, device went offline");
                    // Address should not disappear.
                    assert!(!has_addr || new_has_addr, "out of order events, address disappeared");
                    // Default route should not disappear.
                    assert!(
                        !has_default_ipv4_route || new_has_default_ipv4_route,
                        "out of order events, default IPv4 route disappeared"
                    );
                    if new_present && new_up && new_has_addr && new_has_default_ipv4_route {
                        // We got everything we wanted, end the stream.
                        None
                    } else {
                        // Continue folding with the new state.
                        Some((
                            (),
                            (
                                watcher,
                                new_present,
                                new_up,
                                new_has_addr,
                                new_has_default_ipv4_route,
                            ),
                        ))
                    }
                },
            )
            .collect()
            .await;
    }
}

#[netstack_test]
#[test_case(fidl_subnet!("abcd::1/64"))]
#[test_case(fidl_subnet!("1.2.3.4/24"))]
async fn addresses_while_offline(name: &str, addr_with_prefix: fidl_fuchsia_net::Subnet) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let device = sandbox.create_endpoint(name).await.expect("create endpoint");
    let interface = realm
        .install_endpoint(device, Default::default())
        .await
        .expect("install endpoint to Netstack");

    interface
        .add_address_and_subnet_route(addr_with_prefix)
        .await
        .expect("add address and subnet route");
    // Verify that addresses are present.
    let interface_state = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
        .expect("connect to protocol");
    let event_stream = fidl_fuchsia_net_interfaces_ext::event_stream_from_state::<
        fidl_fuchsia_net_interfaces_ext::AllInterest,
    >(&interface_state, Default::default())
    .expect("event stream from state")
    .fuse();
    let mut event_stream = pin!(event_stream);
    let mut if_state =
        fidl_fuchsia_net_interfaces_ext::InterfaceState::<(), _>::Unknown(interface.id());

    fn contains_address<'a>(
        addresses: impl IntoIterator<
            Item = &'a fidl_fuchsia_net_interfaces_ext::Address<
                fidl_fuchsia_net_interfaces_ext::AllInterest,
            >,
        >,
        want: fidl_fuchsia_net::Subnet,
    ) -> bool {
        addresses.into_iter().any(
            |&fidl_fuchsia_net_interfaces_ext::Address {
                 addr,
                 valid_until,
                 assignment_state,
                 preferred_lifetime_info,
             }| {
                assert!(valid_until.is_infinite());
                assert_eq!(
                    preferred_lifetime_info,
                    fidl_fuchsia_net_interfaces_ext::PreferredLifetimeInfo::preferred_forever()
                );
                assert_eq!(assignment_state, fnet_interfaces::AddressAssignmentState::Assigned);
                addr == want
            },
        )
    }

    fidl_fuchsia_net_interfaces_ext::wait_interface_with_id(
        event_stream.by_ref(),
        &mut if_state,
        |fidl_fuchsia_net_interfaces_ext::PropertiesAndState {
             properties: fidl_fuchsia_net_interfaces_ext::Properties { online, addresses, .. },
             state: _,
         }| { (*online && contains_address(addresses, addr_with_prefix)).then_some(()) },
    )
    .await
    .expect("failed to wait for address to be present");

    // Address should disappear on interface offline.
    assert!(interface.control().disable().await.expect("send FIDL").expect("disable interface"));
    fidl_fuchsia_net_interfaces_ext::wait_interface_with_id(
        event_stream.by_ref(),
        &mut if_state,
        |fidl_fuchsia_net_interfaces_ext::PropertiesAndState {
             properties: fidl_fuchsia_net_interfaces_ext::Properties { online, addresses, .. },
             state: _,
         }| { (!*online && !contains_address(addresses, addr_with_prefix)).then_some(()) },
    )
    .await
    .expect("failed to wait for addresses to disappear due to interface offline");

    // Address should reappear after interface online.
    assert!(interface.control().enable().await.expect("send FIDL").expect("enable interface"));
    fidl_fuchsia_net_interfaces_ext::wait_interface_with_id(
        event_stream.by_ref(),
        &mut if_state,
        |fidl_fuchsia_net_interfaces_ext::PropertiesAndState {
             properties: fidl_fuchsia_net_interfaces_ext::Properties { online, addresses, .. },
             state: _,
         }| { (*online && contains_address(addresses, addr_with_prefix)).then_some(()) },
    )
    .await
    .expect("failed to wait for address to be present");
}

/// Test interface changes are reported through the interface watcher.
#[netstack_test]
async fn watcher(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    type Interest = fnet_interfaces_ext::AllInterest;
    let blocking_stream = realm
        .get_interface_event_stream_with_interest::<Interest>()
        .expect("get interface event stream");
    let mut blocking_stream = pin!(blocking_stream);
    let stream = realm
        .get_interface_event_stream_with_interest::<Interest>()
        .expect("get interface event stream");
    let mut stream = pin!(stream);

    async fn next<S>(stream: &mut S) -> fidl_fuchsia_net_interfaces::Event
    where
        S: futures::stream::TryStream<
                Ok = fnet_interfaces_ext::EventWithInterest<Interest>,
                Error = fidl::Error,
            > + Unpin,
    {
        stream
            .try_next()
            .await
            .expect("stream error")
            .expect("watcher event stream ended")
            .into_inner()
    }
    fn assert_loopback_existing(
        fidl_fuchsia_net_interfaces::Properties {
            port_class,
            ..
        }: fidl_fuchsia_net_interfaces::Properties,
    ) {
        assert_eq!(
            port_class,
            Some(fidl_fuchsia_net_interfaces::PortClass::Loopback(
                fidl_fuchsia_net_interfaces::Empty {}
            ))
        );
    }
    let properties = assert_matches::assert_matches!(
        next(&mut blocking_stream).await,
        fidl_fuchsia_net_interfaces::Event::Existing(properties) => properties
    );
    assert_loopback_existing(properties);
    let properties = assert_matches::assert_matches!(
        next(&mut stream).await,
        fidl_fuchsia_net_interfaces::Event::Existing(properties) => properties
    );
    assert_loopback_existing(properties);
    assert_eq!(
        next(&mut blocking_stream).await,
        fidl_fuchsia_net_interfaces::Event::Idle(fidl_fuchsia_net_interfaces::Empty {})
    );
    assert_eq!(
        next(&mut stream).await,
        fidl_fuchsia_net_interfaces::Event::Idle(fidl_fuchsia_net_interfaces::Empty {})
    );

    async fn assert_blocked<S>(stream: &mut S)
    where
        S: futures::stream::TryStream<Error = fidl::Error> + std::marker::Unpin,
        <S as futures::TryStream>::Ok: std::fmt::Debug,
    {
        stream
            .try_next()
            .map(|event| {
                let event = event.expect("event stream error");
                let event = event.expect("watcher event stream ended");
                Some(event)
            })
            .on_timeout(
                fuchsia_async::MonotonicInstant::after(zx::MonotonicDuration::from_millis(50)),
                || None,
            )
            .map(|e| match e {
                Some(e) => panic!("did not block but yielded {:?}", e),
                None => (),
            })
            .await
    }
    // Add an interface.
    assert_blocked(&mut blocking_stream).await;
    let dev = sandbox
        .create_endpoint("ep")
        .await
        .expect("create endpoint")
        .into_interface_in_realm(&realm)
        .await
        .expect("add endpoint to Netstack");
    dev.set_link_up(true).await.expect("bring device up");

    let id = dev.id();
    let port_identity_koid =
        Some(dev.endpoint().get_port_identity_koid().await.expect("get id event").raw_koid());

    let want = fidl_fuchsia_net_interfaces::Event::Added(fidl_fuchsia_net_interfaces::Properties {
        id: Some(id),
        // We're not explicitly setting the name when adding the interface, so
        // this may break if Netstack changes how it names interfaces.
        name: Some(format!("eth{}", id)),
        online: Some(false),
        port_class: Some(fidl_fuchsia_net_interfaces::PortClass::Device(
            fidl_fuchsia_hardware_network::PortClass::Virtual,
        )),
        addresses: Some(vec![]),
        has_default_ipv4_route: Some(false),
        has_default_ipv6_route: Some(false),
        port_identity_koid,
        __source_breaking: fidl::marker::SourceBreaking,
    });
    assert_eq!(next(&mut blocking_stream).await, want);
    assert_eq!(next(&mut stream).await, want);

    // Set the link to up.
    assert_blocked(&mut blocking_stream).await;
    let did_enable = dev.control().enable().await.expect("send enable").expect("enable");
    assert!(did_enable);
    // NB The following fold function is necessary because IPv6 link-local
    // addresses are configured when the interface is brought up. Assert that:
    //
    // 1. the online property changes exactly once to true,
    // 2. the number of IPv6 addresses reaches the desired count of 1,
    // 3. appearance of LL addresses must come in the same event or after
    // online changing to true,
    //
    // It would be ideal to disable IPv6 LL address configuration for this
    // test, which would simplify this significantly.
    const LL_ADDR_COUNT: usize = 1;
    let fold_fn = |online_changed: bool, event| {
        let (online, addresses) = assert_matches::assert_matches!(
            event,
            fidl_fuchsia_net_interfaces::Event::Changed(fidl_fuchsia_net_interfaces::Properties {
                id: Some(got_id),
                online,
                addresses,
                name: None,
                port_class: None,
                has_default_ipv4_route: None,
                has_default_ipv6_route: None,
                port_identity_koid: None,
                __source_breaking: fidl::marker::SourceBreaking,
            }) if got_id == id => (online, addresses)
        );
        let online_changed = online.map_or(online_changed, |got_online| {
            assert!(
                !online_changed,
                "duplicate online property change to new value of {}",
                got_online
            );
            assert!(got_online, "online must change to true");
            true
        });
        let addresses = addresses.map(|addresses| {
            addresses
                .iter()
                .map(|addr| {
                    let addr = fidl_fuchsia_net_interfaces_ext::Address::<Interest>::try_from(
                        addr.clone(),
                    )
                    .expect("failed to validate address");
                    assert_matches::assert_matches!(
                        addr,
                        fidl_fuchsia_net_interfaces_ext::Address {
                            addr: fidl_fuchsia_net::Subnet {
                                addr: fidl_fuchsia_net::IpAddress::Ipv6(
                                    fidl_fuchsia_net::Ipv6Address { .. }
                                ),
                                prefix_len: _,
                            },
                            assignment_state: fnet_interfaces::AddressAssignmentState::Assigned,
                            ..
                        }
                    );
                    addr
                })
                .collect::<HashSet<_>>()
        });
        futures::future::ready(match addresses {
            Some(addresses) if addresses.len() == LL_ADDR_COUNT => {
                // TODO(https://fxbug.dev/42056224): Make the assertion true
                // regardless of whether DAD is enabled or not, and assert
                // this through this test or perhaps a separate test.
                if !online_changed {
                    panic!("link-local address(es) appeared before online changed to true");
                }
                async_utils::fold::FoldWhile::Done(addresses)
            }
            Some(_) | None => async_utils::fold::FoldWhile::Continue(online_changed),
        })
    };
    let ll_addrs = assert_matches::assert_matches!(
        async_utils::fold::fold_while(
            blocking_stream.by_ref().map(|r| r.expect("blocking event stream error").into_inner()),
            false,
            fold_fn,
        ).await,
        async_utils::fold::FoldResult::ShortCircuited(addresses) => addresses
    );
    {
        let addrs = assert_matches::assert_matches!(
            async_utils::fold::fold_while(
                stream.by_ref().map(|r| r.expect("non-blocking event stream error").into_inner()),
                false,
                fold_fn,
            ).await,
            async_utils::fold::FoldResult::ShortCircuited(addresses) => addresses
        );
        assert_eq!(ll_addrs, addrs);
    }

    // Add an address and subnet route.
    assert_blocked(&mut blocking_stream).await;
    let subnet = fidl_subnet!("192.168.0.1/16");
    let _address_state_provider = interfaces::add_address_wait_assigned(
        dev.control(),
        subnet,
        fidl_fuchsia_net_interfaces_admin::AddressParameters {
            add_subnet_route: Some(true),
            ..Default::default()
        },
    )
    .await
    .expect("add subnet address and route");
    let addresses_changed = |event| {
        assert_matches::assert_matches!(
            event,
            fidl_fuchsia_net_interfaces::Event::Changed(fidl_fuchsia_net_interfaces::Properties {
                id: Some(event_id),
                addresses: Some(addresses),
                name: None,
                port_class: None,
                online: None,
                has_default_ipv4_route: None,
                has_default_ipv6_route: None,
                port_identity_koid: None,
                __source_breaking: fidl::marker::SourceBreaking,
            }) if event_id == id => {
                addresses
                    .iter()
                    .map(|addr| {
                        fidl_fuchsia_net_interfaces_ext::Address::try_from(addr.clone())
                            .expect("failed to validate address")
                    })
                    .collect::<HashSet<_>>()
            }
        )
    };
    let want = ll_addrs
        .iter()
        .cloned()
        .chain(std::iter::once(fidl_fuchsia_net_interfaces_ext::Address {
            addr: subnet,
            valid_until: fidl_fuchsia_net_interfaces_ext::PositiveMonotonicInstant::INFINITE_FUTURE,
            preferred_lifetime_info:
                fidl_fuchsia_net_interfaces_ext::PreferredLifetimeInfo::preferred_forever(),
            assignment_state: fnet_interfaces::AddressAssignmentState::Assigned,
        }))
        .collect();
    assert_eq!(addresses_changed(next(&mut blocking_stream).await), want);
    assert_eq!(addresses_changed(next(&mut stream).await), want);

    // Add a default route.
    assert_blocked(&mut blocking_stream).await;

    let default_gateway = fidl_ip!("192.168.255.254");

    dev.add_default_route(default_gateway).await.expect("add default route");

    let want =
        fidl_fuchsia_net_interfaces::Event::Changed(fidl_fuchsia_net_interfaces::Properties {
            id: Some(id),
            has_default_ipv4_route: Some(true),
            ..Default::default()
        });
    assert_eq!(next(&mut blocking_stream).await, want);
    assert_eq!(next(&mut stream).await, want);

    // Remove the default route.
    assert_blocked(&mut blocking_stream).await;
    dev.remove_default_route(default_gateway).await.expect("remove default route");

    let want =
        fidl_fuchsia_net_interfaces::Event::Changed(fidl_fuchsia_net_interfaces::Properties {
            id: Some(id),
            has_default_ipv4_route: Some(false),
            ..Default::default()
        });
    assert_eq!(next(&mut blocking_stream).await, want);
    assert_eq!(next(&mut stream).await, want);

    // Remove the added address.
    assert_blocked(&mut blocking_stream).await;
    let was_removed = interfaces::remove_subnet_address_and_route(&dev, subnet)
        .await
        .expect("remove subnet address and route");
    assert!(was_removed);
    assert_eq!(addresses_changed(next(&mut blocking_stream).await), ll_addrs);
    assert_eq!(addresses_changed(next(&mut stream).await), ll_addrs);

    // Set the link to down.
    {
        assert_blocked(&mut blocking_stream).await;
        dev.set_link_up(false).await.expect("bring device up");
        let fold_fn = |addresses_empty: bool, event| {
            let (online, addresses) = assert_matches::assert_matches!(
                event,
                fidl_fuchsia_net_interfaces::Event::Changed(fidl_fuchsia_net_interfaces::Properties {
                    id: Some(got_id),
                    online,
                    addresses,
                    name: None,
                    port_class: None,
                    has_default_ipv4_route: None,
                    has_default_ipv6_route: None,
                    port_identity_koid: None,
                    __source_breaking: fidl::marker::SourceBreaking,
                }) if got_id == id => (online, addresses)
            );
            let addresses_empty = addresses.as_ref().map_or(addresses_empty, |addresses| {
                assert!(!addresses_empty, "duplicate addresses property change to {:?}", addresses);
                addresses.len() == 0
            });
            let online_changed = online.is_some_and(|got_online| {
                assert!(!got_online, "online must change to false");
                true
            });
            futures::future::ready(if online_changed {
                if !addresses_empty {
                    panic!("offline event before addresses removed");
                }
                async_utils::fold::FoldWhile::Done(())
            } else {
                async_utils::fold::FoldWhile::Continue(addresses_empty)
            })
        };
        assert_eq!(
            async_utils::fold::fold_while(
                blocking_stream
                    .by_ref()
                    .map(|r| r.expect("blocking event stream error").into_inner()),
                false,
                fold_fn,
            )
            .await,
            async_utils::fold::FoldResult::ShortCircuited(()),
        );
        assert_eq!(
            async_utils::fold::fold_while(
                stream.by_ref().map(|r| r.expect("non-blocking event stream error").into_inner()),
                false,
                fold_fn,
            )
            .await,
            async_utils::fold::FoldResult::ShortCircuited(()),
        );
    }

    // Remove the ethernet interface.
    assert_blocked(&mut blocking_stream).await;
    std::mem::drop(dev);
    let want = fidl_fuchsia_net_interfaces::Event::Removed(id);
    assert_eq!(next(&mut blocking_stream).await, want);
    assert_eq!(next(&mut stream).await, want);
}

#[netstack_test]
async fn test_readded_address_present(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let network = sandbox.create_network(name).await.expect("create network");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let interface = realm.join_network(&network, name).await.expect("join network");

    const SRC_IP: fidl_fuchsia_net::Subnet = fidl_subnet!("192.168.0.1/24");
    interface
        .add_address_and_subnet_route(SRC_IP)
        .await
        .expect("add source address and subnet route");

    const DST_IP: fidl_fuchsia_net::Subnet = fidl_subnet!("192.168.0.254/24");
    interface.add_address(DST_IP).await.expect("add destination address");

    let std_socket_addr = |subnet, port| {
        let fidl_fuchsia_net::Subnet { addr, prefix_len: _ } = subnet;
        let fidl_fuchsia_net_ext::IpAddress(addr) = addr.into();
        std::net::SocketAddr::new(addr, port)
    };
    let dst_sockaddr = std_socket_addr(DST_IP, 6789);
    let listener: fasync::net::TcpListener =
        fasync::net::TcpListener::listen_in_realm(&realm, dst_sockaddr).await.expect("listen");

    // The presence of the connected socket means that the address will still be
    // in-use after it is deleted.
    let src_sockaddr = std_socket_addr(SRC_IP, 12345);
    let _socket: fasync::net::TcpStream =
        fasync::net::TcpStream::bind_and_connect_in_realm(&realm, src_sockaddr, dst_sockaddr)
            .await
            .expect("bind and connect");

    let (_listener, _accepted, from): (fasync::net::TcpListener, fasync::net::TcpStream, _) =
        listener.accept().await.expect("accept");
    assert_eq!(from, src_sockaddr);

    assert!(
        interface
            .del_address_and_subnet_route(SRC_IP)
            .await
            .expect("delete address and subnet route")
    );

    interface.add_address_and_subnet_route(SRC_IP).await.expect("re-add address and subnet route");
    let addresses = interface.get_addrs(Default::default()).await.expect("get addresses");
    assert!(
        addresses.iter().any(
            |&fidl_fuchsia_net_interfaces_ext::Address { addr, assignment_state, .. }| {
                assert_eq!(assignment_state, fnet_interfaces::AddressAssignmentState::Assigned);
                addr == SRC_IP
            }
        ),
        "{:?} missing from addresses {:?}",
        SRC_IP,
        addresses
    );
}

#[derive(Debug)]
enum LifetimeToUpdate {
    Preferred,
    Valid,
}

// Regression test which asserts that property changes on an address that is
// hidden from clients of interface watcher (because the interface is offline)
// does not cause null changes to be emitted via interface watcher.
#[netstack_test]
#[test_case(LifetimeToUpdate::Preferred ; "updating preferred lifetime")]
#[test_case(LifetimeToUpdate::Valid ; "updating valid lifetime")]
async fn test_lifetime_change_on_hidden_addr(name: &str, lifetime_to_update: LifetimeToUpdate) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let interface = sandbox
        .create_endpoint(name)
        .await
        .expect("create endpoint")
        .into_interface_in_realm(&realm)
        .await
        .expect("add endpoint to Netstack");
    interface.set_link_up(true).await.expect("bring device up");
    // Note that the interface is still offline since it is not admin enabled yet.

    let event_stream = realm
        .get_interface_event_stream_with_interest::<fidl_fuchsia_net_interfaces_ext::AllInterest>()
        .expect("get interface event stream");
    let mut event_stream = pin!(event_stream);

    // Must wait until the interface is observed via an Existing/Added event.
    let mut state =
        fidl_fuchsia_net_interfaces_ext::InterfaceState::<(), _>::Unknown(interface.id());
    fidl_fuchsia_net_interfaces_ext::wait_interface_with_id(
        event_stream.by_ref(),
        &mut state,
        |_: &fidl_fuchsia_net_interfaces_ext::PropertiesAndState<_, _>| Some(()),
    )
    .await
    .expect("wait for interface to appear");

    const ADDR: fidl_fuchsia_net::Subnet = fidl_subnet!("1.2.3.4/24");
    let (address_state_provider, server) = fidl::endpoints::create_proxy::<
        fidl_fuchsia_net_interfaces_admin::AddressStateProviderMarker,
    >();
    interface
        .control()
        .add_address(
            &ADDR,
            &fidl_fuchsia_net_interfaces_admin::AddressParameters::default(),
            server,
        )
        .expect("Control.AddAddress FIDL error");

    match lifetime_to_update {
        LifetimeToUpdate::Preferred => {
            let deprecated_properties = fidl_fuchsia_net_interfaces_admin::AddressProperties {
                preferred_lifetime_info: Some(
                    fidl_fuchsia_net_interfaces::PreferredLifetimeInfo::Deprecated(
                        fidl_fuchsia_net_interfaces::Empty,
                    ),
                ),
                ..Default::default()
            };
            address_state_provider
                .update_address_properties(&deprecated_properties)
                .await
                .expect("FIDL error deprecating address");
        }
        LifetimeToUpdate::Valid => {
            address_state_provider
                .update_address_properties(&fidl_fuchsia_net_interfaces_admin::AddressProperties {
                    valid_lifetime_end: Some(123),
                    ..Default::default()
                })
                .await
                .expect("FIDL error deprecating address");
        }
    };

    assert!(interface.control().enable().await.expect("terminal error").expect("enable interface"));

    let res = fidl_fuchsia_net_interfaces_ext::wait_interface_with_id(
        event_stream.by_ref(),
        &mut state,
        |iface| {
            iface
                .properties
                .addresses
                .iter()
                .any(
                    |fidl_fuchsia_net_interfaces_ext::Address {
                         addr, assignment_state, ..
                     }| {
                        assert_eq!(
                            *assignment_state,
                            fnet_interfaces::AddressAssignmentState::Assigned
                        );
                        *addr == ADDR
                    },
                )
                .then_some(())
        },
    )
    .await;
    match res {
        Ok(()) => {}
        // This guards against the regression where a property change while
        // the address is hidden from interface watcher clients induces an
        // empty change.
        Err(
            e @ fidl_fuchsia_net_interfaces_ext::WatcherOperationError::Update(
                fidl_fuchsia_net_interfaces_ext::UpdateError::EmptyChange(_),
            ),
        ) => panic!("violated API expectations while waiting for address to appear: {}", e),
        Err(e) => panic!("wait for address to appear: {}", e),
    }
}

#[netstack_test]
async fn watcher_port_identity_koid_filter(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let endpoint = sandbox.create_endpoint("ep1").await.expect("create endpoint");
    let endpoint2 = sandbox.create_endpoint("ep2").await.expect("create endpoint");
    let id_event = endpoint.get_port_identity_koid().await.expect("get id event");
    let other_id_event = endpoint2.get_port_identity_koid().await.expect("get id event");

    let interface_state = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
        .expect("connect to protocol");

    let mut watcher_match = pin!(
        fnet_interfaces_ext::event_stream_from_state::<fnet_interfaces_ext::DefaultInterest>(
            &interface_state,
            fnet_interfaces_ext::WatchOptions {
                port_identity_koid_filter: Some(id_event),
                ..Default::default()
            },
        )
        .expect("create watcher")
    );
    // Should not observe the loopback interface because we have a filter.
    assert_eq!(
        fnet_interfaces_ext::existing(
            watcher_match.by_ref(),
            HashMap::<u64, fnet_interfaces_ext::PropertiesAndState<(), _>>::new()
        )
        .await
        .expect("read existing"),
        HashMap::new()
    );
    let mut watcher_no_match = pin!(
        fnet_interfaces_ext::event_stream_from_state::<fnet_interfaces_ext::DefaultInterest>(
            &interface_state,
            fnet_interfaces_ext::WatchOptions {
                port_identity_koid_filter: Some(other_id_event),
                ..Default::default()
            },
        )
        .expect("create watcher")
    );
    // Should not observe the loopback interface because we have a filter.
    assert_eq!(
        fnet_interfaces_ext::existing(
            watcher_no_match.by_ref(),
            HashMap::<u64, fnet_interfaces_ext::PropertiesAndState<(), _>>::new()
        )
        .await
        .expect("read existing"),
        HashMap::new()
    );

    // Install the interface.
    let interface =
        endpoint.into_interface_in_realm(&realm).await.expect("add endpoint to Netstack");
    let id = interface.id();

    let event = watcher_match
        .next()
        .await
        .expect("watcher ended unexpectedly")
        .expect("watcher error")
        .into_inner();
    let got_id = assert_matches::assert_matches!(
        event,
        fnet_interfaces::Event::Added(fnet_interfaces::Properties{ id: Some(id), .. }) => id
    );
    assert_eq!(got_id, id);

    // Change interface state and wait for a Changed event.
    interface.set_link_up(true).await.expect("bring device up");
    assert!(interface.control().enable().await.expect("send enable").expect("enable interface"));
    let got_id = watcher_match
        .by_ref()
        .try_filter_map(|event| {
            futures::future::ok(match event.into_inner() {
                fnet_interfaces::Event::Changed(fnet_interfaces::Properties { id, .. }) => {
                    Some(id.expect("missing id"))
                }
                _ => None,
            })
        })
        .next()
        .await
        .expect("watcher ended unexpectedly")
        .expect("watcher error");
    assert_eq!(got_id, id);

    // Drop the interface and wait for a removed event.
    drop(interface);
    let got_id = watcher_match
        .by_ref()
        .try_filter_map(|event| {
            futures::future::ok(match event.into_inner() {
                fnet_interfaces::Event::Removed(id) => Some(id),
                _ => None,
            })
        })
        .next()
        .await
        .expect("watcher ended unexpectedly")
        .expect("watcher error");
    assert_eq!(got_id, id);

    // The other watcher shouldn't have received _any_ of these events. To prove
    // it, add another interface that should make the other watcher unblock with
    // a _single_ added event for the new interface.
    let interface2 =
        endpoint2.into_interface_in_realm(&realm).await.expect("add endpoint to Netstack");
    let event = watcher_no_match
        .next()
        .await
        .expect("watcher ended unexpectedly")
        .expect("watcher error")
        .into_inner();
    let got_id = assert_matches::assert_matches!(
        event,
        fnet_interfaces::Event::Added(fnet_interfaces::Properties{ id: Some(id), .. }) => id
    );
    assert_eq!(got_id, interface2.id());

    // Finally, show that creating new watchers correctly applies the filters to
    // existing interfaces.

    let existing = fnet_interfaces_ext::existing(
        fnet_interfaces_ext::event_stream_from_state::<fnet_interfaces_ext::DefaultInterest>(
            &interface_state,
            fnet_interfaces_ext::WatchOptions {
                port_identity_koid_filter: Some(other_id_event),
                ..Default::default()
            },
        )
        .expect("create watcher"),
        HashMap::<u64, fnet_interfaces_ext::PropertiesAndState<(), _>>::new(),
    )
    .await
    .expect("read existing");
    assert_matches::assert_matches!(existing.get(&interface2.id()), Some(_));
}
