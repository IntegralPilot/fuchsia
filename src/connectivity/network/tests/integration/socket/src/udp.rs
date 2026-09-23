// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

use std::fmt::Debug;
use std::num::NonZeroU16;
use std::os::fd::AsRawFd as _;
use std::pin::pin;
use std::task::Poll;

use anyhow::anyhow;
use assert_matches::assert_matches;
use fidl_fuchsia_hardware_network as fhardware_network;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_ext as fnet_ext;
use fidl_fuchsia_net_ext::IpExt as _;
use fidl_fuchsia_net_interfaces as fnet_interfaces;
use fidl_fuchsia_net_interfaces_admin as fnet_interfaces_admin;
use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
use fidl_fuchsia_net_routes as fnet_routes;
use fidl_fuchsia_net_routes_ext as fnet_routes_ext;
use fidl_fuchsia_net_tun as fnet_tun;
use fidl_fuchsia_posix as fposix;
use fidl_fuchsia_posix_socket as fposix_socket;
use fidl_fuchsia_posix_socket_ext as fposix_socket_ext;
use fuchsia_async::net::{DatagramSocket, UdpSocket};
use fuchsia_async::{self as fasync, DurationExt, TimeoutExt as _};
use futures::future::{self};
use futures::{FutureExt as _, StreamExt as _};
use net_declare::{
    fidl_ip_v4, fidl_ip_v6, fidl_mac, fidl_socket_addr, fidl_subnet, net_ip_v4, net_ip_v6,
    net_subnet_v4, net_subnet_v6, std_ip_v4, std_socket_addr,
};
use net_types::ip::{Ip, IpAddr, IpAddress as _, IpVersion, Ipv4, Ipv6};
use netemul::{RealmUdpSocket as _, TestRealm};
use netstack_testing_common::constants::ipv6 as ipv6_consts;
use netstack_testing_common::interfaces::TestInterfaceExt as _;
use netstack_testing_common::realms::{
    KnownServiceProvider, Netstack, Netstack3, NetstackVersion, TestRealmExt, TestSandboxExt as _,
};
use netstack_testing_common::{
    ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT, ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT, Result, devices, ndp,
};
use netstack_testing_macros::netstack_test;
use packet::{
    NestableSerializer as _, NoOpSerializationContext, ParsablePacket as _, Serializer as _,
};
use packet_formats::ethernet::{
    ETHERNET_MIN_BODY_LEN_NO_TAG, EtherType, EthernetFrameBuilder, EthernetFrameLengthCheck,
};
use packet_formats::icmp::ndp::options::{NdpOptionBuilder, PrefixInformation};
use packet_formats::ip::IpProto;
use packet_formats::ipv4::{Ipv4Header as _, Ipv4Packet, Ipv4PacketBuilder};
use packet_formats::ipv6::Ipv6PacketBuilder;
use packet_formats::udp::UdpPacketBuilder;
use socket2::InterfaceIndexOrAddress;
use test_case::{test_case, test_matrix};

use crate::{
    CLIENT_MAC, CLIENT_SUBNET, Interface, MultiNicAndPeerConfig, MulticastTestIpExt, Network,
    SERVER_MAC, SERVER_SUBNET, TestIpExt,
};

pub(super) async fn run_udp_socket_test(
    server: &netemul::TestRealm<'_>,
    server_addr: fnet::IpAddress,
    client: &netemul::TestRealm<'_>,
    client_addr: fnet::IpAddress,
) {
    let fnet_ext::IpAddress(client_addr) = fnet_ext::IpAddress::from(client_addr);
    let client_addr = std::net::SocketAddr::new(client_addr, 1234);

    let fnet_ext::IpAddress(server_addr) = fnet_ext::IpAddress::from(server_addr);
    let server_addr = std::net::SocketAddr::new(server_addr, 8080);

    let client_sock = fasync::net::UdpSocket::bind_in_realm(client, client_addr)
        .await
        .expect("failed to create client socket");

    let server_sock = fasync::net::UdpSocket::bind_in_realm(server, server_addr)
        .await
        .expect("failed to create server socket");

    const PAYLOAD: &'static str = "Hello World";

    let client_fut = async move {
        let r = client_sock.send_to(PAYLOAD.as_bytes(), server_addr).await.expect("sendto failed");
        assert_eq!(r, PAYLOAD.as_bytes().len());
    };
    let server_fut = async move {
        let mut buf = [0u8; 1024];
        let (r, from) = server_sock.recv_from(&mut buf[..]).await.expect("recvfrom failed");
        assert_eq!(r, PAYLOAD.as_bytes().len());
        assert_eq!(&buf[..r], PAYLOAD.as_bytes());
        // Unspecified addresses will use loopback as their source
        if client_addr.ip().is_unspecified() {
            assert!(from.ip().is_loopback());
        } else {
            assert_eq!(from, client_addr);
        }
    };

    let ((), ()) = futures::future::join(client_fut, server_fut).await;
}

enum UdpProtocol {
    Synchronous,
    Fast,
}

#[netstack_test]
#[variant(N, Netstack)]
#[test_case(
    UdpProtocol::Synchronous, false; "synchronous_protocol not mapped to ipv6")]
#[test_case(
    UdpProtocol::Fast, false; "fast_protocol not mapped to ipv6")]
#[test_case(
    UdpProtocol::Synchronous, true; "synchronous_protocol mapped to ipv6")]
#[test_case(
    UdpProtocol::Fast, true; "fast_protocol mapped to ipv6")]
async fn test_udp_socket<N: Netstack>(name: &str, protocol: UdpProtocol, mapped_to_ipv6: bool) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let net = sandbox.create_network("net").await.expect("failed to create network");

    let _packet_capture = net.start_capture(name).await.expect("starting packet capture");

    let (client, server) = match protocol {
        UdpProtocol::Synchronous => {
            let client = sandbox
                .create_netstack_realm::<N, _>(format!("{}_client", name))
                .expect("failed to create client realm");
            let server = sandbox
                .create_netstack_realm::<N, _>(format!("{}_server", name))
                .expect("failed to create server realm");
            (client, server)
        }
        UdpProtocol::Fast => {
            let version = match N::VERSION {
                NetstackVersion::Netstack2 { tracing, fast_udp: _ } => {
                    NetstackVersion::Netstack2 { tracing, fast_udp: true }
                }
                version => version,
            };
            let client = sandbox
                .create_realm(format!("{}_client", name), [KnownServiceProvider::Netstack(version)])
                .expect("failed to create client realm");
            let server = sandbox
                .create_realm(format!("{}_client", name), [KnownServiceProvider::Netstack(version)])
                .expect("failed to create client realm");
            (client, server)
        }
    };

    let client_ep = client
        .join_network_with(
            &net,
            "client",
            netemul::new_endpoint_config(netemul::DEFAULT_MTU, Some(CLIENT_MAC)),
            Default::default(),
        )
        .await
        .expect("client failed to join network");
    client_ep.add_address_and_subnet_route(CLIENT_SUBNET).await.expect("configure address");
    let server_ep = server
        .join_network_with(
            &net,
            "server",
            netemul::new_endpoint_config(netemul::DEFAULT_MTU, Some(SERVER_MAC)),
            Default::default(),
        )
        .await
        .expect("server failed to join network");
    server_ep.add_address_and_subnet_route(SERVER_SUBNET).await.expect("configure address");

    // Add static ARP entries as we've observed flakes in CQ due to ARP timeouts
    // and ARP resolution is immaterial to this test.
    futures::stream::iter([
        (&server, &server_ep, CLIENT_SUBNET.addr, CLIENT_MAC),
        (&client, &client_ep, SERVER_SUBNET.addr, SERVER_MAC),
    ])
    .for_each_concurrent(None, |(realm, ep, addr, mac)| {
        realm.add_neighbor_entry(ep.id(), addr, mac).map(|r| r.expect("add_neighbor_entry"))
    })
    .await;

    let maybe_map_to_ipv6 = move |orig_addr| match orig_addr {
        fnet::IpAddress::Ipv4(addr) => {
            if mapped_to_ipv6 {
                let addr = net_types::ip::Ipv4Addr::new(addr.addr);
                fnet::IpAddress::Ipv6(fnet::Ipv6Address {
                    addr: addr.to_ipv6_mapped().ipv6_bytes(),
                })
            } else {
                orig_addr
            }
        }
        fnet::IpAddress::Ipv6(_) => {
            unreachable!("SERVER_SUBNET and CLIENT_SUBNET expected to be Ipv4")
        }
    };

    let server_addr = maybe_map_to_ipv6(SERVER_SUBNET.addr);
    let client_addr = maybe_map_to_ipv6(CLIENT_SUBNET.addr);

    run_udp_socket_test(&server, server_addr, &client, client_addr).await
}

enum UdpCacheInvalidationReason {
    ConnectCalled,
    InterfaceDisabled,
    AddressRemoved,
    SetConfigurationCalled,
    RouteRemoved,
    RouteAdded,
}

enum ToAddrExpectation {
    Unspecified,
    Specified(Option<fnet::SocketAddress>),
}

struct UdpSendMsgPreflightSuccessExpectation {
    expected_to_addr: ToAddrExpectation,
    expect_all_eventpairs_valid: bool,
}

enum UdpSendMsgPreflightExpectation {
    Success(UdpSendMsgPreflightSuccessExpectation),
    Failure(fposix::Errno),
}

struct UdpSendMsgPreflight {
    to_addr: Option<fnet::SocketAddress>,
    expected_result: UdpSendMsgPreflightExpectation,
}

async fn setup_fastudp_network<'a>(
    name: &'a str,
    version: NetstackVersion,
    sandbox: &'a netemul::TestSandbox,
    socket_domain: fposix_socket::Domain,
) -> (
    netemul::TestNetwork<'a>,
    netemul::TestRealm<'a>,
    netemul::TestInterface<'a>,
    fposix_socket::DatagramSocketProxy,
) {
    let net = sandbox.create_network("net").await.expect("create network");
    let version = match version {
        NetstackVersion::Netstack2 { tracing, fast_udp: _ } => {
            NetstackVersion::Netstack2 { tracing, fast_udp: true }
        }
        version => version,
    };
    let netstack = sandbox
        .create_realm(name, [KnownServiceProvider::Netstack(version)])
        .expect("create netstack realm");
    let iface = netstack.join_network(&net, "ep").await.expect("failed to join network");

    let socket = {
        let socket_provider = netstack
            .connect_to_protocol::<fposix_socket::ProviderMarker>()
            .expect("connect to socket provider");
        let datagram_socket = socket_provider
            .datagram_socket(socket_domain, fposix_socket::DatagramSocketProtocol::Udp)
            .await
            .expect("call datagram_socket")
            .expect("create datagram socket");
        match datagram_socket {
            fposix_socket::ProviderDatagramSocketResponse::DatagramSocket(socket) => {
                socket.into_proxy()
            }
            socket => panic!("unexpected datagram socket variant: {:?}", socket),
        }
    };

    (net, netstack, iface, socket)
}

fn validate_send_msg_preflight_response(
    response: &fposix_socket::DatagramSocketSendMsgPreflightResponse,
    expectation: UdpSendMsgPreflightSuccessExpectation,
) -> Result {
    let fposix_socket::DatagramSocketSendMsgPreflightResponse {
        to, validity, maximum_size, ..
    } = response;
    let UdpSendMsgPreflightSuccessExpectation { expected_to_addr, expect_all_eventpairs_valid } =
        expectation;

    match expected_to_addr {
        ToAddrExpectation::Specified(to_addr) => {
            assert_eq!(*to, to_addr, "unexpected to address in boarding pass");
        }
        ToAddrExpectation::Unspecified => (),
    }

    const MAXIMUM_UDP_PACKET_SIZE: u32 = 65535;
    const UDP_HEADER_SIZE: u32 = 8;
    assert_eq!(*maximum_size, Some(MAXIMUM_UDP_PACKET_SIZE - UDP_HEADER_SIZE));

    let validity = validity.as_ref().expect("validity was missing");
    assert!(validity.len() > 0, "validity was empty");
    let all_eventpairs_valid = {
        let mut wait_items = validity
            .iter()
            .map(|eventpair| eventpair.wait_item(zx::Signals::EVENTPAIR_PEER_CLOSED))
            .collect::<Vec<_>>();
        zx::object_wait_many(&mut wait_items, zx::MonotonicInstant::INFINITE_PAST)
            == Err(zx::Status::TIMED_OUT)
    };
    if expect_all_eventpairs_valid != all_eventpairs_valid {
        return Err(anyhow!(
            "mismatched expectation on eventpair validity: expected {}, got {}",
            expect_all_eventpairs_valid,
            all_eventpairs_valid
        ));
    }
    Ok(())
}

/// Executes a preflight for each of the passed preflight configs, validating
/// the result against the passed expectation and returning all successful responses.
async fn execute_and_validate_preflights(
    preflights: impl IntoIterator<Item = UdpSendMsgPreflight>,
    proxy: &fposix_socket::DatagramSocketProxy,
) -> Vec<fposix_socket::DatagramSocketSendMsgPreflightResponse> {
    futures::stream::iter(preflights)
        .then(|preflight| {
            let UdpSendMsgPreflight { to_addr, expected_result } = preflight;
            let result =
                proxy.send_msg_preflight(&fposix_socket::DatagramSocketSendMsgPreflightRequest {
                    to: to_addr,
                    ..Default::default()
                });
            async move { (expected_result, result.await) }
        })
        .filter_map(|(expected, actual)| async move {
            let actual = actual.expect("send_msg_preflight fidl error");
            match expected {
                UdpSendMsgPreflightExpectation::Success(success_expectation) => {
                    let response = actual.expect("send_msg_preflight failed");
                    validate_send_msg_preflight_response(&response, success_expectation)
                        .expect("validate preflight response");
                    Some(response)
                }
                UdpSendMsgPreflightExpectation::Failure(expected_errno) => {
                    assert_eq!(Err(expected_errno), actual);
                    None
                }
            }
        })
        .collect::<Vec<_>>()
        .await
}

