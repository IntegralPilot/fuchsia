// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

use std::collections::{HashMap, HashSet};
use std::mem::size_of;
use std::pin::pin;

use assert_matches::assert_matches;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_ext as fnet_ext;
use fidl_fuchsia_net_interfaces as fnet_interfaces;
use fidl_fuchsia_net_interfaces_admin as fnet_interfaces_admin;
use fidl_fuchsia_net_interfaces_ext::{
    self as fnet_interfaces_ext, PositiveMonotonicInstant, PreferredLifetimeInfo,
};
use fidl_fuchsia_net_routes as fnet_routes;
use fidl_fuchsia_net_routes_admin as fnet_routes_admin;
use fidl_fuchsia_net_routes_ext as fnet_routes_ext;
use fidl_fuchsia_posix_socket as fposix_socket;
use fuchsia_async::{DurationExt as _, TimeoutExt as _};

use anyhow::Context as _;
use futures::{
    Future, FutureExt as _, StreamExt as _, TryFutureExt as _, TryStreamExt as _, future,
};
use net_declare::{net_ip_v6, net_subnet_v6};
use net_types::ethernet::Mac;
use net_types::ip::{self as net_types_ip, Ip, Ipv6};
use net_types::{
    LinkLocalAddress as _, MulticastAddr, MulticastAddress as _, SpecifiedAddr,
    SpecifiedAddress as _, Witness as _,
};
use netemul::InterfaceConfig;
use netstack_testing_common::constants::{eth as eth_consts, ipv6 as ipv6_consts};
use netstack_testing_common::ndp::{
    self, DadState, assert_dad_failed, assert_dad_success, expect_dad_neighbor_solicitation,
    fail_dad_with_na, fail_dad_with_ns, send_ra, send_ra_with_router_lifetime,
    wait_for_router_solicitation,
};
use netstack_testing_common::realms::{
    KnownServiceProvider, Netstack, Netstack3, NetstackVersion, TestSandboxExt as _, constants,
};
use netstack_testing_common::{
    ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT, ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT, interfaces,
    setup_network, setup_network_with,
};
use netstack_testing_macros::netstack_test;
use packet::ParsablePacket as _;
use packet_formats::ethernet::{EtherType, EthernetFrame, EthernetFrameLengthCheck};
use packet_formats::icmp::mld::{MldPacket, Mldv2MulticastRecordType};
use packet_formats::icmp::ndp::options::{
    NdpOption, NdpOptionBuilder, PrefixInformation, RouteInformation,
};
use packet_formats::icmp::ndp::{
    NeighborSolicitation, RoutePreference, RouterAdvertisement, RouterSolicitation,
};
use packet_formats::icmp::{IcmpParseArgs, Icmpv6Packet};
use packet_formats::ip::Ipv6Proto;
use packet_formats::testutil::{parse_icmp_packet_in_ip_packet_in_ethernet_frame, parse_ip_packet};
use test_case::test_case;

/// The expected number of Router Solicitations sent by the netstack when an
/// interface is brought up as a host.
const EXPECTED_ROUTER_SOLICIATIONS: u8 = 3;

/// The expected number of Neighbor Solicitations sent by the netstack when
/// performing Duplicate Address Detection.
const EXPECTED_DUP_ADDR_DETECT_TRANSMITS: u8 = 1;

/// The expected interval between sending Neighbor Solicitation messages when
/// performing Duplicate Address Detection.
const EXPECTED_DAD_RETRANSMIT_TIMER: zx::MonotonicDuration = zx::MonotonicDuration::from_seconds(1);

/// The expected interval between sending Router Solicitation messages when
/// soliciting IPv6 routers.
const EXPECTED_ROUTER_SOLICITATION_INTERVAL: zx::MonotonicDuration =
    zx::MonotonicDuration::from_seconds(4);

#[derive(Debug, PartialEq)]
struct StableAddrs {
    link_local: net_types::ip::Ipv6Addr,
    global: net_types::ip::Ipv6Addr,
}

async fn install_and_get_stable_ipv6_addrs_for_endpoint<N: Netstack>(
    realm: &netemul::TestRealm<'_>,
    endpoint: &netemul::TestEndpoint<'_>,
    fake_endpoint: &netemul::TestFakeEndpoint<'_>,
    name: &str,
) -> StableAddrs {
    let (id, control, _device_control) = endpoint
        .add_to_stack(
            realm,
            netemul::InterfaceConfig {
                name: Some(name.into()),
                // Disable temporary addresses so we only observe stable addresses.
                temporary_addresses: Some(false),
                ..Default::default()
            },
        )
        .await
        .expect("installing interface");
    let did_enable = control.enable().await.expect("calling enable").expect("enable failed");
    assert!(did_enable);

    // Wait for a Router Solicitation.
    wait_for_router_solicitation(&fake_endpoint).await;

    // Send a Router Advertisement with information for a SLAAC prefix.
    let options = [NdpOptionBuilder::PrefixInformation(PrefixInformation::new(
        ipv6_consts::GLOBAL_PREFIX.prefix(),  /* prefix_length */
        false,                                /* on_link_flag */
        true,                                 /* autonomous_address_configuration_flag */
        99999,                                /* valid_lifetime */
        99999,                                /* preferred_lifetime */
        ipv6_consts::GLOBAL_PREFIX.network(), /* prefix */
    ))];
    send_ra_with_router_lifetime(&fake_endpoint, 0, &options, ipv6_consts::LINK_LOCAL_ADDR)
        .await
        .expect("failed to send router advertisement");

    let interface_state = realm
        .connect_to_protocol::<fnet_interfaces::StateMarker>()
        .expect("failed to connect to fuchsia.net.interfaces/State service");
    let mut state = fnet_interfaces_ext::InterfaceState::<(), _>::Unknown(id.into());
    let ipv6_addresses = fnet_interfaces_ext::wait_interface_with_id(
        fnet_interfaces_ext::event_stream_from_state::<fnet_interfaces_ext::DefaultInterest>(
            &interface_state,
            Default::default(),
        )
        .expect("creating interface event stream"),
        &mut state,
        |iface| {
            let (link_local, global) = iface.properties.addresses.iter().fold(
                (None, None),
                |(mut link_local, mut global),
                 fnet_interfaces_ext::Address { addr, assignment_state, .. }| {
                    assert_eq!(
                        *assignment_state,
                        fnet_interfaces::AddressAssignmentState::Assigned
                    );
                    let addr = match addr.addr {
                        fnet::IpAddress::Ipv6(fnet::Ipv6Address { addr }) => addr,
                        fnet::IpAddress::Ipv4(fnet::Ipv4Address { .. }) => {
                            return (link_local, global);
                        }
                    };
                    let v6_addr = net_types::ip::Ipv6Addr::from_bytes(addr);
                    if v6_addr.is_unicast_link_local() {
                        link_local = Some(v6_addr);
                    } else if ipv6_consts::GLOBAL_PREFIX.contains(&v6_addr) {
                        global = Some(v6_addr);
                    }
                    (link_local, global)
                },
            );
            match (link_local, global) {
                (Some(link_local), Some(global)) => Some(StableAddrs { link_local, global }),
                _ => None,
            }
        },
    )
    .await
    .expect("failed to observe address generation");

    ipv6_addresses
}

/// Test that across netstack runs, a device will initially be assigned the same
/// stable IPv6 addresses (both link-local and global).
#[netstack_test]
#[variant(N, Netstack)]
async fn consistent_initial_stable_ipv6_addrs<N: Netstack>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let network = sandbox.create_network(name).await.expect("create network");
    let realm = sandbox
        .create_realm(
            name,
            &[
                // This test exercises stash persistence. Netstack-debug, which
                // is the default used by test helpers, does not use
                // persistence.
                KnownServiceProvider::Netstack(match N::VERSION {
                    NetstackVersion::Netstack2 { tracing: false, fast_udp: false } => NetstackVersion::ProdNetstack2,
                    NetstackVersion::Netstack3 => NetstackVersion::Netstack3,
                    v @ (NetstackVersion::Netstack2 { tracing: _, fast_udp: _ }
                    | NetstackVersion::ProdNetstack2
                    | NetstackVersion::ProdNetstack3
                    ) => {
                        panic!("netstack_test should only ever be parameterized with Netstack2 or Netstack3: got {:?}", v)
                    }
                }),
                KnownServiceProvider::SecureStash,
            ],
        )
        .expect("create realm");
    let fake_ep = network.create_fake_endpoint().expect("create fake endpoint");
    let endpoint = network.create_endpoint(name).await.expect("create endpoint");
    endpoint.set_link_up(true).await.expect("set link up");

    // Make sure netstack uses the same addresses across runs for a device.
    let first_run_addrs =
        install_and_get_stable_ipv6_addrs_for_endpoint::<N>(&realm, &endpoint, &fake_ep, name)
            .await;

    // Stop the netstack.
    realm.stop_child_component(constants::netstack::COMPONENT_NAME).await.expect("stop netstack");

    let second_run_addrs =
        install_and_get_stable_ipv6_addrs_for_endpoint::<N>(&realm, &endpoint, &fake_ep, name)
            .await;
    assert_eq!(first_run_addrs, second_run_addrs);
}

/// Enables IPv6 forwarding configuration.
async fn enable_ipv6_forwarding(iface: &netemul::TestInterface<'_>) {
    let config_with_ipv6_forwarding_set = |forwarding| fnet_interfaces_admin::Configuration {
        ipv6: Some(fnet_interfaces_admin::Ipv6Configuration {
            unicast_forwarding: Some(forwarding),
            ..Default::default()
        }),
        ..Default::default()
    };

    let configuration = iface
        .control()
        .set_configuration(&config_with_ipv6_forwarding_set(true))
        .await
        .expect("set_configuration FIDL error")
        .expect("error setting configuration");

    assert_eq!(configuration, config_with_ipv6_forwarding_set(false))
}

