// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

use std::collections::{HashMap, HashSet};
use std::convert::From as _;
use std::pin::pin;

use fidl_fuchsia_net_ext::{FromExt as _, IntoExt as _};

use anyhow::Context as _;
use assert_matches::assert_matches;
use futures::{FutureExt as _, StreamExt as _, TryStreamExt as _};
use net_declare::{fidl_ip, fidl_ip_v4_with_prefix, fidl_mac};
use net_types::ip::{Ip, IpVersion};
use netemul::{RealmUdpSocket as _, TestInterface, TestNetwork, TestRealm, TestSandbox};
use netstack_testing_common::Result;
use netstack_testing_common::interfaces::TestInterfaceExt as _;
use netstack_testing_common::nud::FrameMetadata;
use netstack_testing_common::realms::{Netstack3, TestRealmExt as _, TestSandboxExt as _};
use netstack_testing_macros::netstack_test;
use test_case::test_case;

const ALICE_MAC: fidl_fuchsia_net::MacAddress = fidl_mac!("02:00:01:02:03:04");
const ALICE_IP: fidl_fuchsia_net::IpAddress = fidl_ip!("192.168.0.100");
const BOB_MAC: fidl_fuchsia_net::MacAddress = fidl_mac!("02:0A:0B:0C:0D:0E");
const BOB_IP: fidl_fuchsia_net::IpAddress = fidl_ip!("192.168.0.1");
const SUBNET_PREFIX: u8 = 24;

/// Helper type holding values pertinent to neighbor tests.
struct NeighborRealm<'a> {
    realm: TestRealm<'a>,
    ep: TestInterface<'a>,
    ipv6: fidl_fuchsia_net::IpAddress,
    loopback_id: u64,
}

/// Helper function to create a realm with a static IP address and an
/// endpoint with a set MAC address.
///
/// Returns the created realm, ep, the first observed assigned IPv6
/// address, and the id of the loopback interface.
async fn create_realm<'a>(
    sandbox: &'a TestSandbox,
    network: &'a TestNetwork<'a>,
    test_name: &'a str,
    variant_name: &'static str,
    static_addr: fidl_fuchsia_net::Subnet,
    mac: fidl_fuchsia_net::MacAddress,
) -> NeighborRealm<'a> {
    let realm = sandbox
        .create_netstack_realm::<Netstack3, _>(format!("{}_{}", test_name, variant_name))
        .expect("failed to create realm");
    let ep = realm
        .join_network_with(
            &network,
            format!("ep-{}", variant_name),
            netemul::new_endpoint_config(netemul::DEFAULT_MTU, Some(mac)),
            Default::default(),
        )
        .await
        .expect("failed to join network");
    ep.add_address_and_subnet_route(static_addr).await.expect("configure address");

    let fidl_fuchsia_net_interfaces_ext::Properties { id: loopback_id, .. } = realm
        .loopback_properties()
        .await
        .expect("loopback properties")
        .expect("loopback not found");

    // Get IPv6 address.
    let ipv6 = fidl_fuchsia_net_interfaces_ext::wait_interface(
        realm.get_interface_event_stream().expect("get interface event stream"),
        &mut std::collections::HashMap::<
            u64,
            fidl_fuchsia_net_interfaces_ext::PropertiesAndState<(), _>,
        >::new(),
        |interfaces| {
            interfaces.get(&ep.id())?.properties.addresses.iter().find_map(
                |&fidl_fuchsia_net_interfaces_ext::Address {
                     addr: fidl_fuchsia_net::Subnet { addr, prefix_len: _ },
                     assignment_state,
                     ..
                 }| {
                    assert_eq!(
                        assignment_state,
                        fidl_fuchsia_net_interfaces::AddressAssignmentState::Assigned
                    );
                    match addr {
                        fidl_fuchsia_net::IpAddress::Ipv4(_) => None,
                        addr @ fidl_fuchsia_net::IpAddress::Ipv6(_) => Some(addr),
                    }
                },
            )
        },
    )
    .await
    .expect("failed to retrieve IPv6 address");

    NeighborRealm { realm, ep, ipv6, loopback_id: loopback_id.get() }
}

/// Helper function that creates two realms in the same `network` with
/// default test parameters. Returns a tuple of realms `(alice, bob)`.
async fn create_neighbor_realms<'a>(
    sandbox: &'a TestSandbox,
    network: &'a TestNetwork<'a>,
    test_name: &'a str,
) -> (NeighborRealm<'a>, NeighborRealm<'a>) {
    let alice = create_realm(
        sandbox,
        network,
        test_name,
        "alice",
        fidl_fuchsia_net::Subnet { addr: ALICE_IP, prefix_len: SUBNET_PREFIX },
        ALICE_MAC,
    )
    .await;
    let bob = create_realm(
        sandbox,
        network,
        test_name,
        "bob",
        fidl_fuchsia_net::Subnet { addr: BOB_IP, prefix_len: SUBNET_PREFIX },
        BOB_MAC,
    )
    .await;
    (alice, bob)
}

/// Gets a neighbor entry iterator stream with `options` in `realm`.
fn get_entry_iterator(
    realm: &TestRealm<'_>,
    options: fidl_fuchsia_net_neighbor::EntryIteratorOptions,
) -> impl futures::Stream<Item = fidl_fuchsia_net_neighbor::EntryIteratorItem> {
    let view = realm
        .connect_to_protocol::<fidl_fuchsia_net_neighbor::ViewMarker>()
        .expect("failed to connect to fuchsia.net.neighbor/View");
    let (proxy, server_end) =
        fidl::endpoints::create_proxy::<fidl_fuchsia_net_neighbor::EntryIteratorMarker>();
    view.open_entry_iterator(server_end, &options).expect("failed to open EntryIterator");
    futures::stream::unfold(proxy, |proxy| {
        proxy.get_next().map(|r| {
            let it = r.expect("fuchsia.net.neighbor/EntryIterator.GetNext FIDL error");
            Some((futures::stream::iter(it.into_iter()), proxy))
        })
    })
    .flatten()
}