trait UdpSendMsgPreflightTestIpExt: Ip {
    const PORT: u16;
    const SOCKET_DOMAIN: fposix_socket::Domain;
    const INSTALLED_ADDR: fnet::Subnet;
    const REACHABLE_ADDR1: fnet::SocketAddress;
    const REACHABLE_ADDR2: fnet::SocketAddress;
    const UNREACHABLE_ADDR: fnet::SocketAddress;
    const OTHER_SUBNET: fnet::Subnet;

    fn forwarding_config() -> fnet_interfaces_admin::Configuration;
}

impl UdpSendMsgPreflightTestIpExt for net_types::ip::Ipv4 {
    const PORT: u16 = 80;
    const SOCKET_DOMAIN: fposix_socket::Domain = fposix_socket::Domain::Ipv4;
    const INSTALLED_ADDR: fnet::Subnet = fidl_subnet!("192.0.2.1/24");
    const REACHABLE_ADDR1: fnet::SocketAddress =
        fnet::SocketAddress::Ipv4(fnet::Ipv4SocketAddress {
            address: fidl_ip_v4!("192.0.2.101"),
            port: Self::PORT,
        });
    const REACHABLE_ADDR2: fnet::SocketAddress =
        fnet::SocketAddress::Ipv4(fnet::Ipv4SocketAddress {
            address: fidl_ip_v4!("192.0.2.102"),
            port: Self::PORT,
        });
    const UNREACHABLE_ADDR: fnet::SocketAddress =
        fnet::SocketAddress::Ipv4(fnet::Ipv4SocketAddress {
            address: fidl_ip_v4!("198.51.100.1"),
            port: Self::PORT,
        });
    const OTHER_SUBNET: fnet::Subnet = fidl_subnet!("203.0.113.0/24");

    fn forwarding_config() -> fnet_interfaces_admin::Configuration {
        fnet_interfaces_admin::Configuration {
            ipv4: Some(fnet_interfaces_admin::Ipv4Configuration {
                unicast_forwarding: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        }
    }
}

impl UdpSendMsgPreflightTestIpExt for net_types::ip::Ipv6 {
    const PORT: u16 = 80;
    const SOCKET_DOMAIN: fposix_socket::Domain = fposix_socket::Domain::Ipv6;
    const INSTALLED_ADDR: fnet::Subnet = fidl_subnet!("2001:db8::1/64");
    const REACHABLE_ADDR1: fnet::SocketAddress =
        fnet::SocketAddress::Ipv6(fnet::Ipv6SocketAddress {
            address: fidl_ip_v6!("2001:db8::1001"),
            port: Self::PORT,
            zone_index: 0,
        });
    const REACHABLE_ADDR2: fnet::SocketAddress =
        fnet::SocketAddress::Ipv6(fnet::Ipv6SocketAddress {
            address: fidl_ip_v6!("2001:db8::1002"),
            port: Self::PORT,
            zone_index: 0,
        });
    const UNREACHABLE_ADDR: fnet::SocketAddress =
        fnet::SocketAddress::Ipv6(fnet::Ipv6SocketAddress {
            address: fidl_ip_v6!("2001:db8:ffff:ffff::1"),
            port: Self::PORT,
            zone_index: 0,
        });
    const OTHER_SUBNET: fnet::Subnet = fidl_subnet!("2001:db8:eeee:eeee::/64");

    fn forwarding_config() -> fnet_interfaces_admin::Configuration {
        fnet_interfaces_admin::Configuration {
            ipv6: Some(fnet_interfaces_admin::Ipv6Configuration {
                unicast_forwarding: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        }
    }
}

async fn udp_send_msg_preflight_fidl_setup<I: UdpSendMsgPreflightTestIpExt>(
    iface: &netemul::TestInterface<'_>,
    socket: &fposix_socket::DatagramSocketProxy,
) -> Vec<fposix_socket::DatagramSocketSendMsgPreflightResponse> {
    iface
        .add_address_and_subnet_route(I::INSTALLED_ADDR)
        .await
        .expect("failed to add subnet route");

    let successful_preflights = execute_and_validate_preflights(
        [
            UdpSendMsgPreflight {
                to_addr: Some(I::UNREACHABLE_ADDR),
                expected_result: UdpSendMsgPreflightExpectation::Failure(
                    fposix::Errno::Ehostunreach,
                ),
            },
            UdpSendMsgPreflight {
                to_addr: None,
                expected_result: UdpSendMsgPreflightExpectation::Failure(
                    fposix::Errno::Edestaddrreq,
                ),
            },
        ],
        &socket,
    )
    .await;
    assert_eq!(successful_preflights, []);

    let connected_addr = I::REACHABLE_ADDR1;
    socket.connect(&connected_addr).await.expect("connect fidl error").expect("connect failed");

    // We deliberately repeat an address here to ensure that the preflight can
    // be called > 1 times with the same address.
    let mut preflights: Vec<UdpSendMsgPreflight> =
        vec![I::REACHABLE_ADDR1, I::REACHABLE_ADDR2, I::REACHABLE_ADDR2]
            .iter()
            .map(|socket_address| UdpSendMsgPreflight {
                to_addr: Some(*socket_address),
                expected_result: UdpSendMsgPreflightExpectation::Success(
                    UdpSendMsgPreflightSuccessExpectation {
                        expected_to_addr: ToAddrExpectation::Specified(None),
                        expect_all_eventpairs_valid: true,
                    },
                ),
            })
            .collect();
    preflights.push(UdpSendMsgPreflight {
        to_addr: None,
        expected_result: UdpSendMsgPreflightExpectation::Success(
            UdpSendMsgPreflightSuccessExpectation {
                expected_to_addr: ToAddrExpectation::Specified(Some(connected_addr)),
                expect_all_eventpairs_valid: true,
            },
        ),
    });

    execute_and_validate_preflights(preflights, &socket).await
}

fn assert_preflights_invalidated(
    successful_preflights: impl IntoIterator<
        Item = fposix_socket::DatagramSocketSendMsgPreflightResponse,
    >,
) {
    for successful_preflight in successful_preflights {
        validate_send_msg_preflight_response(
            &successful_preflight,
            UdpSendMsgPreflightSuccessExpectation {
                expected_to_addr: ToAddrExpectation::Unspecified,
                expect_all_eventpairs_valid: false,
            },
        )
        .expect("validate preflight response");
    }
}

#[netstack_test]
#[variant(N, Netstack)]
#[variant(I, Ip)]
#[test_case("connect_called", UdpCacheInvalidationReason::ConnectCalled)]
#[test_case("Control.Disable", UdpCacheInvalidationReason::InterfaceDisabled)]
#[test_case("Control.RemoveAddress", UdpCacheInvalidationReason::AddressRemoved)]
#[test_case("Control.SetConfiguration", UdpCacheInvalidationReason::SetConfigurationCalled)]
#[test_case("route_removed", UdpCacheInvalidationReason::RouteRemoved)]
#[test_case("route_added", UdpCacheInvalidationReason::RouteAdded)]
async fn udp_send_msg_preflight_fidl<N: Netstack, I: UdpSendMsgPreflightTestIpExt>(
    root_name: &str,
    test_name: &str,
    invalidation_reason: UdpCacheInvalidationReason,
) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm_name = format!("{}_{}", root_name, test_name);
    let (_net, _netstack, iface, socket) =
        setup_fastudp_network(&realm_name, N::VERSION, &sandbox, I::SOCKET_DOMAIN).await;

    let successful_preflights = udp_send_msg_preflight_fidl_setup::<I>(&iface, &socket).await;

    match invalidation_reason {
        UdpCacheInvalidationReason::ConnectCalled => {
            let connected_addr = I::REACHABLE_ADDR2;
            socket
                .connect(&connected_addr)
                .await
                .expect("connect fidl error")
                .expect("connect failed");
        }
        UdpCacheInvalidationReason::InterfaceDisabled => {
            let disabled = iface
                .control()
                .disable()
                .await
                .expect("disable_interface fidl error")
                .expect("failed to disable interface");
            assert_eq!(disabled, true);
        }
        UdpCacheInvalidationReason::AddressRemoved => {
            let installed_subnet = I::INSTALLED_ADDR;
            let removed = iface
                .control()
                .remove_address(&installed_subnet)
                .await
                .expect("remove_address fidl error")
                .expect("failed to remove address");
            assert!(removed, "address was not removed from interface");
        }
        UdpCacheInvalidationReason::RouteRemoved => {
            iface.del_subnet_route(I::INSTALLED_ADDR).await.expect("failed to delete subnet route");
        }
        UdpCacheInvalidationReason::RouteAdded => {
            let () =
                iface.add_subnet_route(I::OTHER_SUBNET).await.expect("failed to add subnet route");
        }
        UdpCacheInvalidationReason::SetConfigurationCalled => {
            let _prev_config = iface
                .control()
                .set_configuration(&I::forwarding_config())
                .await
                .expect("set_configuration fidl error")
                .expect("failed to set interface configuration");
        }
    }

    assert_preflights_invalidated(successful_preflights);
}

enum UdpCacheInvalidationReasonV4 {
    BroadcastCalled,
}

#[netstack_test]
#[variant(N, Netstack)]
#[test_case("broadcast_called", UdpCacheInvalidationReasonV4::BroadcastCalled)]
async fn udp_send_msg_preflight_fidl_v4only<N: Netstack>(
    root_name: &str,
    test_name: &str,
    invalidation_reason: UdpCacheInvalidationReasonV4,
) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm_name = format!("{}_{}", root_name, test_name);
    let (_net, _netstack, iface, socket) =
        setup_fastudp_network(&realm_name, N::VERSION, &sandbox, Ipv4::SOCKET_DOMAIN).await;

    let successful_preflights = udp_send_msg_preflight_fidl_setup::<Ipv4>(&iface, &socket).await;

    match invalidation_reason {
        UdpCacheInvalidationReasonV4::BroadcastCalled => {
            socket
                .set_broadcast(true)
                .await
                .expect("set_so_broadcast fidl error")
                .expect("failed to set so_broadcast");
        }
    }

    assert_preflights_invalidated(successful_preflights);
}

enum UdpCacheInvalidationReasonV6 {
    Ipv6OnlyCalled,
}

#[netstack_test]
#[variant(N, Netstack)]
#[test_case("ipv6_only_called", UdpCacheInvalidationReasonV6::Ipv6OnlyCalled)]
async fn udp_send_msg_preflight_fidl_v6only<N: Netstack>(
    root_name: &str,
    test_name: &str,
    invalidation_reason: UdpCacheInvalidationReasonV6,
) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm_name = format!("{}_{}", root_name, test_name);
    let (_net, _netstack, iface, socket) =
        setup_fastudp_network(&realm_name, N::VERSION, &sandbox, Ipv6::SOCKET_DOMAIN).await;

    let successful_preflights = udp_send_msg_preflight_fidl_setup::<Ipv6>(&iface, &socket).await;

    match invalidation_reason {
        UdpCacheInvalidationReasonV6::Ipv6OnlyCalled => {
            socket
                .set_ipv6_only(true)
                .await
                .expect("set_ipv6_only fidl error")
                .expect("failed to set ipv6 only");
        }
    }

    assert_preflights_invalidated(successful_preflights);
}

enum UdpCacheInvalidationReasonNdp {
    RouterAdvertisement,
    RouterAdvertisementWithPrefix,
}

#[netstack_test]
#[variant(N, Netstack)]
#[test_case("ra", UdpCacheInvalidationReasonNdp::RouterAdvertisement)]
#[test_case("ra_with_prefix", UdpCacheInvalidationReasonNdp::RouterAdvertisementWithPrefix)]
async fn udp_send_msg_preflight_fidl_ndp<N: Netstack>(
    root_name: &str,
    test_name: &str,
    invalidation_reason: UdpCacheInvalidationReasonNdp,
) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm_name = format!("{}_{}", root_name, test_name);
    let (net, realm, iface, socket) =
        setup_fastudp_network(&realm_name, N::VERSION, &sandbox, Ipv6::SOCKET_DOMAIN).await;
    let fake_ep = net.create_fake_endpoint().expect("create fake endpoint");

    let successful_preflights = udp_send_msg_preflight_fidl_setup::<Ipv6>(&iface, &socket).await;

    // Note that the following prefix must not overlap with
    // `<Ipv6 as UdpSendMsgPreflightTestIpExt>::INSTALLED_ADDR`, as there is already a subnet
    // route for the installed addr and so discovering the same prefix will not cause a route
    // to be added and induce cache invalidation.
    const PREFIX: net_types::ip::Subnet<net_types::ip::Ipv6Addr> =
        net_subnet_v6!("2001:db8:ffff:ffff::/64");
    const SOCKADDR_IN_PREFIX: fnet::SocketAddress =
        fidl_socket_addr!("[2001:db8:ffff:ffff::1]:9999");