/// Tests that `EXPECTED_ROUTER_SOLICIATIONS` Router Solicitation messages are transmitted
/// when the interface is brought up.
#[netstack_test]
#[variant(N, Netstack)]
#[test_case("host", false ; "host")]
#[test_case("router", true ; "router")]
async fn sends_router_solicitations<N: Netstack>(
    test_name: &str,
    sub_test_name: &str,
    forwarding: bool,
) {
    let name = format!("{}_{}", test_name, sub_test_name);
    let name = name.as_str();

    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let (_network, _realm, iface, fake_ep) =
        setup_network::<N>(&sandbox, name, None).await.expect("error setting up network");

    if forwarding {
        enable_ipv6_forwarding(&iface).await;
    }

    // Make sure exactly `EXPECTED_ROUTER_SOLICIATIONS` RS messages are transmitted
    // by the netstack.
    let mut observed_rs = 0;
    loop {
        // When we have already observed the expected number of RS messages, do a
        // negative check to make sure that we don't send anymore.
        let extra_timeout = if observed_rs == EXPECTED_ROUTER_SOLICIATIONS {
            ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT
        } else {
            ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT
        };

        let ret = fake_ep
            .frame_stream()
            .try_filter_map(|(data, dropped)| {
                assert_eq!(dropped, 0);
                let mut observed_slls = Vec::new();
                future::ok(
                    parse_icmp_packet_in_ip_packet_in_ethernet_frame::<
                        net_types_ip::Ipv6,
                        _,
                        RouterSolicitation,
                        _,
                    >(&data, EthernetFrameLengthCheck::NoCheck, |p| {
                        for option in p.body().iter() {
                            if let NdpOption::SourceLinkLayerAddress(a) = option {
                                let mut mac_bytes = [0; 6];
                                mac_bytes.copy_from_slice(&a[..size_of::<Mac>()]);
                                observed_slls.push(Mac::new(mac_bytes));
                            } else {
                                // We should only ever have an NDP Source Link-Layer Address
                                // option in a RS.
                                panic!("unexpected option in RS = {:?}", option);
                            }
                        }
                    })
                    .map_or(
                        None,
                        |(_src_mac, dst_mac, src_ip, dst_ip, ttl, _message, _code)| {
                            Some((dst_mac, src_ip, dst_ip, ttl, observed_slls))
                        },
                    ),
                )
            })
            .try_next()
            .map(|r| r.context("error getting OnData event"))
            .on_timeout((EXPECTED_ROUTER_SOLICITATION_INTERVAL + extra_timeout).after_now(), || {
                // If we already observed `EXPECTED_ROUTER_SOLICIATIONS` RS, then we shouldn't
                // have gotten any more; the timeout is expected.
                if observed_rs == EXPECTED_ROUTER_SOLICIATIONS {
                    return Ok(None);
                }

                return Err(anyhow::anyhow!("timed out waiting for the {}-th RS", observed_rs));
            })
            .await
            .unwrap();

        let (dst_mac, src_ip, dst_ip, ttl, observed_slls) = match ret {
            Some((dst_mac, src_ip, dst_ip, ttl, observed_slls)) => {
                (dst_mac, src_ip, dst_ip, ttl, observed_slls)
            }
            None => break,
        };

        assert_eq!(
            dst_mac,
            Mac::from(&net_types_ip::Ipv6::ALL_ROUTERS_LINK_LOCAL_MULTICAST_ADDRESS)
        );

        // DAD should have resolved for the link local IPv6 address that is assigned to
        // the interface when it is first brought up. When a link local address is
        // assigned to the interface, it should be used for transmitted RS messages.
        if observed_rs > 0 {
            assert!(src_ip.is_specified())
        }

        assert_eq!(dst_ip, net_types_ip::Ipv6::ALL_ROUTERS_LINK_LOCAL_MULTICAST_ADDRESS.get());

        assert_eq!(ttl, ndp::MESSAGE_TTL);

        // The Router Solicitation should only ever have at max 1 source
        // link-layer option.
        assert!(observed_slls.len() <= 1);
        let observed_sll = observed_slls.into_iter().nth(0);
        if src_ip.is_specified() {
            if observed_sll.is_none() {
                panic!(
                    "expected source-link-layer address option if RS has a specified source IP address"
                );
            }
        } else if observed_sll.is_some() {
            panic!(
                "unexpected source-link-layer address option for RS with unspecified source IP address"
            );
        }

        observed_rs += 1;
    }

    assert_eq!(observed_rs, EXPECTED_ROUTER_SOLICIATIONS);
}

/// Tests that both stable and temporary SLAAC addresses are generated for a SLAAC prefix.
#[netstack_test]
#[variant(N, Netstack)]
#[test_case("host", false ; "host")]
#[test_case("router", true ; "router")]
async fn slaac_with_privacy_extensions<N: Netstack>(
    test_name: &str,
    sub_test_name: &str,
    forwarding: bool,
) {
    let name = format!("{}_{}", test_name, sub_test_name);
    let name = name.as_str();
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let (_network, realm, iface, fake_ep) =
        setup_network::<N>(&sandbox, name, None).await.expect("error setting up network");

    if forwarding {
        enable_ipv6_forwarding(&iface).await;
    }

    // Wait for a Router Solicitation.
    //
    // The first RS should be sent immediately.
    wait_for_router_solicitation(&fake_ep).await;

    // Send a Router Advertisement with information for a SLAAC prefix.
    let options = [NdpOptionBuilder::PrefixInformation(PrefixInformation::new(
        ipv6_consts::GLOBAL_PREFIX.prefix(),  /* prefix_length */
        false,                                /* on_link_flag */
        true,                                 /* autonomous_address_configuration_flag */
        99999,                                /* valid_lifetime */
        99999,                                /* preferred_lifetime */
        ipv6_consts::GLOBAL_PREFIX.network(), /* prefix */
    ))];
    send_ra_with_router_lifetime(&fake_ep, 0, &options, ipv6_consts::LINK_LOCAL_ADDR)
        .await
        .expect("failed to send router advertisement");

    // Wait for the SLAAC addresses to be generated.
    //
    // We expect two addresses for the SLAAC prefixes to be assigned to the NIC as the
    // netstack should generate both a stable and temporary SLAAC address.
    let expected_addrs = 2;
    fnet_interfaces_ext::wait_interface_with_id(
        realm.get_interface_event_stream().expect("error getting interface state event stream"),
        &mut fnet_interfaces_ext::InterfaceState::<(), _>::Unknown(iface.id()),
        |iface| {
            (iface
                .properties
                .addresses
                .iter()
                .filter_map(
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
                        match addr {
                            fnet::IpAddress::Ipv4(fnet::Ipv4Address { .. }) => None,
                            fnet::IpAddress::Ipv6(fnet::Ipv6Address { addr }) => {
                                ipv6_consts::GLOBAL_PREFIX
                                    .contains(&net_types_ip::Ipv6Addr::from_bytes(addr))
                                    .then_some(())
                            }
                        }
                    },
                )
                .count()
                == expected_addrs as usize)
                .then_some(())
        },
    )
    .map_err(anyhow::Error::from)
    .on_timeout(
        (EXPECTED_DAD_RETRANSMIT_TIMER * EXPECTED_DUP_ADDR_DETECT_TRANSMITS * expected_addrs
            + ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT)
            .after_now(),
        || Err(anyhow::anyhow!("timed out")),
    )
    .await
    .expect("failed to wait for SLAAC addresses to be generated")
}

/// Returns the assignment state for an address if it appears in `addrs`.
fn address_assignment_state(
    (expected_addr, expected_prefix_len): (net_types_ip::Ipv6Addr, u8),
    addrs: impl IntoIterator<Item = fnet_interfaces_ext::Address<fnet_interfaces_ext::AllInterest>>,
) -> Option<fnet_interfaces::AddressAssignmentState> {
    addrs
        .into_iter()
        .filter_map(
            |fnet_interfaces_ext::Address {
                 addr: fnet::Subnet { addr, prefix_len },
                 valid_until: _,
                 preferred_lifetime_info: _,
                 assignment_state,
             }| {
                if prefix_len == expected_prefix_len
                    && match addr {
                        fnet::IpAddress::Ipv4(_) => false,
                        fnet::IpAddress::Ipv6(fnet::Ipv6Address { addr }) => {
                            addr == expected_addr.ipv6_bytes()
                        }
                    }
                {
                    Some(assignment_state)
                } else {
                    None
                }
            },
        )
        .next()
}

/// Adds `ipv6_consts::LINK_LOCAL_ADDR` to the interface and makes sure a Neighbor Solicitation
/// message is transmitted by the netstack for DAD.
///
/// Calls `dad_fn` after the DAD message is observed so callers can simulate a remote
/// node that has some interest in the same address, passing in the fake endpoint and any
/// neighbor solicitation message observed so far on the endpoint.
async fn add_address_for_dad<
    'a,
    'b: 'a,
    R: 'b + Future<Output = ()>,
    FN: FnOnce(&'b netemul::TestFakeEndpoint<'a>, Option<Vec<u8>>) -> R,
>(
    iface: &'b netemul::TestInterface<'a>,
    fake_ep: &'b netemul::TestFakeEndpoint<'a>,
    control: &'b fnet_interfaces_ext::admin::Control,
    interface_up: bool,
    dad_fn: FN,
) -> impl futures::stream::Stream<Item = DadState> {
    let (address_state_provider, server) =
        fidl::endpoints::create_proxy::<fnet_interfaces_admin::AddressStateProviderMarker>();
    // Create the state stream before adding the address to observe all events.
    let state_stream = fnet_interfaces_ext::admin::assignment_state_stream(address_state_provider);

    // Note that DAD completes successfully after 1 second has elapsed if it
    // has not received a response to it's neighbor solicitation. This
    // introduces some inherent flakiness, as certain aspects of this test
    // (limited to this scope) MUST execute within this one second window
    // for the test to pass.
    let (get_addrs_fut, get_addrs_poll) = {
        control
            .add_address(
                &fnet::Subnet {
                    addr: fnet::IpAddress::Ipv6(fnet::Ipv6Address {
                        addr: ipv6_consts::LINK_LOCAL_ADDR.ipv6_bytes(),
                    }),
                    prefix_len: ipv6_consts::LINK_LOCAL_SUBNET_PREFIX,
                },
                &fnet_interfaces_admin::AddressParameters::default(),
                server,
            )
            .expect("Control.AddAddress FIDL error");
        // `Box::pin` rather than `pin_mut!` allows `get_addr_fut` to be
        // moved out of this scope.
        let mut get_addrs_fut = Box::pin(iface.get_addrs(Default::default()));
        let get_addrs_poll = futures::poll!(&mut get_addrs_fut);
        let ns_message =
            if interface_up { Some(expect_dad_neighbor_solicitation(fake_ep).await) } else { None };
        dad_fn(fake_ep, ns_message).await;
        (get_addrs_fut, get_addrs_poll)
    }; // This marks the end of the time-sensitive operations.

    let addrs = match get_addrs_poll {
        std::task::Poll::Ready(addrs) => addrs,
        std::task::Poll::Pending => get_addrs_fut.await,
    };

    // Ensure that fuchsia.net.interfaces/Watcher doesn't erroneously report
    // the address as added before DAD completes successfully or otherwise.
    assert_eq!(
        address_assignment_state(
            (ipv6_consts::LINK_LOCAL_ADDR, ipv6_consts::LINK_LOCAL_SUBNET_PREFIX),
            addrs.expect("failed to get addresses"),
        ),
        None,
        "added IPv6 LL address already present even though it is tentative"
    );

    state_stream
}