/// Retrieves all existing neighbor entries in `realm`.
///
/// Entries are identified by unique `(interface_id, ip_address)` tuples in the
/// returned map.
async fn list_existing_entries(
    realm: &TestRealm<'_>,
) -> HashMap<(u64, fidl_fuchsia_net::IpAddress), fidl_fuchsia_net_neighbor::Entry> {
    use async_utils::fold::*;
    fold_while(
        get_entry_iterator(realm, fidl_fuchsia_net_neighbor::EntryIteratorOptions::default()),
        HashMap::new(),
        |mut map, item| {
            futures::future::ready(match item {
                fidl_fuchsia_net_neighbor::EntryIteratorItem::Existing(e) => {
                    if let fidl_fuchsia_net_neighbor::Entry {
                        interface: Some(interface),
                        neighbor: Some(neighbor),
                        ..
                    } = &e
                    {
                        if let Some(e) = map.insert((*interface, neighbor.clone()), e) {
                            panic!("duplicate entry detected in map: {:?}", e)
                        } else {
                            FoldWhile::Continue(map)
                        }
                    } else {
                        panic!("missing interface or neighbor in existing entry: {:?}", e)
                    }
                }
                fidl_fuchsia_net_neighbor::EntryIteratorItem::Idle(
                    fidl_fuchsia_net_neighbor::IdleEvent {},
                ) => FoldWhile::Done(map),
                x @ fidl_fuchsia_net_neighbor::EntryIteratorItem::Added(_)
                | x @ fidl_fuchsia_net_neighbor::EntryIteratorItem::Changed(_)
                | x @ fidl_fuchsia_net_neighbor::EntryIteratorItem::Removed(_) => {
                    panic!("unexpected EntryIteratorItem before Idle: {:?}", x)
                }
            })
        },
    )
    .await
    .short_circuited()
    .expect("entry iterator stream ended unexpectedly")
}

/// Helper function to exchange a single UDP datagram.
///
/// `alice` will send a single UDP datagram to `bob`. This function will block
/// until `bob` receives the datagram.
async fn exchange_dgram(
    alice: &NeighborRealm<'_>,
    alice_addr: fidl_fuchsia_net::IpAddress,
    bob: &NeighborRealm<'_>,
    bob_addr: fidl_fuchsia_net::IpAddress,
) {
    #[track_caller]
    fn socket_addr_with_scope_id(
        addr: fidl_fuchsia_net::IpAddress,
        port: u16,
        interface_id: u64,
    ) -> std::net::SocketAddr {
        let fidl_fuchsia_net_ext::IpAddress(addr) = fidl_fuchsia_net_ext::IpAddress::from(addr);
        match addr {
            std::net::IpAddr::V4(_) => std::net::SocketAddr::new(addr, port),
            std::net::IpAddr::V6(addr) => std::net::SocketAddrV6::new(
                addr,
                port,
                0, /* flowinfo */
                interface_id.try_into().expect("interface ID should fit into u32"),
            )
            .into(),
        }
    }

    const ALICE_PORT: u16 = 1234;
    const BOB_PORT: u16 = 8080;

    let alice_sock = fuchsia_async::net::UdpSocket::bind_in_realm(
        &alice.realm,
        socket_addr_with_scope_id(alice_addr, ALICE_PORT, alice.ep.id()),
    )
    .await
    .expect("failed to create client socket");

    let bob_sock = fuchsia_async::net::UdpSocket::bind_in_realm(
        &bob.realm,
        socket_addr_with_scope_id(bob_addr, BOB_PORT, bob.ep.id()),
    )
    .await
    .expect("failed to create server socket");

    const PAYLOAD: &'static str = "Hello Neighbor";
    let mut buf = [0u8; 512];
    let (sent, (rcvd, from)) = futures::future::try_join(
        alice_sock
            .send_to(
                PAYLOAD.as_bytes(),
                socket_addr_with_scope_id(bob_addr, BOB_PORT, alice.ep.id()),
            )
            .map(|r| r.context("UDP send_to failed")),
        bob_sock.recv_from(&mut buf[..]).map(|r| r.context("UDP recv_from failed")),
    )
    .await
    .expect("failed to send datagram from alice to bob");
    assert_eq!(sent, PAYLOAD.as_bytes().len());
    assert_eq!(rcvd, PAYLOAD.as_bytes().len());
    assert_eq!(&buf[..rcvd], PAYLOAD.as_bytes());
    assert_eq!(from, socket_addr_with_scope_id(alice_addr, ALICE_PORT, bob.ep.id()));
}

#[derive(Debug, Clone)]
struct EntryMatch {
    interface: u64,
    neighbor: fidl_fuchsia_net::IpAddress,
    state: fidl_fuchsia_net_neighbor::EntryState,
    mac: Option<fidl_fuchsia_net::MacAddress>,
}

impl EntryMatch {
    fn new(
        interface: u64,
        neighbor: fidl_fuchsia_net::IpAddress,
        state: fidl_fuchsia_net_neighbor::EntryState,
        mac: Option<fidl_fuchsia_net::MacAddress>,
    ) -> Self {
        Self { interface, neighbor, state, mac }
    }
}

#[derive(Debug)]
enum ItemMatch {
    Added(EntryMatch),
    Changed(EntryMatch),
    Removed(EntryMatch),
    Idle,
}

#[track_caller]
fn assert_entry(entry: fidl_fuchsia_net_neighbor::Entry, entry_match: EntryMatch) {
    let EntryMatch {
        interface: match_interface,
        neighbor: match_neighbor,
        state: match_state,
        mac: match_mac,
    } = entry_match;
    assert_matches::assert_matches!(
        entry,
        fidl_fuchsia_net_neighbor::Entry {
            interface: Some(interface),
            neighbor: Some(neighbor),
            state: Some(state),
            mac,
            updated_at: Some(updated_at), ..
        } => {
            assert!(
                interface == match_interface
                    && neighbor == match_neighbor
                    && state == match_state
                    && mac == match_mac
                    && updated_at != 0,
                "{entry:?} does not match {entry_match:?}"
            )
        }
    );
}

async fn assert_entries<
    S: futures::stream::Stream<Item = fidl_fuchsia_net_neighbor::EntryIteratorItem> + Unpin,
    I: IntoIterator<Item = ItemMatch>,
>(
    st: &mut S,
    entries: I,
) {
    for expect in entries {
        let item = st
            .next()
            .await
            .unwrap_or_else(|| panic!("stream ended unexpectedly waiting for {:?}", expect));
        match (item, expect) {
            (
                fidl_fuchsia_net_neighbor::EntryIteratorItem::Added(entry),
                ItemMatch::Added(expect),
            )
            | (
                fidl_fuchsia_net_neighbor::EntryIteratorItem::Removed(entry),
                ItemMatch::Removed(expect),
            )
            | (
                fidl_fuchsia_net_neighbor::EntryIteratorItem::Changed(entry),
                ItemMatch::Changed(expect),
            ) => {
                assert_entry(entry, expect);
            }
            (
                fidl_fuchsia_net_neighbor::EntryIteratorItem::Idle(
                    fidl_fuchsia_net_neighbor::IdleEvent {},
                ),
                ItemMatch::Idle,
            ) => (),
            (got, want) => {
                panic!("got iterator item {:?}, want {:?}", got, want);
            }
        }
    }
}