    // These are arbitrary large lifetime values so that the information
    // contained within the RA are not deprecated/invalidated over the course
    // of the test.
    const LARGE_ROUTER_LIFETIME: u16 = 9000;
    const LARGE_PREFIX_LIFETIME: u32 = 99999;
    async fn send_ra(
        fake_ep: &netemul::TestFakeEndpoint<'_>,
        router_lifetime: u16,
        prefix_lifetime: Option<u32>,
    ) {
        let options = prefix_lifetime
            .into_iter()
            .map(|lifetime| {
                NdpOptionBuilder::PrefixInformation(PrefixInformation::new(
                    PREFIX.prefix(),  /* prefix_length */
                    true,             /* on_link_flag */
                    true,             /* autonomous_address_configuration_flag */
                    lifetime,         /* valid_lifetime */
                    lifetime,         /* preferred_lifetime */
                    PREFIX.network(), /* prefix */
                ))
            })
            .collect::<Vec<_>>();
        ndp::send_ra_with_router_lifetime(
            &fake_ep,
            router_lifetime,
            &options,
            ipv6_consts::LINK_LOCAL_ADDR,
        )
        .await
        .expect("failed to fake RA message");
    }
    fn route_found(
        fnet_routes_ext::InstalledRoute {
            route: fnet_routes_ext::Route { destination, action, properties: _ },
            effective_properties: _,
            table_id: _,
        }: fnet_routes_ext::InstalledRoute<Ipv6>,
        want: net_types::ip::Subnet<net_types::ip::Ipv6Addr>,
        interface_id: u64,
    ) -> bool {
        let route_found = destination == want;
        if route_found {
            assert_eq!(
                action,
                fnet_routes_ext::RouteAction::Forward(fnet_routes_ext::RouteTarget {
                    outbound_interface: interface_id,
                    next_hop: None,
                }),
            );
        }
        route_found
    }
    let routes_state = realm
        .connect_to_protocol::<fnet_routes::StateV6Marker>()
        .expect("connect to route state FIDL");
    let event_stream = fnet_routes_ext::event_stream_from_state::<Ipv6>(&routes_state)
        .expect("routes event stream from state");
    let mut event_stream = pin!(event_stream);
    let mut routes = std::collections::HashSet::new();

    match invalidation_reason {
        // Send a RA message with an arbitrarily chosen but large router lifetime value to
        // indicate to Netstack that a router is present. Netstack will add a default route,
        // and invalidate the cache.
        UdpCacheInvalidationReasonNdp::RouterAdvertisement => {
            send_ra(&fake_ep, LARGE_ROUTER_LIFETIME, None /* prefix_lifetime */).await;

            // Wait until a default IPv6 route is added in response to the RA.
            let mut interface_state =
                fnet_interfaces_ext::InterfaceState::<(), _>::Unknown(iface.id());
            fnet_interfaces_ext::wait_interface_with_id(
                realm.get_interface_event_stream().expect("get interface event stream"),
                &mut interface_state,
                |iface| iface.properties.has_default_ipv6_route.then_some(()),
            )
            .await
            .expect("failed to wait for default IPv6 route");
        }
        // Send a RA message with router lifetime of 0 (otherwise the router information
        // also induces a default route and this test case tests a strict superset of the
        // `RouterAdvertisement` test case), but containing a prefix information option. Since
        // the prefix is on-link, Netstack will add a subnet route, and invalidate the cache.
        UdpCacheInvalidationReasonNdp::RouterAdvertisementWithPrefix => {
            send_ra(
                &fake_ep,
                0,                           /* router_lifetime */
                Some(LARGE_PREFIX_LIFETIME), /* prefix_lifetime */
            )
            .await;

            fnet_routes_ext::wait_for_routes(event_stream.by_ref(), &mut routes, |routes| {
                routes
                    .iter()
                    .any(|installed_route| route_found(*installed_route, PREFIX, iface.id()))
            })
            .await
            .expect("failed to wait for subnet route to appear");
        }
    }

    assert_preflights_invalidated(successful_preflights);

    // Note that `SOCKADDR_IN_PREFIX` is reachable in both cases because there
    // is either a route to the prefix or a default route.
    let successful_preflights = execute_and_validate_preflights(
        [SOCKADDR_IN_PREFIX, Ipv6::REACHABLE_ADDR1].into_iter().map(|socket_address| {
            UdpSendMsgPreflight {
                to_addr: Some(socket_address),
                expected_result: UdpSendMsgPreflightExpectation::Success(
                    UdpSendMsgPreflightSuccessExpectation {
                        expected_to_addr: ToAddrExpectation::Specified(None),
                        expect_all_eventpairs_valid: true,
                    },
                ),
            }
        }),
        &socket,
    )
    .await;

    match invalidation_reason {
        // Send an RA message invalidating the existence of the router, causing
        // the default route to be removed, and the cache to be invalidated.
        UdpCacheInvalidationReasonNdp::RouterAdvertisement => {
            send_ra(&fake_ep, 0 /* router_lifetime */, None /* prefix_lifetime */).await;

            // Wait until the default IPv6 route is removed.
            let mut interface_state =
                fnet_interfaces_ext::InterfaceState::<(), _>::Unknown(iface.id());
            fnet_interfaces_ext::wait_interface_with_id(
                realm.get_interface_event_stream().expect("get interface event stream"),
                &mut interface_state,
                |iface| (!iface.properties.has_default_ipv6_route).then_some(()),
            )
            .await
            .expect("failed to wait for default IPv6 route");
        }
        // Send an RA message invalidating the prefix, causing the subnet
        // route to be removed, and the cache to be invalidated.
        UdpCacheInvalidationReasonNdp::RouterAdvertisementWithPrefix => {
            let routes_state = realm
                .connect_to_protocol::<fnet_routes::StateV6Marker>()
                .expect("connect to route state FIDL");
            let event_stream = fnet_routes_ext::event_stream_from_state::<Ipv6>(&routes_state)
                .expect("routes event stream from state");
            let mut event_stream = pin!(event_stream);
            let _: Vec<_> = fnet_routes_ext::collect_routes_until_idle(event_stream.by_ref())
                .await
                .expect("collect routes until idle");

            send_ra(&fake_ep, 0 /* router_lifetime */, Some(0) /* prefix_lifetime */).await;

            fnet_routes_ext::wait_for_routes(event_stream, &mut routes, |routes| {
                routes
                    .iter()
                    .all(|installed_route| !route_found(*installed_route, PREFIX, iface.id()))
            })
            .await
            .expect("failed to wait for subnet route to disappear");
        }
    }

    assert_preflights_invalidated(successful_preflights);
}

async fn connect_socket_and_validate_preflight(
    socket: &fposix_socket::DatagramSocketProxy,
    addr: fnet::SocketAddress,
) -> fposix_socket::DatagramSocketSendMsgPreflightResponse {
    socket.connect(&addr).await.expect("call connect").expect("connect socket");

    let response = socket
        .send_msg_preflight(&fposix_socket::DatagramSocketSendMsgPreflightRequest::default())
        .await
        .expect("call send_msg_preflight")
        .expect("preflight check should succeed");

    validate_send_msg_preflight_response(
        &response,
        UdpSendMsgPreflightSuccessExpectation {
            expected_to_addr: ToAddrExpectation::Specified(Some(addr)),
            expect_all_eventpairs_valid: true,
        },
    )
    .expect("validate preflight response");

    response
}

async fn assert_preflight_response_invalidated(
    preflight: &fposix_socket::DatagramSocketSendMsgPreflightResponse,
) {
    async fn invoke_with_retries(
        retries: usize,
        delay: zx::MonotonicDuration,
        op: impl Fn() -> Result,
    ) -> Result {
        for _ in 0..retries {
            if let Ok(()) = op() {
                return Ok(());
            }
            fasync::Timer::new(delay).await;
        }
        op()
    }

    // NB: cache invalidation that results from internal state changes (such as
    // auto-generated address invalidation or DAD failure) is not guaranteed to
    // occur synchronously with the associated events emitted by the Netstack (such
    // as notification of address removal on the interface watcher or address state
    // provider). This means that the cache might not have been invalidated
    // immediately after observing the relevant emitted event.
    //
    // We avoid flakes due to this behavior by retrying multiple times with an
    // arbitrary delay.
    const RETRY_COUNT: usize = 3;
    const RETRY_DELAY: zx::MonotonicDuration = zx::MonotonicDuration::from_millis(500);
    let result = invoke_with_retries(RETRY_COUNT, RETRY_DELAY, || {
        validate_send_msg_preflight_response(
            &preflight,
            UdpSendMsgPreflightSuccessExpectation {
                expected_to_addr: ToAddrExpectation::Unspecified,
                expect_all_eventpairs_valid: false,
            },
        )
    })
    .await;
    assert_matches!(
        result,
        Ok(()),
        "failed to observe expected cache invalidation after auto-generated address was invalidated"
    );
}

#[netstack_test]
#[variant(N, Netstack)]
async fn udp_send_msg_preflight_autogen_addr_invalidation<N: Netstack>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let (net, netstack, iface, socket) =
        setup_fastudp_network(name, N::VERSION, &sandbox, fposix_socket::Domain::Ipv6).await;

    let interfaces_state = netstack
        .connect_to_protocol::<fnet_interfaces::StateMarker>()
        .expect("connect to protocol");

    // Send a Router Advertisement with the autoconf flag set to trigger
    // SLAAC, but specify a very short valid lifetime so the address
    // will expire quickly.
    let fake_ep = net.create_fake_endpoint().expect("create fake endpoint");
    // NB: we want this lifetime to be short so the test does not take too long
    // to run. However, if we make it too short, the test will be flaky, because
    // it's possible for the address lifetime to expire before the subsequent
    // SendMsgPreflight call.
    const VALID_LIFETIME_SECONDS: u32 = 10;
    let options = [NdpOptionBuilder::PrefixInformation(PrefixInformation::new(
        ipv6_consts::GLOBAL_PREFIX.prefix(),  /* prefix_length */
        false,                                /* on_link_flag */
        true,                                 /* autonomous_address_configuration_flag */
        VALID_LIFETIME_SECONDS,               /* valid_lifetime */
        0,                                    /* preferred_lifetime */
        ipv6_consts::GLOBAL_PREFIX.network(), /* prefix */
    ))];
    ndp::send_ra_with_router_lifetime(&fake_ep, 0, &options, ipv6_consts::LINK_LOCAL_ADDR)
        .await
        .expect("send router advertisement");

    // Wait for an address to be auto generated.
    let autogen_address = fnet_interfaces_ext::wait_interface_with_id(
        fnet_interfaces_ext::event_stream_from_state::<fnet_interfaces_ext::DefaultInterest>(
            &interfaces_state,
            Default::default(),
        )
        .expect("create event stream"),
        &mut fnet_interfaces_ext::InterfaceState::<(), _>::Unknown(iface.id()),
        |iface| {
            iface.properties.addresses.iter().find_map(
                |fnet_interfaces_ext::Address {
                     addr: fnet::Subnet { addr, prefix_len: _ },
                     assignment_state,
                     ..
                 }| {
                    assert_eq!(
                        *assignment_state,
                        fnet_interfaces::AddressAssignmentState::Assigned
                    );
                    match addr {
                        fnet::IpAddress::Ipv4(_) => None,
                        fnet::IpAddress::Ipv6(addr @ fnet::Ipv6Address { addr: bytes }) => {
                            ipv6_consts::GLOBAL_PREFIX
                                .contains(&net_types::ip::Ipv6Addr::from_bytes(*bytes))
                                .then_some(*addr)
                        }
                    }
                },
            )
        },
    )
    .await
    .expect("wait for address assignment");

    let preflight = connect_socket_and_validate_preflight(
        &socket,
        fnet::SocketAddress::Ipv6(fnet::Ipv6SocketAddress {
            address: autogen_address,
            port: 9999, // arbitrary remote port
            zone_index: 0,
        }),
    )
    .await;

    // Wait for the address to be invalidated and removed.
    fnet_interfaces_ext::wait_interface_with_id(
        fnet_interfaces_ext::event_stream_from_state::<fnet_interfaces_ext::DefaultInterest>(
            &interfaces_state,
            Default::default(),
        )
        .expect("create event stream"),
        &mut fnet_interfaces_ext::InterfaceState::<(), _>::Unknown(iface.id()),
        |iface| {
            (!iface.properties.addresses.iter().any(
                |fnet_interfaces_ext::Address {
                     addr: fnet::Subnet { addr, prefix_len: _ },
                     assignment_state,
                     ..
                 }| {
                    assert_eq!(
                        *assignment_state,
                        fnet_interfaces::AddressAssignmentState::Assigned
                    );
                    match addr {
                        fnet::IpAddress::Ipv4(_) => false,
                        fnet::IpAddress::Ipv6(addr) => addr == &autogen_address,
                    }
                },
            ))
            .then_some(())
        },
    )
    .await
    .expect("wait for address removal");

    assert_preflight_response_invalidated(&preflight).await;

    // Now that the address has been invalidated and removed, subsequent calls to
    // preflight using the connected address should fail.
    let result = socket
        .send_msg_preflight(&fposix_socket::DatagramSocketSendMsgPreflightRequest {
            to: None,
            ..Default::default()
        })
        .await
        .expect("call send_msg_preflight");
    assert_eq!(result, Err(fposix::Errno::Ehostunreach));
}