/// Tests that if the netstack attempts to assign an address to an interface,
/// and a remote node is already assigned the address or attempts to assign the
/// address at the same time, DAD fails on the local interface.
#[netstack_test]
#[variant(N, Netstack)]
#[test_case(true ; "ns")]
#[test_case(false ; "na")]
async fn dad_fails_due_to_remote_address_conflict<N: Netstack>(name: &str, with_ns: bool) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let (_network, _realm, iface, fake_ep) =
        setup_network::<N>(&sandbox, name, None).await.expect("error setting up network");

    let control = iface.control();

    // The next steps want to observe a DAD failure. To prevent flakes in CQ due
    // to DAD retransmission time, set the number of DAD transmits very high.
    let _ = iface.set_ipv6_dad_transmits(u16::MAX).await.expect("set dad transmits");

    let state_stream = add_address_for_dad(&iface, &fake_ep, &control, true, |ep, _| async move {
        if with_ns {
            fail_dad_with_ns(ep).await;
        } else {
            fail_dad_with_na(ep).await;
        }
    })
    .await;
    assert_dad_failed(state_stream).await;
}

/// Tests that duplicate address detection succeeds when no remote node has any
/// interest in the address.
#[netstack_test]
#[variant(N, Netstack)]
async fn dad_succeeds_if_no_remote_address_conflict<N: Netstack>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let (_network, _realm, iface, fake_ep) =
        setup_network::<N>(&sandbox, name, None).await.expect("error setting up network");

    let control = iface.control();

    // Add the address, and make sure it gets assigned.
    let mut state_stream =
        add_address_for_dad(&iface, &fake_ep, &control, true, |_, _| futures::future::ready(()))
            .await;

    assert_dad_success(&mut state_stream).await;

    let addresses = iface.get_addrs(Default::default()).await.expect("addrs");
    assert_eq!(
        address_assignment_state(
            (ipv6_consts::LINK_LOCAL_ADDR, ipv6_consts::LINK_LOCAL_SUBNET_PREFIX),
            addresses,
        ),
        Some(fnet_interfaces::AddressAssignmentState::Assigned),
        "added IPv6 LL address not in Assigned state",
    );
}

/// Tests that duplicate address detection repeats and succeeds when an
/// interface with an assigned address is disabled and re-enabled.
#[netstack_test]
#[variant(N, Netstack)]
async fn dad_link_state_change<N: Netstack>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let (_network, _realm, iface, fake_ep) =
        setup_network::<N>(&sandbox, name, None).await.expect("error setting up network");

    let control = iface.control();

    let mut state_stream =
        add_address_for_dad(&iface, &fake_ep, &control, true, |_, _| futures::future::ready(()))
            .await;
    assert_dad_success(&mut state_stream).await;

    assert!(control.disable().await.expect("send disable").expect("disable"));

    assert_matches::assert_matches!(
        state_stream.by_ref().next().await,
        Some(Ok(fnet_interfaces::AddressAssignmentState::Unavailable))
    );

    // Re-enable the interface, expect DAD to repeat and have it succeed.
    assert!(control.enable().await.expect("send enable").expect("enable"));
    let _: Vec<u8> = expect_dad_neighbor_solicitation(&fake_ep).await;
    assert_dad_success(&mut state_stream).await;

    let addresses = iface.get_addrs(Default::default()).await.expect("addrs");
    assert_eq!(
        address_assignment_state(
            (ipv6_consts::LINK_LOCAL_ADDR, ipv6_consts::LINK_LOCAL_SUBNET_PREFIX),
            addresses,
        ),
        Some(fnet_interfaces::AddressAssignmentState::Assigned),
        "added IPv6 LL address not in Assigned state",
    );
}

/// Tests that if an address is added while an interface is disabled, duplicate
/// address detection runs and succeeds when the interface is re-enabled.
#[netstack_test]
#[variant(N, Netstack)]
async fn dad_address_assigned_while_interface_down<N: Netstack>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let (_network, _realm, iface, fake_ep) =
        setup_network::<N>(&sandbox, name, None).await.expect("error setting up network");

    let control = iface.control();

    assert!(control.disable().await.expect("send disable").expect("disable"));
    let mut state_stream =
        add_address_for_dad(&iface, &fake_ep, &control, false, |_, _| futures::future::ready(()))
            .await;

    assert_matches::assert_matches!(
        state_stream.by_ref().next().await,
        Some(Ok(fnet_interfaces::AddressAssignmentState::Unavailable))
    );

    // Re-enable the interface, DAD should run.
    assert!(control.enable().await.expect("send enable").expect("enable"));
    let _: Vec<u8> = expect_dad_neighbor_solicitation(&fake_ep).await;
    assert_dad_success(&mut state_stream).await;

    let addresses = iface.get_addrs(Default::default()).await.expect("addrs");
    assert_eq!(
        address_assignment_state(
            (ipv6_consts::LINK_LOCAL_ADDR, ipv6_consts::LINK_LOCAL_SUBNET_PREFIX),
            addresses,
        ),
        Some(fnet_interfaces::AddressAssignmentState::Assigned),
        "added IPv6 LL address not in Assigned state",
    );
}

/// Tests that we successfully complete duplicate address detection (and allow
/// the address to be assigned) even if our DAD probes are being echoed back
/// at us.
#[netstack_test]
#[variant(N, Netstack)]
async fn dad_assigns_when_echoed<N: Netstack>(name: &str) {
    const MAXIMUM_RETRIES: usize = 10;
    for _ in 0..MAXIMUM_RETRIES {
        let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
        let (_network, _realm, iface, fake_ep) =
            setup_network::<N>(&sandbox, name, None).await.expect("error setting up network");

        let control = iface.control();

        let (stop_echo_signal_sender, mut stop_echo_signal_receiver) =
            futures::channel::oneshot::channel();

        let mut state_stream =
            add_address_for_dad(&iface, &fake_ep, &control, true, |fake_ep, ns| async move {
                fake_ep
                    .write(&ns.expect("should have seen neighbor solicitation")[..])
                    .await
                    .expect("echoing back ns should succeed");
            })
            .await;

        let assert_dad_success_fut = async move {
            assert_dad_success(&mut state_stream).await;
            stop_echo_signal_sender.send(()).expect("failed to stop the echo task");
        };

        // 1 solicitation by default + 3 additional solicitations due to the looped-back probe.
        const WANT_NUM_PROBES: usize = 4usize;
        // This test can be flaky because the DAD can finish before the echo future gets any NS,
        // in that case we will just retry.
        const FLAKE_NUM_PROBES: usize = 1usize;

        let echo_ns_fut = async {
            // NB: we've already seen one probe from the above `add_address_for_dad` invocation.
            let mut got_num_probes = 1usize;

            let mut recv_stream = fake_ep.frame_stream().fuse();
            while got_num_probes < WANT_NUM_PROBES {
                let (data, _dropped) = futures::select_biased! {
                    r = recv_stream.next() => {
                        r
                        .expect("recv_stream shouldn't end")
                        .expect("reading from fake_ep should succeed")
                    },
                    r = stop_echo_signal_receiver => {
                        r.expect("sender should never be dropped");
                        // The following condition means the DAD succeeded before any additional
                        // probes, we break out early so that we can retry. Otherwise we already
                        // received at least one additional probe, continue the loop so that we
                        // receive at least `WANT_NUM_PROBES`.
                        if got_num_probes == 1 {
                            break;
                        } else {
                            continue;
                        }
                    }
                };

                if let Ok((_src_mac, _dst_mac, _src_ip, _dst_ip, _ttl, ns, _code)) =
                    parse_icmp_packet_in_ip_packet_in_ethernet_frame::<
                        net_types::ip::Ipv6,
                        _,
                        NeighborSolicitation,
                        _,
                    >(&data, EthernetFrameLengthCheck::NoCheck, |p| {
                        assert_matches!(
                            &p.body().iter().collect::<Vec<_>>()[..],
                            [NdpOption::Nonce(_)]
                        )
                    })
                {
                    if *ns.target_address() == ipv6_consts::LINK_LOCAL_ADDR {
                        got_num_probes += 1;
                        fake_ep.write(&data).await.expect("echoing back ns should succeed");
                    }
                }
            }

            got_num_probes
        };

        let ((), got_num_probes) = futures::join!(assert_dad_success_fut, echo_ns_fut);
        // The test has passed because we got the expected number of probes.
        if got_num_probes == WANT_NUM_PROBES {
            return;
        }
        // A flake happened, meaning the only valid number of probes the ns_echo future can receive
        // is 1, anything else is a failure.
        assert_eq!(got_num_probes, FLAKE_NUM_PROBES);
    }
    panic!("maximum trial number exceeded");
}