#[netstack_test]
async fn neigh_list_entries(name: &str) {
    let sandbox = TestSandbox::new().expect("failed to create sandbox");
    let network = sandbox.create_network("net").await.expect("failed to create network");

    let (alice, bob) = create_neighbor_realms(&sandbox, &network, name).await;
    // Apply the NUD flake workaround, since we expect all neighbor resolution
    // to succeed in this test case.
    alice.ep.apply_nud_flake_workaround().await.expect("nud flake workaround");
    bob.ep.apply_nud_flake_workaround().await.expect("nud flake workaround");

    let mut alice_iter = get_entry_iterator(
        &alice.realm,
        fidl_fuchsia_net_neighbor::EntryIteratorOptions::default(),
    );
    let mut bob_iter =
        get_entry_iterator(&bob.realm, fidl_fuchsia_net_neighbor::EntryIteratorOptions::default());

    // No Neighbors should exist initially.
    assert_entries(&mut alice_iter, [ItemMatch::Idle]).await;
    assert_entries(&mut bob_iter, [ItemMatch::Idle]).await;

    // Assert the same by iterating only on existing.
    let alice_entries = list_existing_entries(&alice.realm).await;
    assert!(alice_entries.is_empty(), "expected empty set of entries: {:?}", alice_entries);

    // Send a single UDP datagram between alice and bob. Prove this is a hanging
    // get by performing the exchange at the same time as we're hanging for the
    // entry updates.
    let ((), ()) = futures::future::join(
        exchange_dgram(&alice, ALICE_IP, &bob, BOB_IP),
        assert_entries(&mut alice_iter, incomplete_then_reachable(alice.ep.id(), BOB_IP, BOB_MAC)),
    )
    .await;
    let ((), ()) = futures::future::join(
        exchange_dgram(&alice, alice.ipv6, &bob, bob.ipv6),
        assert_entries(
            &mut alice_iter,
            incomplete_then_reachable(alice.ep.id(), bob.ipv6.clone(), BOB_MAC),
        ),
    )
    .await;

    // Check that bob is listed as a neighbor for alice.
    let mut alice_entries = list_existing_entries(&alice.realm).await;
    // IPv4 entry.
    assert_entry(
        alice_entries.remove(&(alice.ep.id(), BOB_IP)).expect("missing IPv4 neighbor entry"),
        EntryMatch::new(
            alice.ep.id(),
            BOB_IP,
            fidl_fuchsia_net_neighbor::EntryState::Reachable,
            Some(BOB_MAC),
        ),
    );
    // IPv6 entry.
    assert_entry(
        alice_entries.remove(&(alice.ep.id(), bob.ipv6)).expect("missing IPv6 neighbor entry"),
        EntryMatch::new(
            alice.ep.id(),
            bob.ipv6,
            fidl_fuchsia_net_neighbor::EntryState::Reachable,
            Some(BOB_MAC),
        ),
    );
    assert!(
        alice_entries.is_empty(),
        "unexpected neighbors remaining in list: {:?}",
        alice_entries
    );

    // Check that alice is listed as a neighbor for bob. Bob should have alice
    // listed as STALE entries due to having received solicitations as part of
    // the UDP exchange.
    assert_entries(
        &mut bob_iter,
        [
            ItemMatch::Added(EntryMatch {
                interface: bob.ep.id(),
                neighbor: ALICE_IP,
                state: fidl_fuchsia_net_neighbor::EntryState::Stale,
                mac: Some(ALICE_MAC),
            }),
            ItemMatch::Added(EntryMatch {
                interface: bob.ep.id(),
                neighbor: alice.ipv6.clone(),
                state: fidl_fuchsia_net_neighbor::EntryState::Stale,
                mac: Some(ALICE_MAC),
            }),
        ],
    )
    .await;

    // Check that the same state is present on a new channel.
    let mut bob_entries = list_existing_entries(&bob.realm).await;

    // IPv4 entry.
    assert_entry(
        bob_entries.remove(&(bob.ep.id(), ALICE_IP)).expect("missing IPv4 neighbor entry"),
        EntryMatch::new(
            bob.ep.id(),
            ALICE_IP,
            fidl_fuchsia_net_neighbor::EntryState::Stale,
            Some(ALICE_MAC),
        ),
    );
    // IPv6 entry.
    assert_entry(
        bob_entries.remove(&(bob.ep.id(), alice.ipv6)).expect("missing IPv6 neighbor entry"),
        EntryMatch::new(
            bob.ep.id(),
            alice.ipv6,
            // TODO(https://fxbug.dev/42081683): Simpify to just asserting that the
            // state is STALE when NS3 no longer consults the neighbor table
            // when sending the NA message which causes a state transition from
            // STALE to DELAY.
            fidl_fuchsia_net_neighbor::EntryState::Delay,
            Some(ALICE_MAC),
        ),
    );
    assert!(bob_entries.is_empty(), "unexpected neighbors remaining in list: {:?}", bob_entries);
}

fn incomplete_then_reachable(
    interface: u64,
    neighbor: fidl_fuchsia_net::IpAddress,
    mac: fidl_fuchsia_net::MacAddress,
) -> [ItemMatch; 2] {
    [
        ItemMatch::Added(EntryMatch {
            interface,
            neighbor: neighbor.clone(),
            state: fidl_fuchsia_net_neighbor::EntryState::Incomplete,
            mac: None,
        }),
        ItemMatch::Changed(EntryMatch {
            interface,
            neighbor,
            state: fidl_fuchsia_net_neighbor::EntryState::Reachable,
            mac: Some(mac),
        }),
    ]
}

/// Helper function to observe the next neighbor solicitation on a [`FrameMetadata`] stream.
///
/// This function returns the IP of the first neighbor solicitation it sees.
async fn next_solicitation(
    stream: &mut (impl futures::Stream<Item = Result<FrameMetadata>> + std::marker::Unpin),
) -> fidl_fuchsia_net::IpAddress {
    loop {
        match stream
            .try_next()
            .await
            .expect("failed to read from metadata stream")
            .expect("metadata stream ended unexpectedly")
        {
            FrameMetadata::NeighborSolicitation(ip) => return ip,
            _ => (),
        }
    }
}