#[netstack_test]
#[variant(N, Netstack)]
async fn udp_send_msg_preflight_dad_failure<N: Netstack>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let (net, _netstack, iface, socket) =
        setup_fastudp_network(name, N::VERSION, &sandbox, fposix_socket::Domain::Ipv6).await;

    let preflight = connect_socket_and_validate_preflight(
        &socket,
        fnet_ext::SocketAddress((std::net::Ipv6Addr::LOCALHOST, 9999).into()).into(),
    )
    .await;

    // Create the fake endpoint before adding an address to the netstack to ensure
    // that we receive all NDP messages sent by the client.
    let fake_ep = net.create_fake_endpoint().expect("create fake endpoint");

    let (address_state_provider, server) =
        fidl::endpoints::create_proxy::<fnet_interfaces_admin::AddressStateProviderMarker>();
    // Create the state stream before adding the address to ensure that all
    // generated events are observed.
    let state_stream = fnet_interfaces_ext::admin::assignment_state_stream(address_state_provider);
    iface
        .control()
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
        .expect("call add address");

    // Expect the netstack to send a DAD message, and simulate another node already
    // owning the address. Expect DAD to fail as a result.
    let _: Vec<u8> = ndp::expect_dad_neighbor_solicitation(&fake_ep).await;
    ndp::fail_dad_with_na(&fake_ep).await;
    ndp::assert_dad_failed(state_stream).await;

    assert_preflight_response_invalidated(&preflight).await;
}

#[derive(Clone, Copy, PartialEq)]
enum CmsgType {
    IpTos,
    IpTtl,
    Ipv6Tclass,
    Ipv6Hoplimit,
    Ipv6PktInfo,
    SoTimestamp,
    SoTimestampNs,
}

struct RequestedCmsgSetExpectation {
    requested_cmsg_type: Option<CmsgType>,
    valid: bool,
}

fn validate_recv_msg_postflight_response(
    response: &fposix_socket::DatagramSocketRecvMsgPostflightResponse,
    expectation: RequestedCmsgSetExpectation,
) {
    let fposix_socket::DatagramSocketRecvMsgPostflightResponse {
        validity,
        requests,
        timestamp,
        ..
    } = response;
    let RequestedCmsgSetExpectation { valid, requested_cmsg_type } = expectation;
    let cmsg_expected =
        |cmsg_type| requested_cmsg_type.is_some_and(|req_type| req_type == cmsg_type);

    use fposix_socket::{CmsgRequests, TimestampOption};

    let bits_cmsg_requested = |cmsg_type| {
        !(requests.unwrap_or_else(|| CmsgRequests::from_bits_allow_unknown(0)) & cmsg_type)
            .is_empty()
    };

    assert_eq!(bits_cmsg_requested(CmsgRequests::IP_TOS), cmsg_expected(CmsgType::IpTos));
    assert_eq!(bits_cmsg_requested(CmsgRequests::IP_TTL), cmsg_expected(CmsgType::IpTtl));
    assert_eq!(bits_cmsg_requested(CmsgRequests::IPV6_TCLASS), cmsg_expected(CmsgType::Ipv6Tclass));
    assert_eq!(
        bits_cmsg_requested(CmsgRequests::IPV6_HOPLIMIT),
        cmsg_expected(CmsgType::Ipv6Hoplimit)
    );
    assert_eq!(
        bits_cmsg_requested(CmsgRequests::IPV6_PKTINFO),
        cmsg_expected(CmsgType::Ipv6PktInfo)
    );
    assert_eq!(
        *timestamp == Some(TimestampOption::Nanosecond),
        cmsg_expected(CmsgType::SoTimestampNs)
    );
    assert_eq!(
        *timestamp == Some(TimestampOption::Microsecond),
        cmsg_expected(CmsgType::SoTimestamp)
    );

    let expected_validity =
        if valid { Err(zx::Status::TIMED_OUT) } else { Ok(zx::Signals::EVENTPAIR_PEER_CLOSED) };
    let validity = validity.as_ref().expect("expected validity present");
    assert_eq!(
        validity
            .wait_one(zx::Signals::EVENTPAIR_PEER_CLOSED, zx::MonotonicInstant::INFINITE_PAST)
            .to_result(),
        expected_validity,
    );
}

async fn toggle_cmsg(
    requested: bool,
    proxy: &fposix_socket::DatagramSocketProxy,
    cmsg_type: CmsgType,
) {
    match cmsg_type {
        CmsgType::IpTos => {
            proxy
                .set_ip_receive_type_of_service(requested)
                .await
                .expect("set_ip_receive_type_of_service fidl error")
                .expect("set_ip_receive_type_of_service failed");
        }
        CmsgType::IpTtl => {
            proxy
                .set_ip_receive_ttl(requested)
                .await
                .expect("set_ip_receive_ttl fidl error")
                .expect("set_ip_receive_ttl failed");
        }
        CmsgType::Ipv6Tclass => {
            proxy
                .set_ipv6_receive_traffic_class(requested)
                .await
                .expect("set_ipv6_receive_traffic_class fidl error")
                .expect("set_ipv6_receive_traffic_class failed");
        }
        CmsgType::Ipv6Hoplimit => {
            proxy
                .set_ipv6_receive_hop_limit(requested)
                .await
                .expect("set_ipv6_receive_hop_limit fidl error")
                .expect("set_ipv6_receive_hop_limit failed");
        }
        CmsgType::Ipv6PktInfo => {
            proxy
                .set_ipv6_receive_packet_info(requested)
                .await
                .expect("set_ipv6_receive_packet_info fidl error")
                .expect("set_ipv6_receive_packet_info failed");
        }
        CmsgType::SoTimestamp => {
            let option = if requested {
                fposix_socket::TimestampOption::Microsecond
            } else {
                fposix_socket::TimestampOption::Disabled
            };
            proxy
                .set_timestamp(option)
                .await
                .expect("set_timestamp fidl error")
                .expect("set_timestamp failed");
        }
        CmsgType::SoTimestampNs => {
            let option = if requested {
                fposix_socket::TimestampOption::Nanosecond
            } else {
                fposix_socket::TimestampOption::Disabled
            };
            proxy
                .set_timestamp(option)
                .await
                .expect("set_timestamp fidl error")
                .expect("set_timestamp failed");
        }
    }
}

#[netstack_test]
#[variant(N, Netstack)]
#[test_case("ip_tos", CmsgType::IpTos)]
#[test_case("ip_ttl", CmsgType::IpTtl)]
#[test_case("ipv6_tclass", CmsgType::Ipv6Tclass)]
#[test_case("ipv6_hoplimit", CmsgType::Ipv6Hoplimit)]
#[test_case("ipv6_pktinfo", CmsgType::Ipv6PktInfo)]
#[test_case("so_timestamp_ns", CmsgType::SoTimestampNs)]
#[test_case("so_timestamp", CmsgType::SoTimestamp)]
async fn udp_recv_msg_postflight_fidl<N: Netstack>(
    root_name: &str,
    test_name: &str,
    cmsg_type: CmsgType,
) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let version = match N::VERSION {
        NetstackVersion::Netstack2 { tracing, fast_udp: _ } => {
            NetstackVersion::Netstack2 { tracing, fast_udp: true }
        }
        version => version,
    };
    let netstack = sandbox
        .create_realm(
            format!("{}_{}", root_name, test_name),
            [KnownServiceProvider::Netstack(version)],
        )
        .expect("failed to create netstack realm");

    let socket_provider = netstack
        .connect_to_protocol::<fposix_socket::ProviderMarker>()
        .expect("failed to connect to socket provider");

    let datagram_socket = socket_provider
        .datagram_socket(fposix_socket::Domain::Ipv4, fposix_socket::DatagramSocketProtocol::Udp)
        .await
        .expect("datagram_socket fidl error")
        .expect("failed to create datagram socket");

    let datagram_socket = match datagram_socket {
        fposix_socket::ProviderDatagramSocketResponse::DatagramSocket(socket) => socket,
        socket => panic!("unexpected datagram socket variant: {:?}", socket),
    };

    let proxy = datagram_socket.into_proxy();

    // Expect no cmsgs requested by default.
    let response = proxy
        .recv_msg_postflight()
        .await
        .expect("recv_msg_postflight fidl error")
        .expect("recv_msg_postflight failed");
    validate_recv_msg_postflight_response(
        &response,
        RequestedCmsgSetExpectation { requested_cmsg_type: None, valid: true },
    );

    toggle_cmsg(true, &proxy, cmsg_type).await;

    // Expect requesting a cmsg invalidates the returned cmsg set.
    validate_recv_msg_postflight_response(
        &response,
        RequestedCmsgSetExpectation { requested_cmsg_type: None, valid: false },
    );

    // Expect the cmsg is returned in the latest requested set.
    let response = proxy
        .recv_msg_postflight()
        .await
        .expect("recv_msg_postflight fidl error")
        .expect("recv_msg_postflight failed");
    validate_recv_msg_postflight_response(
        &response,
        RequestedCmsgSetExpectation { requested_cmsg_type: Some(cmsg_type), valid: true },
    );

    toggle_cmsg(false, &proxy, cmsg_type).await;

    // Expect unrequesting a cmsg invalidates the returned cmsg set.
    validate_recv_msg_postflight_response(
        &response,
        RequestedCmsgSetExpectation { requested_cmsg_type: Some(cmsg_type), valid: false },
    );

    // Expect the cmsg is no longer returned in the latest requested set.
    let response = proxy
        .recv_msg_postflight()
        .await
        .expect("recv_msg_postflight fidl error")
        .expect("recv_msg_postflight failed");
    validate_recv_msg_postflight_response(
        &response,
        RequestedCmsgSetExpectation { requested_cmsg_type: None, valid: true },
    );
}

#[netstack_test]
#[variant(N, Netstack)]
async fn udp_sendto_unroutable_leaves_socket_bound<N: Netstack>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let network = sandbox.create_network("net").await.expect("failed to create network");
    let realm = sandbox.create_netstack_realm::<N, _>(name).expect("create realm");
    let interface = realm.join_network(&network, "stack").await.expect("join network failed");
    interface
        .add_address_and_subnet_route(fidl_subnet!("192.168.1.10/16"))
        .await
        .expect("configure address");

    let socket = realm
        .datagram_socket(fposix_socket::Domain::Ipv4, fposix_socket::DatagramSocketProtocol::Udp)
        .await
        .and_then(|d| DatagramSocket::new_from_socket(d).map_err(Into::into))
        .expect("create UDP datagram socket");

    let addr = std_socket_addr!("8.8.8.8:8080");
    let buf = [0; 8];
    let send_result = socket
        .send_to(&buf, addr.into())
        .await
        .map_err(|e| e.raw_os_error().and_then(fposix::Errno::from_primitive));
    assert_eq!(
        send_result,
        Err(Some(if N::VERSION == NetstackVersion::Netstack3 {
            // TODO(https://fxbug.dev/42051708): Figure out what code is expected
            // here and make Netstack2 and Netstack3 return codes consistent.
            fposix::Errno::Enetunreach
        } else {
            fposix::Errno::Ehostunreach
        }))
    );

    let bound_addr = socket.local_addr().expect("should be bound");
    let bound_ipv4 = bound_addr.as_socket_ipv4().expect("must be IPv4");
    assert_eq!(bound_ipv4.ip(), &std_ip_v4!("0.0.0.0"));
    assert_ne!(bound_ipv4.port(), 0);
}

#[netstack_test]
#[variant(N, Netstack)]
async fn udp_receive_on_bound_to_devices<N: Netstack>(name: &str) {
    const NUM_PEERS: u8 = 3;
    const PORT: u16 = 80;
    const BUFFER_SIZE: usize = 1024;
    crate::with_multinic_and_peers::<N, UdpSocket, Ipv4, _, _>(
        name,
        NUM_PEERS,
        net_subnet_v4!("192.168.0.0/16"),
        PORT,
        |multinic_and_peers| async move {
            // Now send traffic from the peer to the addresses for each of the multinic
            // NICs. The traffic should come in on the correct sockets.

            futures::stream::iter(multinic_and_peers.iter())
                .for_each_concurrent(
                    None,
                    |MultiNicAndPeerConfig {
                         peer_socket,
                         multinic_ip,
                         peer_ip,
                         multinic_socket: _,
                     }| async move {
                        let buf = peer_ip.to_string();
                        let addr = (*multinic_ip, PORT).into();
                        assert_eq!(
                            peer_socket.send_to(buf.as_bytes(), addr).await.expect("send failed"),
                            buf.len()
                        );
                    },
                )
                .await;

            futures::stream::iter(multinic_and_peers.into_iter())
                .for_each_concurrent(
                    None,
                    |MultiNicAndPeerConfig {
                         multinic_socket,
                         peer_ip,
                         multinic_ip: _,
                         peer_socket: _,
                     }| async move {
                        let mut buffer = [0u8; BUFFER_SIZE];
                        let (len, send_addr) =
                            multinic_socket.recv_from(&mut buffer).await.expect("recv_from failed");

                        assert_eq!(send_addr, (peer_ip, PORT).into());
                        // The received packet should contain the IP address of the
                        // sending interface, which is also the source address.
                        let expected = peer_ip.to_string();
                        assert_eq!(len, expected.len());
                        assert_eq!(&buffer[..len], expected.as_bytes());
                    },
                )
                .await
        },
    )
    .await
}