async fn check_route_table(
    realm: &netemul::TestRealm<'_>,
    want_routes: &[fnet_routes_ext::InstalledRoute<Ipv6>],
) {
    let ipv6_route_stream = {
        let state_v6 =
            realm.connect_to_protocol::<fnet_routes::StateV6Marker>().expect("connect to protocol");
        fnet_routes_ext::event_stream_from_state::<Ipv6>(&state_v6)
            .expect("failed to connect to watcher")
    };
    let ipv6_route_stream = pin!(ipv6_route_stream);
    let mut routes = HashSet::new();
    fnet_routes_ext::wait_for_routes(ipv6_route_stream, &mut routes, |accumulated_routes| {
        want_routes.iter().all(|route| accumulated_routes.contains(route))
    })
    .await
    .expect("error while waiting for routes to satisfy predicate");
}

/// Tests to make sure default router discovery, prefix discovery and more-specific
/// route discovery works.
#[netstack_test]
#[variant(N, Netstack)]
#[test_case("host", false ; "host")]
#[test_case("router", true ; "router")]
async fn on_and_off_link_route_discovery<N: Netstack>(
    test_name: &str,
    sub_test_name: &str,
    forwarding: bool,
) {
    pub const SUBNET_WITH_MORE_SPECIFIC_ROUTE: net_types_ip::Subnet<net_types_ip::Ipv6Addr> = unsafe {
        net_types_ip::Subnet::new_unchecked(
            net_types_ip::Ipv6Addr::new([0xa001, 0xf1f0, 0x4060, 0x0001, 0, 0, 0, 0]),
            64,
        )
    };

    let name = format!("{}_{}", test_name, sub_test_name);
    let name = name.as_str();

    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    const METRIC: u32 = 200;
    let (_network, realm, iface, fake_ep) =
        setup_network::<N>(&sandbox, name, Some(METRIC)).await.expect("failed to setup network");

    let main_route_table = realm
        .connect_to_protocol::<fnet_routes_admin::RouteTableV6Marker>()
        .expect("failed to connect to protocol");
    let main_table_id = fnet_routes_ext::TableId::new(
        main_route_table.get_table_id().await.expect("failed to get table id"),
    );

    if forwarding {
        enable_ipv6_forwarding(&iface).await;
    }

    let options = [
        NdpOptionBuilder::PrefixInformation(PrefixInformation::new(
            ipv6_consts::GLOBAL_PREFIX.prefix(),  /* prefix_length */
            true,                                 /* on_link_flag */
            false,                                /* autonomous_address_configuration_flag */
            6234,                                 /* valid_lifetime */
            0,                                    /* preferred_lifetime */
            ipv6_consts::GLOBAL_PREFIX.network(), /* prefix */
        )),
        NdpOptionBuilder::RouteInformation(RouteInformation::new(
            SUBNET_WITH_MORE_SPECIFIC_ROUTE,
            1337, /* route_lifetime_seconds */
            RoutePreference::default(),
        )),
    ];
    send_ra_with_router_lifetime(&fake_ep, 1234, &options, ipv6_consts::LINK_LOCAL_ADDR)
        .await
        .expect("failed to send router advertisement");

    let nicid = iface.id();
    let routes = &[
        // Test that a default route through the router is installed.
        fnet_routes_ext::InstalledRoute {
            route: fnet_routes_ext::Route {
                destination: net_types::ip::Subnet::new(
                    net_types::ip::Ipv6::UNSPECIFIED_ADDRESS,
                    0,
                )
                .unwrap(),
                action: fnet_routes_ext::RouteAction::Forward(fnet_routes_ext::RouteTarget {
                    outbound_interface: nicid,
                    next_hop: Some(net_types::SpecifiedAddr::new(ipv6_consts::LINK_LOCAL_ADDR))
                        .unwrap(),
                }),
                properties: fnet_routes_ext::RouteProperties {
                    specified_properties: fnet_routes_ext::SpecifiedRouteProperties {
                        metric: fnet_routes::SpecifiedMetric::InheritedFromInterface(
                            fnet_routes::Empty,
                        ),
                    },
                },
            },
            effective_properties: fnet_routes_ext::EffectiveRouteProperties { metric: METRIC },
            table_id: main_table_id,
        },
        // Test that a route to `SUBNET_WITH_MORE_SPECIFIC_ROUTE` exists
        // through the router.
        fnet_routes_ext::InstalledRoute {
            route: fnet_routes_ext::Route {
                destination: SUBNET_WITH_MORE_SPECIFIC_ROUTE,
                action: fnet_routes_ext::RouteAction::Forward(fnet_routes_ext::RouteTarget {
                    outbound_interface: nicid,
                    next_hop: Some(net_types::SpecifiedAddr::new(ipv6_consts::LINK_LOCAL_ADDR))
                        .unwrap(),
                }),
                properties: fnet_routes_ext::RouteProperties {
                    specified_properties: fnet_routes_ext::SpecifiedRouteProperties {
                        metric: fnet_routes::SpecifiedMetric::InheritedFromInterface(
                            fnet_routes::Empty,
                        ),
                    },
                },
            },
            effective_properties: fnet_routes_ext::EffectiveRouteProperties { metric: METRIC },
            table_id: main_table_id,
        },
        // Test that the prefix should be discovered after it is advertised.
        fnet_routes_ext::InstalledRoute::<Ipv6> {
            route: fnet_routes_ext::Route {
                destination: ipv6_consts::GLOBAL_PREFIX,
                action: fnet_routes_ext::RouteAction::Forward(fnet_routes_ext::RouteTarget {
                    outbound_interface: nicid,
                    next_hop: None,
                }),
                properties: fnet_routes_ext::RouteProperties {
                    specified_properties: fnet_routes_ext::SpecifiedRouteProperties {
                        metric: fnet_routes::SpecifiedMetric::InheritedFromInterface(
                            fnet_routes::Empty,
                        ),
                    },
                },
            },
            effective_properties: fnet_routes_ext::EffectiveRouteProperties { metric: METRIC },
            table_id: main_table_id,
        },
    ][..];

    check_route_table(&realm, routes)
        .on_timeout(
            fuchsia_async::MonotonicInstant::after(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT),
            || {
                panic!(
                    "timed out on waiting for a route table entry after {} seconds",
                    ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT.into_seconds()
                )
            },
        )
        .await;
}

