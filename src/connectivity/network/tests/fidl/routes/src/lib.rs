// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

mod rules;

use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::hash::Hash;
use std::pin::pin;

use anyhow::Context as _;
use assert_matches::assert_matches;
use fidl::endpoints::Proxy;
use fidl_fuchsia_net_ext::IntoExt;
use fuchsia_async::TimeoutExt;
use futures::{FutureExt, StreamExt};
use net_declare::{fidl_ip, fidl_ip_v4, fidl_mac, fidl_subnet, net_subnet_v4, net_subnet_v6};
use net_types::SpecifiedAddr;
use net_types::ip::{GenericOverIp, Ip, IpAddress, IpVersion, Ipv4, Ipv4Addr, Ipv6, Ipv6Addr};
use netemul::InterfaceConfig;
use netstack_testing_common::interfaces::{self, TestInterfaceExt as _};
use netstack_testing_common::realms::{
    KnownServiceProvider, Netstack3, OutOfStack, TestRealmExt as _, TestSandboxExt as _,
};
use netstack_testing_macros::netstack_test;
use packet_formats::icmp::ndp::options::{NdpOptionBuilder, PrefixInformation, RouteInformation};
use routes_common::{TestSetup, test_route};
use test_case::test_case;

use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
use fidl_fuchsia_net_resources as fnet_resources;
use fidl_fuchsia_net_routes as fnet_routes;
use fidl_fuchsia_net_routes_ext as fnet_routes_ext;
use fidl_fuchsia_net_routes_ext::FidlRouteIpExt;
use fidl_fuchsia_net_routes_ext::admin::FidlRouteAdminIpExt;

async fn resolve(
    routes: &fidl_fuchsia_net_routes::StateProxy,
    remote: fidl_fuchsia_net::IpAddress,
) -> fidl_fuchsia_net_routes::Resolved {
    routes
        .resolve(&remote)
        .await
        .expect("routes/State.Resolve FIDL error")
        .map_err(zx::Status::err_from_raw)
        .context("routes/State.Resolve error")
        .expect("failed to resolve remote")
}

#[netstack_test]
async fn resolve_loopback_route(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm =
        sandbox.create_netstack_realm::<Netstack3, _>(name).expect("failed to create realm");
    let routes = realm
        .connect_to_protocol::<fidl_fuchsia_net_routes::StateMarker>()
        .expect("failed to connect to routes/State");
    let routes = &routes;

    let test = |remote: fidl_fuchsia_net::IpAddress, source: fidl_fuchsia_net::IpAddress| async move {
        assert_eq!(
            resolve(routes, remote).await,
            fidl_fuchsia_net_routes::Resolved::Direct(fidl_fuchsia_net_routes::Destination {
                address: Some(remote),
                mac: None,
                interface_id: Some(1),
                source_address: Some(source),
                ..Default::default()
            }),
        );
    };

    test(fidl_ip!("127.0.0.1"), fidl_ip!("127.0.0.1")).await;
    test(fidl_ip!("::1"), fidl_ip!("::1")).await;
}

#[netstack_test]
async fn resolve_route(name: &str) {
    const GATEWAY_IP_V4: fidl_fuchsia_net::Subnet = fidl_subnet!("192.168.0.1/24");
    const GATEWAY_IP_V6: fidl_fuchsia_net::Subnet = fidl_subnet!("3080::1/64");
    const GATEWAY_MAC: fidl_fuchsia_net::MacAddress = fidl_mac!("02:01:02:03:04:05");
    const HOST_IP_V4: fidl_fuchsia_net::Subnet = fidl_subnet!("192.168.0.2/24");
    const HOST_IP_V6: fidl_fuchsia_net::Subnet = fidl_subnet!("3080::2/64");

    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let net = sandbox.create_network("net").await.expect("failed to create network");

    // Configure a host.
    let host = sandbox
        .create_netstack_realm::<Netstack3, _>(format!("{}_host", name))
        .expect("failed to create client realm");

    let host_ep = host.join_network(&net, "host").await.expect("host failed to join network");
    host_ep.add_address_and_subnet_route(HOST_IP_V4).await.expect("configure address");
    let _host_address_state_provider = interfaces::add_address_wait_assigned(
        host_ep.control(),
        HOST_IP_V6,
        fidl_fuchsia_net_interfaces_admin::AddressParameters {
            add_subnet_route: Some(true),
            ..Default::default()
        },
    )
    .await
    .expect("add subnet address and route");

    // Configure a gateway.
    let gateway = sandbox
        .create_netstack_realm::<Netstack3, _>(format!("{}_gateway", name))
        .expect("failed to create server realm");

    let gateway_ep = gateway
        .join_network_with(
            &net,
            "gateway",
            netemul::new_endpoint_config(netemul::DEFAULT_MTU, Some(GATEWAY_MAC)),
            Default::default(),
        )
        .await
        .expect("gateway failed to join network");
    gateway_ep.add_address_and_subnet_route(GATEWAY_IP_V4).await.expect("configure address");
    let _gateway_address_state_provider = interfaces::add_address_wait_assigned(
        gateway_ep.control(),
        GATEWAY_IP_V6,
        fidl_fuchsia_net_interfaces_admin::AddressParameters {
            add_subnet_route: Some(true),
            ..Default::default()
        },
    )
    .await
    .expect("add subnet address and route");

    let routes = host
        .connect_to_protocol::<fidl_fuchsia_net_routes::StateMarker>()
        .expect("failed to connect to routes/State");
    let routes = &routes;

    let resolve_fails = move |remote: fidl_fuchsia_net::IpAddress| async move {
        assert_eq!(
            routes
                .resolve(&remote)
                .await
                .expect("resolve FIDL error")
                .map_err(zx::Status::err_from_raw),
            Err(zx::Status::ADDRESS_UNREACHABLE)
        )
    };

    let interface_id = host_ep.id();

    let do_test = |gateway: fidl_fuchsia_net::IpAddress,
                   unspecified: fidl_fuchsia_net::IpAddress,
                   public_ip: fidl_fuchsia_net::IpAddress,
                   source_address: fidl_fuchsia_net::IpAddress| {
        let host_ep = &host_ep;
        async move {
            let gateway_node = fidl_fuchsia_net_routes::Destination {
                address: Some(gateway),
                mac: Some(GATEWAY_MAC),
                interface_id: Some(interface_id),
                source_address: Some(source_address),
                ..Default::default()
            };

            // Start asking for a route for something that is directly accessible on the
            // network.
            let resolved = resolve(routes, gateway).await;
            assert_eq!(resolved, fidl_fuchsia_net_routes::Resolved::Direct(gateway_node.clone()));
            // Fails if route unreachable.
            resolve_fails(public_ip).await;

            // Install a default route and try to resolve through the gateway.
            host_ep.add_default_route(gateway).await.expect("add default route");

            // Resolve a public IP again and check that we get the gateway response.
            let resolved = resolve(routes, public_ip).await;
            assert_eq!(resolved, fidl_fuchsia_net_routes::Resolved::Gateway(gateway_node.clone()));
            // And that the unspecified address resolves to the gateway node as well.
            let resolved = resolve(routes, unspecified).await;
            assert_eq!(resolved, fidl_fuchsia_net_routes::Resolved::Gateway(gateway_node));
        }
    };

    // Test the peer unreachable case before we apply NUD flake workaround.
    resolve_fails(fidl_ip!("192.168.0.3")).await;
    resolve_fails(fidl_ip!("3080::3")).await;

    // Apply NUD flake workaround to both nodes since we'll be resolving
    // neighbors now.
    gateway_ep.apply_nud_flake_workaround().await.expect("nud flake workaround");
    host_ep.apply_nud_flake_workaround().await.expect("nud flake workaround");

    do_test(GATEWAY_IP_V4.addr, fidl_ip!("0.0.0.0"), fidl_ip!("8.8.8.8"), HOST_IP_V4.addr).await;

    do_test(GATEWAY_IP_V6.addr, fidl_ip!("::"), fidl_ip!("2001:4860:4860::8888"), HOST_IP_V6.addr)
        .await;
}