#[netstack_test]
#[variant(N, Netstack)]
async fn udp_send_from_bound_to_device<N: Netstack>(name: &str) {
    const NUM_PEERS: u8 = 3;
    const PORT: u16 = 80;
    const BUFFER_SIZE: usize = 1024;

    crate::with_multinic_and_peers::<N, UdpSocket, Ipv4, _, _>(
        name,
        NUM_PEERS,
        net_subnet_v4!("192.168.0.0/16"),
        PORT,
        |configs| async move {
            // Now send traffic from each of the multinic sockets to the
            // corresponding peer. The traffic should be sent from the address
            // corresponding to each socket's bound device.
            futures::stream::iter(configs.iter())
                .for_each_concurrent(
                    None,
                    |MultiNicAndPeerConfig {
                         multinic_ip,
                         multinic_socket,
                         peer_ip,
                         peer_socket: _,
                     }| async move {
                        let peer_addr = (*peer_ip, PORT).into();
                        let buf = multinic_ip.to_string();
                        assert_eq!(
                            multinic_socket
                                .send_to(buf.as_bytes(), peer_addr)
                                .await
                                .expect("send failed"),
                            buf.len()
                        );
                    },
                )
                .await;

            futures::stream::iter(configs)
            .for_each(
                |MultiNicAndPeerConfig {
                     peer_socket,
                     peer_ip: _,
                     multinic_ip: _,
                     multinic_socket: _,
                 }| async move {
                    let mut buffer = [0u8; BUFFER_SIZE];
                    let (len, source_addr) =
                        peer_socket.recv_from(&mut buffer).await.expect("recv_from failed");
                    let source_ip =
                        assert_matches!(source_addr, std::net::SocketAddr::V4(addr) => *addr.ip());
                    // The received packet should contain the IP address of the interface.
                    let expected = source_ip.to_string();
                    assert_eq!(len, expected.len());
                    assert_eq!(&buffer[..expected.len()], expected.as_bytes());
                },
            )
            .await;
        },
    )
    .await
}

#[netstack_test]
#[variant(N, Netstack)]
async fn test_udp_source_address_has_zone<N: Netstack>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let net = sandbox.create_network("net").await.expect("failed to create network");

    let client = sandbox
        .create_netstack_realm::<N, _>(format!("{}_client", name))
        .expect("failed to create client realm");
    let server = sandbox
        .create_netstack_realm::<N, _>(format!("{}_server", name))
        .expect("failed to create server realm");

    let client_ep = client
        .join_network_with(
            &net,
            "client",
            netemul::new_endpoint_config(netemul::DEFAULT_MTU, Some(CLIENT_MAC)),
            Default::default(),
        )
        .await
        .expect("client failed to join network");
    client_ep.add_address_and_subnet_route(Ipv6::CLIENT_SUBNET).await.expect("configure address");
    client_ep.apply_nud_flake_workaround().await.expect("apply NUD flake workaround");
    let server_ep = server
        .join_network_with(
            &net,
            "server",
            netemul::new_endpoint_config(netemul::DEFAULT_MTU, Some(SERVER_MAC)),
            Default::default(),
        )
        .await
        .expect("server failed to join network");
    server_ep.add_address_and_subnet_route(Ipv6::SERVER_SUBNET).await.expect("configure address");
    server_ep.apply_nud_flake_workaround().await.expect("apply NUD flake workaround");

    // Get the link local address for the client.
    let link_local_addr = std::pin::pin!(
        client.get_interface_event_stream().expect("get_interface_event_stream failed").filter_map(
            |event| async {
                match event.expect("event error").into_inner() {
                    fnet_interfaces::Event::Existing(properties)
                    | fnet_interfaces::Event::Added(properties) => {
                        if let Some(addresses) = properties.addresses {
                            for address in addresses {
                                if let Some(fnet::Subnet {
                                    addr: fnet::IpAddress::Ipv6(addr),
                                    ..
                                }) = address.addr
                                {
                                    if addr.is_unicast_link_local() {
                                        return Some(addr);
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
                None
            }
        )
    )
    .next()
    .await
    .expect("unexpected end of events");

    let client_addr = assert_matches!(fnet::IpAddress::Ipv6(link_local_addr).into(),
                                      fnet_ext::IpAddress(std::net::IpAddr::V6(client_addr)) => client_addr);
    let client_addr = std::net::SocketAddr::V6(std::net::SocketAddrV6::new(
        client_addr,
        1234,
        0,
        client_ep.id().try_into().unwrap(),
    ));

    let fnet_ext::IpAddress(server_addr) = fnet_ext::IpAddress::from(Ipv6::SERVER_SUBNET.addr);
    let server_addr = std::net::SocketAddr::new(server_addr, 8080);

    let client_sock = fasync::net::UdpSocket::bind_in_realm(&client, client_addr)
        .await
        .expect("failed to create client socket");

    let server_sock = fasync::net::UdpSocket::bind_in_realm(&server, server_addr)
        .await
        .expect("failed to create server socket");

    const PAYLOAD: &'static str = "Hello World";

    let client_fut = async move {
        let r = client_sock.send_to(PAYLOAD.as_bytes(), server_addr).await.expect("sendto failed");
        assert_eq!(r, PAYLOAD.as_bytes().len());
    };
    let server_fut = async move {
        let mut buf = [0u8; 1024];
        let (_, from) = server_sock.recv_from(&mut buf[..]).await.expect("recvfrom failed");
        // This will also check the zone.
        assert_eq!(from, client_addr);
    };

    let ((), ()) = futures::future::join(client_fut, server_fut).await;
}

#[netstack_test]
#[variant(N, Netstack)]
async fn get_bound_device_errors_after_device_deleted<N: Netstack>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let net = sandbox.create_network("net").await.expect("failed to create network");

    let host = sandbox.create_netstack_realm::<N, _>(format!("{name}_host")).expect("create realm");

    let bound_interface =
        host.join_network(&net, "bound-device").await.expect("host failed to join network");
    bound_interface
        .add_address_and_subnet_route(fidl_subnet!("192.168.0.1/16"))
        .await
        .expect("configure address");

    let host_sock =
        fasync::net::UdpSocket::bind_in_realm(&host, (std::net::Ipv4Addr::UNSPECIFIED, 0).into())
            .await
            .expect("failed to create host socket");

    host_sock
        .bind_device(Some(
            bound_interface.get_interface_name().await.expect("get_name failed").as_bytes(),
        ))
        .expect("set SO_BINDTODEVICE");

    let id = bound_interface.id();

    let interface_state =
        host.connect_to_protocol::<fnet_interfaces::StateMarker>().expect("connect to protocol");

    let stream =
        fnet_interfaces_ext::event_stream_from_state::<fnet_interfaces_ext::DefaultInterest>(
            &interface_state,
            Default::default(),
        )
        .expect("error getting interface state event stream");
    let mut stream = pin!(stream);
    let mut state =
        std::collections::HashMap::<u64, fnet_interfaces_ext::PropertiesAndState<(), _>>::new();

    // Wait for the interface to be present.
    fnet_interfaces_ext::wait_interface(stream.by_ref(), &mut state, |interfaces| {
        interfaces.get(&id).map(|_| ())
    })
    .await
    .expect("waiting for interface addition");

    let (_endpoint, _device_control) =
        bound_interface.remove().await.expect("failed to remove interface");

    // Wait for the interface to be removed.
    fnet_interfaces_ext::wait_interface(stream, &mut state, |interfaces| {
        interfaces.get(&id).is_none().then(|| ())
    })
    .await
    .expect("waiting interface removal");

    let bound_device =
        host_sock.device().map_err(|e| e.raw_os_error().and_then(fposix::Errno::from_primitive));
    assert_eq!(bound_device, Err(Some(fposix::Errno::Enodev)));
}

#[netstack_test]
#[variant(N, Netstack)]
async fn send_to_remote_with_zone<N: Netstack>(name: &str) {
    const PORT: u16 = 80;
    const NUM_BYTES: usize = 10;

    async fn make_socket(realm: &netemul::TestRealm<'_>) -> fasync::net::UdpSocket {
        fasync::net::UdpSocket::bind_in_realm(realm, (std::net::Ipv6Addr::UNSPECIFIED, PORT).into())
            .await
            .expect("failed to create socket")
    }

    crate::with_multinic_and_peer_networks::<N, net_types::ip::Ipv6, _>(
        name,
        2,
        net_types::ip::Ipv6::LINK_LOCAL_UNICAST_SUBNET,
        |networks, multinic, ()| {
            Box::pin(async move {
                let networks_and_peer_sockets =
                    future::join_all(networks.iter().map(|network| async move {
                        let Network { peer_realm, peer_interface, _network, multinic_interface } =
                            network;
                        let Interface { iface: _, ip: peer_ip } = peer_interface;
                        let peer_socket = make_socket(&peer_realm).await;
                        (multinic_interface, (peer_socket, *peer_ip))
                    }))
                    .await;

                let host_sock = make_socket(&multinic).await;
                let host_sock = &host_sock;

                let _: Vec<()> = future::join_all(networks_and_peer_sockets.iter().map(
                    |(multinic_interface, (peer_socket, peer_ip))| async move {
                        let Interface { iface: interface, ip: _ } = multinic_interface;
                        let id: u8 = interface.id().try_into().unwrap();
                        assert_eq!(
                            host_sock
                                .send_to(
                                    &[id; NUM_BYTES],
                                    std::net::SocketAddrV6::new(
                                        peer_ip.clone().into(),
                                        PORT,
                                        0,
                                        id.into()
                                    )
                                    .into(),
                                )
                                .await
                                .expect("send should succeed"),
                            NUM_BYTES
                        );

                        let mut buf = [0; NUM_BYTES + 1];
                        let (bytes, _sender) =
                            peer_socket.recv_from(&mut buf).await.expect("recv succeeds");
                        assert_eq!(bytes, NUM_BYTES);
                        assert_eq!(&buf[..NUM_BYTES], &[id; NUM_BYTES]);
                    },
                ))
                .await;
            })
        },
    )
    .await
}

#[netstack_test]
#[variant(N, Netstack)]
#[variant(I, Ip)]
#[test_case(0)]
#[test_case(1)]
async fn multicast_send<N: Netstack, I: MulticastTestIpExt>(name: &str, target_interface: usize) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let client = sandbox
        .create_netstack_realm::<N, _>(format!("{name}_client"))
        .expect("failed to create client realm");
    let networks = crate::init_multicast_test_networks::<I>(&sandbox, &client).await;

    let sock = client
        .datagram_socket(I::DOMAIN, fposix_socket::DatagramSocketProtocol::Udp)
        .await
        .expect("failed to create socket");

    match I::VERSION {
        IpVersion::V4 => {
            let addr = match I::NETWORKS[target_interface].addr {
                fnet::IpAddress::Ipv4(a) => a.addr.into(),
                fnet::IpAddress::Ipv6(_) => unreachable!("NETWORKS expected to be Ipv4"),
            };
            sock.set_multicast_if_v4(&addr).expect("failed to set IP_MULTICAST_IF")
        }
        IpVersion::V6 => sock
            .set_multicast_if_v6(networks[target_interface].iface.id().try_into().unwrap())
            .expect("failed to set IPV6_MULTICAST_IF"),
    };

    let _ = sock
        .send_to(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12], &I::MCAST_ADDR.into())
        .expect("failed to send multicast packet");

    // Check that the packet is sent to the selected network.
    for (index, network) in networks.iter().enumerate() {
        let mut stream = std::pin::pin!(
            network.receiver.frame_stream().map(|r| r.expect("failed to read frame")).filter_map(
                |(data, dropped)| async move {
                    assert_eq!(dropped, 0);
                    let (_payload, _src_mac, _dst_mac, _src_ip, dst_ip, proto, _ttl) =
                        match packet_formats::testutil::parse_ip_packet_in_ethernet_frame::<I>(
                            &data[..],
                            EthernetFrameLengthCheck::NoCheck,
                        ) {
                            Ok(result) => result,
                            Err(_e) => {
                                // Packet may fail to parse if it was for a
                                // different IP version. Just skip it.
                                return None;
                            }
                        };

                    if proto != IpProto::Udp.into() {
                        return None;
                    }

                    if dst_ip.to_ip_addr() != I::MCAST_ADDR.ip().into() {
                        panic!("UDP Packet send to an unexpected address: {:?}", dst_ip);
                    }

                    Some(())
                }
            )
        );

        if index == target_interface {
            // Check that the packet is delivered to the target interface.
            stream
                .next()
                .on_timeout(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT.after_now(), || {
                    panic!("timed out waiting for the multicast packet")
                })
                .await
                .expect("didn't receive the packet before end of the stream");
        } else {
            // Check that the packet is not sent to the other interface.
            stream
                .next()
                .map(|_| panic!("MulticastPacket was sent to a wrong interface"))
                .on_timeout(ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT.after_now(), || ())
                .await;
        }
    }
}

#[netstack_test]
#[variant(N, Netstack)]
#[variant(I, Ip)]
#[test_case(None, 0, false)]
#[test_case(Some(true), 0, false)]
#[test_case(Some(true), 1, true)]
#[test_case(Some(false), 0, false)]
#[test_case(Some(false), 1, true)]
async fn multicast_loop<N: Netstack, I: MulticastTestIpExt>(
    name: &str,
    multicast_loop_value: Option<bool>,
    target_interface: usize,
    dual_stack: bool,
) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let client = sandbox
        .create_netstack_realm::<N, _>(format!("{name}_client"))
        .expect("failed to create client realm");

    let networks = crate::init_multicast_test_networks::<I>(&sandbox, &client).await;

    // Initialize send socket to send the packet on the `target_interface`.
    let send_socket = client
        .datagram_socket(
            if dual_stack { Ipv6::DOMAIN } else { I::DOMAIN },
            fposix_socket::DatagramSocketProtocol::Udp,
        )
        .await
        .expect("failed to create UDP socket");

    match I::VERSION {
        IpVersion::V4 => {
            let addr = match I::NETWORKS[target_interface].addr {
                fnet::IpAddress::Ipv4(a) => a.addr.into(),
                fnet::IpAddress::Ipv6(_) => unreachable!("NETWORKS expected to be Ipv4"),
            };
            send_socket.set_multicast_if_v4(&addr).expect("failed to set IP_MULTICAST_IF");
            if let Some(value) = multicast_loop_value {
                send_socket.set_multicast_loop_v4(value).expect("failed to set IP_MULTICAST_LOOP");
            }
        }
        IpVersion::V6 => {
            let iface_id = networks[target_interface].iface.id().try_into().unwrap();
            send_socket.set_multicast_if_v6(iface_id).expect("Failed to set IPV6_MULTICAST_LOOP");
            if let Some(value) = multicast_loop_value {
                send_socket
                    .set_multicast_loop_v6(value)
                    .expect("failed to set IPV6_MULTICAST_LOOP");

                // Set the IPv4 option to the reverse value. It's expected to
                // have no effect on IPv6 packets. NS2 doesn't implement this
                // correctly, so we only set this option in NS3.
                if N::VERSION == NetstackVersion::Netstack3 {
                    send_socket
                        .set_multicast_loop_v4(!value)
                        .expect("failed to set IP_MULTICAST_LOOP");
                }
            }
        }
    };

    // Create one socket per interface and join the same multicast group from each.
    let recv_sockets = future::join_all(networks.iter().map(|network| async {
        let recv_socket = client
            .datagram_socket(I::DOMAIN, fposix_socket::DatagramSocketProtocol::Udp)
            .await
            .expect("failed to create socket");
        recv_socket
            .bind_device(Some(
                network
                    .iface
                    .get_interface_name()
                    .await
                    .expect("get_interface_name failed")
                    .as_bytes(),
            ))
            .expect("failed to bind socket to an interface");
        recv_socket.bind(&I::MCAST_ADDR.into()).expect("failed to bind UDP socket");

        let iface_id = network.iface.id().try_into().unwrap();
        match I::MCAST_ADDR.ip() {
            std::net::IpAddr::V4(addr_v4) => recv_socket
                .join_multicast_v4_n(&addr_v4.into(), &InterfaceIndexOrAddress::Index(iface_id))
                .expect("failed to join multicast group"),
            std::net::IpAddr::V6(addr_v6) => recv_socket
                .join_multicast_v6(&addr_v6.into(), iface_id)
                .expect("failed to join multicast group"),
        }
        fasync::net::UdpSocket::from_socket(recv_socket.into()).unwrap()
    }))
    .await;

    // IP_MULTICAST_LOOP should be enabled if not set explicitly.
    let multicast_loop_value = multicast_loop_value.unwrap_or(true);

    let data = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
    assert_eq!(
        send_socket.send_to(&data, &I::MCAST_ADDR.into()).expect("failed to send multicast packet"),
        data.len()
    );

    // Check that the packet is delivered where it's expected.
    for (i, recv_socket) in recv_sockets.iter().enumerate() {
        let mut buf = [0u8; 200];
        let recv_fut = recv_socket.recv_from(&mut buf);
        let packet_expected = multicast_loop_value && i == target_interface;
        if packet_expected {
            let (size, addr) = recv_fut
                .on_timeout(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT, || {
                    Err(std::io::ErrorKind::TimedOut.into())
                })
                .await
                .expect("recv_from failed");
            assert_eq!(size, data.len());
            assert_eq!(&buf[..size], &data[..]);
            assert_eq!(addr.ip(), I::iface_ip(i));
        } else {
            recv_fut
                .map(|output| panic!("unexpected received packet {output:?}"))
                .on_timeout(ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT, || ())
                .await;
        }
    }
}

#[netstack_test]
#[variant(N, Netstack)]
#[variant(I, Ip)]
#[test_case(true)]
#[test_case(false)]
async fn multicast_loop_on_loopback_dev<N: Netstack, I: MulticastTestIpExt>(
    name: &str,
    multicast_loop_value: bool,
) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let client = sandbox
        .create_netstack_realm::<N, _>(format!("{name}_client"))
        .expect("failed to create client realm");

    let loopback_id: u32 =
        client.loopback_properties().await.unwrap().unwrap().id.get().try_into().unwrap();

    // Initialize send socket to send the packet on the `target_interface`.
    let send_socket = client
        .datagram_socket(I::DOMAIN, fposix_socket::DatagramSocketProtocol::Udp)
        .await
        .expect("failed to create UDP socket");
    let loopback_ip: std::net::IpAddr = I::LOOPBACK_ADDRESS.to_ip_addr().into();
    send_socket
        .bind(&std::net::SocketAddr::new(loopback_ip, 0).into())
        .expect("failed to bind UDP socket");

    match I::VERSION {
        IpVersion::V4 => send_socket.set_multicast_loop_v4(multicast_loop_value),
        IpVersion::V6 => send_socket.set_multicast_loop_v6(multicast_loop_value),
    }
    .expect("failed to set IPV6_MULTICAST_LOOP");

    let recv_socket = client
        .datagram_socket(I::DOMAIN, fposix_socket::DatagramSocketProtocol::Udp)
        .await
        .expect("failed to create socket");
    recv_socket.bind(&I::MCAST_ADDR.into()).expect("failed to bind UDP socket");

    match I::MCAST_ADDR.ip() {
        std::net::IpAddr::V4(addr_v4) => recv_socket
            .join_multicast_v4_n(&addr_v4.into(), &InterfaceIndexOrAddress::Index(loopback_id))
            .expect("failed to join multicast group"),
        std::net::IpAddr::V6(addr_v6) => recv_socket
            .join_multicast_v6(&addr_v6.into(), loopback_id)
            .expect("failed to join multicast group"),
    }

    let recv_socket = fasync::net::UdpSocket::from_socket(recv_socket.into()).unwrap();

    let data = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
    assert_eq!(
        send_socket.send_to(&data, &I::MCAST_ADDR.into()).expect("failed to send multicast packet"),
        data.len()
    );

    // `recv_socket` is expected to receive one and only one packet.
    let mut buf = [0u8; 200];
    let (size, addr) = recv_socket
        .recv_from(&mut buf)
        .on_timeout(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT, || Err(std::io::ErrorKind::TimedOut.into()))
        .await
        .expect("recv_from failed");
    assert_eq!(size, data.len());
    assert_eq!(&buf[..size], &data[..]);
    assert_eq!(addr.ip(), loopback_ip);

    recv_socket
        .recv_from(&mut buf)
        .map(|output| panic!("unexpected received duplicate packet {output:?}"))
        .on_timeout(ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT, || ())
        .await;
}