/// Helper function to observe the next solicitation resolution on a
/// [`FrameMetadata`] stream.
///
/// This function collects all the neighbor solicitations it sees until it
/// receives a UDP packet. It then asserts that the UDP packet's destination IP
/// matches the given `dst_ip`.
async fn next_solicitation_resolution(
    stream: &mut (impl futures::Stream<Item = Result<FrameMetadata>> + std::marker::Unpin),
    dst_ip: fidl_fuchsia_net::IpAddress,
) -> HashSet<fidl_fuchsia_net::IpAddress> {
    let mut solicitations = HashSet::new();
    loop {
        match stream
            .try_next()
            .await
            .expect("failed to read from metadata stream")
            .expect("metadata stream ended unexpectedly")
        {
            FrameMetadata::NeighborSolicitation(ip) => {
                let _ = solicitations.insert(ip);
            }
            FrameMetadata::Udp(ip) => {
                assert_eq!(ip, dst_ip);
                return solicitations;
            }
            FrameMetadata::Other => (),
        }
    }
}

#[netstack_test]
#[test_case(
    |controller, id| {
        controller.clear_entries(id, fidl_fuchsia_net::IpVersion::V4)
    };
    "clear_entries"
)]
#[test_case(|controller, id| controller.add_entry(id, &BOB_IP, &BOB_MAC); "add_entry")]
#[test_case(|controller, id| controller.remove_entry(id, &BOB_IP); "remove_entry")]
async fn neigh_wrong_interface(
    name: &str,
    fidl_method: fn(
        &fidl_fuchsia_net_neighbor::ControllerProxy,
        u64,
    ) -> fidl::client::QueryResponseFut<
        std::result::Result<(), fidl_fuchsia_net_neighbor::ControllerError>,
    >,
) {
    let sandbox = TestSandbox::new().expect("failed to create sandbox");
    let network = sandbox.create_network("net").await.expect("failed to create network");

    let NeighborRealm { realm, ep, ipv6: _, loopback_id } = create_realm(
        &sandbox,
        &network,
        name,
        "alice",
        fidl_fuchsia_net::Subnet { addr: ALICE_IP, prefix_len: SUBNET_PREFIX },
        ALICE_MAC,
    )
    .await;

    // Connect to the service and check some error cases.
    let controller = realm
        .connect_to_protocol::<fidl_fuchsia_net_neighbor::ControllerMarker>()
        .expect("failed to connect to Controller");
    // Clearing neighbors on loopback interface is not supported.
    assert_eq!(
        fidl_method(&controller, loopback_id).await.expect("clear_entries FIDL error"),
        Err(fidl_fuchsia_net_neighbor::ControllerError::InterfaceNotSupported)
    );
    // Clearing neighbors on non-existing interface returns the proper error.
    assert_eq!(
        fidl_method(&controller, ep.id() + 100).await.expect("clear_entries FIDL error"),
        Err(fidl_fuchsia_net_neighbor::ControllerError::InterfaceNotFound)
    );
}

#[netstack_test]
#[variant(I, Ip)]
async fn neigh_clear_entries<I: Ip>(name: &str) {
    let sandbox = TestSandbox::new().expect("failed to create sandbox");
    let network = sandbox.create_network("net").await.expect("failed to create network");

    // Attach a fake endpoint that will capture all the ARP and NDP neighbor
    // solicitations.
    let fake_ep = network.create_fake_endpoint().expect("failed to create fake endpoint");
    let mut solicit_stream = netstack_testing_common::nud::create_metadata_stream(&fake_ep);

    let (alice, bob) = create_neighbor_realms(&sandbox, &network, name).await;
    // Apply the NUD flake workaround, since we expect all neighbor resolution
    // to succeed in this test case.
    alice.ep.apply_nud_flake_workaround().await.expect("nud flake workaround");
    bob.ep.apply_nud_flake_workaround().await.expect("nud flake workaround");

    let alice_controller = alice
        .realm
        .connect_to_protocol::<fidl_fuchsia_net_neighbor::ControllerMarker>()
        .expect("failed to connect to Controller");

    let mut iter = get_entry_iterator(
        &alice.realm,
        fidl_fuchsia_net_neighbor::EntryIteratorOptions::default(),
    );
    assert_entries(&mut iter, [ItemMatch::Idle]).await;

    let (alice_ip, bob_ip) = match I::VERSION {
        IpVersion::V4 => (ALICE_IP, BOB_IP),
        IpVersion::V6 => (alice.ipv6, bob.ipv6),
    };

    // Exchange some datagrams to add some entries to the list and check that we
    // observe the neighbor solicitations over the network.
    exchange_dgram(&alice, alice_ip, &bob, bob_ip).await;

    assert_eq!(
        next_solicitation_resolution(&mut solicit_stream, bob_ip).await,
        HashSet::from_iter(std::iter::once(bob_ip))
    );
    assert_entries(&mut iter, incomplete_then_reachable(alice.ep.id(), bob_ip, BOB_MAC)).await;

    // Clear entries and verify they go away.
    alice_controller
        .clear_entries(alice.ep.id(), I::VERSION.into_ext())
        .await
        .expect("clear_entries FIDL error")
        .expect("clear_entries failed");

    assert_entries(
        &mut iter,
        [ItemMatch::Removed(EntryMatch {
            interface: alice.ep.id(),
            neighbor: bob_ip,
            state: fidl_fuchsia_net_neighbor::EntryState::Reachable,
            mac: Some(BOB_MAC),
        })],
    )
    .await;

    let bob_controller = bob
        .realm
        .connect_to_protocol::<fidl_fuchsia_net_neighbor::ControllerMarker>()
        .expect("failed to connect to Controller");
    // Add static entries on Bob so that it will never send out neighbor
    // solicitations that could cause confusion for the assertions at the
    // end of this test.
    bob_controller
        .add_entry(bob.ep.id(), &alice_ip, &ALICE_MAC)
        .await
        .expect("add_entry FIDL error")
        .expect("add_entry failed");

    const MAX_RETRIES: usize = 5;
    let mut all_solicitations = HashSet::new();
    for _ in 0..MAX_RETRIES {
        // Exchange datagrams again and assert that new solicitation requests were
        // sent.
        exchange_dgram(&alice, alice_ip, &bob, bob_ip).await;
        let solicitations = next_solicitation_resolution(&mut solicit_stream, bob_ip).await;
        if solicitations == HashSet::from_iter(std::iter::once(bob_ip)) {
            return;
        } else {
            // We failed the test because Bob's delayed advertisement may have installed
            // a new neighbor entry at Alice. Since Alice must have a neighbor entry
            // either way because of the `exchange_dgram`, we need to clear the entries
            // and go again.
            all_solicitations.extend(solicitations.into_iter());
            alice_controller
                .clear_entries(alice.ep.id(), I::VERSION.into_ext())
                .await
                .expect("clear_entries FIDL error")
                .expect("clear_entries failed")
        }
    }
    panic!(
        "maximum retries {} exceeded, all solicitations received: {:?}",
        MAX_RETRIES, all_solicitations
    );
}