#[netstack_test]
async fn resolve_default_route_while_dhcp_is_running(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let net = sandbox.create_network("net").await.expect("failed to create network");

    // Configure a host.
    let realm = sandbox
        .create_netstack_realm_with::<Netstack3, _, _>(name, [KnownServiceProvider::DhcpClient])
        .expect("failed to create client realm");

    let ep = realm.join_network(&net, "host").await.expect("host failed to join network");
    ep.start_dhcp::<OutOfStack>().await.expect("failed to start DHCP");

    let routes = realm
        .connect_to_protocol::<fidl_fuchsia_net_routes::StateMarker>()
        .expect("failed to connect to routes/State");

    let resolved = routes
        .resolve(&fidl_ip!("0.0.0.0"))
        .await
        .expect("routes/State.Resolve FIDL error")
        .map_err(zx::Status::err_from_raw);

    assert_eq!(resolved, Err(zx::Status::ADDRESS_UNREACHABLE));

    const EP_ADDR: fidl_fuchsia_net::Ipv4Address = fidl_ip_v4!("192.168.0.3");
    const PREFIX_LEN: u8 = 24;
    const GATEWAY_ADDR: fidl_fuchsia_net::IpAddress = fidl_ip!("192.168.0.1");
    const GATEWAY_MAC: fidl_fuchsia_net::MacAddress = fidl_mac!("02:01:02:03:04:05");
    const UNSPECIFIED_IP: fidl_fuchsia_net::IpAddress = fidl_ip!("0.0.0.0");

    // Configure stack statically with an address and a default route while DHCP is still running.
    let _host_address_state_provider = interfaces::add_address_wait_assigned(
        ep.control(),
        fidl_fuchsia_net::Subnet {
            addr: fidl_fuchsia_net::IpAddress::Ipv4(EP_ADDR),
            prefix_len: PREFIX_LEN,
        },
        fidl_fuchsia_net_interfaces_admin::AddressParameters::default(),
    )
    .await
    .expect("add address");

    let neigh = realm
        .connect_to_protocol::<fidl_fuchsia_net_neighbor::ControllerMarker>()
        .expect("failed to connect to neighbor API");
    neigh
        .add_entry(ep.id(), &GATEWAY_ADDR, &GATEWAY_MAC)
        .await
        .expect("add_entry FIDL error")
        .expect("add_entry error");

    // Install a default route and try to resolve through the gateway.
    ep.add_default_route(GATEWAY_ADDR).await.expect("add default route");

    let resolved = routes
        .resolve(&UNSPECIFIED_IP)
        .await
        .expect("routes/State.Resolve FIDL error")
        .map_err(zx::Status::err_from_raw);

    assert_eq!(
        resolved,
        Ok(fidl_fuchsia_net_routes::Resolved::Gateway(fidl_fuchsia_net_routes::Destination {
            address: Some(GATEWAY_ADDR),
            mac: Some(GATEWAY_MAC),
            interface_id: Some(ep.id()),
            source_address: Some(fidl_fuchsia_net::IpAddress::Ipv4(EP_ADDR)),
            ..Default::default()
        }))
    );
}