#[netstack_test]
#[variant(N, Netstack)]
async fn broadcast_recv<N: Netstack>(name: &str) {
    const SUBNET: fnet::Subnet = fidl_subnet!("192.0.2.1/24");
    const PORT: u16 = 3513;

    const SRC_IP: net_types::ip::Ipv4Addr = net_ip_v4!("192.0.2.2");
    const SRC_PORT: u16 = 2141;
    const DST_IP: net_types::ip::Ipv4Addr = net_ip_v4!("192.0.2.255");

    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let client = sandbox
        .create_netstack_realm::<N, _>(format!("{name}_client"))
        .expect("failed to create client realm");
    let net = sandbox.create_network(format!("net0")).await.expect("failed to create network");
    let iface = client.join_network(&net, format!("if0")).await.expect("failed to join network");
    iface.add_address_and_subnet_route(SUBNET.clone()).await.expect("failed to set ip");
    let fake_ep = net.create_fake_endpoint().expect("failed to create endpoint");

    // Connect to the socket Provider to ensure all sockets are created with the same UID.
    // TODO(https://fxbug.dev/451615802): Remove this once FDIO is updated to pass
    // `SharingDomainToken` to `SetReusePort`.
    let socket_provider = client
        .connect_to_protocol::<fposix_socket::ProviderMarker>()
        .expect("failed to connect to socket provider");

    let sockets = future::join_all(std::iter::repeat(()).take(2).map(|()| async {
        let socket = fposix_socket_ext::datagram_socket(
            &socket_provider,
            Ipv4::DOMAIN,
            fposix_socket::DatagramSocketProtocol::Udp,
        )
        .await
        .expect("Failed to send request to create UDP socket")
        .expect("Failed to create UDP socket");

        socket.set_reuse_port(true).expect("failed to set SO_REUSEPORT");

        socket
            .bind(
                &std::net::SocketAddr::from((Ipv4::UNSPECIFIED_ADDRESS.to_ip_addr(), PORT)).into(),
            )
            .expect("failed to bind socket");

        fasync::net::UdpSocket::from_socket(socket.into()).unwrap()
    }))
    .await;

    let mut test_packet = [1, 2, 3, 4, 5];
    let broadcast_packet = packet::Buf::new(&mut test_packet, ..)
        .wrap_in(UdpPacketBuilder::new(
            SRC_IP,
            DST_IP,
            core::num::NonZero::new(SRC_PORT),
            core::num::NonZero::new(PORT).unwrap(),
        ))
        .wrap_in(Ipv4PacketBuilder::new(SRC_IP, DST_IP, /*ttl=*/ 30, IpProto::Udp.into()))
        .wrap_in(EthernetFrameBuilder::new(
            /*src_mac=*/ netstack_testing_common::constants::eth::MAC_ADDR,
            /*dst_mac=*/ net_types::ethernet::Mac::BROADCAST,
            EtherType::Ipv4,
            ETHERNET_MIN_BODY_LEN_NO_TAG,
        ))
        .serialize_vec_outer(&mut NoOpSerializationContext)
        .expect("failed to serialize UDP packet")
        .unwrap_b();
    fake_ep.write(broadcast_packet.as_ref()).await.expect("failed to write UDP packet");

    // Check that the packet was delivered to all sockets.
    for socket in sockets.iter() {
        let mut buf = [0u8; 1024];
        let (size, _addr) = socket
            .recv_from(&mut buf)
            .on_timeout(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT, || {
                panic!("Broadcast packet wasn't delivered to a listening socket")
            })
            .await
            .expect("recv_from failed");
        assert_eq!(size, test_packet.len());
    }
}

#[netstack_test]
#[variant(N, Netstack)]
#[variant(I, Ip)]
async fn broadcast_send<N: Netstack, I: TestIpExt>(name: &str) {
    const NETWORK: fnet::Subnet = fidl_subnet!("192.0.2.1/24");
    const PORT: u16 = 3513;
    const BROADCAST_ADDR: std::net::SocketAddr = std_socket_addr!("192.0.2.255:3513");

    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let client = sandbox
        .create_netstack_realm::<N, _>(format!("{name}_client"))
        .expect("failed to create client realm");

    let net = sandbox.create_network(format!("net0")).await.expect("failed to create network");
    let iface = client.join_network(&net, format!("if0")).await.expect("failed to join network");
    iface.add_address_and_subnet_route(NETWORK.clone()).await.expect("failed to set ip");
    let receiver = net.create_fake_endpoint().expect("failed to create endpoint");

    let recv_socket = client
        .datagram_socket(I::DOMAIN, fposix_socket::DatagramSocketProtocol::Udp)
        .await
        .expect("failed to create socket");
    recv_socket
        .bind(&std::net::SocketAddr::from((I::UNSPECIFIED_ADDRESS.to_ip_addr(), PORT)).into())
        .expect("failed to bind socket");
    let recv_socket = fasync::net::UdpSocket::from_socket(recv_socket.into()).unwrap();

    let socket = client
        .datagram_socket(I::DOMAIN, fposix_socket::DatagramSocketProtocol::Udp)
        .await
        .expect("failed to create socket");

    assert_eq!(socket.broadcast().expect("getsockopt(SO_BROADCAST) failed"), false);

    let test_packet = [1, 2, 3, 4, 5];
    let err = socket
        .send_to(&test_packet, &BROADCAST_ADDR.into())
        .expect_err("sendto is expected to fail to send broadcast packets by default");
    assert_eq!(err.raw_os_error(), Some(libc::EACCES));

    socket.set_broadcast(true).expect("failed to set SO_BROADCAST");
    assert_eq!(socket.broadcast().expect("getsockopt(SO_BROADCAST) failed"), true);

    assert_eq!(
        socket
            .send_to(&test_packet, &BROADCAST_ADDR.into())
            .expect("failed to send broadcast packet"),
        test_packet.len()
    );

    // Check that the packet is sent to the network.
    std::pin::pin!(receiver.frame_stream().map(|r| r.expect("failed to read frame")).filter_map(
        |(data, dropped)| async move {
            assert_eq!(dropped, 0);
            let (_payload, _src_mac, _dst_mac, _src_ip, dst_ip, proto, _ttl) =
                match packet_formats::testutil::parse_ip_packet_in_ethernet_frame::<Ipv4>(
                    &data[..],
                    EthernetFrameLengthCheck::NoCheck,
                ) {
                    Ok(result) => result,
                    Err(_e) => {
                        // Packet may fail to parse if it was for a
                        // different IP version. Just skip it.
                        return None;
                    }
                };

            if proto != IpProto::Udp.into() {
                return None;
            }

            assert_eq!(dst_ip.to_ip_addr(), BROADCAST_ADDR.ip().into());

            Some(())
        }
    ))
    .next()
    .on_timeout(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT.after_now(), || {
        panic!("timed out waiting for the multicast packet")
    })
    .await
    .expect("didn't receive the packet before end of the stream");

    // Check that the packet is delivered to local sockets.
    let mut buf = [0u8; 1024];
    let (size, _addr) = recv_socket
        .recv_from(&mut buf)
        .on_timeout(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT, || {
            panic!("Broadcast packet wasn't delivered to a listening socket")
        })
        .await
        .expect("recv_from failed");
    assert_eq!(size, test_packet.len());
}