#[netstack_test]
#[test_case(fidl_ip!("255.255.255.255"); "ipv4_limited_broadcast")]
#[test_case(fidl_ip!("127.0.0.1"); "ipv4_loopback")]
#[test_case(fidl_ip!("::1"); "ipv6_loopback")]
#[test_case(fidl_ip!("224.0.0.0"); "ipv4_multicast")]
#[test_case(fidl_ip!("ff00::"); "ipv6_multicast")]
#[test_case(fidl_ip!("0.0.0.0"); "ipv4_unspecified")]
#[test_case(fidl_ip!("::"); "ipv6_unspecified")]
#[test_case(fidl_ip!("::ffff:0:1"); "ipv6_mapped")]
async fn neigh_add_remove_entry_invalid_addr(
    name: &str,
    invalid_addr: fidl_fuchsia_net::IpAddress,
) {
    let sandbox = TestSandbox::new().expect("failed to create sandbox");
    let network = sandbox.create_network("net").await.expect("failed to create network");

    let alice = create_realm(
        &sandbox,
        &network,
        name,
        "alice",
        fidl_fuchsia_net::Subnet { addr: ALICE_IP, prefix_len: SUBNET_PREFIX },
        ALICE_MAC,
    )
    .await;

    let controller = alice
        .realm
        .connect_to_protocol::<fidl_fuchsia_net_neighbor::ControllerMarker>()
        .expect("failed to connect to Controller");

    assert_eq!(
        controller
            .add_entry(alice.ep.id(), &invalid_addr, &BOB_MAC)
            .await
            .expect("add_entry FIDL error"),
        Err(fidl_fuchsia_net_neighbor::ControllerError::InvalidIpAddress),
        "{} is an invalid neighbor addr and add_entry should fail",
        net_types::ip::IpAddr::from_ext(invalid_addr)
    );
    assert_eq!(
        controller
            .remove_entry(alice.ep.id(), &invalid_addr)
            .await
            .expect("remove_entry FIDL error"),
        Err(fidl_fuchsia_net_neighbor::ControllerError::InvalidIpAddress),
        "{} is an invalid neighbor addr and remove_entry should fail",
        net_types::ip::IpAddr::from_ext(invalid_addr)
    );
}

#[netstack_test]
#[test_case(fidl_mac!("ff:ff:ff:ff:ff:ff"); "broadcast_mac")]
#[test_case(fidl_mac!("01:00:00:00:00:00"); "multicast_mac")]
async fn neigh_add_entry_invalid_mac(name: &str, invalid_mac: fidl_fuchsia_net::MacAddress) {
    let sandbox = TestSandbox::new().expect("failed to create sandbox");
    let network = sandbox.create_network("net").await.expect("failed to create network");

    let alice = create_realm(
        &sandbox,
        &network,
        name,
        "alice",
        fidl_fuchsia_net::Subnet { addr: ALICE_IP, prefix_len: SUBNET_PREFIX },
        ALICE_MAC,
    )
    .await;

    let controller = alice
        .realm
        .connect_to_protocol::<fidl_fuchsia_net_neighbor::ControllerMarker>()
        .expect("failed to connect to Controller");

    for valid_addr in [fidl_ip!("2001:db8::1"), fidl_ip!("192.0.2.1")] {
        assert_eq!(
            controller
                .add_entry(alice.ep.id(), &valid_addr, &invalid_mac)
                .await
                .expect("add_entry FIDL error"),
            Err(fidl_fuchsia_net_neighbor::ControllerError::MacAddressNotUnicast),
            "{} is not a unicast mac addr and add_entry should fail",
            net_types::ethernet::Mac::from_ext(invalid_mac)
        );
    }
}

// TODO(https://fxbug.dev/42075782): Remove this test when NS3 passes
// neigh_add_remove_entry since that test is a superset of this test but it
// doesn't yet pass due to the lack of fuchsia.net.neighbor/EntryIterator.
#[netstack_test]
async fn neigh_remove_entry_not_found(name: &str) {
    let sandbox = TestSandbox::new().expect("failed to create sandbox");
    let network = sandbox.create_network("net").await.expect("failed to create network");

    let alice = create_realm(
        &sandbox,
        &network,
        name,
        "alice",
        fidl_fuchsia_net::Subnet { addr: ALICE_IP, prefix_len: SUBNET_PREFIX },
        ALICE_MAC,
    )
    .await;

    let controller = alice
        .realm
        .connect_to_protocol::<fidl_fuchsia_net_neighbor::ControllerMarker>()
        .expect("failed to connect to Controller");

    assert_eq!(
        controller.remove_entry(alice.ep.id(), &BOB_IP).await.expect("remove_entry FIDL error"),
        Err(fidl_fuchsia_net_neighbor::ControllerError::NeighborNotFound)
    );
}