#[netstack_test]
#[variant(I, Ip)]
// Resolve returns the preferred source address used when communicating with the
// destination. Expect Resolve to fail on interfaces without any assigned
// addresses, even if the destination is reachable and routable.
async fn resolve_fails_with_no_src_address<I: Ip>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm =
        sandbox.create_netstack_realm::<Netstack3, _>(name).expect("failed to create realm");
    let device = sandbox.create_endpoint(name).await.expect("create endpoint");
    let interface = realm
        .install_endpoint(device, InterfaceConfig::default())
        .await
        .expect("install interface");

    let (local, remote, subnet) = match I::VERSION {
        IpVersion::V4 => {
            (fidl_ip!("192.0.2.1"), fidl_ip!("192.0.2.2"), fidl_subnet!("192.0.2.0/24"))
        }
        IpVersion::V6 => {
            (fidl_ip!("2001:0db8::1"), fidl_ip!("2001:0db8::2"), fidl_subnet!("2001:0db8::/32"))
        }
    };
    const REMOTE_MAC: fidl_fuchsia_net::MacAddress = fidl_mac!("02:01:02:03:04:05");

    // Install a route to the remote.
    interface.add_subnet_route(subnet).await.expect("add subnet route");

    // Configure the remote as a neighbor.
    let neigh = realm
        .connect_to_protocol::<fidl_fuchsia_net_neighbor::ControllerMarker>()
        .expect("failed to connect to neighbor API");
    neigh
        .add_entry(interface.id(), &remote, &REMOTE_MAC)
        .await
        .expect("add_entry FIDL error")
        .expect("add_entry error");

    let routes = realm
        .connect_to_protocol::<fidl_fuchsia_net_routes::StateMarker>()
        .expect("failed to connect to routes/State");

    // Remove the autogenerated SLAAC addresses, so that the interface truly has
    // no address.
    assert_eq!(
        interface
            .remove_ipv6_linklocal_addresses()
            .await
            .expect("removing IPv6 linklocal addresses should succeed")
            .len(),
        1
    );

    // Verify that resolving the route fails.
    assert_eq!(
        routes
            .resolve(&remote)
            .await
            .expect("resolve FIDL error")
            .map_err(zx::Status::err_from_raw),
        Err(zx::Status::ADDRESS_UNREACHABLE)
    );

    // Install an address on the device.
    interface
        .add_address(fidl_fuchsia_net::Subnet { addr: local, prefix_len: I::Addr::BYTES * 8 })
        .await
        .expect("failed to add address");

    // Verify that resolving the route succeeds.
    assert_eq!(
        routes
            .resolve(&remote)
            .await
            .expect("resolve FIDL error")
            .map_err(zx::Status::err_from_raw)
            .expect("resolve failed"),
        fidl_fuchsia_net_routes::Resolved::Direct(fidl_fuchsia_net_routes::Destination {
            address: Some(remote),
            mac: Some(REMOTE_MAC),
            interface_id: Some(interface.id()),
            source_address: Some(local),
            ..Default::default()
        })
    );
}

fn new_installed_route<I: fnet_routes_ext::FidlRouteIpExt>(
    subnet: net_types::ip::Subnet<I::Addr>,
    interface: u64,
    metric: u32,
    metric_is_inherited: bool,
    table_id: u32,
) -> fnet_routes_ext::InstalledRoute<I> {
    let specified_metric = if metric_is_inherited {
        fnet_routes::SpecifiedMetric::InheritedFromInterface(fnet_routes::Empty)
    } else {
        fnet_routes::SpecifiedMetric::ExplicitMetric(metric)
    };
    fnet_routes_ext::InstalledRoute {
        route: fnet_routes_ext::Route {
            destination: subnet,
            action: fnet_routes_ext::RouteAction::Forward(fnet_routes_ext::RouteTarget {
                outbound_interface: interface,
                next_hop: None,
            }),
            properties: fnet_routes_ext::RouteProperties {
                specified_properties: fnet_routes_ext::SpecifiedRouteProperties {
                    metric: specified_metric,
                },
            },
        },
        effective_properties: fnet_routes_ext::EffectiveRouteProperties { metric: metric },
        table_id: fnet_routes_ext::TableId::new(table_id),
    }
}

// Asserts that two vectors contain the same entries, order independent.
#[track_caller]
fn assert_eq_unordered<T: Debug + Eq + Hash + PartialEq>(a: Vec<T>, b: Vec<T>) {
    // Converts a `Vec<T>` into a `HashMap` where the key is `T` and the value
    // is the count of occurrences of `T` in the vec.
    fn into_counted_set<T: Eq + Hash>(set: Vec<T>) -> HashMap<T, usize> {
        set.into_iter().fold(HashMap::new(), |mut map, entry| {
            *map.entry(entry).or_default() += 1;
            map
        })
    }
    assert_eq!(into_counted_set(a), into_counted_set(b));
}

// Default metric values used by the netstack when creating implicit routes.
const DEFAULT_INTERFACE_METRIC: u32 = 100;
const LOOPBACK_METRIC: u32 = 1000;

// The initial IPv4 routes that are installed on the loopback interface.
fn initial_loopback_routes_v4(
    loopback_id: u64,
    table_id: u32,
) -> impl Iterator<Item = fnet_routes_ext::InstalledRoute<Ipv4>> {
    [
        new_installed_route(
            net_subnet_v4!("127.0.0.0/8"),
            loopback_id,
            LOOPBACK_METRIC,
            true,
            table_id,
        ),
        new_installed_route(
            net_subnet_v4!("224.0.0.0/4"),
            loopback_id,
            LOOPBACK_METRIC,
            true,
            table_id,
        ),
    ]
    .into_iter()
}

// The initial IPv6 routes that are installed on the loopback interface.
fn initial_loopback_routes_v6(
    loopback_id: u64,
    table_id: u32,
) -> impl Iterator<Item = fnet_routes_ext::InstalledRoute<Ipv6>> {
    [
        new_installed_route(
            net_subnet_v6!("::1/128"),
            loopback_id,
            LOOPBACK_METRIC,
            true,
            table_id,
        ),
        new_installed_route(
            net_subnet_v6!("ff00::/8"),
            loopback_id,
            LOOPBACK_METRIC,
            true,
            table_id,
        ),
    ]
    .into_iter()
}

// The initial IPv4 routes that are installed on an ethernet interface.
fn initial_ethernet_routes_v4(
    ethernet_id: u64,
    table_id: u32,
) -> impl Iterator<Item = fnet_routes_ext::InstalledRoute<Ipv4>> {
    [new_installed_route(
        net_subnet_v4!("224.0.0.0/4"),
        ethernet_id,
        DEFAULT_INTERFACE_METRIC,
        true,
        table_id,
    )]
    .into_iter()
}