#[netstack_test]
#[variant(N, Netstack)]
#[variant(I, Ip)]
async fn tos_tclass_send<
    N: Netstack,
    I: TestIpExt + packet_formats::ethernet::EthernetIpExt + packet_formats::ip::IpExt,
>(
    name: &str,
) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let client = sandbox
        .create_netstack_realm::<N, _>(format!("{name}_client"))
        .expect("failed to create client realm");

    let net = sandbox.create_network(format!("net0")).await.expect("failed to create network");
    let iface = client.join_network(&net, format!("if0")).await.expect("failed to join network");
    iface.add_address_and_subnet_route(I::CLIENT_SUBNET.clone()).await.expect("failed to set ip");
    let receiver = net.create_fake_endpoint().expect("failed to create endpoint");

    // Add a neighbor entry to ensure the packet is sent without having to resolve MAC.
    client
        .add_neighbor_entry(iface.id(), I::SERVER_SUBNET.addr.clone(), SERVER_MAC)
        .await
        .expect("add_neighbor_entry");

    let socket = client
        .datagram_socket(I::DOMAIN, fposix_socket::DatagramSocketProtocol::Udp)
        .await
        .expect("failed to create socket");

    let fnet_ext::IpAddress(dst_ip) = fnet_ext::IpAddress::from(I::SERVER_SUBNET.addr);
    let dst_addr = std::net::SocketAddr::new(dst_ip, 3513);

    let socket: std::net::UdpSocket = socket.into();
    let traffic_class = 0xa7;
    let r = match I::VERSION {
        IpVersion::V4 => unsafe {
            let v = traffic_class as libc::c_int;
            libc::setsockopt(
                socket.as_raw_fd(),
                libc::IPPROTO_IP,
                libc::IP_TOS,
                &v as *const libc::c_int as *const libc::c_void,
                std::mem::size_of_val(&v) as u32,
            )
        },
        IpVersion::V6 => unsafe {
            let v = traffic_class as libc::c_int;
            libc::setsockopt(
                socket.as_raw_fd(),
                libc::IPPROTO_IPV6,
                libc::IPV6_TCLASS,
                &v as *const libc::c_int as *const libc::c_void,
                std::mem::size_of_val(&v) as u32,
            )
        },
    };
    assert_eq!(r, 0, "Failed to set TOS/TCLASS option");

    let test_packet = [1, 2, 3, 4, 5];
    assert_eq!(
        socket.send_to(&test_packet, &dst_addr).expect("failed to send multicast packet"),
        test_packet.len()
    );

    // Check that the packet is sent to the network.
    std::pin::pin!(receiver.frame_stream().map(|r| r.expect("failed to read frame")).filter_map(
        |(data, dropped)| async move {
            assert_eq!(dropped, 0);
            let (mut body, _src_mac, _dst_mac, ethertype) =
                packet_formats::testutil::parse_ethernet_frame(
                    &data,
                    EthernetFrameLengthCheck::NoCheck,
                )
                .expect("Failed to parse ethernet packet");
            if ethertype != Some(I::ETHER_TYPE) {
                return None;
            }

            let ip_packet = <I::Packet<_> as packet::ParsablePacket<_, _>>::parse(&mut body, ())
                .expect("Failed to parse IP packet");
            use packet_formats::ip::IpPacket;
            if ip_packet.proto() != IpProto::Udp.into() {
                return None;
            }

            let received_traffic_class = ip_packet.dscp_and_ecn().raw();
            assert_eq!(traffic_class, received_traffic_class);

            Some(())
        }
    ))
    .next()
    .on_timeout(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT.after_now(), || {
        panic!("timed out waiting for the UDP packet packet")
    })
    .await
    .expect("didn't receive the packet before end of the stream");
}

#[netstack_test]
#[variant(N, Netstack)]
async fn udp_send_backpressure<N: Netstack>(name: &str) {
    const CLIENT_ADDR: fnet::Subnet = fidl_subnet!("192.0.2.1/24");
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm = sandbox
        .create_netstack_realm::<N, _>(format!("{name}_client"))
        .expect("failed to create client realm");

    let (tun_device, _device) = devices::create_tun_device_with(fnet_tun::DeviceConfig {
        blocking: Some(true),
        ..Default::default()
    });
    let (port, client_port) =
        devices::create_ip_tun_port(&tun_device, devices::TUN_DEFAULT_PORT_ID).await;
    port.set_online(true).await.expect("set port online");
    let (_id, _interface_control, _device_control) =
        crate::install_ip_device(&realm, client_port, [CLIENT_ADDR]).await;

    let socket = realm
        .datagram_socket(fposix_socket::Domain::Ipv4, fposix_socket::DatagramSocketProtocol::Udp)
        .await
        .expect("failed to create socket");
    // Set the send buffer size to the minimum possible.
    socket.set_send_buffer_size(0).expect("setting send buffer size");
    // Create an async nonblock socket.
    let socket = DatagramSocket::new_from_socket(socket).expect("creating async socket");
    const PAYLOAD: &[u8] = b"Hello";

    let server_addr = std_socket_addr!("192.0.2.2:8080");

    // Write into the socket until we observe EWOULDBLOCK, i.e., the send future
    // doesn't resolve immediately.
    let mut sent = 0;
    while let Some(r) = socket.send_to(PAYLOAD, server_addr.into()).now_or_never() {
        assert_matches!(r, Ok(_));
        sent += 1;
    }
    // At least one frame must've been sent.
    assert_ne!(sent, 0);

    // Create a new future that should unblock only when we read frames.
    let mut fut = socket.send_to(PAYLOAD, server_addr.into());
    assert_matches!(futures::poll!(&mut fut), Poll::Pending);

    // Wait for all sent frames to show up in the device queue.
    while sent != 0 {
        let fnet_tun::Frame { data, frame_type, .. } =
            tun_device.read_frame().await.expect("got frame").expect("reading frame");
        let data = data.expect("missing data");
        let frame_type = frame_type.expect("missing frame type");
        if frame_type != fhardware_network::FrameType::Ipv4 {
            continue;
        }
        let mut body = &data[..];
        let ipv4 = Ipv4Packet::parse(&mut body, ()).expect("failed to parse IPv4 packet");
        if ipv4.proto() == IpProto::Udp.into() {
            sent -= 1;
        }
    }
    // Future should unblock now that we've allowed the frames to be popped from
    // the device FIFO.
    assert_eq!(fut.await.expect("send_to error"), PAYLOAD.len());
}

fn set_socket_ipv6_only(socket: &socket2::Socket, ipv6_only: bool) -> Result {
    let fd = socket.as_raw_fd();
    let optval = ipv6_only as libc::c_int;
    let optval_ptr = &optval as *const libc::c_int as *const libc::c_void;
    let optval_size = std::mem::size_of_val(&optval) as u32;
    // SAFETY: Calling setsockop with valid arguments.
    let r =
        unsafe { libc::setsockopt(fd, libc::SOL_IPV6, libc::IPV6_V6ONLY, optval_ptr, optval_size) };
    if r != 0 {
        return Err(anyhow!("setsockopt failed"));
    }
    Ok(())
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
enum SocketFamily {
    Ipv4,
    Ipv6,
    DualStack,
}

impl SocketFamily {
    fn domain(&self) -> fposix_socket::Domain {
        match self {
            Self::Ipv4 => Ipv4::DOMAIN,
            Self::Ipv6 | Self::DualStack => Ipv6::DOMAIN,
        }
    }

    fn unspec_address(&self) -> IpAddr {
        match self {
            Self::Ipv4 => Ipv4::UNSPECIFIED_ADDRESS.to_ip_addr(),
            Self::Ipv6 | Self::DualStack => Ipv6::UNSPECIFIED_ADDRESS.to_ip_addr(),
        }
    }

    async fn create_socket(&self, realm: &TestRealm<'_>) -> socket2::Socket {
        let socket = realm
            .datagram_socket(self.domain(), fposix_socket::DatagramSocketProtocol::Udp)
            .await
            .expect("failed to create socket");
        if *self == SocketFamily::Ipv6 {
            set_socket_ipv6_only(&socket, true).expect("failed to set SO_IPV6_V6ONLY");
        }
        socket
    }
}

// When multiple sockets are bound to the same address and device is updated
// on one of them, the update should not affect the other sockets.
// This is a regression test for https://fxrev.dev/479568320 .
#[netstack_test]
#[test_matrix(
    [SocketFamily::Ipv4, SocketFamily::Ipv6, SocketFamily::DualStack]
)]
async fn set_so_bindtodevice_bound_socket(name: &str, socket_family: SocketFamily) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm = sandbox
        .create_netstack_realm::<Netstack3, _>(format!("{name}_client"))
        .expect("failed to create client realm");

    const PORT: u16 = 53535;

    let socket1 = socket_family.create_socket(&realm).await;
    socket1.set_reuse_address(true).expect("failed to set reuse address");
    let socket2 = socket_family.create_socket(&realm).await;
    socket2.set_reuse_address(true).expect("failed to set reuse address");

    // Bind both sockets to the same port.
    let addr: socket2::SockAddr =
        std::net::SocketAddr::from((socket_family.unspec_address(), PORT)).into();
    socket1.bind(&addr).expect("failed to bind");
    socket2.bind(&addr).expect("failed to bind");

    // Bind `socket1` to loopback device. This should not affect `socket2`.
    socket1.bind_device(Some("lo".as_bytes())).expect("failed to bind to device");

    // Try sending a packet. It is expected to be received on `socket1`
    // since it's bound to the device.
    let loopback_addr = match socket_family {
        SocketFamily::Ipv4 => Ipv4::LOOPBACK_ADDRESS.to_ip_addr(),
        SocketFamily::Ipv6 | SocketFamily::DualStack => Ipv6::LOOPBACK_ADDRESS.to_ip_addr(),
    };
    let send_socket = socket_family.create_socket(&realm).await;
    let addr: socket2::SockAddr = std::net::SocketAddr::from((loopback_addr, PORT)).into();
    let sent = send_socket.send_to(b"hello", &addr).expect("failed to send");
    assert_eq!(sent, 5);

    let socket1 = fasync::net::UdpSocket::from_socket(socket1.into())
        .expect("Failed to create async UDP socket");
    let mut buf = [0; 1024];
    let (bytes_read, _addr) = socket1
        .recv_from(&mut buf)
        .on_timeout(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT, || {
            panic!("UDP packet wasn't delivered to a listening socket")
        })
        .await
        .expect("failed to receive");
    assert_eq!(bytes_read, 5);
    assert_eq!(&buf[..bytes_read], b"hello");
}

#[netstack_test]
#[test_matrix(
    [SocketFamily::Ipv4, SocketFamily::Ipv6, SocketFamily::DualStack],
    [SocketFamily::Ipv4, SocketFamily::Ipv6, SocketFamily::DualStack]
)]
async fn set_so_bindtodevice_conflict(
    name: &str,
    socket1_family: SocketFamily,
    socket2_family: SocketFamily,
) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let net = sandbox.create_network(format!("net0")).await.expect("failed to create network");
    let realm = sandbox
        .create_netstack_realm::<Netstack3, _>(format!("{name}_client"))
        .expect("failed to create client realm");
    let iface = realm.join_network(&net, format!("if0")).await.expect("failed to join network");
    let if_name = iface.get_interface_name().await.expect("get_name failed");

    const PORT: u16 = 53535;

    let socket1 = socket1_family.create_socket(&realm).await;
    socket1.bind_device(Some("lo".as_bytes())).expect("failed to bind to device");
    let addr1: socket2::SockAddr =
        std::net::SocketAddr::from((socket1_family.unspec_address(), PORT)).into();
    socket1.bind(&addr1).expect("failed to bind");

    let socket2 = socket2_family.create_socket(&realm).await;
    socket2.bind_device(Some(if_name.as_bytes())).expect("failed to bind to device");
    let addr2: socket2::SockAddr =
        std::net::SocketAddr::from((socket2_family.unspec_address(), PORT)).into();
    socket2.bind(&addr2).expect("failed to bind");

    let result = socket1.bind_device(Some("".as_bytes()));

    let expect_failure = socket1_family == socket2_family
        || socket1_family == SocketFamily::DualStack
        || socket2_family == SocketFamily::DualStack;
    if expect_failure {
        let error = result.expect_err("expected to fail to unbind device");
        assert_eq!(error.raw_os_error(), Some(libc::EADDRINUSE));
    } else {
        result.expect("expected to succeed to unbind device");
    }
}