#[netstack_test]
#[variant(I, Ip)]
async fn neigh_add_remove_entry<I: Ip>(name: &str) {
    let sandbox = TestSandbox::new().expect("failed to create sandbox");
    let network = sandbox.create_network("net").await.expect("failed to create network");

    // Attach a fake endpoint that will observe neighbor solicitations and UDP
    // frames.
    let fake_ep = network.create_fake_endpoint().expect("failed to create fake endpoint");
    let mut meta_stream = netstack_testing_common::nud::create_metadata_stream(&fake_ep)
        .try_filter_map(|m| {
            futures::future::ok(match m {
                m @ FrameMetadata::NeighborSolicitation(_) | m @ FrameMetadata::Udp(_) => Some(m),
                FrameMetadata::Other => None,
            })
        });

    let (alice, bob) = create_neighbor_realms(&sandbox, &network, name).await;
    // Apply the NUD flake workaround, since we expect all neighbor resolution
    // to succeed in this test case.
    alice.ep.apply_nud_flake_workaround().await.expect("nud flake workaround");
    bob.ep.apply_nud_flake_workaround().await.expect("nud flake workaround");

    let controller = alice
        .realm
        .connect_to_protocol::<fidl_fuchsia_net_neighbor::ControllerMarker>()
        .expect("failed to connect to Controller");

    let mut alice_iter = get_entry_iterator(
        &alice.realm,
        fidl_fuchsia_net_neighbor::EntryIteratorOptions::default(),
    );
    assert_entries(&mut alice_iter, [ItemMatch::Idle]).await;

    let (alice_ip, bob_ip) = match I::VERSION {
        IpVersion::V4 => (ALICE_IP, BOB_IP),
        IpVersion::V6 => (alice.ipv6, bob.ipv6),
    };

    // Check error conditions.
    // Add and remove entry not supported on loopback.
    assert_eq!(
        controller
            .add_entry(alice.loopback_id, &bob_ip, &BOB_MAC)
            .await
            .expect("add_entry FIDL error"),
        Err(fidl_fuchsia_net_neighbor::ControllerError::InterfaceNotSupported)
    );
    assert_eq!(
        controller.remove_entry(alice.loopback_id, &bob_ip).await.expect("remove_entry FIDL error"),
        Err(fidl_fuchsia_net_neighbor::ControllerError::InterfaceNotSupported)
    );
    // Add entry and remove entry return not found on non-existing interface.
    assert_eq!(
        controller
            .add_entry(alice.ep.id() + 100, &bob_ip, &BOB_MAC)
            .await
            .expect("add_entry FIDL error"),
        Err(fidl_fuchsia_net_neighbor::ControllerError::InterfaceNotFound)
    );
    assert_eq!(
        controller
            .remove_entry(alice.ep.id() + 100, &bob_ip)
            .await
            .expect("remove_entry FIDL error"),
        Err(fidl_fuchsia_net_neighbor::ControllerError::InterfaceNotFound)
    );
    // Remove entry returns not found for non-existing entry.
    assert_eq!(
        controller.remove_entry(alice.ep.id(), &bob_ip).await.expect("remove_entry FIDL error"),
        Err(fidl_fuchsia_net_neighbor::ControllerError::NeighborNotFound)
    );

    // Add a static entry and verify that it is listable.
    controller
        .add_entry(alice.ep.id(), &bob_ip, &BOB_MAC)
        .await
        .expect("add_entry FIDL error")
        .expect("add_entry failed");
    let static_entry = EntryMatch {
        interface: alice.ep.id(),
        neighbor: bob_ip,
        state: fidl_fuchsia_net_neighbor::EntryState::Static,
        mac: Some(BOB_MAC),
    };

    assert_entries(&mut alice_iter, [ItemMatch::Added(static_entry.clone())]).await;

    exchange_dgram(&alice, alice_ip, &bob, bob_ip).await;
    assert_eq!(
        meta_stream
            .try_next()
            .await
            .expect("failed to read from fake endpoint")
            .expect("fake endpoint stream ended unexpectedly"),
        FrameMetadata::Udp(bob_ip)
    );

    // Remove the entry and check that the list is empty afterwards.
    controller
        .remove_entry(alice.ep.id(), &bob_ip)
        .await
        .expect("remove_entry FIDL error")
        .expect("remove_entry failed");
    assert_entries(&mut alice_iter, [ItemMatch::Removed(static_entry.clone())]).await;

    // Exchange datagrams again and assert that new solicitation requests were
    // sent (ignoring any UDP metadata this time).
    exchange_dgram(&alice, alice_ip, &bob, bob_ip).await;
    assert_eq!(
        next_solicitation_resolution(&mut meta_stream, bob_ip).await,
        HashSet::from_iter(std::iter::once(bob_ip))
    );
}

#[netstack_test]
async fn neigh_unreachable_entries(name: &str) {
    let sandbox = TestSandbox::new().expect("failed to create sandbox");
    let network = sandbox.create_network("net").await.expect("failed to create network");

    let alice = create_realm(
        &sandbox,
        &network,
        name,
        "alice",
        fidl_fuchsia_net::Subnet { addr: ALICE_IP, prefix_len: SUBNET_PREFIX },
        ALICE_MAC,
    )
    .await;

    let mut iter = get_entry_iterator(
        &alice.realm,
        fidl_fuchsia_net_neighbor::EntryIteratorOptions::default(),
    );

    // No Neighbors should exist initially.
    assert_entries(&mut iter, [ItemMatch::Idle]).await;

    // Intentionally fail to send a packet to Bob so we can create a neighbor
    // entry and have it go to UNREACHABLE state.
    let fidl_fuchsia_net_ext::IpAddress(alice_addr) = ALICE_IP.into();
    let alice_addr = std::net::SocketAddr::new(alice_addr, 1234);

    let fidl_fuchsia_net_ext::IpAddress(bob_addr) = BOB_IP.into();
    let bob_addr = std::net::SocketAddr::new(bob_addr, 8080);

    const PAYLOAD: &'static str = "Hello, is it me you're looking for?";
    assert_eq!(
        fuchsia_async::net::UdpSocket::bind_in_realm(&alice.realm, alice_addr)
            .await
            .expect("failed to create client socket")
            .send_to(PAYLOAD.as_bytes(), bob_addr)
            .await
            .expect("UDP send_to failed"),
        PAYLOAD.as_bytes().len()
    );

    let want_incomplete_entry = EntryMatch {
        interface: alice.ep.id(),
        neighbor: BOB_IP,
        state: fidl_fuchsia_net_neighbor::EntryState::Incomplete,
        mac: None,
    };
    assert_entries(
        &mut iter,
        [
            ItemMatch::Added(want_incomplete_entry.clone()),
            // TODO(https://fxbug.dev/42082448): Expect the entry to change to sentinel
            // state instead of being removed entirely.
            ItemMatch::Removed(want_incomplete_entry),
        ],
    )
    .await;
}

#[netstack_test]
async fn cant_hang_twice(name: &str) {
    let sandbox = TestSandbox::new().expect("failed to create sandbox");

    let realm =
        sandbox.create_netstack_realm::<Netstack3, _>(name).expect("failed to create realm");

    let view = realm
        .connect_to_protocol::<fidl_fuchsia_net_neighbor::ViewMarker>()
        .expect("failed to connect to fuchsia.net.neighbor/View");
    let (iter, server_end) =
        fidl::endpoints::create_proxy::<fidl_fuchsia_net_neighbor::EntryIteratorMarker>();
    view.open_entry_iterator(
        server_end,
        &fidl_fuchsia_net_neighbor::EntryIteratorOptions::default(),
    )
    .expect("failed to open EntryIterator");

    assert_eq!(
        iter.get_next().await.expect("failed to fetch idle item"),
        vec![fidl_fuchsia_net_neighbor::EntryIteratorItem::Idle(
            fidl_fuchsia_net_neighbor::IdleEvent {}
        )]
    );

    let get1 = iter.get_next();
    let get2 = iter.get_next();

    let (r1, r2) = futures::future::join(get1, get2).await;
    assert_matches::assert_matches!(r1, Err(e) if e.is_closed());
    assert_matches::assert_matches!(r2, Err(e) if e.is_closed());
}