// The initial IPv6 routes that are installed on the ethernet interface.
fn initial_ethernet_routes_v6(
    ethernet_id: u64,
    table_id: u32,
) -> impl Iterator<Item = fnet_routes_ext::InstalledRoute<Ipv6>> {
    [
        new_installed_route(
            net_subnet_v6!("fe80::/64"),
            ethernet_id,
            DEFAULT_INTERFACE_METRIC,
            true,
            table_id,
        ),
        new_installed_route(
            net_subnet_v6!("ff00::/8"),
            ethernet_id,
            DEFAULT_INTERFACE_METRIC,
            true,
            table_id,
        ),
    ]
    .into_iter()
}

// Verifies the startup behavior of the watcher protocols; including the
// expected preinstalled routes.
#[netstack_test]
#[variant(I, Ip)]
async fn watcher_existing<
    I: fnet_routes_ext::FidlRouteIpExt + fnet_routes_ext::admin::FidlRouteAdminIpExt,
>(
    name: &str,
) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    let loopback_id = realm
        .loopback_properties()
        .await
        .expect("failed to get loopback properties")
        .expect("loopback properties unexpectedly None")
        .id;

    let device = sandbox.create_endpoint(name).await.expect("create endpoint");
    // Using `install_endpoint` installs the interface, enables it, and waits
    // for it to come online.
    let interface = realm
        .install_endpoint(device, InterfaceConfig::default())
        .await
        .expect("install interface");
    let interface_id = interface.id();

    let main_table_id = realm.main_table_id::<I>().await;
    // The routes we expected to be installed in the netstack by default.
    #[derive(GenericOverIp)]
    #[generic_over_ip(I, Ip)]
    struct RoutesHolder<I: fnet_routes_ext::FidlRouteIpExt>(
        Vec<fnet_routes_ext::InstalledRoute<I>>,
    );
    let RoutesHolder(expected_routes) = I::map_ip_out(
        (loopback_id, interface_id),
        |(loopback_id, interface_id)| {
            RoutesHolder(
                initial_loopback_routes_v4(loopback_id.get(), main_table_id)
                    .chain(initial_ethernet_routes_v4(interface_id, main_table_id))
                    .collect::<Vec<_>>(),
            )
        },
        |(loopback_id, interface_id)| {
            RoutesHolder(
                initial_loopback_routes_v6(loopback_id.get(), main_table_id)
                    .chain(initial_ethernet_routes_v6(interface_id, main_table_id))
                    .collect::<Vec<_>>(),
            )
        },
    );

    let state_proxy =
        realm.connect_to_protocol::<I::StateMarker>().expect("failed to connect to routes/State");
    let event_stream = fnet_routes_ext::event_stream_from_state::<I>(&state_proxy)
        .expect("failed to connect to routes watcher");
    let mut event_stream = pin!(event_stream);

    // Collect the routes installed in the Netstack.
    let mut routes = Vec::new();
    while let Some(event) = event_stream
        .next()
        .on_timeout(
            fuchsia_async::MonotonicInstant::after(
                netstack_testing_common::ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT,
            ),
            || None,
        )
        .await
    {
        match event.expect("unexpected error in event stream") {
            // Treat 'Added' and 'Existing' events the same, since we have no
            // mechanism to synchronize and ensure the Netstack has finished
            // initialization before connecting the routes watcher.
            fnet_routes_ext::Event::Existing(route) | fnet_routes_ext::Event::Added(route) => {
                routes.push(route)
            }
            fnet_routes_ext::Event::Idle => continue,
            fnet_routes_ext::Event::Removed(route) => {
                panic!("unexpectedly observed route removal: {:?}", route)
            }
            fnet_routes_ext::Event::Unknown => panic!("unexpectedly observed unknown event"),
        }
    }

    // Assert that the existing routes contain exactly the expected routes.
    assert_eq_unordered(routes, expected_routes);
}

// Declare subnet routes for tests to add/delete. These are known to not collide
// with routes implicitly installed by the Netstack.
const TEST_SUBNET_V4: net_types::ip::Subnet<Ipv4Addr> = net_subnet_v4!("192.168.0.0/24");
const TEST_SUBNET_V6: net_types::ip::Subnet<Ipv6Addr> = net_subnet_v6!("fd::/64");

// Verifies that a client-installed route is observed as `existing` if added
// before the watcher client connects.
#[netstack_test]
#[variant(I, Ip)]
async fn watcher_add_before_watch<
    I: fnet_routes_ext::FidlRouteIpExt + fnet_routes_ext::admin::FidlRouteAdminIpExt,
>(
    name: &str,
) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let device = sandbox.create_endpoint(name).await.expect("create endpoint");
    let interface = device.into_interface_in_realm(&realm).await.expect("add endpoint to Netstack");
    let main_table_id = realm.main_table_id::<I>().await;

    let subnet: net_types::ip::Subnet<I::Addr> =
        I::map_ip((), |()| TEST_SUBNET_V4, |()| TEST_SUBNET_V6);

    // Add a test route.
    interface.add_subnet_route(subnet.into_ext()).await.expect("failed to add route");

    // Connect to the watcher protocol.
    let state_proxy =
        realm.connect_to_protocol::<I::StateMarker>().expect("failed to connect to routes/State");
    let event_stream = fnet_routes_ext::event_stream_from_state::<I>(&state_proxy)
        .expect("failed to connect to routes watcher");

    // Verify that the previously added route is observed as `existing`.
    let event_stream = pin!(event_stream);
    let existing = fnet_routes_ext::collect_routes_until_idle::<I, Vec<_>>(event_stream)
        .await
        .expect("failed to collect existing routes");
    let expected_route =
        new_installed_route(subnet, interface.id(), DEFAULT_INTERFACE_METRIC, true, main_table_id);
    assert!(
        existing.contains(&expected_route),
        "route: {:?}, existing: {:?}",
        expected_route,
        existing
    )
}

