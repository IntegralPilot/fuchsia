// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Tests for filter actions.

use assert_matches::assert_matches;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_filter_ext::{Action, IpHook, RejectType};
use fuchsia_async::TimeoutExt as _;
use net_types::ip::{Ip, IpAddress, IpVersion};
use netstack_testing_common::ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT;
use netstack_testing_macros::netstack_test;
use packet::ParsablePacket as _;
use packet_formats::ethernet::{EthernetFrame, EthernetFrameLengthCheck};
use packet_formats::icmp::{
    IcmpParseArgs, Icmpv4DestUnreachableCode, Icmpv4Packet, Icmpv6DestUnreachableCode, Icmpv6Packet,
};
use packet_formats::ip::{IpPacket as _, Ipv4Proto, Ipv6Proto};
use test_case::test_case;

use crate::ip_hooks::{
    ExpectedConnectivity, LOW_RULE_PRIORITY, TcpSocket, TestIpExt, TestNet, UdpSocket,
};
use crate::matchers::{Tcp, Udp};

#[netstack_test]
#[variant(I, Ip)]
#[test_case(RejectType::PortUnreachable; "port_unreachable")]
#[test_case(RejectType::NetUnreachable; "net_unreachable")]
#[test_case(RejectType::HostUnreachable; "host_unreachable")]
#[test_case(RejectType::ProtoUnreachable; "proto_unreachable")]
#[test_case(RejectType::RoutePolicyFail; "route_policy_fail")]
#[test_case(RejectType::RejectRoute; "reject_route")]
#[test_case(RejectType::AdminProhibited; "admin_prohibited")]
async fn reject_incoming<I: TestIpExt>(name: &str, reject_type: RejectType) {
    // TODO(https://fxbug.dev/488116504): Implement ProtoUnreachable for IPv6.
    if I::VERSION == IpVersion::V6 && reject_type == RejectType::ProtoUnreachable {
        return;
    }

    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let network = sandbox.create_network("net").await.expect("create network");
    let _packet_capture = network.start_capture(&name).await.expect("starting packet capture");

    let mut net = TestNet::new::<I>(
        &sandbox,
        &network,
        &name,
        Some(IpHook::LocalIngress),
        None, /* nat_hook */
    )
    .await;

    let fake_endpoint = network.create_fake_endpoint().expect("create fake endpoint");

    // Install a rule that rejects all traffic.

    let expected_error = match (I::VERSION, reject_type) {
        (_, RejectType::NetUnreachable) | (_, RejectType::HostUnreachable) => None,
        (_, RejectType::PortUnreachable) => Some(libc::ECONNREFUSED),
        (_, RejectType::ProtoUnreachable) => Some(libc::ECONNREFUSED),
        (IpVersion::V4, RejectType::RoutePolicyFail) => None,
        (IpVersion::V6, RejectType::RoutePolicyFail) => Some(libc::EACCES),
        (IpVersion::V4, RejectType::RejectRoute) => None,
        (IpVersion::V6, RejectType::RejectRoute) => Some(libc::EACCES),
        (IpVersion::V4, RejectType::AdminProhibited) => Some(libc::EHOSTUNREACH),
        (IpVersion::V6, RejectType::AdminProhibited) => Some(libc::EACCES),
        (_, RejectType::TcpReset) => Some(libc::ECONNREFUSED),
    };

    let expected_connectivity = match expected_error {
        Some(err) => ExpectedConnectivity::Reject(err),
        None => ExpectedConnectivity::None,
    };

    let (_server_matcher, _sockets) = {
        let matcher = Udp;
        net.run_test_with::<I, UdpSocket, _, _>(
            expected_connectivity,
            |TestNet { client: _, server }, addrs, ()| {
                Box::pin(async move {
                    server
                        .install_rule_for_incoming_traffic::<I, _>(
                            LOW_RULE_PRIORITY,
                            &matcher,
                            addrs.client_ports(),
                            Action::Reject(reject_type),
                        )
                        .await
                })
            },
        )
        .await
    };

    fn ip_addr_to_fnet_ip_address<A: IpAddress>(ip: A) -> fnet::IpAddress {
        A::Version::map_ip_in(
            ip,
            |v4_addr| fnet::IpAddress::Ipv4(fnet::Ipv4Address { addr: v4_addr.ipv4_bytes() }),
            |v6_addr| fnet::IpAddress::Ipv6(fnet::Ipv6Address { addr: v6_addr.ipv6_bytes() }),
        )
    }

    fn expected_icmpv4_code(reject_type: RejectType) -> Icmpv4DestUnreachableCode {
        match reject_type {
            RejectType::PortUnreachable => Icmpv4DestUnreachableCode::DestPortUnreachable,
            RejectType::NetUnreachable => Icmpv4DestUnreachableCode::DestNetworkUnreachable,
            RejectType::HostUnreachable => Icmpv4DestUnreachableCode::DestHostUnreachable,
            RejectType::ProtoUnreachable => Icmpv4DestUnreachableCode::DestProtocolUnreachable,
            RejectType::RoutePolicyFail => {
                Icmpv4DestUnreachableCode::NetworkAdministrativelyProhibited
            }
            RejectType::RejectRoute => Icmpv4DestUnreachableCode::HostAdministrativelyProhibited,
            RejectType::AdminProhibited => {
                Icmpv4DestUnreachableCode::CommAdministrativelyProhibited
            }
            RejectType::TcpReset => unreachable!(),
        }
    }

    fn expected_icmpv6_code(reject_type: RejectType) -> Icmpv6DestUnreachableCode {
        match reject_type {
            RejectType::PortUnreachable => Icmpv6DestUnreachableCode::PortUnreachable,
            RejectType::NetUnreachable => Icmpv6DestUnreachableCode::NoRoute,
            RejectType::HostUnreachable => Icmpv6DestUnreachableCode::AddrUnreachable,
            RejectType::RoutePolicyFail => Icmpv6DestUnreachableCode::SrcAddrFailedPolicy,
            RejectType::RejectRoute => Icmpv6DestUnreachableCode::RejectRoute,
            RejectType::AdminProhibited => {
                Icmpv6DestUnreachableCode::CommAdministrativelyProhibited
            }
            RejectType::ProtoUnreachable => unreachable!(),
            RejectType::TcpReset => unreachable!(),
        }
    }

    let fake_ep_loop = async move {
        loop {
            let (buf, dropped_frames): (Vec<u8>, u64) =
                fake_endpoint.read().await.expect("failed to read from endpoint");
            assert_eq!(dropped_frames, 0);
            let eth = EthernetFrame::parse(&mut &buf[..], EthernetFrameLengthCheck::NoCheck)
                .expect("valid ethernet frame");
            let Ok(packet) = I::Packet::parse(&mut eth.body(), ()) else {
                continue;
            };
            eprintln!(
                "proto: {:?}, src: {:?}, dst: {:?}",
                packet.proto(),
                packet.src_ip(),
                packet.dst_ip()
            );
            let is_icmp = I::map_ip_in(
                packet.proto(),
                |v4_proto| v4_proto == Ipv4Proto::Icmp,
                |v6_proto| v6_proto == Ipv6Proto::Icmpv6,
            );
            if !is_icmp {
                return;
            }

            if ip_addr_to_fnet_ip_address(packet.src_ip()) != I::SERVER_ADDR_WITH_PREFIX.addr
                || ip_addr_to_fnet_ip_address(packet.dst_ip()) != I::CLIENT_ADDR_WITH_PREFIX.addr
            {
                return;
            }

            I::map_ip_in(
                packet,
                |v4_packet| {
                    let icmp_packet = Icmpv4Packet::parse(
                        &mut v4_packet.body(),
                        IcmpParseArgs::new(v4_packet.src_ip(), v4_packet.dst_ip()),
                    )
                    .expect("valid icmp packet");
                    assert_matches!(icmp_packet, Icmpv4Packet::DestUnreachable (p) => {
                        assert_eq!(p.code(), expected_icmpv4_code(reject_type));
                    });
                },
                |v6_packet| {
                    let icmp_packet = Icmpv6Packet::parse(
                        &mut v6_packet.body(),
                        IcmpParseArgs::new(v6_packet.src_ip(), v6_packet.dst_ip()),
                    )
                    .expect("valid icmp packet");
                    assert_matches!(icmp_packet, Icmpv6Packet::DestUnreachable (p) => {
                        assert_eq!(p.code(), expected_icmpv6_code(reject_type));
                    });
                },
            );

            return;
        }
    };

    let () = fake_ep_loop
        .on_timeout(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT, || {
            panic!("timed out waiting for ICMP error")
        })
        .await;
}