#[netstack_test]
async fn channel_is_closed_if_not_polled(name: &str) {
    let sandbox = TestSandbox::new().expect("failed to create sandbox");
    let network = sandbox.create_network("net").await.expect("failed to create network");

    let alice = create_realm(
        &sandbox,
        &network,
        name,
        "alice",
        fidl_fuchsia_net::Subnet { addr: ALICE_IP, prefix_len: SUBNET_PREFIX },
        ALICE_MAC,
    )
    .await;

    let view = alice
        .realm
        .connect_to_protocol::<fidl_fuchsia_net_neighbor::ViewMarker>()
        .expect("failed to connect to fuchsia.net.neighbor/View");
    let (iter, server_end) =
        fidl::endpoints::create_proxy::<fidl_fuchsia_net_neighbor::EntryIteratorMarker>();
    view.open_entry_iterator(
        server_end,
        &fidl_fuchsia_net_neighbor::EntryIteratorOptions::default(),
    )
    .expect("failed to open EntryIterator");
    // Poll at least once for the idle event to ensure that EntryIterator is
    // actually implemented, otherwise this test passes against a netstack
    // that doesn't expose the capability of implementing View at all.
    assert_eq!(
        iter.get_next().await.expect("get_next"),
        vec![fidl_fuchsia_net_neighbor::EntryIteratorItem::Idle(
            fidl_fuchsia_net_neighbor::IdleEvent {}
        )]
    );

    let controller = alice
        .realm
        .connect_to_protocol::<fidl_fuchsia_net_neighbor::ControllerMarker>()
        .expect("failed to connect to Controller");

    let create_and_remove_entry = || async {
        controller
            .add_entry(alice.ep.id(), &BOB_IP, &BOB_MAC)
            .await
            .expect("add_entry FIDL error")
            .expect("add_entry failed");
        controller
            .remove_entry(alice.ep.id(), &BOB_IP)
            .await
            .expect("remove_entry FIDL error")
            .expect("remove_entry failed");
    };

    // Ensure some entries exist before polling.
    create_and_remove_entry().await;
    let create_entries = async {
        loop {
            create_and_remove_entry().await
        }
    }
    .fuse();
    let mut create_entries = pin!(create_entries);

    let mut event_stream = iter.take_event_stream();
    futures::select! {
        o = event_stream.next() => assert_matches::assert_matches!(o, None),
        _ = create_entries => unreachable!(),
    }
}

#[netstack_test]
async fn remove_device_clears_neighbors(name: &str) {
    let sandbox = TestSandbox::new().expect("failed to create sandbox");
    let network = sandbox.create_network("net").await.expect("failed to create network");

    let NeighborRealm { realm, ep, ipv6: _, loopback_id: _ } = create_realm(
        &sandbox,
        &network,
        name,
        "alice",
        fidl_fuchsia_net::Subnet { addr: ALICE_IP, prefix_len: SUBNET_PREFIX },
        ALICE_MAC,
    )
    .await;

    let mut iter =
        get_entry_iterator(&realm, fidl_fuchsia_net_neighbor::EntryIteratorOptions::default());
    assert_entries(&mut iter, [ItemMatch::Idle]).await;

    let controller = realm
        .connect_to_protocol::<fidl_fuchsia_net_neighbor::ControllerMarker>()
        .expect("failed to connect to Controller");

    controller
        .add_entry(ep.id(), &BOB_IP, &BOB_MAC)
        .await
        .expect("add_entry FIDL error")
        .expect("add_entry failed");

    let interface = ep.id();

    assert_entries(
        &mut iter,
        [ItemMatch::Added(EntryMatch {
            interface,
            neighbor: BOB_IP,
            state: fidl_fuchsia_net_neighbor::EntryState::Static,
            mac: Some(BOB_MAC),
        })],
    )
    .await;

    std::mem::drop(ep);

    assert_entries(
        &mut iter,
        [ItemMatch::Removed(EntryMatch {
            interface,
            neighbor: BOB_IP,
            state: fidl_fuchsia_net_neighbor::EntryState::Static,
            mac: Some(BOB_MAC),
        })],
    )
    .await;
}

#[netstack_test]
// Verify that the `fuchsia.net.neighbor/EntryIterator` connection "survives"
// a large burst of neighbor events. In particular, when a neighbor with many
// addresses disconnects.
async fn neighbor_with_many_addresses_disconnects(name: &str) {
    // Use the three /24 `TEST-NET` IPv4 subnets, for a total of 768 addresses.
    const NEIGHBOR_SUBNETS: [fidl_fuchsia_net::Ipv4AddressWithPrefix; 3] = [
        fidl_ip_v4_with_prefix!("192.0.2.0/24"),
        fidl_ip_v4_with_prefix!("198.51.100.0/24"),
        fidl_ip_v4_with_prefix!("203.0.113.0/24"),
    ];

    let sandbox = TestSandbox::new().expect("failed to create sandbox");
    let network = sandbox.create_network("net").await.expect("failed to create network");

    let NeighborRealm { realm, ep, ipv6: _, loopback_id: _ } = create_realm(
        &sandbox,
        &network,
        name,
        "alice",
        fidl_fuchsia_net::Subnet { addr: ALICE_IP, prefix_len: SUBNET_PREFIX },
        ALICE_MAC,
    )
    .await;
    let iface_id = ep.id();

    let mut iter =
        get_entry_iterator(&realm, fidl_fuchsia_net_neighbor::EntryIteratorOptions::default());
    assert_entries(&mut iter, [ItemMatch::Idle]).await;

    let controller = realm
        .connect_to_protocol::<fidl_fuchsia_net_neighbor::ControllerMarker>()
        .expect("failed to connect to Controller");

    let mut all_addrs = HashSet::new();
    for subnet in NEIGHBOR_SUBNETS {
        let num_addrs = 1u32 << 32u8.checked_sub(subnet.prefix_len).unwrap();
        all_addrs.reserve(num_addrs.try_into().unwrap());
        for host_bits in 0..num_addrs {
            // Compute the addr at offset `host_bits` within the subnet.
            let network_u32 = u32::from_be_bytes(subnet.addr.addr);
            let addr_u32 = u32::checked_add(network_u32, host_bits).unwrap();
            let addr = fidl_fuchsia_net::IpAddress::Ipv4(fidl_fuchsia_net::Ipv4Address {
                addr: addr_u32.to_be_bytes(),
            });
            assert!(all_addrs.insert(addr.clone()));

            // Add the addr as a static neighbor associated with BOB's MAC.
            controller
                .add_entry(ep.id(), &addr, &BOB_MAC)
                .await
                .expect("add_entry FIDL error")
                .expect("add_entry failed");
            assert_entries(
                &mut iter,
                [ItemMatch::Added(EntryMatch {
                    interface: iface_id,
                    neighbor: addr,
                    state: fidl_fuchsia_net_neighbor::EntryState::Static,
                    mac: Some(BOB_MAC),
                })],
            )
            .await;
        }
    }

    // Remove the interface, which should cause all neighbor entries to be
    // simultaneously removed.
    ep.control().remove().await.expect("request sent").expect("remove initiated successfully");
    assert_eq!(
        ep.wait_removal().await.expect("wait for removal"),
        fidl_fuchsia_net_interfaces_admin::InterfaceRemovedReason::User
    );

    let del_addrs = iter
        .map(|item| match item {
            fidl_fuchsia_net_neighbor::EntryIteratorItem::Removed(entry) => {
                let fidl_fuchsia_net_neighbor::Entry {
                    interface,
                    state,
                    mac,
                    neighbor,
                    updated_at: _,
                    ..
                } = entry;
                assert_eq!(interface, Some(iface_id));
                assert_eq!(state, Some(fidl_fuchsia_net_neighbor::EntryState::Static));
                assert_eq!(mac, Some(BOB_MAC));
                assert_matches!(neighbor, Some(addr) => addr)
            }
            e @ fidl_fuchsia_net_neighbor::EntryIteratorItem::Existing(_)
            | e @ fidl_fuchsia_net_neighbor::EntryIteratorItem::Added(_)
            | e @ fidl_fuchsia_net_neighbor::EntryIteratorItem::Changed(_)
            | e @ fidl_fuchsia_net_neighbor::EntryIteratorItem::Idle(_) => {
                panic!("unexpected event in neighbor stream: {:?}", e)
            }
        })
        .take(all_addrs.len())
        .collect::<HashSet<_>>()
        .await;
    assert_eq!(del_addrs, all_addrs);
}