// Verifies the watcher protocols correctly report `added` and `removed` events.
#[netstack_test]
#[variant(I, Ip)]
async fn watcher_add_remove<
    I: fnet_routes_ext::FidlRouteIpExt + fnet_routes_ext::admin::FidlRouteAdminIpExt,
>(
    name: &str,
) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let device = sandbox.create_endpoint(name).await.expect("create endpoint");
    let interface = device.into_interface_in_realm(&realm).await.expect("add endpoint to Netstack");
    let main_table_id = realm.main_table_id::<I>().await;

    let subnet: net_types::ip::Subnet<I::Addr> =
        I::map_ip((), |()| TEST_SUBNET_V4, |()| TEST_SUBNET_V6);

    // Connect to the watcher protocol and consume all existing events.
    let state_proxy =
        realm.connect_to_protocol::<I::StateMarker>().expect("failed to connect to routes/State");
    let event_stream = fnet_routes_ext::event_stream_from_state::<I>(&state_proxy)
        .expect("failed to connect to routes watcher");
    let mut event_stream = pin!(event_stream);

    // Skip all `existing` events.
    let _existing_routes =
        fnet_routes_ext::collect_routes_until_idle::<I, Vec<_>>(event_stream.by_ref())
            .await
            .expect("failed to collect existing routes");

    // Add a test route.
    interface.add_subnet_route(subnet.into_ext()).await.expect("failed to add route");

    // Verify the `Added` event is observed.
    let added_route = assert_matches!(
        event_stream.next().await,
        Some(Ok(fnet_routes_ext::Event::<I>::Added(route))) => route
    );
    let expected_route =
        new_installed_route(subnet, interface.id(), DEFAULT_INTERFACE_METRIC, true, main_table_id);
    assert_eq!(added_route, expected_route);

    // Remove the test route.
    interface.del_subnet_route(subnet.into_ext()).await.expect("failed to remove route");

    // Verify the removed event is observed.
    let removed_route = assert_matches!(
        event_stream.next().await,
        Some(Ok(fnet_routes_ext::Event::<I>::Removed(route))) => route
    );
    assert_eq!(removed_route, expected_route);
}

// Verifies the watcher protocols close if the client incorrectly calls `Watch()`
// while there is already a pending `Watch()` call parked in the server.
#[netstack_test]
#[variant(I, Ip)]
async fn watcher_already_pending<I: fnet_routes_ext::FidlRouteIpExt>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let state_proxy =
        realm.connect_to_protocol::<I::StateMarker>().expect("failed to connect to routes/State");
    let watcher_proxy = fnet_routes_ext::get_watcher::<I>(&state_proxy, Default::default())
        .expect("failed to connect to watcher protocol");

    // Call `Watch` in a loop until the idle event is observed.
    while fnet_routes_ext::watch::<I>(&watcher_proxy)
        .map(|event_batch| {
            event_batch.expect("error while calling watch").into_iter().all(|event| {
                use fnet_routes_ext::Event::*;
                match event.try_into().expect("failed to process event") {
                    Existing(_) => true,
                    Idle => false,
                    e @ Unknown | e @ Added(_) | e @ Removed(_) => {
                        panic!("unexpected event received from the routes watcher: {e:?}")
                    }
                }
            })
        })
        .await
    {}

    // Call `Watch` twice and observe the protocol close.
    assert_matches!(
        futures::future::join(
            fnet_routes_ext::watch::<I>(&watcher_proxy),
            fnet_routes_ext::watch::<I>(&watcher_proxy),
        )
        .await,
        (
            Err(fidl::Error::ClientChannelClosed { epitaph: fidl::Epitaph::PeerClosed, .. }),
            Err(fidl::Error::ClientChannelClosed { epitaph: fidl::Epitaph::PeerClosed, .. }),
        )
    );
    assert!(watcher_proxy.is_closed());
}

// Verifies the watcher protocol does not get torn down when the `State`
// protocol is closed.
#[netstack_test]
#[variant(I, Ip)]
async fn watcher_outlives_state<I: fnet_routes_ext::FidlRouteIpExt>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    // Connect to the watcher protocol and consume all existing events.
    let state_proxy =
        realm.connect_to_protocol::<I::StateMarker>().expect("failed to connect to routes/State");
    let event_stream = fnet_routes_ext::event_stream_from_state::<I>(&state_proxy)
        .expect("failed to connect to routes watcher");
    let event_stream = pin!(event_stream);

    // Drop the state proxy and verify the event_stream stays open
    drop(state_proxy);
    event_stream
        // Ignore `Ok` events; the stream closing will generate an `Err`.
        .filter(|event| futures::future::ready(event.is_err()))
        .next()
        .map(Err)
        .on_timeout(
            fuchsia_async::MonotonicInstant::after(
                netstack_testing_common::ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT,
            ),
            || Ok(()),
        )
        .await
        .expect("Unexpected event in event stream");
}

/// Verifies several instantiations of the watcher protocol can exist independent
/// of one another.
#[netstack_test]
#[variant(I, Ip)]
async fn watcher_multiple_instances<
    I: fnet_routes_ext::FidlRouteIpExt + fnet_routes_ext::admin::FidlRouteAdminIpExt,