// Test Reject action in the LOCAL_EGRESS hook.
//
// TODO(https://fxbug.dev/322214321): Run this test with UDP sockets once
// `SO_ERROR` is implemented.
#[netstack_test]
#[variant(I, Ip)]
#[test_case(RejectType::PortUnreachable; "port_unreachable")]
#[test_case(RejectType::NetUnreachable; "net_unreachable")]
#[test_case(RejectType::HostUnreachable; "host_unreachable")]
#[test_case(RejectType::ProtoUnreachable; "proto_unreachable")]
#[test_case(RejectType::RoutePolicyFail; "route_policy_fail")]
#[test_case(RejectType::RejectRoute; "reject_route")]
#[test_case(RejectType::AdminProhibited; "admin_prohibited")]
async fn reject_outgoing<I: TestIpExt>(name: &str, reject_type: RejectType) {
    // TODO(https://fxbug.dev/488116504): Implement ProtoUnreachable for IPv6.
    if I::VERSION == IpVersion::V6 && reject_type == RejectType::ProtoUnreachable {
        return;
    }

    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let network = sandbox.create_network("net").await.expect("create network");
    let _packet_capture = network.start_capture(&name).await.expect("starting packet capture");

    let mut net = TestNet::new::<I>(
        &sandbox,
        &network,
        &name,
        Some(IpHook::LocalEgress),
        None, /* nat_hook */
    )
    .await;

    // Current TCP implementation returns the following error codes in response
    // to ICMP errors, but some of these are incorrect and should be fixed.
    // TODO(https://fxbug.dev/489094027): connect() should handle
    // `ProtoUnreachable` error as `ECONNREFUSED`.
    // TODO(https://fxbug.dev/489090939): IPv6 TCP should return `ENETUNREACH`
    // and `EHOSTUNREACH` instead of `EACCESS`.
    let expected_error = match (I::VERSION, reject_type) {
        (_, RejectType::PortUnreachable) => libc::ECONNREFUSED,
        (_, RejectType::NetUnreachable) => libc::ENETUNREACH,
        (_, RejectType::HostUnreachable) => libc::EHOSTUNREACH,
        (_, RejectType::ProtoUnreachable) => libc::ENOPROTOOPT,
        (IpVersion::V4, RejectType::RoutePolicyFail) => libc::ENETUNREACH,
        (IpVersion::V6, RejectType::RoutePolicyFail) => libc::EACCES,
        (IpVersion::V4, RejectType::RejectRoute) => libc::EHOSTUNREACH,
        (IpVersion::V6, RejectType::RejectRoute) => libc::EACCES,
        (IpVersion::V4, RejectType::AdminProhibited) => libc::EHOSTUNREACH,
        (IpVersion::V6, RejectType::AdminProhibited) => libc::EACCES,
        (_, RejectType::TcpReset) => libc::ECONNREFUSED,
    };

    // Install a rule that explicitly accepts traffic of a certain type on the
    // incoming hook for both the client and server. This should not change the
    // two-way connectivity because accepting traffic is the default.
    let (_server_matcher, _sockets) = {
        let matcher = Tcp;
        net.run_test_with::<I, TcpSocket, _, _>(
            ExpectedConnectivity::Reject(expected_error),
            |TestNet { client, server: _ }, addrs, ()| {
                Box::pin(async move {
                    client
                        .install_rule_for_outgoing_traffic::<I, _>(
                            LOW_RULE_PRIORITY,
                            &matcher,
                            addrs.client_ports(),
                            Action::Reject(reject_type),
                        )
                        .await
                })
            },
        )
        .await
    };
}