#[netstack_test]
#[variant(I, Ip)]
async fn neigh_probe_dynamic_entry<I: Ip>(name: &str) {
    let sandbox = TestSandbox::new().expect("failed to create sandbox");
    let network = sandbox.create_network("net").await.expect("failed to create network");

    // Attach a fake endpoint that will capture all the ARP and NDP neighbor
    // solicitations.
    let fake_ep = network.create_fake_endpoint().expect("failed to create fake endpoint");
    let mut solicit_stream = netstack_testing_common::nud::create_metadata_stream(&fake_ep);

    let (alice, bob) = create_neighbor_realms(&sandbox, &network, name).await;
    // Apply the NUD flake workaround, since we expect all neighbor resolution
    // to succeed in this test case.
    alice.ep.apply_nud_flake_workaround().await.expect("nud flake workaround");
    bob.ep.apply_nud_flake_workaround().await.expect("nud flake workaround");

    let alice_controller = alice
        .realm
        .connect_to_protocol::<fidl_fuchsia_net_neighbor::ControllerMarker>()
        .expect("failed to connect to Controller");

    let mut iter = get_entry_iterator(
        &alice.realm,
        fidl_fuchsia_net_neighbor::EntryIteratorOptions::default(),
    );
    assert_entries(&mut iter, [ItemMatch::Idle]).await;

    let (alice_ip, bob_ip) = match I::VERSION {
        IpVersion::V4 => (ALICE_IP, BOB_IP),
        IpVersion::V6 => (alice.ipv6, bob.ipv6),
    };

    // Exchange some datagrams to add a STALE entry for alice.
    exchange_dgram(&alice, alice_ip, &bob, bob_ip).await;
    assert_entries(&mut iter, incomplete_then_reachable(alice.ep.id(), bob_ip, BOB_MAC)).await;

    // Consume the probe and UDP datagram from the stream.
    assert_eq!(
        next_solicitation_resolution(&mut solicit_stream, bob_ip).await,
        HashSet::from_iter(std::iter::once(bob_ip))
    );

    // Trigger probe via FIDL.
    alice_controller
        .probe_entry(alice.ep.id(), &bob_ip)
        .await
        .expect("probe_entry FIDL error")
        .expect("probe_entry failed");

    // Neighbor should transition to PROBE.
    assert_entries(
        &mut iter,
        [ItemMatch::Changed(EntryMatch {
            interface: alice.ep.id(),
            neighbor: bob_ip,
            state: fidl_fuchsia_net_neighbor::EntryState::Probe,
            mac: Some(BOB_MAC),
        })],
    )
    .await;

    // A unicast probe should have been sent.
    assert_eq!(next_solicitation(&mut solicit_stream).await, bob_ip);
}

#[netstack_test]
#[variant(I, Ip)]
async fn neigh_probe_static_entry<I: Ip>(name: &str) {
    let sandbox = TestSandbox::new().expect("failed to create sandbox");
    let network = sandbox.create_network("net").await.expect("failed to create network");

    // Attach a fake endpoint that will capture all the ARP and NDP neighbor
    // solicitations.
    let fake_ep = network.create_fake_endpoint().expect("failed to create fake endpoint");
    let mut solicit_stream = netstack_testing_common::nud::create_metadata_stream(&fake_ep);

    let (alice, bob) = create_neighbor_realms(&sandbox, &network, name).await;

    let alice_controller = alice
        .realm
        .connect_to_protocol::<fidl_fuchsia_net_neighbor::ControllerMarker>()
        .expect("failed to connect to Controller");

    let mut iter = get_entry_iterator(
        &alice.realm,
        fidl_fuchsia_net_neighbor::EntryIteratorOptions::default(),
    );
    assert_entries(&mut iter, [ItemMatch::Idle]).await;

    let bob_ip = match I::VERSION {
        IpVersion::V4 => BOB_IP,
        IpVersion::V6 => bob.ipv6,
    };

    // Add a STATIC entry for alice.
    alice_controller
        .add_entry(alice.ep.id(), &bob_ip, &BOB_MAC)
        .await
        .expect("add_entry FIDL error")
        .expect("add_entry failed");
    assert_entries(
        &mut iter,
        [ItemMatch::Added(EntryMatch {
            interface: alice.ep.id(),
            neighbor: bob_ip,
            state: fidl_fuchsia_net_neighbor::EntryState::Static,
            mac: Some(BOB_MAC),
        })],
    )
    .await;

    // Trigger probe via FIDL.
    alice_controller
        .probe_entry(alice.ep.id(), &bob_ip)
        .await
        .expect("probe_entry FIDL error")
        .expect("probe_entry failed");

    // Neighbor should transition to PROBE.
    assert_entries(
        &mut iter,
        [ItemMatch::Changed(EntryMatch {
            interface: alice.ep.id(),
            neighbor: bob_ip,
            state: fidl_fuchsia_net_neighbor::EntryState::Probe,
            mac: Some(BOB_MAC),
        })],
    )
    .await;

    // A unicast probe should have been sent.
    assert_eq!(next_solicitation(&mut solicit_stream).await, bob_ip);
}