>(
    name: &str,
) {
    const NUM_INSTANCES: u8 = 10;

    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let device = sandbox.create_endpoint(name).await.expect("create endpoint");
    let interface = device.into_interface_in_realm(&realm).await.expect("add endpoint to Netstack");
    let main_table_id = realm.main_table_id::<I>().await;

    let state_proxy =
        realm.connect_to_protocol::<I::StateMarker>().expect("failed to connect to routes/State");

    let mut watchers = Vec::new();
    let mut expected_existing_routes = Vec::new();

    // For each iteration, instantiate a watcher and add a unique route. The
    // route will appear as `added` for all "already-existing" watcher clients,
    // but `existing` for all "not-yet-instantiated" watcher clients in future
    // iterations. This ensures that each client is operating over a unique
    // event stream.
    for i in 0..NUM_INSTANCES {
        // Connect to the watcher protocol and observe all expected existing
        // events
        let mut event_stream = fnet_routes_ext::event_stream_from_state::<I>(&state_proxy)
            .expect("failed to connect to routes watcher")
            .boxed_local();
        let existing =
            fnet_routes_ext::collect_routes_until_idle::<I, Vec<_>>(event_stream.by_ref())
                .await
                .expect("failed to collect existing routes");
        for route in &expected_existing_routes {
            assert!(existing.contains(&route), "route: {:?}, existing: {:?}", route, existing)
        }
        watchers.push(event_stream);

        // Add a test route whose subnet is unique based on `i`.
        let subnet: net_types::ip::Subnet<I::Addr> = I::map_ip_out(
            i,
            |i| {
                net_types::ip::Subnet::new(net_types::ip::Ipv4Addr::new([192, 168, i, 0]), 24)
                    .unwrap()
            },
            |i| {
                net_types::ip::Subnet::new(
                    net_types::ip::Ipv6Addr::from_bytes([
                        0xfd, 0, i, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    ]),
                    64,
                )
                .unwrap()
            },
        );
        interface.add_subnet_route(subnet.into_ext()).await.expect("failed to add route");
        let expected_route = new_installed_route(
            subnet,
            interface.id(),
            DEFAULT_INTERFACE_METRIC,
            true,
            main_table_id,
        );
        expected_existing_routes.push(expected_route);

        // Observe an `added` event on all connected watchers.
        for event_stream in watchers.iter_mut() {
            let added_route = assert_matches!(
                event_stream.next().await,
                Some(Ok(fnet_routes_ext::Event::<I>::Added(route))) => route
            );
            assert_eq!(added_route, expected_route);
        }
    }
}

#[netstack_test]
#[variant(I, Ip)]
async fn watch_nonexisting_table<
    I: fnet_routes_ext::FidlRouteIpExt + fnet_routes_ext::admin::FidlRouteAdminIpExt,
>(
    name: &str,
) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let state_proxy =
        realm.connect_to_protocol::<I::StateMarker>().expect("failed to connect to routes/State");

    let watcher = fnet_routes_ext::get_watcher::<I>(
        &state_proxy,
        fnet_routes_ext::WatcherOptions {
            table_interest: Some(fnet_routes::TableInterest::Only(100)),
        },
    )
    .expect("failed to create watcher");

    let mut events = pin!(
        fnet_routes_ext::event_stream_from_watcher::<I>(watcher).expect("convert to event stream")
    );
    assert_matches!(events.next().await, Some(Ok(fnet_routes_ext::Event::Idle)));
}

#[netstack_test]
#[variant(I, Ip)]
async fn route_watcher_in_specific_table<
    I: fnet_routes_ext::admin::FidlRouteAdminIpExt + fnet_routes_ext::FidlRouteIpExt,
>(
    name: &str,
) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let TestSetup {
        realm,
        network: _network,
        interface,
        route_table: _,
        global_route_table: _,
        state,
    } = TestSetup::<I>::new(&sandbox, name).await;
    let route_table_provider = realm
        .connect_to_protocol::<I::RouteTableProviderMarker>()
        .expect("connect to main route table");
    let user_route_table =
        fnet_routes_ext::admin::new_route_table::<I>(&route_table_provider, None)
            .expect("create new user table");
    let user_table_id =
        fnet_routes_ext::admin::get_table_id::<I>(&user_route_table).await.expect("get table id");
    let user_route_set = fnet_routes_ext::admin::new_route_set::<I>(&user_route_table)
        .expect("failed to create a new user route set");

    let mut main_table_routes_stream = pin!(
        fnet_routes_ext::event_stream_from_state_with_options::<I>(
            &state,
            fnet_routes_ext::WatcherOptions {
                table_interest: Some(fnet_routes::TableInterest::Main(fnet_routes::Main)),
            },
        )
        .expect("failed to watch the main table")
    );

    let existing_main_routes =
        fnet_routes_ext::collect_routes_until_idle::<I, Vec<_>>(&mut main_table_routes_stream)
            .await
            .expect("collect routes should succeed");

    assert_eq!(
        existing_main_routes
            .iter()
            .find(|installed_route| installed_route.table_id == user_table_id),
        None
    );

    let grant = interface.get_authorization().await.expect("getting grant should succeed");
    let proof = fnet_interfaces_ext::admin::proof_from_grant(&grant);
    fnet_routes_ext::admin::authenticate_for_interface::<I>(&user_route_set, proof)
        .await
        .expect("no FIDL error")
        .expect("authentication should succeed");

    let route_to_add =
        test_route::<I>(&interface, fnet_routes::SpecifiedMetric::ExplicitMetric(10));

    assert!(
        fnet_routes_ext::admin::add_route::<I>(
            &user_route_set,
            &route_to_add.try_into().expect("convert to FIDL")
        )
        .await
        .expect("no FIDL error")
        .expect("add route")
    );

    let mut user_table_routes_stream = pin!(
        fnet_routes_ext::event_stream_from_state_with_options::<I>(
            &state,
            fnet_routes_ext::WatcherOptions {
                table_interest: Some(fnet_routes::TableInterest::Only(user_table_id.get()))
            }
        )
        .expect("failed to create event stream")
    );

    let user_table_routes =
        fnet_routes_ext::collect_routes_until_idle::<I, Vec<_>>(&mut user_table_routes_stream)
            .await
            .expect("collect routes should succeed");

    assert_matches!(
        &user_table_routes[..],
        [installed] => assert!(installed.matches_route_and_table_id(&route_to_add, user_table_id))
    );

    fnet_routes_ext::admin::remove_route_table::<I>(&user_route_table)
        .await
        .expect("fidl error")
        .expect("failed to remove table");

    assert_matches!(
        user_table_routes_stream.next().await,
        Some(Ok(fnet_routes_ext::Event::Removed(removed))) => assert!(
            removed.matches_route_and_table_id(&route_to_add, user_table_id)
        )
    );
    // The main table watcher must not have received any update for the user
    // table.
    assert_matches!(
        main_table_routes_stream
            .next()
            .on_timeout(
                fuchsia_async::MonotonicInstant::after(
                    netstack_testing_common::ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT
                ),
                || None,
            )
            .await,
        None
    );
}