// A regression test for https://fxbug.dev/515411156.
//
// Verify that Netstack3 ignores packets it receives from the network that are
// destined to localhost.
#[netstack_test]
#[variant(N, Netstack)]
#[variant(I, Ip)]
async fn ignore_localhost_traffic_from_net<N: Netstack, I: Ip>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm = sandbox.create_netstack_realm::<N, _>(name).expect("failed to create realm");

    let net = sandbox.create_network("net").await.expect("failed to create network");

    const LOCAL_MAC: fnet::MacAddress = fidl_mac!("02:00:00:00:00:01");
    const REMOTE_MAC: fnet::MacAddress = fidl_mac!("02:00:00:00:00:02");

    let iface = realm
        .join_network_with(
            &net,
            "if0",
            netemul::new_endpoint_config(netemul::DEFAULT_MTU, Some(LOCAL_MAC)),
            Default::default(),
        )
        .await
        .expect("failed to join network");

    let (iface_ip, domain) = match I::VERSION {
        IpVersion::V4 => (fidl_subnet!("192.0.2.1/24"), fposix_socket::Domain::Ipv4),
        IpVersion::V6 => (fidl_subnet!("2001:db8::1/64"), fposix_socket::Domain::Ipv6),
    };

    iface.add_address_and_subnet_route(iface_ip).await.expect("failed to set ip");

    let mut config = fnet_interfaces_admin::Configuration::default();
    match I::VERSION {
        IpVersion::V4 => {
            config.ipv4 = Some(fnet_interfaces_admin::Ipv4Configuration {
                unicast_forwarding: Some(true),
                ..Default::default()
            });
        }
        IpVersion::V6 => {
            config.ipv6 = Some(fnet_interfaces_admin::Ipv6Configuration {
                unicast_forwarding: Some(true),
                ..Default::default()
            });
        }
    }
    let _prev = iface
        .control()
        .set_configuration(&config)
        .await
        .expect("set_configuration fidl error")
        .expect("failed to set interface configuration");

    const LOCAL_PORT: NonZeroU16 = NonZeroU16::new(1234).unwrap();
    const REMOTE_PORT: NonZeroU16 = NonZeroU16::new(5678).unwrap();

    let socket = realm
        .datagram_socket(domain, fposix_socket::DatagramSocketProtocol::Udp)
        .await
        .expect("failed to create socket");

    let bind_addr = match I::VERSION {
        IpVersion::V4 => {
            std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, LOCAL_PORT.get()))
        }
        IpVersion::V6 => {
            std::net::SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, LOCAL_PORT.get()))
        }
    };
    socket.bind(&bind_addr.into()).expect("failed to bind socket");

    let fake_ep = net.create_fake_endpoint().expect("failed to create endpoint");

    let mut payload = [1, 2, 3, 4, 5];
    let packet = match I::VERSION {
        IpVersion::V4 => {
            let src = net_ip_v4!("192.0.2.2");
            let dst = net_ip_v4!("127.0.0.1");
            packet::Buf::new(&mut payload, ..)
                .wrap_in(UdpPacketBuilder::new(src, dst, Some(REMOTE_PORT), LOCAL_PORT))
                .wrap_in(Ipv4PacketBuilder::new(src, dst, 64, IpProto::Udp.into()))
                .wrap_in(EthernetFrameBuilder::new(
                    net_types::ethernet::Mac::new(REMOTE_MAC.octets),
                    net_types::ethernet::Mac::new(LOCAL_MAC.octets),
                    EtherType::Ipv4,
                    ETHERNET_MIN_BODY_LEN_NO_TAG,
                ))
                .serialize_vec_outer(&mut NoOpSerializationContext)
                .expect("failed to serialize UDP packet")
                .unwrap_b()
        }
        IpVersion::V6 => {
            let src = net_ip_v6!("2001:db8::2");
            let dst = net_ip_v6!("::1");
            packet::Buf::new(&mut payload, ..)
                .wrap_in(UdpPacketBuilder::new(src, dst, Some(REMOTE_PORT), LOCAL_PORT))
                .wrap_in(Ipv6PacketBuilder::new(src, dst, 64, IpProto::Udp.into()))
                .wrap_in(EthernetFrameBuilder::new(
                    net_types::ethernet::Mac::new(REMOTE_MAC.octets),
                    net_types::ethernet::Mac::new(LOCAL_MAC.octets),
                    EtherType::Ipv6,
                    ETHERNET_MIN_BODY_LEN_NO_TAG,
                ))
                .serialize_vec_outer(&mut NoOpSerializationContext)
                .expect("failed to serialize UDP packet")
                .unwrap_b()
        }
    };

    fake_ep.write(packet.as_ref()).await.expect("failed to write UDP packet");

    let mut buf = [0u8; 1024];
    let socket = fasync::net::UdpSocket::from_socket(socket.into()).unwrap();

    let recv_result = socket
        .recv_from(&mut buf)
        .map(Some)
        .on_timeout(ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT, || None)
        .await;

    assert_matches!(recv_result, None);
}

// Integration test for `SOL_IP -> IP_PKTINFO`.
//
// IPv4 sockets should receive pktinfo and dual-stack sockets should only
// receive pktinfo for IPv4 packets.
#[netstack_test]
#[test_case(
    fposix_socket::Domain::Ipv4, net_ip_v4!("192.0.2.1"), false;
    "v4_unicast"
)]
#[test_case(
    fposix_socket::Domain::Ipv4, net_ip_v4!("255.255.255.255"), false;
    "v4_broadcast"
)]
#[test_case(
    fposix_socket::Domain::Ipv4, net_ip_v4!("192.0.2.255"), false;
    "v4_subnet_broadcast"
)]
#[test_case(
    fposix_socket::Domain::Ipv6, net_ip_v4!("192.0.2.1"), false;
    "v6_dual_stack_unicast_no_v6_pktinfo"
)]
#[test_case(
    fposix_socket::Domain::Ipv6, net_ip_v4!("255.255.255.255"), false;
    "v6_dual_stack_broadcast_no_v6_pktinfo"
)]
#[test_case(
    fposix_socket::Domain::Ipv6, net_ip_v4!("192.0.2.255"), false;
    "v6_dual_stack_subnet_broadcast_no_v6_pktinfo"
)]
#[test_case(
    fposix_socket::Domain::Ipv6, net_ip_v4!("192.0.2.1"), true;
    "v6_dual_stack_unicast_v6_pktinfo"
)]
#[test_case(
    fposix_socket::Domain::Ipv6, net_ip_v4!("255.255.255.255"), true;
    "v6_dual_stack_broadcast_v6_pktinfo"
)]
async fn ip_pktinfo(
    name: &str,
    domain: fposix_socket::Domain,
    v4_dst: net_types::ip::Ipv4Addr,
    v6_recv_pktinfo: bool,
) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm =
        sandbox.create_netstack_realm::<Netstack3, _>(name).expect("failed to create realm");

    let net = sandbox.create_network("net").await.expect("failed to create network");

    const LOCAL_MAC: fnet::MacAddress = fidl_mac!("02:00:00:00:00:01");
    const REMOTE_MAC: fnet::MacAddress = fidl_mac!("02:00:00:00:00:02");
    const LOCAL_V4_ADDR: net_types::ip::Ipv4Addr = net_ip_v4!("192.0.2.1");
    const LOCAL_V6_ADDR: net_types::ip::Ipv6Addr = net_ip_v6!("2001:db8::1");

    let iface = realm
        .join_network_with(
            &net,
            "if0",
            netemul::new_endpoint_config(netemul::DEFAULT_MTU, Some(LOCAL_MAC)),
            Default::default(),
        )
        .await
        .expect("failed to join network");
    let iface_id = iface.id();

    let v4_addr = fidl_subnet!("192.0.2.1/24");
    let v6_addr = fidl_subnet!("2001:db8::1/64");
    iface.add_address_and_subnet_route(v4_addr).await.expect("failed to set ipv4");
    iface.add_address_and_subnet_route(v6_addr).await.expect("failed to set ipv6");

    let socket = realm
        .datagram_socket(domain, fposix_socket::DatagramSocketProtocol::Udp)
        .await
        .expect("failed to create socket");

    let channel = fdio::clone_channel(&socket).expect("failed to clone channel");
    let proxy = fposix_socket::SynchronousDatagramSocketProxy::new(
        fidl::AsyncChannel::from_channel(channel),
    );

    let bind_addr = match domain {
        fposix_socket::Domain::Ipv4 => fidl_socket_addr!("0.0.0.0:1234"),
        fposix_socket::Domain::Ipv6 => {
            proxy.set_ipv6_only(false).await.expect("fidl error").expect("set_ipv6_only failed");
            if v6_recv_pktinfo {
                proxy
                    .set_ipv6_receive_packet_info(true)
                    .await
                    .expect("fidl error")
                    .expect("set_ipv6_receive_packet_info failed");
                assert!(
                    proxy
                        .get_ipv6_receive_packet_info()
                        .await
                        .expect("fidl error")
                        .expect("get_ipv6_receive_packet_info failed")
                );
            }
            fidl_socket_addr!("[::]:1234")
        }
    };

    proxy.set_ip_packet_info(true).await.expect("fidl error").expect("set_ip_packet_info failed");
    assert!(
        proxy.get_ip_packet_info().await.expect("fidl error").expect("get_ip_packet_info failed")
    );
    proxy.bind(&bind_addr).await.expect("fidl error").expect("bind failed");

    let fake_ep = net.create_fake_endpoint().expect("failed to create endpoint");

    const LOCAL_PORT: NonZeroU16 = NonZeroU16::new(1234).unwrap();
    const REMOTE_PORT: NonZeroU16 = NonZeroU16::new(5678).unwrap();

    let make_v4_packet = |mut payload: Vec<u8>| {
        let src = net_ip_v4!("192.0.2.2");
        packet::Buf::new(&mut payload, ..)
            .wrap_in(UdpPacketBuilder::new(src, v4_dst, Some(REMOTE_PORT), LOCAL_PORT))
            .wrap_in(Ipv4PacketBuilder::new(src, v4_dst, 64, IpProto::Udp.into()))
            .wrap_in(EthernetFrameBuilder::new(
                net_types::ethernet::Mac::new(REMOTE_MAC.octets),
                net_types::ethernet::Mac::new(LOCAL_MAC.octets),
                EtherType::Ipv4,
                ETHERNET_MIN_BODY_LEN_NO_TAG,
            ))
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .expect("failed to serialize IPv4 UDP packet")
            .unwrap_b()
    };

    let make_v6_packet = |mut payload: Vec<u8>| {
        let src = net_ip_v6!("2001:db8::2");
        packet::Buf::new(&mut payload, ..)
            .wrap_in(UdpPacketBuilder::new(src, LOCAL_V6_ADDR, Some(REMOTE_PORT), LOCAL_PORT))
            .wrap_in(Ipv6PacketBuilder::new(src, LOCAL_V6_ADDR, 64, IpProto::Udp.into()))
            .wrap_in(EthernetFrameBuilder::new(
                net_types::ethernet::Mac::new(REMOTE_MAC.octets),
                net_types::ethernet::Mac::new(LOCAL_MAC.octets),
                EtherType::Ipv6,
                ETHERNET_MIN_BODY_LEN_NO_TAG,
            ))
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .expect("failed to serialize IPv6 UDP packet")
            .unwrap_b()
    };

    async fn wait_for_incoming_datagram(proxy: &fposix_socket::SynchronousDatagramSocketProxy) {
        let event =
            proxy.describe().await.expect("describe should succeed").event.expect("event missing");
        let _ = fasync::OnSignals::new(
            event,
            zx::Signals::from_bits(fposix_socket::SIGNAL_DATAGRAM_INCOMING).unwrap(),
        )
        .await
        .expect("waiting for signals failed");
    }

    fake_ep
        .write(make_v4_packet(vec![1, 2, 3]).as_ref())
        .await
        .expect("failed to write UDP packet");

    wait_for_incoming_datagram(&proxy).await;

    let (_addr, _data, control, _truncated) = proxy
        .recv_msg(false, u16::MAX.into(), true, fposix_socket::RecvMsgFlags::empty())
        .await
        .expect("recv_msg fidl error")
        .expect("recv_msg failed");

    let expected_ipv6_control = v6_recv_pktinfo.then_some(fposix_socket::Ipv6RecvControlData {
        pktinfo: Some(fposix_socket::Ipv6PktInfoRecvControlData {
            iface: iface_id,
            header_destination_addr: fnet::Ipv6Address {
                addr: v4_dst.to_ipv6_mapped().ipv6_bytes(),
            },
        }),
        ..Default::default()
    });

    assert_eq!(
        control.network,
        Some(fposix_socket::NetworkSocketRecvControlData {
            socket: None,
            ip: Some(fposix_socket::IpRecvControlData {
                tos: None,
                ttl: None,
                original_destination_address: None,
                pktinfo: Some(fposix_socket::Ipv4PktInfoRecvControlData {
                    iface: iface_id,
                    local_addr: fnet::Ipv4Address { addr: LOCAL_V4_ADDR.ipv4_bytes() },
                    header_destination_addr: fnet::Ipv4Address { addr: v4_dst.ipv4_bytes() },
                }),
                ..Default::default()
            }),
            // If `IPV6_RECVPKTINFO` is enabled, then IPv6 packet info is also
            // received for IPv6-mapped IPv4 packets.
            ipv6: expected_ipv6_control,
            ..Default::default()
        })
    );

    if domain == fposix_socket::Domain::Ipv6 {
        fake_ep
            .write(make_v6_packet(vec![7, 8, 9]).as_ref())
            .await
            .expect("failed to write UDP packet");

        wait_for_incoming_datagram(&proxy).await;

        let (_addr, _data, control, _truncated) = proxy
            .recv_msg(false, u16::MAX.into(), true, fposix_socket::RecvMsgFlags::empty())
            .await
            .expect("recv_msg fidl error")
            .expect("recv_msg failed");

        let expected_control =
            v6_recv_pktinfo.then_some(fposix_socket::NetworkSocketRecvControlData {
                socket: None,
                ip: None,
                ipv6: Some(fposix_socket::Ipv6RecvControlData {
                    pktinfo: Some(fposix_socket::Ipv6PktInfoRecvControlData {
                        iface: iface_id,
                        header_destination_addr: fnet::Ipv6Address {
                            addr: LOCAL_V6_ADDR.ipv6_bytes(),
                        },
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            });

        assert_eq!(control.network, expected_control);
    }
}