#[netstack_test]
async fn route_discovery_no_default_route(name: &str) {
    pub const SUBNET_WITH_MORE_SPECIFIC_ROUTE: net_types_ip::Subnet<net_types_ip::Ipv6Addr> =
        net_subnet_v6!("a001:f1f0:4060:1::/64");
    pub const ROUTER_2: net_types::ip::Ipv6Addr = net_ip_v6!("fe80::2");
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    const METRIC: u32 = 200;
    let (_network, realm, iface, fake_ep) =
        setup_network::<Netstack3>(&sandbox, name, Some(METRIC))
            .await
            .expect("failed to setup network");

    let main_route_table = realm
        .connect_to_protocol::<fnet_routes_admin::RouteTableV6Marker>()
        .expect("failed to connect to protocol");
    let main_table_id = fnet_routes_ext::TableId::new(
        main_route_table.get_table_id().await.expect("failed to get table id"),
    );
    // Prepopulate the route table with a default route via ROUTER_2.
    // This route should remain once allow_default_route is disabled.
    send_ra_with_router_lifetime(&fake_ep, 1234, &[], ROUTER_2)
        .await
        .expect("failed to send router advertisement");
    let nicid = iface.id();
    let default_route_via_router_2 = fnet_routes_ext::InstalledRoute::<Ipv6> {
        route: fnet_routes_ext::Route {
            destination: net_subnet_v6!("::/0"),
            action: fnet_routes_ext::RouteAction::Forward(fnet_routes_ext::RouteTarget {
                outbound_interface: nicid,
                next_hop: Some(SpecifiedAddr::new(ROUTER_2).unwrap()),
            }),
            properties: fnet_routes_ext::RouteProperties {
                specified_properties: fnet_routes_ext::SpecifiedRouteProperties {
                    metric: fnet_routes::SpecifiedMetric::InheritedFromInterface(
                        fnet_routes::Empty,
                    ),
                },
            },
        },
        effective_properties: fnet_routes_ext::EffectiveRouteProperties { metric: METRIC },
        table_id: main_table_id,
    };
    check_route_table(&realm, &[default_route_via_router_2.clone()][..])
        .on_timeout(
            fuchsia_async::MonotonicInstant::after(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT),
            || {
                panic!(
                    "timed out on waiting for a route table entry after {} seconds",
                    ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT.into_seconds()
                )
            },
        )
        .await;

    let _ = iface
        .control()
        .set_configuration(&fnet_interfaces_admin::Configuration {
            ipv6: Some(fnet_interfaces_admin::Ipv6Configuration {
                ndp: Some(fnet_interfaces_admin::NdpConfiguration {
                    route_discovery: Some(fnet_interfaces_admin::RouteDiscoveryConfiguration {
                        allow_default_route: Some(false),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
        .expect("interface removal")
        .expect("failed to set configuration");

    let options = [
        NdpOptionBuilder::PrefixInformation(PrefixInformation::new(
            ipv6_consts::GLOBAL_PREFIX.prefix(),  /* prefix_length */
            true,                                 /* on_link_flag */
            false,                                /* autonomous_address_configuration_flag */
            6234,                                 /* valid_lifetime */
            0,                                    /* preferred_lifetime */
            ipv6_consts::GLOBAL_PREFIX.network(), /* prefix */
        )),
        NdpOptionBuilder::RouteInformation(RouteInformation::new(
            SUBNET_WITH_MORE_SPECIFIC_ROUTE,
            1337, /* route_lifetime_seconds */
            RoutePreference::default(),
        )),
    ];
    send_ra_with_router_lifetime(&fake_ep, 1234, &options, ipv6_consts::LINK_LOCAL_ADDR)
        .await
        .expect("failed to send router advertisement");

    let wanted_routes = [
        default_route_via_router_2,
        // Test that a route to `SUBNET_WITH_MORE_SPECIFIC_ROUTE` exists
        // through the router.
        fnet_routes_ext::InstalledRoute {
            route: fnet_routes_ext::Route {
                destination: SUBNET_WITH_MORE_SPECIFIC_ROUTE,
                action: fnet_routes_ext::RouteAction::Forward(fnet_routes_ext::RouteTarget {
                    outbound_interface: nicid,
                    next_hop: Some(net_types::SpecifiedAddr::new(ipv6_consts::LINK_LOCAL_ADDR))
                        .unwrap(),
                }),
                properties: fnet_routes_ext::RouteProperties {
                    specified_properties: fnet_routes_ext::SpecifiedRouteProperties {
                        metric: fnet_routes::SpecifiedMetric::InheritedFromInterface(
                            fnet_routes::Empty,
                        ),
                    },
                },
            },
            effective_properties: fnet_routes_ext::EffectiveRouteProperties { metric: METRIC },
            table_id: main_table_id,
        },
        // Test that the prefix should be discovered after it is advertised.
        fnet_routes_ext::InstalledRoute::<Ipv6> {
            route: fnet_routes_ext::Route {
                destination: ipv6_consts::GLOBAL_PREFIX,
                action: fnet_routes_ext::RouteAction::Forward(fnet_routes_ext::RouteTarget {
                    outbound_interface: nicid,
                    next_hop: None,
                }),
                properties: fnet_routes_ext::RouteProperties {
                    specified_properties: fnet_routes_ext::SpecifiedRouteProperties {
                        metric: fnet_routes::SpecifiedMetric::InheritedFromInterface(
                            fnet_routes::Empty,
                        ),
                    },
                },
            },
            effective_properties: fnet_routes_ext::EffectiveRouteProperties { metric: METRIC },
            table_id: main_table_id,
        },
    ];

    let unwanted_routes = [
        // The default route is not installed.
        fnet_routes_ext::InstalledRoute {
            route: fnet_routes_ext::Route {
                destination: net_types::ip::Subnet::new(
                    net_types::ip::Ipv6::UNSPECIFIED_ADDRESS,
                    0,
                )
                .unwrap(),
                action: fnet_routes_ext::RouteAction::Forward(fnet_routes_ext::RouteTarget {
                    outbound_interface: nicid,
                    next_hop: Some(net_types::SpecifiedAddr::new(ipv6_consts::LINK_LOCAL_ADDR))
                        .unwrap(),
                }),
                properties: fnet_routes_ext::RouteProperties {
                    specified_properties: fnet_routes_ext::SpecifiedRouteProperties {
                        metric: fnet_routes::SpecifiedMetric::InheritedFromInterface(
                            fnet_routes::Empty,
                        ),
                    },
                },
            },
            effective_properties: fnet_routes_ext::EffectiveRouteProperties { metric: METRIC },
            table_id: main_table_id,
        },
    ];

    check_route_table(&realm, &wanted_routes[..])
        .on_timeout(
            fuchsia_async::MonotonicInstant::after(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT),
            || {
                panic!(
                    "timed out on waiting for a route table entry after {} seconds",
                    ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT.into_seconds()
                )
            },
        )
        .await;

    assert_eq!(
        check_route_table(&realm, &unwanted_routes[..])
            .map(Some)
            .on_timeout(
                fuchsia_async::MonotonicInstant::after(ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT),
                || None
            )
            .await,
        None
    );

    // Make sure learned routes can still be removed by a RA with a zero router lifetime.
    send_ra_with_router_lifetime(&fake_ep, 0, &[], ROUTER_2)
        .await
        .expect("failed to send router advertisement");

    // Verify that the default route is removed.
    let ipv6_route_stream = {
        let state_v6 =
            realm.connect_to_protocol::<fnet_routes::StateV6Marker>().expect("connect to protocol");
        fnet_routes_ext::event_stream_from_state::<Ipv6>(&state_v6)
            .expect("failed to connect to watcher")
    };
    let ipv6_route_stream = pin!(ipv6_route_stream);
    let mut routes = HashSet::new();
    fnet_routes_ext::wait_for_routes(ipv6_route_stream, &mut routes, |accumulated_routes| {
        accumulated_routes.iter().all(|route| route.route.destination != net_subnet_v6!("::/0"))
    })
    .await
    .expect("wait for the routes to be removed");
}

#[netstack_test]
#[variant(N, Netstack)]
async fn route_discovery_preference_resolve<N: Netstack>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    const METRIC: u32 = 100;
    let (_network, realm, iface1, fake_ep1) =
        setup_network::<N>(&sandbox, name, Some(METRIC)).await.expect("failed to setup network");

    let network2 = sandbox.create_network(format!("{}_2", name)).await.expect("create network");
    let fake_ep2 = network2.create_fake_endpoint().expect("create fake endpoint 2");
    let iface2 = realm
        .join_network_with_if_config(
            &network2,
            "eth2",
            netemul::InterfaceConfig { metric: Some(METRIC), ..Default::default() },
        )
        .await
        .expect("failed to join network");

    pub const ROUTER_1: net_types::ip::Ipv6Addr = net_declare::net_ip_v6!("fe80::1");
    pub const ROUTER_2: net_types::ip::Ipv6Addr = net_declare::net_ip_v6!("fe80::2");
    pub const TARGET_SUBNET: net_types_ip::Subnet<net_types_ip::Ipv6Addr> =
        net_declare::net_subnet_v6!("2001:db8::/64");
    const IN_TARGET_SUBNET: net_types::ip::Ipv6Addr = net_declare::net_ip_v6!("2001:db8::1234");
    const OUT_OF_TARGET_SUBNET: net_types::ip::Ipv6Addr = net_declare::net_ip_v6!("2002::1");

    // Router 1 advertises the default route with High preference,
    // and TARGET_SUBNET with Low preference.
    send_ra(
        &fake_ep1,
        RouterAdvertisement::with_prf(
            0,     /* current_hop_limit */
            false, /* managed_flag */
            false, /* other_config_flag */
            RoutePreference::High,
            1234, /* router_lifetime */
            0,    /* reachable_time */
            0,    /* retransmit_timer */
        ),
        &[
            NdpOptionBuilder::SourceLinkLayerAddress(&[0, 0, 0, 0, 0, 1]),
            NdpOptionBuilder::RouteInformation(RouteInformation::new(
                TARGET_SUBNET,
                1337,
                RoutePreference::Low,
            )),
        ],
        ROUTER_1,
    )
    .await
    .expect("failed to send router advertisement 1");

    // Router 2 advertises the default route with Low preference,
    // and TARGET_SUBNET with High preference.
    send_ra(
        &fake_ep2,
        RouterAdvertisement::with_prf(
            0,     /* current_hop_limit */
            false, /* managed_flag */
            false, /* other_config_flag */
            RoutePreference::Low,
            1234, /* router_lifetime */
            0,    /* reachable_time */
            0,    /* retransmit_timer */
        ),
        &[
            NdpOptionBuilder::SourceLinkLayerAddress(&[0, 0, 0, 0, 0, 2]),
            NdpOptionBuilder::RouteInformation(RouteInformation::new(
                TARGET_SUBNET,
                1338,
                RoutePreference::High,
            )),
        ],
        ROUTER_2,
    )
    .await
    .expect("failed to send router advertisement 2");

    let nicid1 = iface1.id();
    let nicid2 = iface2.id();

    let ipv6_route_stream = {
        let state_v6 =
            realm.connect_to_protocol::<fnet_routes::StateV6Marker>().expect("connect to protocol");
        fnet_routes_ext::event_stream_from_state::<Ipv6>(&state_v6)
            .expect("failed to connect to watcher")
    };
    let ipv6_route_stream = pin!(ipv6_route_stream);
    let mut routes = HashSet::new();
    fnet_routes_ext::wait_for_routes(ipv6_route_stream, &mut routes, |accumulated_routes| {
        let target_subnet_routes = accumulated_routes
            .iter()
            .filter(|route| route.route.destination == TARGET_SUBNET)
            .count();
        let default_routes = accumulated_routes
            .iter()
            .filter(|route| route.route.destination == net_declare::net_subnet_v6!("::/0"))
            .count();
        target_subnet_routes == 2 && default_routes == 2
    })
    .on_timeout(fuchsia_async::MonotonicInstant::after(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT), || {
        panic!("timed out waiting for 4 routes to be installed");
    })
    .await
    .expect("error waiting for routes");

    let interface_state = realm
        .connect_to_protocol::<fnet_interfaces::StateMarker>()
        .expect("failed to connect to fuchsia.net.interfaces/State service");
    for nicid in [nicid1, nicid2] {
        let mut state_iface = fnet_interfaces_ext::InterfaceState::<(), _>::Unknown(nicid.into());
        let _ = fnet_interfaces_ext::wait_interface_with_id(
            fnet_interfaces_ext::event_stream_from_state::<fnet_interfaces_ext::DefaultInterest>(
                &interface_state,
                fnet_interfaces_ext::WatchOptions {
                    included_addresses: fnet_interfaces_ext::IncludedAddresses::OnlyAssigned,
                    ..Default::default()
                },
            )
            .expect("creating interface event stream"),
            &mut state_iface,
            |iface| (!iface.properties.addresses.is_empty()).then_some(()),
        )
        .await
        .expect("wait for address to become assigned");
    }

    // Query the route resolution to ensure the High preference one was selected!
    let state =
        realm.connect_to_protocol::<fnet_routes::StateMarker>().expect("connect to protocol");

    for (target, expected_router, expected_nicid) in
        [(IN_TARGET_SUBNET, ROUTER_2, nicid2), (OUT_OF_TARGET_SUBNET, ROUTER_1, nicid1)]
    {
        let response = state
            .resolve(&fnet::IpAddress::Ipv6(fnet::Ipv6Address { addr: target.ipv6_bytes() }))
            .await
            .expect("resolve fidl error")
            .expect("resolve failed");

        let expected_gateway_addr =
            fnet::IpAddress::Ipv6(fnet::Ipv6Address { addr: expected_router.ipv6_bytes() });

        assert_matches!(
            response,
            fnet_routes::Resolved::Gateway(dst)
                if dst.address == Some(expected_gateway_addr)
                   && dst.interface_id == Some(expected_nicid)
        );
    }
}

#[netstack_test]
#[variant(N, Netstack)]
async fn slaac_regeneration_after_dad_failure<N: Netstack>(name: &str) {
    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    enum SlaacAddrState {
        #[default]
        PrefixAdvertised,
        DadFailed {
            attempt: net_types_ip::Ipv6Addr,
        },
        Regenerated,
    }

    impl SlaacAddrState {
        async fn handle(
            &mut self,
            fake_ep: &netemul::TestFakeEndpoint<'_>,
            addr: net_types_ip::Ipv6Addr,
            state: fnet_interfaces::AddressAssignmentState,
        ) {
            match self {
                Self::PrefixAdvertised => {
                    assert_eq!(
                        state,
                        fnet_interfaces::AddressAssignmentState::Tentative,
                        "address should be tentative until DAD succeeds"
                    );
                    // Pretend that there is a duplicate address situation.
                    let snmc = addr.to_solicited_node_address();
                    ndp::write_message(
                        eth_consts::MAC_ADDR,
                        Mac::from(&snmc),
                        net_types_ip::Ipv6::UNSPECIFIED_ADDRESS,
                        snmc.get(),
                        NeighborSolicitation::new(addr),
                        &[],
                        &fake_ep,
                    )
                    .await
                    .expect("failed to write DAD message");

                    *self = Self::DadFailed { attempt: addr };
                }
                Self::DadFailed { attempt } => {
                    if addr != *attempt
                        && state == fnet_interfaces::AddressAssignmentState::Assigned
                    {
                        *self = Self::Regenerated;
                    }
                }
                Self::Regenerated => {}
            }
        }
    }

    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let (_network, realm, iface, fake_ep) = setup_network_with::<N, _>(
        &sandbox,
        name,
        InterfaceConfig {
            name: Some(name.into()),
            // Increase the number of transmits required for DAD to succeed to reduce the
            // likelihood of flakes where DAD erroneously succeeds when it should fail. This
            // number is chosen to be high enough to reduce flakiness while not extending
            // the runtime of the test by too much.
            ipv6_dad_transmits: Some(4),
            ..Default::default()
        },
        &[KnownServiceProvider::SecureStash],
    )
    .await
    .expect("error setting up network");

    // Send a Router Advertisement with information for a SLAAC prefix.
    //
    // Advertise an infinite valid and preferred lifetime. This will allow us to
    // differentiate between stable and temporary addresses, since temporary
    // addresses have maximum values for these lifetimes, whereas stable addresses
    // can use the infinite lifetimes as advertised.
    let options = [NdpOptionBuilder::PrefixInformation(PrefixInformation::new(
        ipv6_consts::GLOBAL_PREFIX.prefix(),  /* prefix_length */
        false,                                /* on_link_flag */
        true,                                 /* autonomous_address_configuration_flag */
        u32::MAX,                             /* valid_lifetime */
        u32::MAX,                             /* preferred_lifetime */
        ipv6_consts::GLOBAL_PREFIX.network(), /* prefix */
    ))];
    send_ra_with_router_lifetime(&fake_ep, 0, &options, ipv6_consts::LINK_LOCAL_ADDR)
        .await
        .expect("failed to send router advertisement");

    let interface_state = realm
        .connect_to_protocol::<fnet_interfaces::StateMarker>()
        .expect("failed to connect to fuchsia.net.interfaces/State");

    // We expect three SLAAC addresses to be assigned: one link-local address, and
    // two global addresses for the advertised SLAAC prefix (one stable and one
    // temporary).
    //
    // Register for interest in all addresses and all fields so we can observe
    // addition of tentative addresses before DAD succeeds, along with their valid
    // and preferred lifetimes in order to differentiate between stable and
    // temporary addresses.
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let mut state = fnet_interfaces_ext::InterfaceState::<(), _>::Unknown(iface.id());
    let mut watcher_fut = Box::pin(
        fnet_interfaces_ext::wait_interface_with_id(
            fnet_interfaces_ext::event_stream_from_state::<fnet_interfaces_ext::AllInterest>(
                &interface_state,
                fnet_interfaces_ext::WatchOptions {
                    included_addresses: fnet_interfaces_ext::IncludedAddresses::All,
                    ..Default::default()
                },
            )
            .expect("error getting interfaces state event stream"),
            &mut state,
            |iface| {
                for addr in &iface.properties.addresses {
                    tx.unbounded_send(addr.clone()).expect("receiver should not be dropped");
                }
                None
            },
        )
        .map(|result| result.expect("observe SLAAC addresses on watcher")),
    );

    let [mut link_local, mut stable_global, mut temporary_global] = [SlaacAddrState::default(); 3];
    loop {
        futures::select! {
            () = watcher_fut => panic!("watcher future should not complete"),
            () = fuchsia_async::Timer::new(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT) => {
                panic!(
                    "timed out waiting for SLAAC addresses to fail DAD and be regenerated; final \
                    state: link_local={link_local:?} stable_global={stable_global:?} \
                    temporary_global={temporary_global:?}"
                );
            }
            event = rx.next() => {
                let fnet_interfaces_ext::Address {
                    addr: fnet::Subnet { addr, .. },
                    assignment_state,
                    valid_until,
                    preferred_lifetime_info,
                } = event.expect("sender should not be dropped");
                let addr = match addr {
                    fnet::IpAddress::Ipv4(fnet::Ipv4Address { .. }) => continue,
                    fnet::IpAddress::Ipv6(fnet::Ipv6Address { addr }) => {
                        net_types_ip::Ipv6Addr::from_bytes(addr)
                    }
                };

                let state = if addr.is_link_local() {
                    &mut link_local
                } else if ipv6_consts::GLOBAL_PREFIX.contains(&addr) {
                    if valid_until == PositiveMonotonicInstant::INFINITE_FUTURE
                        && preferred_lifetime_info == PreferredLifetimeInfo::preferred_forever()
                    {
                        &mut stable_global
                    } else {
                        &mut temporary_global
                    }
                } else {
                    panic!(
                        "observed unexpected IPv6 addr that is neither link-local nor in \
                        advertised prefix {addr}"
                    );
                };

                state.handle(&fake_ep, addr, assignment_state).await;

                let addrs = [link_local, stable_global, temporary_global];
                if addrs.iter().all(|state| *state == SlaacAddrState::Regenerated) {
                    break;
                }
            }
        }
    }
}

fn check_mldv2_report(
    dst_ip: net_types_ip::Ipv6Addr,
    mld: MldPacket<&[u8]>,
    group: MulticastAddr<net_types_ip::Ipv6Addr>,
) -> bool {
    // Ignore non-report messages.
    let MldPacket::MulticastListenerReportV2(report) = mld else { return false };

    let has_snmc_record = report.body().iter_multicast_records().any(|record| {
        assert_eq!(record.sources(), &[]);
        let hdr = record.header();
        *hdr.multicast_addr() == group.get()
            && hdr.record_type() == Ok(Mldv2MulticastRecordType::ChangeToExcludeMode)
    });

    assert_eq!(
        dst_ip,
        net_ip_v6!("ff02::16"),
        "MLDv2 reports should be sent to the MLDv2 routers address"
    );

    has_snmc_record
}

fn check_mldv1_report(
    dst_ip: net_types_ip::Ipv6Addr,
    mld: MldPacket<&[u8]>,
    expected_group: MulticastAddr<net_types_ip::Ipv6Addr>,
) -> bool {
    // Ignore non-report messages.
    let MldPacket::MulticastListenerReport(report) = mld else { return false };

    let group_addr = report.body().group_addr;
    assert!(
        group_addr.is_multicast(),
        "MLD reports must only be sent for multicast addresses; group_addr = {}",
        group_addr
    );
    if group_addr != expected_group.get() {
        return false;
    }

    assert_eq!(
        dst_ip, group_addr,
        "the destination of an MLD report should be the multicast group the report is for",
    );

    true
}

fn check_mld_report(
    mld_version: Option<fnet_interfaces_admin::MldVersion>,
    dst_ip: net_types_ip::Ipv6Addr,
    mld: MldPacket<&[u8]>,
    expected_group: MulticastAddr<net_types_ip::Ipv6Addr>,
) -> bool {
    match mld_version.unwrap_or(fnet_interfaces_admin::MldVersion::V2) {
        fnet_interfaces_admin::MldVersion::V1 => check_mldv1_report(dst_ip, mld, expected_group),
        fnet_interfaces_admin::MldVersion::V2 => check_mldv2_report(dst_ip, mld, expected_group),
        other => panic!("unknown MLD version {:?}", other),
    }
}

#[netstack_test]
#[variant(N, Netstack)]
#[test_case(Some(fnet_interfaces_admin::MldVersion::V1); "mldv1")]
#[test_case(Some(fnet_interfaces_admin::MldVersion::V2); "mldv2")]
#[test_case(None; "default")]
async fn sends_mld_reports<N: Netstack>(
    name: &str,
    mld_version: Option<fnet_interfaces_admin::MldVersion>,
) {
    let sandbox = netemul::TestSandbox::new().expect("error creating sandbox");
    let (_network, _realm, iface, fake_ep) =
        setup_network::<N>(&sandbox, name, None).await.expect("error setting up networking");

    if let Some(mld_version) = mld_version {
        let gen_config = |mld_version| fnet_interfaces_admin::Configuration {
            ipv6: Some(fnet_interfaces_admin::Ipv6Configuration {
                mld: Some(fnet_interfaces_admin::MldConfiguration {
                    version: Some(mld_version),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        let control = iface.control();
        let new_config = gen_config(mld_version);
        let old_config = gen_config(fnet_interfaces_admin::MldVersion::V2);
        assert_eq!(
            control
                .set_configuration(&new_config)
                .await
                .expect("set_configuration fidl error")
                .expect("failed to set interface configuration"),
            old_config,
        );
        // Set configuration again to check the returned value is the new config
        // to show that nothing actually changed.
        assert_eq!(
            control
                .set_configuration(&new_config)
                .await
                .expect("set_configuration fidl error")
                .expect("failed to set interface configuration"),
            new_config,
        );
        assert_matches::assert_matches!(
            control
                .get_configuration()
                .await
                .expect("get_configuration fidl error")
                .expect("failed to get interface configuration"),
            fnet_interfaces_admin::Configuration {
                ipv6: Some(fnet_interfaces_admin::Ipv6Configuration {
                    mld: Some(fnet_interfaces_admin::MldConfiguration {
                        version: Some(got),
                        ..
                    }),
                    ..
                }),
                ..
            } => assert_eq!(got, mld_version)
        );
    }
    let _address_state_provider = {
        let subnet = fnet::Subnet {
            addr: fnet::IpAddress::Ipv6(fnet::Ipv6Address {
                addr: ipv6_consts::LINK_LOCAL_ADDR.ipv6_bytes(),
            }),
            prefix_len: 64,
        };

        interfaces::add_address_wait_assigned(
            &iface.control(),
            subnet,
            fnet_interfaces_admin::AddressParameters {
                add_subnet_route: Some(true),
                ..Default::default()
            },
        )
        .await
        .expect("add_address_wait_assigned failed")
    };
    let snmc = ipv6_consts::LINK_LOCAL_ADDR.to_solicited_node_address();

    let stream = fake_ep
        .frame_stream()
        .map(|r| r.context("error getting OnData event"))
        .try_filter_map(|(data, dropped)| {
            async move {
                assert_eq!(dropped, 0);
                let mut data = &data[..];

                let eth = EthernetFrame::parse(&mut data, EthernetFrameLengthCheck::NoCheck)
                    .expect("error parsing ethernet frame");

                if eth.ethertype() != Some(EtherType::Ipv6) {
                    // Ignore non-IPv6 packets.
                    return Ok(None);
                }

                let (mut payload, src_ip, dst_ip, proto, ttl) =
                    parse_ip_packet::<net_types_ip::Ipv6>(&data)
                        .expect("error parsing IPv6 packet");

                if proto != Ipv6Proto::Icmpv6 {
                    // Ignore non-ICMPv6 packets.
                    return Ok(None);
                }

                let icmp = Icmpv6Packet::parse(&mut payload, IcmpParseArgs::new(src_ip, dst_ip))
                    .expect("error parsing ICMPv6 packet");

                let mld = if let Icmpv6Packet::Mld(mld) = icmp {
                    mld
                } else {
                    // Ignore non-MLD packets.
                    return Ok(None);
                };

                // As per RFC 3590 section 4,
                //
                //   MLD Report and Done messages are sent with a link-local address as
                //   the IPv6 source address, if a valid address is available on the
                //   interface. If a valid link-local address is not available (e.g., one
                //   has not been configured), the message is sent with the unspecified
                //   address (::) as the IPv6 source address.
                assert!(!src_ip.is_specified() || src_ip.is_link_local(), "MLD messages must be sent from the unspecified or link local address; src_ip = {}", src_ip);


                // As per RFC 2710 section 3,
                //
                //   All MLD messages described in this document are sent with a
                //   link-local IPv6 Source Address, an IPv6 Hop Limit of 1, ...
                assert_eq!(ttl, 1, "MLD messages must have a hop limit of 1");

                Ok(check_mld_report(mld_version, dst_ip, mld, snmc).then_some(()))
            }
        });
    let mut stream = pin!(stream);
    stream
        .try_next()
        .on_timeout(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT.after_now(), || {
            return Err(anyhow::anyhow!("timed out waiting for the MLD report"));
        })
        .await
        .unwrap()
        .expect("error getting our expected MLD report");
}

#[netstack_test]
#[variant(N, Netstack)]
async fn sending_ra_with_autoconf_flag_triggers_slaac<N: Netstack>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("error creating sandbox");
    let (network, realm, iface, _fake_ep) =
        setup_network::<N>(&sandbox, name, None).await.expect("error setting up networking");

    let interfaces_state =
        &realm.connect_to_protocol::<fnet_interfaces::StateMarker>().expect("connect to protocol");

    // Wait for the netstack to be up before sending the RA.
    let iface_ip: net_types_ip::Ipv6Addr =
        netstack_testing_common::interfaces::wait_for_v6_ll(&interfaces_state, iface.id())
            .await
            .expect("waiting for link local address");

    // Pick a source address that is not the same as the interface's address by
    // flipping the low bytes.
    let src_ip = net_types_ip::Ipv6Addr::from_bytes(
        iface_ip
            .ipv6_bytes()
            .into_iter()
            .enumerate()
            .map(|(i, b)| if i < 8 { b } else { !b })
            .collect::<Vec<_>>()
            .try_into()
            .unwrap(),
    );

    let fake_router = network.create_fake_endpoint().expect("endpoint created");

    let options = [NdpOptionBuilder::PrefixInformation(PrefixInformation::new(
        ipv6_consts::GLOBAL_PREFIX.prefix(),  /* prefix_length */
        true,                                 /* on_link_flag */
        true,                                 /* autonomous_address_configuration_flag */
        6234,                                 /* valid_lifetime */
        0,                                    /* preferred_lifetime */
        ipv6_consts::GLOBAL_PREFIX.network(), /* prefix */
    ))];
    send_ra_with_router_lifetime(&fake_router, /* lifetime */ 1234, &options, src_ip)
        .await
        .expect("RA sent");

    fnet_interfaces_ext::wait_interface_with_id(
        fnet_interfaces_ext::event_stream_from_state::<fnet_interfaces_ext::DefaultInterest>(
            &interfaces_state,
            Default::default(),
        )
        .expect("creating interface event stream"),
        &mut fnet_interfaces_ext::InterfaceState::<(), _>::Unknown(iface.id()),
        |iface| {
            iface.properties.addresses.iter().find_map(
                |fnet_interfaces_ext::Address {
                     addr: fnet::Subnet { addr, prefix_len: _ },
                     valid_until: _,
                     preferred_lifetime_info: _,
                     assignment_state,
                 }| {
                    assert_eq!(
                        *assignment_state,
                        fnet_interfaces::AddressAssignmentState::Assigned
                    );
                    let addr = match addr {
                        fnet::IpAddress::Ipv4(fnet::Ipv4Address { .. }) => {
                            return None;
                        }
                        fnet::IpAddress::Ipv6(fnet::Ipv6Address { addr }) => {
                            net_types_ip::Ipv6Addr::from_bytes(*addr)
                        }
                    };
                    ipv6_consts::GLOBAL_PREFIX.contains(&addr).then(|| ())
                },
            )
        },
    )
    .await
    .expect("error waiting for address assignment");
}

#[netstack_test]
#[variant(N, Netstack)]
async fn add_device_adds_link_local_subnet_route<N: Netstack>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<N, _>(name).expect("create realm");
    let endpoint = sandbox.create_endpoint(name).await.expect("create endpoint");
    let iface = realm
        .install_endpoint(endpoint, InterfaceConfig::default())
        .await
        .expect("install interface");

    let id = iface.id();

    // Connect to the routes watcher and filter out all routing events unrelated
    // to the link-local subnet route.
    let ipv6_route_stream = {
        let state_v6 =
            realm.connect_to_protocol::<fnet_routes::StateV6Marker>().expect("connect to protocol");
        fnet_routes_ext::event_stream_from_state::<Ipv6>(&state_v6)
            .expect("failed to connect to watcher")
    };

    let link_local_subnet_route_events = ipv6_route_stream.filter_map(|event| {
        let event = event.expect("error in stream");
        futures::future::ready(match &event {
            fnet_routes_ext::Event::Existing(route)
            | fnet_routes_ext::Event::Added(route)
            | fnet_routes_ext::Event::Removed(route) => {
                let fnet_routes_ext::InstalledRoute {
                    route: fnet_routes_ext::Route { destination, action, properties: _ },
                    effective_properties: _,
                    table_id: _,
                } = route;
                let (outbound_interface, next_hop) = match action {
                    fnet_routes_ext::RouteAction::Forward(fnet_routes_ext::RouteTarget {
                        outbound_interface,
                        next_hop,
                    }) => (outbound_interface, next_hop),
                    fnet_routes_ext::RouteAction::Unknown => {
                        panic!("observed route with unknown action")
                    }
                };
                (*outbound_interface == id
                    && next_hop.is_none()
                    && *destination == net_declare::net_subnet_v6!("fe80::/64"))
                .then(|| event)
            }
            fnet_routes_ext::Event::Idle => None,
            fnet_routes_ext::Event::Unknown => {
                panic!("observed unknown event in stream");
            }
        })
    });
    let mut link_local_subnet_route_events = pin!(link_local_subnet_route_events);

    // Verify the link local subnet route is added.
    assert_matches!(
        link_local_subnet_route_events.next().await.expect("stream unexpectedly ended"),
        fnet_routes_ext::Event::Existing(_) | fnet_routes_ext::Event::Added(_)
    );

    // Removing the device should also remove the subnet route.
    drop(iface);
    assert_matches!(
        link_local_subnet_route_events.next().await.expect("stream unexpectedly ended"),
        fnet_routes_ext::Event::Removed(_)
    );
}

/// Tests that temporary IPv6 addresses are preferred.
#[netstack_test]
#[variant(N, Netstack)]
async fn prefers_temporary<N: Netstack>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<N, _>(name).expect("create realm");
    let endpoint = sandbox.create_endpoint(name).await.expect("create endpoint");
    let iface = realm
        .install_endpoint(endpoint, InterfaceConfig::default())
        .await
        .expect("install interface");

    const ADDR1: fnet::Subnet = net_declare::fidl_subnet!("2001:db8::1/64");
    const ADDR2: fnet::Subnet = net_declare::fidl_subnet!("2001:db8::2/64");
    const ADDR3: std::net::SocketAddr = net_declare::std_socket_addr!("[2001:db8::3]:1234");

    // Do everything twice so we show we're not relying on some other property
    // of address selection.
    let control = iface.control();
    for temp in [ADDR1, ADDR2] {
        let asp = futures::stream::iter([ADDR1, ADDR2])
            .map(|addr| async move {
                interfaces::add_address_wait_assigned(
                    control,
                    addr,
                    fnet_interfaces_admin::AddressParameters {
                        add_subnet_route: Some(true),
                        temporary: Some(addr == temp),
                        ..Default::default()
                    },
                )
                .await
                .expect("add_address_wait_assigned failed")
            })
            .buffer_unordered(usize::MAX)
            .collect::<Vec<_>>()
            .await;

        let sock = realm
            .datagram_socket(
                fposix_socket::Domain::Ipv6,
                fposix_socket::DatagramSocketProtocol::Udp,
            )
            .await
            .expect("udp socket");
        sock.connect(&ADDR3.into()).expect("connect UDP");
        let addr = sock.local_addr().expect("get local addr").as_socket().expect("network socket");
        let ip: fnet::IpAddress = fnet_ext::IpAddress(addr.ip()).into();
        assert_eq!(ip, temp.addr);

        // Remove the addresses cleanly so we can add them again switching which
        // one is the temporary addr.
        futures::stream::iter(asp)
            .for_each_concurrent(None, |asp| async move {
                asp.remove().expect("remove addr");
                assert_matches!(
                    asp.take_event_stream().next().await,
                    Some(Ok(fnet_interfaces_admin::AddressStateProviderEvent::OnAddressRemoved {
                        error: fnet_interfaces_admin::AddressRemovalReason::UserRemoved
                    }))
                );
            })
            .await;
    }
}

/// Tests that addresses generated by SLAAC report new lifetimes over the
/// watcher API appropriately.
#[netstack_test]
#[variant(N, Netstack)]
async fn slaac_addrs_report_lifetimes<N: Netstack>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let (_network, realm, iface, fake_ep) =
        setup_network::<N>(&sandbox, name, None).await.expect("error setting up network");

    const INITIAL_VALID_LIFETIME_SECS: u32 = 100_000;
    const INITIAL_PREFERRED_LIFETIME_SECS: u32 = 100_00;
    const UPDATED_LIFETIME_SECS: u32 = 1;

    // Send a Router Advertisement with information for a SLAAC prefix.
    let options = [NdpOptionBuilder::PrefixInformation(PrefixInformation::new(
        ipv6_consts::GLOBAL_PREFIX.prefix(),  /* prefix_length */
        false,                                /* on_link_flag */
        true,                                 /* autonomous_address_configuration_flag */
        INITIAL_VALID_LIFETIME_SECS,          /* valid_lifetime */
        INITIAL_PREFERRED_LIFETIME_SECS,      /* preferred_lifetime */
        ipv6_consts::GLOBAL_PREFIX.network(), /* prefix */
    ))];
    send_ra_with_router_lifetime(&fake_ep, 0, &options, ipv6_consts::LINK_LOCAL_ADDR)
        .await
        .expect("failed to send router advertisement");

    let mut watcher = pin!(
        realm
            .get_interface_event_stream_with_interest::<fnet_interfaces_ext::AllInterest>()
            .expect("error getting interface state event stream")
    );
    let mut watcher_state = &mut fnet_interfaces_ext::InterfaceState::<(), _>::Unknown(iface.id());

    #[derive(Debug, Eq, PartialEq)]
    struct Lifetimes {
        valid_until: fnet_interfaces_ext::PositiveMonotonicInstant,
        preferred_lifetime_info: fnet_interfaces_ext::PositiveMonotonicInstant,
        updated_valid_until: Option<fnet_interfaces_ext::PositiveMonotonicInstant>,
        updated_preferred_lifetime: Option<fnet_interfaces_ext::PositiveMonotonicInstant>,
        seen_deprecation: bool,
    }

    impl Lifetimes {
        fn update(
            &mut self,
            addr: &fnet_interfaces_ext::Address<fnet_interfaces_ext::AllInterest>,
        ) {
            let fnet_interfaces_ext::Address {
                valid_until: cur_valid_until,
                preferred_lifetime_info: cur_preferred,
                ..
            } = addr;

            let Lifetimes {
                valid_until,
                preferred_lifetime_info,
                updated_valid_until,
                updated_preferred_lifetime,
                seen_deprecation,
            } = self;

            match (updated_preferred_lifetime.as_ref(), *seen_deprecation) {
                (Some(_), true) => assert_eq!(
                    cur_preferred,
                    &fnet_interfaces_ext::PreferredLifetimeInfo::Deprecated,
                    "deprecated changed"
                ),
                (Some(i), false) => match cur_preferred {
                    fnet_interfaces_ext::PreferredLifetimeInfo::PreferredUntil(cur) => {
                        assert_eq!(cur, i, "preferred lifetime changed");
                    }
                    fnet_interfaces_ext::PreferredLifetimeInfo::Deprecated => {
                        *seen_deprecation = true;
                    }
                },
                (None, false) => {
                    let preferred = assert_matches!(cur_preferred,
                         fnet_interfaces_ext::PreferredLifetimeInfo::PreferredUntil(v) => *v);
                    if preferred != *preferred_lifetime_info {
                        // Must have decreased, we changed the preferred
                        // lifetime to earlier.
                        assert!(preferred < *preferred_lifetime_info);
                        *updated_preferred_lifetime = Some(preferred);
                    }
                }
                // Can't mark deprecation without having seen updated lifetime.
                (None, true) => unreachable!(),
            }

            match updated_valid_until.as_ref() {
                Some(u) => {
                    assert_eq!(cur_valid_until, u);
                }
                None => {
                    if cur_valid_until != valid_until {
                        *updated_valid_until = Some(*cur_valid_until);
                    }
                }
            }
        }
    }

    // Extract the initial addresses. We expect one stable and one temporary
    // address.
    const EXPECTED_ADDRS: usize = 2;
    let mut addrs = fnet_interfaces_ext::wait_interface_with_id(
        watcher.by_ref(),
        &mut watcher_state,
        |iface| {
            let addrs = iface
                .properties
                .addresses
                .iter()
                .filter_map(
                    |&fnet_interfaces_ext::Address {
                         addr: fnet::Subnet { addr, prefix_len: _ },
                         valid_until,
                         preferred_lifetime_info,
                         assignment_state,
                     }| {
                        assert_eq!(
                            assignment_state,
                            fnet_interfaces::AddressAssignmentState::Assigned
                        );
                        // Must not yet be deprecated.
                        let preferred_lifetime_info = assert_matches!(preferred_lifetime_info,
                            fnet_interfaces_ext::PreferredLifetimeInfo::PreferredUntil(v) => v);
                        match addr {
                            fnet::IpAddress::Ipv4(fnet::Ipv4Address { .. }) => None,
                            v6 @ fnet::IpAddress::Ipv6(fnet::Ipv6Address { addr }) => {
                                ipv6_consts::GLOBAL_PREFIX
                                    .contains(&net_types_ip::Ipv6Addr::from_bytes(addr))
                                    .then_some((
                                        v6,
                                        Lifetimes {
                                            valid_until,
                                            preferred_lifetime_info,
                                            updated_preferred_lifetime: None,
                                            updated_valid_until: None,
                                            seen_deprecation: false,
                                        },
                                    ))
                            }
                        }
                    },
                )
                .collect::<HashMap<_, _>>();
            match addrs.len() {
                EXPECTED_ADDRS => Some(addrs),
                x if x < EXPECTED_ADDRS => None,
                _ => panic!("collected more addrs than expected: {addrs:?}"),
            }
        },
    )
    .map(|r| r.expect("watcher error"))
    .on_timeout(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT, || panic!("timed out waiting for addresses"))
    .await;

    // Send a router advertisement with an updated preferred lifetime.
    let options = [NdpOptionBuilder::PrefixInformation(PrefixInformation::new(
        ipv6_consts::GLOBAL_PREFIX.prefix(),  /* prefix_length */
        false,                                /* on_link_flag */
        true,                                 /* autonomous_address_configuration_flag */
        INITIAL_VALID_LIFETIME_SECS,          /* valid_lifetime */
        UPDATED_LIFETIME_SECS,                /* preferred_lifetime */
        ipv6_consts::GLOBAL_PREFIX.network(), /* prefix */
    ))];
    send_ra_with_router_lifetime(&fake_ep, 0, &options, ipv6_consts::LINK_LOCAL_ADDR)
        .await
        .expect("failed to send router advertisement");

    // Wait until we see both addresses deprecate. The updated preferred
    // lifetime is short enough that deprecation should happen soon after.
    fnet_interfaces_ext::wait_interface_with_id(watcher.by_ref(), &mut watcher_state, |iface| {
        // Update state.
        for addr in iface.properties.addresses.iter() {
            let Some(entry) = addrs.get_mut(&addr.addr.addr) else { continue };
            entry.update(addr)
        }
        // Exit when we've observed all addresses being deprecated.
        addrs.values().all(|a| a.seen_deprecation).then_some(())
    })
    .on_timeout(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT, || panic!("timed out waiting for deprecation"))
    .await
    .expect("watcher error");

    // All addresses are now deprecated, check that updating valid until with a
    // zero preferred lifetime will update only valid and then eventually remove
    // the addresses.
    for addr in addrs.values_mut() {
        addr.valid_until = addr.updated_valid_until.take().unwrap();
    }

    // Send a router advertisement with an updated preferred lifetime.
    let options = [NdpOptionBuilder::PrefixInformation(PrefixInformation::new(
        ipv6_consts::GLOBAL_PREFIX.prefix(),  /* prefix_length */
        false,                                /* on_link_flag */
        true,                                 /* autonomous_address_configuration_flag */
        UPDATED_LIFETIME_SECS,                /* valid_lifetime */
        0,                                    /* preferred_lifetime */
        ipv6_consts::GLOBAL_PREFIX.network(), /* prefix */
    ))];
    send_ra_with_router_lifetime(&fake_ep, 0, &options, ipv6_consts::LINK_LOCAL_ADDR)
        .await
        .expect("failed to send router advertisement");

    // Wait until we see both addresses update valid_until. Note that there's a
    // guard against very short valid until times so we can't observe removal
    // without faking the clock which is more than this test is attempting to
    // do.
    fnet_interfaces_ext::wait_interface_with_id(watcher.by_ref(), &mut watcher_state, |iface| {
        // Update state.
        for addr in iface.properties.addresses.iter() {
            let Some(entry) = addrs.get_mut(&addr.addr.addr) else { continue };
            entry.update(addr)
        }
        // Exit when we've observed all `valid_until`s be updated.
        addrs.values().all(|a| a.updated_valid_until.is_some()).then_some(())
    })
    .on_timeout(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT, || {
        panic!("timed out waiting for valid until update")
    })
    .await
    .expect("watcher error");
}