#[netstack_test]
#[variant(I, Ip)]
async fn get_route_table_name<I: Ip + FidlRouteIpExt + FidlRouteAdminIpExt>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm =
        sandbox.create_netstack_realm::<Netstack3, _>(name).expect("failed to create realm");
    let routes = realm
        .connect_to_protocol::<fnet_routes::StateMarker>()
        .expect("failed to connect to routes/State");
    let routes = &routes;
    let route_table_provider = realm
        .connect_to_protocol::<I::RouteTableProviderMarker>()
        .expect("failed to connect to fuchsia.net.routes.admin.RouteTableProvider");
    let route_table_provider = &route_table_provider;
    let main_route_table = realm
        .connect_to_protocol::<I::RouteTableMarker>()
        .expect("failed to connect to main route table");
    let main_route_table_id = fnet_routes_ext::admin::get_table_id::<I>(&main_route_table)
        .await
        .expect("get main route table ID");

    const USER_TABLE_NAME: &str = "route-table-name";
    const MAIN_V4_NAME: &str = "main_v4";
    const MAIN_V6_NAME: &str = "main_v6";

    let create_route_table_and_get_id = |name: Option<&str>| {
        let name = name.map(str::to_owned);
        async move {
            let table = fnet_routes_ext::admin::new_route_table::<I>(&route_table_provider, name)
                .expect("create named route table");
            let table_id =
                fnet_routes_ext::admin::get_table_id::<I>(&table).await.expect("get table ID");
            (table, table_id)
        }
    };

    let nonexistent_table_id = fnet_routes_ext::TableId::new(5555);

    let (named_table, named_table_id) = create_route_table_and_get_id(Some(USER_TABLE_NAME)).await;
    let (unnamed_table, unnamed_table_id) = create_route_table_and_get_id(None).await;

    // Main table
    {
        let want = Ok(match I::VERSION {
            IpVersion::V4 => MAIN_V4_NAME,
            IpVersion::V6 => MAIN_V6_NAME,
        }
        .to_owned());
        let got = routes
            .get_route_table_name(main_route_table_id.get())
            .await
            .expect("should not get FIDL error");
        assert_eq!(got, want);
    }

    // Named table
    {
        let want = Ok(USER_TABLE_NAME.to_owned());
        let got = routes
            .get_route_table_name(named_table_id.get())
            .await
            .expect("should not get FIDL error");
        assert_eq!(got, want);
    }

    // Unnamed table
    {
        let want = Ok("".to_owned());
        let got = routes
            .get_route_table_name(unnamed_table_id.get())
            .await
            .expect("should not get FIDL error");
        assert_eq!(got, want);
    }

    // Nonexistent table
    {
        let want = Err(fnet_routes::StateGetRouteTableNameError::NoTable);
        let got = routes
            .get_route_table_name(nonexistent_table_id.get())
            .await
            .expect("should not get FIDL error");
        assert_eq!(got, want);
    }

    // After removing the tables, `get_route_table_name` should return `Err(NoTable)`.
    for table in [named_table, unnamed_table] {
        fnet_routes_ext::admin::remove_route_table::<I>(&table)
            .await
            .expect("should not get FIDL error")
            .expect("remove should succeed");
    }

    for table_id in [named_table_id, unnamed_table_id] {
        let want = Err(fnet_routes::StateGetRouteTableNameError::NoTable);
        let got =
            routes.get_route_table_name(table_id.get()).await.expect("should not get FIDL error");
        assert_eq!(got, want);
    }
}

#[netstack_test]
#[variant(I, Ip)]
async fn interface_local_route_table_initial_routes<
    I: Ip + FidlRouteIpExt + FidlRouteAdminIpExt,
>(
    name: &str,
) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox
        .create_netstack_realm::<Netstack3, _>(format!("routes-admin-{name}"))
        .expect("create realm");
    let network = sandbox.create_network(name).await.expect("create network");
    let interface = realm
        .join_network_with_if_config(&network, "ep1", netemul::InterfaceConfig::use_local_table())
        .await
        .expect("join network");
    let route_table_provider = realm
        .connect_to_protocol::<I::RouteTableProviderMarker>()
        .expect("connect to routes State");
    let fnet_resources::GrantForInterfaceAuthorization { interface_id, token } =
        interface.get_authorization().await.expect("failed to get authorization");

    let local_table = fnet_routes_ext::admin::get_interface_local_table::<I>(
        &route_table_provider,
        fnet_resources::ProofOfInterfaceAuthorization { interface_id, token },
    )
    .await
    .expect("fidl")
    .expect("failed to get interface local table");

    let local_table_id = fnet_routes_ext::admin::get_table_id::<I>(&local_table)
        .await
        .expect("failed to get table id")
        .get();
    let main_table_id = realm.main_table_id::<I>().await;
    assert_ne!(local_table_id, main_table_id);

    let state = realm.connect_to_protocol::<I::StateMarker>().expect("failed to connect");
    let routes_stream =
        fnet_routes_ext::event_stream_from_state::<I>(&state).expect("should succeed");
    let mut routes_stream = pin!(routes_stream);
    let got_routes =
        fnet_routes_ext::collect_routes_until_idle::<I, HashSet<_>>(&mut routes_stream)
            .await
            .expect("collect routes should succeed");
    #[derive(GenericOverIp)]
    #[generic_over_ip(I, Ip)]
    struct RoutesHolder<I: fnet_routes_ext::FidlRouteIpExt>(
        HashSet<fnet_routes_ext::InstalledRoute<I>>,
    );
    let loopback_id = realm
        .loopback_properties()
        .await
        .expect("failed to get loopback properties")
        .expect("loopback properties unexpectedly None")
        .id;
    let RoutesHolder(expected_routes) = I::map_ip_out(
        (loopback_id, interface_id),
        |(loopback_id, interface_id)| {
            RoutesHolder(
                initial_loopback_routes_v4(loopback_id.get(), main_table_id)
                    .chain(initial_ethernet_routes_v4(interface_id, local_table_id))
                    .collect::<HashSet<_>>(),
            )
        },
        |(loopback_id, interface_id)| {
            RoutesHolder(
                initial_loopback_routes_v6(loopback_id.get(), main_table_id)
                    .chain(initial_ethernet_routes_v6(interface_id, local_table_id))
                    .collect::<HashSet<_>>(),
            )
        },
    );
    assert_eq!(expected_routes, got_routes);
}

#[netstack_test]
#[test_case(true; "prefix advertisement")]
#[test_case(false; "route advertisement")]
async fn interface_local_route_table_ndp_routes(name: &str, on_link_route: bool) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let (_network, realm, interface, fake_ep) =
        netstack_testing_common::setup_network_with::<Netstack3, _>(
            &sandbox,
            name,
            netemul::InterfaceConfig::use_local_table(),
            std::iter::empty::<netstack_testing_common::realms::KnownServiceProvider>(),
        )
        .await
        .expect("failed to set network");
    let route_table_provider = realm
        .connect_to_protocol::<<Ipv6 as FidlRouteAdminIpExt>::RouteTableProviderMarker>()
        .expect("connect to routes State");
    let fnet_resources::GrantForInterfaceAuthorization { interface_id, token } =
        interface.get_authorization().await.expect("failed to get authorization");

    let local_table = fnet_routes_ext::admin::get_interface_local_table::<Ipv6>(
        &route_table_provider,
        fnet_resources::ProofOfInterfaceAuthorization { interface_id, token },
    )
    .await
    .expect("fidl")
    .expect("failed to get interface local table");

    let local_table_id = fnet_routes_ext::admin::get_table_id::<Ipv6>(&local_table)
        .await
        .expect("failed to get table id")
        .get();

    let state = realm
        .connect_to_protocol::<<Ipv6 as FidlRouteIpExt>::StateMarker>()
        .expect("failed to connect");
    let routes_stream =
        fnet_routes_ext::event_stream_from_state::<Ipv6>(&state).expect("should succeed");
    let mut routes_stream = pin!(routes_stream);
    let mut routes =
        fnet_routes_ext::collect_routes_until_idle::<Ipv6, HashSet<_>>(&mut routes_stream)
            .await
            .expect("collect routes should succeed");
    let options = if on_link_route {
        [NdpOptionBuilder::PrefixInformation(PrefixInformation::new(
            netstack_testing_common::constants::ipv6::GLOBAL_PREFIX.prefix(),
            true,
            false,
            9999,
            9999,
            netstack_testing_common::constants::ipv6::GLOBAL_PREFIX.network(),
        ))]
    } else {
        [NdpOptionBuilder::RouteInformation(RouteInformation::new(
            netstack_testing_common::constants::ipv6::GLOBAL_PREFIX,
            9999,
            Default::default(),
        ))]
    };

    netstack_testing_common::ndp::send_ra_with_router_lifetime(
        &fake_ep,
        9999,
        &options,
        netstack_testing_common::constants::ipv6::LINK_LOCAL_ADDR,
    )
    .await
    .expect("failed to send the RA");

    let expected_routes = [
        // The default route advertised by the RA.
        fnet_routes_ext::InstalledRoute {
            route: fnet_routes_ext::Route {
                destination: net_types::ip::Subnet::new(
                    net_types::ip::Ipv6::UNSPECIFIED_ADDRESS,
                    0,
                )
                .unwrap(),
                action: fnet_routes_ext::RouteAction::Forward(fnet_routes_ext::RouteTarget {
                    outbound_interface: interface.id(),
                    next_hop: Some(
                        SpecifiedAddr::new(
                            netstack_testing_common::constants::ipv6::LINK_LOCAL_ADDR,
                        )
                        .expect("is specified"),
                    ),
                }),
                properties: fnet_routes_ext::RouteProperties {
                    specified_properties: fnet_routes_ext::SpecifiedRouteProperties {
                        metric: fnet_routes::SpecifiedMetric::InheritedFromInterface(
                            fnet_routes::Empty,
                        ),
                    },
                },
            },
            effective_properties: fnet_routes_ext::EffectiveRouteProperties {
                metric: DEFAULT_INTERFACE_METRIC,
            },
            table_id: fnet_routes_ext::TableId::new(local_table_id),
        },
        // The route advertised in the NDP option.
        fnet_routes_ext::InstalledRoute {
            route: fnet_routes_ext::Route {
                destination: netstack_testing_common::constants::ipv6::GLOBAL_PREFIX,
                action: fnet_routes_ext::RouteAction::Forward(fnet_routes_ext::RouteTarget {
                    outbound_interface: interface.id(),
                    next_hop: (!on_link_route).then_some(
                        SpecifiedAddr::new(
                            netstack_testing_common::constants::ipv6::LINK_LOCAL_ADDR,
                        )
                        .expect("is specified"),
                    ),
                }),
                properties: fnet_routes_ext::RouteProperties {
                    specified_properties: fnet_routes_ext::SpecifiedRouteProperties {
                        metric: fnet_routes::SpecifiedMetric::InheritedFromInterface(
                            fnet_routes::Empty,
                        ),
                    },
                },
            },
            effective_properties: fnet_routes_ext::EffectiveRouteProperties {
                metric: DEFAULT_INTERFACE_METRIC,
            },
            table_id: fnet_routes_ext::TableId::new(local_table_id),
        },
    ];

    fnet_routes_ext::wait_for_routes(&mut routes_stream, &mut routes, |routes| {
        expected_routes.iter().all(|expected_route| routes.contains(expected_route))
    })
    .await
    .expect("failed to wait for the NDP route to show up");
}
