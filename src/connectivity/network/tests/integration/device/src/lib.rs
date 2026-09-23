// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

use std::convert::TryFrom as _;
use std::num::NonZeroU64;
use std::pin::pin;

use fidl_fuchsia_hardware_network as fhardware_network;
use fidl_fuchsia_hardware_network_driver as fhardware_network_driver;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_interfaces_admin as fnet_interfaces_admin;
use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
use fidl_fuchsia_net_tun as fnet_tun;
use fidl_fuchsia_posix_socket_packet as fposix_socket_packet;
use fuchsia_async as fasync;

use fidl_fuchsia_net_ext::IntoExt as _;
use fuchsia_async::TimeoutExt as _;
use futures::{FutureExt as _, SinkExt as _, StreamExt as _, TryStreamExt as _};
use net_declare::{fidl_mac, fidl_subnet, net_ip_v4, std_socket_addr_v4};
use net_types::MulticastAddr;
use net_types::ethernet::Mac;
use net_types::ip::{Ip as _, IpAddress as _, Ipv4, Ipv4Addr, Ipv6, Ipv6Addr};
use netemul::RealmUdpSocket as _;
use netstack_testing_common::realms::{Netstack, Netstack3, TestSandboxExt as _};
use netstack_testing_common::{
    ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT, ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT,
};
use netstack_testing_macros::netstack_test;
use packet::{
    Buf, NestablePacketBuilder as _, NoOpSerializationContext, ParsablePacket as _, Serializer as _,
};
use packet_formats::error::ParseError;
use packet_formats::ethernet::{
    ETHERNET_HDR_LEN_NO_TAG, ETHERNET_MIN_BODY_LEN_NO_TAG, EtherType, EthernetFrame,
    EthernetFrameBuilder, EthernetFrameLengthCheck,
};
use packet_formats::icmp::MessageBody as _;
use packet_formats::ipv4::Ipv4Header as _;
use sockaddr::{EthernetSockaddr, IntoSockAddr as _};
use static_assertions::const_assert_eq;
use test_case::test_case;

const INTERFACE_ADDR: Ipv4Addr = net_ip_v4!("192.168.2.1");
const SOURCE_SUBNET: fnet::Subnet = fidl_subnet!("192.168.255.1/16");
const TARGET_SUBNET: fnet::Subnet = fidl_subnet!("192.168.254.1/16");
const SOURCE_MAC_ADDRESS: fnet::MacAddress = fidl_mac!("02:00:00:00:00:01");
const TARGET_MAC_ADDRESS: fnet::MacAddress = fidl_mac!("02:00:00:00:00:02");
/// Arbitrary Ethernet frame type that doesn't correspond to any frames
/// normally sent by the netstack.
const ARBITRARY_ETHERTYPE: u16 = 1600;

/// An MTU that will result in an ICMP packet that is entirely used. Since the
/// payload length is rounded down to the nearest 8 bytes, this value minus the
/// IP header and the ethernet header must be divisible by 8.
const FULLY_USABLE_MTU: usize = 1490;

// Verifies that `FULLY_USABLE_MTU` corresponds to an MTU value that can have
// all bytes leveraged when sending a ping.
const_assert_eq!(
    max_icmp_payload_length(FULLY_USABLE_MTU),
    possible_icmp_payload_length(FULLY_USABLE_MTU)
);

/// Sends a single ICMP echo request from `source_realm` to `addr`, and waits
/// for the echo reply.
///
/// The body of the ICMP packet will be filled with `payload_length` 0 bytes.
/// Panics if the ping fails.
async fn expect_successful_ping<Ip: ping::FuchsiaIpExt>(
    source_realm: &netemul::TestRealm<'_>,
    addr: Ip::SockAddr,
    payload_length: usize,
) {
    let icmp_sock = source_realm.icmp_socket::<Ip>().await.expect("failed to create ICMP socket");

    let payload: Vec<u8> = vec![0; payload_length];
    let (mut sink, mut stream) = ping::new_unicast_sink_and_stream::<Ip, _, { u16::MAX as usize }>(
        &icmp_sock, &addr, &payload,
    );

    const SEQ: u16 = 1;
    sink.send(SEQ).await.expect("failed to send ping");
    let got = stream
        .try_next()
        .await
        .expect("echo reply failure")
        .expect("echo stream ended unexpectedly");
    assert_eq!(got, SEQ);
}

#[derive(Debug, PartialEq)]
struct IcmpPacketMetadata {
    source_address: fnet::IpAddress,
    target_address: fnet::IpAddress,
    payload_length: usize,
}

#[derive(Debug, PartialEq)]
enum IcmpEvent {
    EchoRequest(IcmpPacketMetadata),
    EchoReply(IcmpPacketMetadata),
}

// TODO(https://fxbug.dev/42173593): Replace this with a shared solution.
fn to_fidl_address(addr: net_types::ip::Ipv4Addr) -> fnet::IpAddress {
    fnet::IpAddress::Ipv4(fnet::Ipv4Address { addr: addr.ipv4_bytes() })
}

/// Extracts an Ipv4 `IcmpEvent` from the provided `data`.
fn extract_icmp_event(ipv4_packet: &packet_formats::ipv4::Ipv4Packet<&[u8]>) -> Option<IcmpEvent> {
    let fidl_src_ip = to_fidl_address(ipv4_packet.src_ip());
    let fidl_dst_ip = to_fidl_address(ipv4_packet.dst_ip());

    if ipv4_packet.proto() != packet_formats::ip::Ipv4Proto::Icmp {
        // Ignore non-ICMP packets.
        return None;
    }

    let mut payload = ipv4_packet.body();
    let icmp_packet = packet_formats::icmp::Icmpv4Packet::parse(
        &mut payload,
        packet_formats::icmp::IcmpParseArgs::new(ipv4_packet.src_ip(), ipv4_packet.dst_ip()),
    )
    .expect("failed to parse ICMP packet");

    match icmp_packet {
        packet_formats::icmp::Icmpv4Packet::EchoRequest(echo) => {
            Some(IcmpEvent::EchoRequest(IcmpPacketMetadata {
                source_address: fidl_src_ip,
                target_address: fidl_dst_ip,
                payload_length: echo.body().len(),
            }))
        }
        packet_formats::icmp::Icmpv4Packet::EchoReply(echo) => {
            Some(IcmpEvent::EchoReply(IcmpPacketMetadata {
                source_address: fidl_src_ip,
                target_address: fidl_dst_ip,
                payload_length: echo.body().len(),
            }))
        }
        packet_formats::icmp::Icmpv4Packet::DestUnreachable(_)
        | packet_formats::icmp::Icmpv4Packet::ParameterProblem(_)
        | packet_formats::icmp::Icmpv4Packet::Redirect(_)
        | packet_formats::icmp::Icmpv4Packet::TimeExceeded(_)
        | packet_formats::icmp::Icmpv4Packet::TimestampReply(_)
        | packet_formats::icmp::Icmpv4Packet::TimestampRequest(_) => None,
    }
}

/// Returns the maximum payload length of a packet given the `mtu`.
const fn max_icmp_payload_length(mtu: usize) -> usize {
    // Based on the following logic:
    // https://osscs.corp.google.com/fuchsia/fuchsia/+/main:third_party/golibs/vendor/gvisor.dev/gvisor/pkg/tcpip/network/ipv4/ipv4.go;l=402;drc=42abed01bbe5e1fb34f17f64f63f6e7ba27a74c7
    // The max payload length is rounded down to the nearest number that is
    // divisible by 8 bytes.
    ((mtu
        - packet_formats::ethernet::testutil::ETHERNET_HDR_LEN_NO_TAG
        - packet_formats::ipv4::testutil::IPV4_MIN_HDR_LEN)
        & !7)
        - ping::ICMP_HEADER_LEN
}

/// Returns the maximum number of bytes that an ICMP packet body could
/// potentially fill given an `mtu`.
///
/// The returned value excludes the relevant headers.
const fn possible_icmp_payload_length(mtu: usize) -> usize {
    mtu - packet_formats::ipv4::testutil::IPV4_MIN_HDR_LEN
        - packet_formats::ethernet::testutil::ETHERNET_HDR_LEN_NO_TAG
        - ping::ICMP_HEADER_LEN
}

/// Returns a stream of `IcmpEvent`s from the provided `fake_endpoint`.
fn icmp_event_stream<'a>(
    fake_endpoint: &'a netemul::TestFakeEndpoint<'_>,
) -> impl futures::Stream<Item = IcmpEvent> + 'a {
    fake_endpoint.frame_stream().map(|r| r.expect("error getting OnData event")).filter_map(
        |(data, _dropped)| async move {
            let mut data = &data[..];

            let eth = packet_formats::ethernet::EthernetFrame::parse(
                &mut data,
                packet_formats::ethernet::EthernetFrameLengthCheck::NoCheck,
            )
            .expect("error parsing ethernet frame");

            if eth.ethertype() != Some(packet_formats::ethernet::EtherType::Ipv4) {
                // Ignore non-IPv4 packets.
                return None;
            }

            let mut frame_body = eth.body();
            let ipv4_packet = packet_formats::ipv4::Ipv4Packet::parse(&mut frame_body, ())
                .expect("failed to parse IPv4 packet");

            match ipv4_packet.fragment_type() {
                packet_formats::ipv4::Ipv4FragmentType::InitialFragment => {
                    extract_icmp_event(&ipv4_packet)
                }
                // Only consider initial fragments as we are simply verifying that the
                // payload length is of an expected size for a given MTU. Note that
                // non-initial fragments do not contain an ICMP header and therefore
                // would not parse as an ICMP packet
                packet_formats::ipv4::Ipv4FragmentType::NonInitialFragment => None,
            }
        },
    )
}

#[netstack_test]
#[variant(N, Netstack)]
#[test_case(
    "fragmented",
    netemul::DEFAULT_MTU.into(),
    // Set the `payload_length` to a value larger than the `mtu`. This will
    // force the ICMP packets to be fragmented.
    (netemul::DEFAULT_MTU + 100).into(),
    max_icmp_payload_length(netemul::DEFAULT_MTU.into());
    "fragmented")]
#[test_case(
    "fully_used_mtu",
    FULLY_USABLE_MTU,
    possible_icmp_payload_length(FULLY_USABLE_MTU),
    possible_icmp_payload_length(FULLY_USABLE_MTU);
    "fully used mtu"
)]
async fn ping_succeeds_with_expected_payload<N: Netstack>(
    name: &str,
    sub_name: &str,
    mtu: usize,
    payload_length: usize,
    // Corresponds to the ICMP packet body length. This length excludes the
    // relevant headers (e.g. ICMP, ethernet, and IP). When fragmented, only the
    // first packet is considered.
    expected_payload_length: usize,
) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let network = sandbox
        .create_network(format!("network_{}_{}", name, sub_name))
        .await
        .expect("failed to create network");
    let source_realm = sandbox
        .create_netstack_realm::<N, _>(format!("source_{}_{}", name, sub_name))
        .expect("failed to create source realm");
    let target_realm = sandbox
        .create_netstack_realm::<N, _>(format!("target_{}_{}", name, sub_name))
        .expect("failed to reate target realm");
    let fake_ep = network.create_fake_endpoint().expect("failed to create fake endpoint");

    let mtu = u16::try_from(mtu).expect("failed to convert mtu to u16");
    let source_if = source_realm
        .join_network_with(
            &network,
            "source_ep",
            netemul::new_endpoint_config(mtu, Some(SOURCE_MAC_ADDRESS)),
            Default::default(),
        )
        .await
        .expect("failed to join network with source");
    source_if.add_address_and_subnet_route(SOURCE_SUBNET).await.expect("configure address");
    let target_if = target_realm
        .join_network_with(
            &network,
            "target_ep",
            netemul::new_endpoint_config(mtu, Some(TARGET_MAC_ADDRESS)),
            Default::default(),
        )
        .await
        .expect("failed to join network with target");
    target_if.add_address_and_subnet_route(TARGET_SUBNET).await.expect("configure address");

    // Add static ARP entries as we've observed flakes in CQ due to ARP timeouts
    // and ARP resolution is immaterial to this test.
    futures::stream::iter([
        (&source_realm, &source_if, TARGET_SUBNET.addr, TARGET_MAC_ADDRESS),
        (&target_realm, &target_if, SOURCE_SUBNET.addr, SOURCE_MAC_ADDRESS),
    ])
    .for_each_concurrent(None, |(realm, ep, addr, mac)| {
        realm.add_neighbor_entry(ep.id(), addr, mac).map(|r| r.expect("add_neighbor_entry failed"))
    })
    .await;

    expect_successful_ping::<Ipv4>(
        &source_realm,
        // TODO(https://github.com/rust-lang/rust/issues/67390): Make the
        // address const.
        std_socket_addr_v4!("192.168.254.1:0"),
        payload_length,
    )
    // Box because it's a large future.
    .boxed()
    .await;

    let icmp_event_stream = icmp_event_stream(&fake_ep);
    let mut icmp_event_stream = pin!(icmp_event_stream);

    assert_eq!(
        icmp_event_stream.next().await,
        Some(IcmpEvent::EchoRequest(IcmpPacketMetadata {
            source_address: SOURCE_SUBNET.addr,
            target_address: TARGET_SUBNET.addr,
            payload_length: expected_payload_length,
        })),
    );

    assert_eq!(
        icmp_event_stream.next().await,
        Some(IcmpEvent::EchoReply(IcmpPacketMetadata {
            source_address: TARGET_SUBNET.addr,
            target_address: SOURCE_SUBNET.addr,
            payload_length: expected_payload_length,
        })),
    );
}

#[netstack_test]
#[cfg(not(target_arch = "riscv64"))]
async fn starts_device_in_multicast_promiscuous_ns2(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm = sandbox
        .create_netstack_realm::<netstack_testing_common::realms::Netstack2, _>(name)
        .expect("failed to create source realm");

    let (tun, netdevice) = netstack_testing_common::devices::create_tun_device();
    let (tun_port, dev_port) = netstack_testing_common::devices::create_eth_tun_port(
        &tun,
        /* port_id */ 1,
        TARGET_MAC_ADDRESS,
    )
    .await;

    let mac_state_stream = futures::stream::unfold(
        (tun_port, Option::<fnet_tun::MacState>::None),
        |(tun_port, last_observed)| async move {
            loop {
                let fnet_tun::InternalState { mac, .. } =
                    tun_port.watch_state().await.expect("watch_state");
                let mac = mac.expect("missing mac state");
                if last_observed.as_ref().is_none_or(|l| l != &mac) {
                    let last_observed = Some(mac.clone());
                    break Some((mac, (tun_port, last_observed)));
                }
            }
        },
    );
    let mut mac_state_stream = pin!(mac_state_stream);

    assert_matches::assert_matches!(
        mac_state_stream.next().await,
        Some(fnet_tun::MacState {
            mode: Some(fhardware_network::MacFilterMode::MulticastFilter),
            multicast_filters: Some(mcast_filters),
            ..
        }) if mcast_filters == vec![]
    );

    let device_control = netstack_testing_common::devices::install_device(&realm, netdevice);
    let port_id = dev_port.get_info().await.expect("get info").id.expect("missing port id");
    let (control, server_end) =
        fidl::endpoints::create_proxy::<fnet_interfaces_admin::ControlMarker>();
    device_control
        .create_interface(&port_id, server_end, fnet_interfaces_admin::Options::default())
        .expect("create interface");

    // Read the interface ID to make sure device install succeeded.
    let _id: u64 = control.get_id().await.expect("get id");

    assert_matches::assert_matches!(
        mac_state_stream.next().await,
        Some(fnet_tun::MacState {
            mode: Some(fhardware_network::MacFilterMode::MulticastPromiscuous),
            multicast_filters: Some(mcast_filters),
            ..
        }) if mcast_filters == vec![]
    );
}

const ETH_HDR: usize = ETHERNET_HDR_LEN_NO_TAG;
const ETH_BODY: usize = ETHERNET_MIN_BODY_LEN_NO_TAG;
const MIN_ETH_FRAME: usize = ETHERNET_HDR_LEN_NO_TAG + ETH_BODY;
const LARGE_BODY: usize = ETH_BODY + 1024;
const LARGE_FRAME: usize = ETHERNET_HDR_LEN_NO_TAG + LARGE_BODY;
#[netstack_test]
#[variant(N, Netstack)]
#[test_case(fposix_socket_packet::Kind::Network, 1, 0; "network no padding")]
#[test_case(fposix_socket_packet::Kind::Link, 1, 0; "link no padding")]
#[test_case(fposix_socket_packet::Kind::Network, ETH_HDR, 0; "network header only")]
#[test_case(fposix_socket_packet::Kind::Link, ETH_HDR, 0; "link header only")]
#[test_case(fposix_socket_packet::Kind::Network, MIN_ETH_FRAME, ETH_BODY ; "network min eth")]
#[test_case(fposix_socket_packet::Kind::Link, MIN_ETH_FRAME, ETH_BODY; "link min eth")]
#[test_case(fposix_socket_packet::Kind::Network, LARGE_FRAME, LARGE_BODY ; "network large body")]
#[test_case(fposix_socket_packet::Kind::Link, LARGE_FRAME, LARGE_BODY; "link large body")]
async fn device_minimum_tx_frame_size<N: Netstack>(
    name: &str,
    socket_kind: fposix_socket_packet::Kind,
    min_tx_len: usize,
    expected_body_len: usize,
) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm = sandbox.create_netstack_realm::<N, _>(name).expect("failed to create source realm");

    let (tun, netdevice) =
        netstack_testing_common::devices::create_tun_device_with(fnet_tun::DeviceConfig {
            base: Some(fnet_tun::BaseDeviceConfig {
                min_tx_buffer_length: Some(min_tx_len.try_into().unwrap()),
                ..Default::default()
            }),
            blocking: Some(true),
            ..Default::default()
        });
    let (tun_port, dev_port) =
        netstack_testing_common::devices::create_eth_tun_port(&tun, 1, SOURCE_MAC_ADDRESS).await;

    let device_control = netstack_testing_common::devices::install_device(&realm, netdevice);
    let port_id = dev_port.get_info().await.expect("get info").id.expect("missing port id");
    let (control, server_end) =
        fidl::endpoints::create_proxy::<fnet_interfaces_admin::ControlMarker>();
    device_control
        .create_interface(&port_id, server_end, fnet_interfaces_admin::Options::default())
        .expect("create interface");
    assert_eq!(control.enable().await.expect("can enabled"), Ok(true));
    tun_port.set_online(true).await.expect("can set online");

    let id = control.get_id().await.expect("get ID");

    let state = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
        .expect("connect to protocol");
    netstack_testing_common::interfaces::wait_for_online(&state, id, true)
        .await
        .expect("waiting interface online");

    let id = NonZeroU64::new(id).unwrap();

    fn expected_frame(body_len: usize) -> Buf<Vec<u8>> {
        EthernetFrameBuilder::new(
            SOURCE_MAC_ADDRESS.into_ext(),
            TARGET_MAC_ADDRESS.into_ext(),
            ARBITRARY_ETHERTYPE.into(),
            0,
        )
        .wrap_body(Buf::new(vec![0; body_len], ..))
        .serialize_vec_outer(&mut NoOpSerializationContext)
        .unwrap()
        .into_inner()
    }

    // Send an ethernet frame with an empty body from a packet socket to see how
    // much padding is applied.
    let socket = realm.packet_socket(socket_kind).await.expect("can create socket");

    let message = match socket_kind {
        fposix_socket_packet::Kind::Link => expected_frame(0),
        fposix_socket_packet::Kind::Network => Buf::new(vec![], ..),
    };

    let sent = socket
        .send_to(
            message.as_ref(),
            &libc::sockaddr_ll::from(EthernetSockaddr {
                interface_id: Some(id),
                addr: TARGET_MAC_ADDRESS.into_ext(),
                protocol: ARBITRARY_ETHERTYPE.into(),
            })
            .into_sockaddr(),
        )
        .expect("can send");
    assert_eq!(sent, message.as_ref().len());

    let full_frame: Vec<u8> = async {
        loop {
            let frame = tun.read_frame().await.expect("can read").expect("has frame").data.unwrap();
            let mut body = &frame[..];

            match EthernetFrame::parse(&mut body, EthernetFrameLengthCheck::NoCheck) {
                Ok(ethernet) => {
                    if ethernet
                        .ethertype()
                        .is_some_and(|e| e == EtherType::from(ARBITRARY_ETHERTYPE))
                    {
                        break frame;
                    }
                }
                Err(e) => {
                    // We're looking for an Ethernet frame, so ignore anything
                    // that doesn't parse as one.
                    let _: ParseError = e;
                }
            }
        }
    }
    .on_timeout(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT, || panic!("failed to find sent frame"))
    .await;

    assert_eq!(full_frame, expected_frame(expected_body_len).as_ref());
}

/// Tests that frames parked in the TX queue wait for device buffers to become
/// available.
#[netstack_test]
#[variant(N, Netstack)]
async fn tx_queue_drops<N: Netstack>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm = sandbox.create_netstack_realm::<N, _>(name).expect("failed to create source realm");

    let (tun, netdevice) =
        netstack_testing_common::devices::create_tun_device_with(fnet_tun::DeviceConfig {
            blocking: Some(true),
            ..Default::default()
        });
    let (tun_port, dev_port) = netstack_testing_common::devices::create_eth_tun_port(
        &tun,
        /* port_id */ 1,
        TARGET_MAC_ADDRESS,
    )
    .await;

    let device_control = netstack_testing_common::devices::install_device(&realm, netdevice);
    let port_id = dev_port.get_info().await.expect("get info").id.expect("missing port id");
    let (control, server_end) =
        fidl::endpoints::create_proxy::<fnet_interfaces_admin::ControlMarker>();
    device_control
        .create_interface(&port_id, server_end, fnet_interfaces_admin::Options::default())
        .expect("create interface");
    assert_eq!(control.enable().await.expect("can enabled"), Ok(true));
    tun_port.set_online(true).await.expect("can set online");

    let id = control.get_id().await.expect("get ID");
    let state = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
        .expect("connect to protocol");
    netstack_testing_common::interfaces::wait_for_online(&state, id, true)
        .await
        .expect("waiting interface online");
    let id = NonZeroU64::new(id).unwrap();

    let socket =
        realm.packet_socket(fposix_socket_packet::Kind::Link).await.expect("can create socket");

    let message = EthernetFrameBuilder::new(
        SOURCE_MAC_ADDRESS.into_ext(),
        TARGET_MAC_ADDRESS.into_ext(),
        ARBITRARY_ETHERTYPE.into(),
        0,
    )
    .wrap_body(Buf::new(vec![0; 20], ..))
    .serialize_vec_outer(&mut NoOpSerializationContext)
    .unwrap()
    .into_inner();

    // Send more packets than the underlying device can handle at once, so we're
    // certain to exercise netstack waiting for tx buffers to be available
    // again.
    //
    // NB: There's some knowledge at a distance here that the maximum TX QUEUE
    // depth size is larger than the arbitrary multiplier here to the device
    // FIFO depth. If this value exceeded the maximum TX QUEUE size packets
    // would be dropped in netstack before reaching netdevice.
    let packet_send_count = 4 * fidl_fuchsia_net_tun::FIFO_DEPTH;

    for _ in 0..packet_send_count {
        let sent = socket
            .send_to(
                message.as_ref(),
                &libc::sockaddr_ll::from(EthernetSockaddr {
                    interface_id: Some(id),
                    addr: TARGET_MAC_ADDRESS.into_ext(),
                    protocol: ARBITRARY_ETHERTYPE.into(),
                })
                .into_sockaddr(),
            )
            .expect("can send");
        assert_eq!(sent, message.as_ref().len());
    }

    // We should see `packet_send_count` frames in total.
    let mut seen = 0;
    while seen < packet_send_count {
        let frame = tun
            .read_frame()
            .on_timeout(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT, || {
                panic!("failed to receive {}th frame", seen + 1)
            })
            .await
            .expect("can read")
            .expect("has frame")
            .data
            .unwrap();
        let mut body = &frame[..];
        let ethernet = EthernetFrame::parse(&mut body, EthernetFrameLengthCheck::NoCheck)
            .expect("bad ethernet frame");
        {
            if ethernet.ethertype().is_some_and(|e| e == EtherType::from(ARBITRARY_ETHERTYPE)) {
                seen += 1;
            }
        }
    }
}

fn get_mac_state_stream(
    tun_port: fnet_tun::PortProxy,
) -> impl futures::Stream<Item = fnet_tun::MacState> {
    futures::stream::unfold(
        (tun_port, Option::<fnet_tun::MacState>::None),
        |(tun_port, last_observed)| async move {
            loop {
                let fnet_tun::InternalState { mac, .. } =
                    tun_port.watch_state().await.expect("watch_state");
                let mac = mac.expect("missing mac state");
                if last_observed.as_ref().is_none_or(|l| l != &mac) {
                    let last_observed = Some(mac.clone());
                    break Some((mac, (tun_port, last_observed)));
                }
            }
        },
    )
}

async fn wait_for_mac_state_eq(
    mac_state_stream: &mut (impl futures::Stream<Item = fnet_tun::MacState> + Unpin),
    expected_mode: fhardware_network::MacFilterMode,
    mut expected_filters: Vec<fidl_fuchsia_net::MacAddress>,
) -> Option<()> {
    expected_filters.sort();
    let expected_state = fnet_tun::MacState {
        mode: Some(expected_mode),
        multicast_filters: Some(expected_filters),
        ..Default::default()
    };

    pin!(mac_state_stream.filter_map(|mut state| {
        let expected_state = &expected_state;
        async move {
            if let Some(filters) = state.multicast_filters.as_mut() {
                filters.sort();
            }

            println!("state is: {:?}; waiting for: {:?}", state, expected_state);

            (state == *expected_state).then_some(())
        }
    }))
    .next()
    .await
}

enum MulticastCleanup {
    LeaveGroup,
    DisableInterface,
    RemoveDevice,
    PhyOffline,
}

#[netstack_test]
#[test_case(MulticastCleanup::LeaveGroup)]
#[test_case(MulticastCleanup::DisableInterface)]
#[test_case(MulticastCleanup::RemoveDevice)]
#[test_case(MulticastCleanup::PhyOffline)] // This also results in the interface getting disabled.
async fn emits_multicast_groups_to_mac_addressing_service(
    name: &str,
    cleanup_strategy: MulticastCleanup,
) {
    const MULTICAST_ADDR: Ipv4Addr = net_ip_v4!("224.1.2.3");

    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm =
        sandbox.create_netstack_realm::<Netstack3, _>(name).expect("failed to create source realm");

    let (tun, netdevice) = netstack_testing_common::devices::create_tun_device();
    let (tun_port, dev_port) = netstack_testing_common::devices::create_eth_tun_port(
        &tun,
        /* port_id */ 1,
        TARGET_MAC_ADDRESS,
    )
    .await;

    let mut mac_state_stream = pin!(get_mac_state_stream(tun_port.clone()));

    let mcast_filters = assert_matches::assert_matches!(
        mac_state_stream.next().await,
        Some(fnet_tun::MacState {
            mode: Some(fhardware_network::MacFilterMode::MulticastFilter),
            multicast_filters: Some(mcast_filters),
            ..
        }) => mcast_filters
    );
    assert_eq!(mcast_filters, vec![]);

    let device_control = netstack_testing_common::devices::install_device(&realm, netdevice);
    let port_id = dev_port.get_info().await.expect("get info").id.expect("missing port id");
    let (control_proxy, server_end) =
        fidl::endpoints::create_proxy::<fnet_interfaces_admin::ControlMarker>();
    device_control
        .create_interface(&port_id, server_end, fnet_interfaces_admin::Options::default())
        .expect("create interface");
    let control = fnet_interfaces_ext::admin::Control::new(control_proxy);

    // Read the interface ID to make sure device install succeeded.
    let id: u64 = control.get_id().await.expect("get id");

    assert_eq!(control.enable().await.expect("can enabled"), Ok(true));
    tun_port.set_online(true).await.expect("can set online");
    let state = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
        .expect("connect to protocol");
    netstack_testing_common::interfaces::wait_for_online(&state, id, true)
        .await
        .expect("waiting interface online");

    let _address_state_provider = netstack_testing_common::interfaces::add_address_wait_assigned(
        &control,
        fnet::Subnet { addr: to_fidl_address(INTERFACE_ADDR), prefix_len: 24 },
        fidl_fuchsia_net_interfaces_admin::AddressParameters {
            perform_dad: Some(false),
            ..Default::default()
        },
    )
    .await
    .expect("add subnet address");

    let ipv6_ll = netstack_testing_common::interfaces::wait_for_v6_ll(&state, id)
        .await
        .expect("wait LL address");

    let default_mcast_filters = [
        Ipv4::ALL_SYSTEMS_MULTICAST_ADDRESS.into(),
        Ipv6::ALL_NODES_LINK_LOCAL_MULTICAST_ADDRESS.into(),
        ipv6_ll.to_solicited_node_address().into(),
    ]
    .map(|addr: MulticastAddr<Mac>| fidl_fuchsia_net::MacAddress { octets: addr.bytes() });

    // Watch until all the default groups are joined.
    wait_for_mac_state_eq(
        &mut mac_state_stream,
        fhardware_network::MacFilterMode::MulticastFilter,
        default_mcast_filters.to_vec(),
    )
    .await
    .expect("installing default filters");

    let sock = fasync::net::UdpSocket::bind_in_realm(
        &realm,
        std::net::SocketAddrV4::new(std::net::Ipv4Addr::UNSPECIFIED, 0).into(),
    )
    .await
    .expect("error creating socket");

    sock.as_ref()
        .join_multicast_v4(&MULTICAST_ADDR.into(), &INTERFACE_ADDR.into())
        .expect("error joining multicast group");

    let multicast_mac = fidl_fuchsia_net::MacAddress {
        octets: MulticastAddr::<Mac>::from(MulticastAddr::new(MULTICAST_ADDR).unwrap()).bytes(),
    };

    let mut mcast_filters = assert_matches::assert_matches!(
        mac_state_stream.next().await,
        Some(fnet_tun::MacState {
            mode: Some(fhardware_network::MacFilterMode::MulticastFilter),
            multicast_filters: Some(mcast_filters),
            ..
        }) => mcast_filters
    );
    mcast_filters.sort();

    let mut expected_mcast_filters = [default_mcast_filters.as_slice(), &[multicast_mac]].concat();
    expected_mcast_filters.sort();
    assert_eq!(mcast_filters, expected_mcast_filters);

    let expected_mcast_filters = match cleanup_strategy {
        MulticastCleanup::LeaveGroup => {
            sock.as_ref()
                .leave_multicast_v4(&MULTICAST_ADDR.into(), &INTERFACE_ADDR.into())
                .expect("error leaving multicast group");

            default_mcast_filters.to_vec()
        }
        MulticastCleanup::DisableInterface => {
            let did_disable = control
                .disable()
                .await
                .expect("send disable interface request")
                .expect("disable interface");
            assert!(did_disable);

            // TODO(https://fxbug.dev/435009352): All groups should be left on interface disable.
            vec![multicast_mac]
        }
        MulticastCleanup::PhyOffline => {
            tun_port.set_online(false).await.expect("can set offline");

            // TODO(https://fxbug.dev/435009352): All groups should be left on interface disable.
            vec![multicast_mac]
        }
        MulticastCleanup::RemoveDevice => {
            control
                .remove()
                .await
                .expect("send remove interface request")
                .expect("remove interface");
            assert_matches::assert_matches!(
                control.wait_termination().await,
                fidl_fuchsia_net_interfaces_ext::admin::TerminalError::Terminal(
                    fidl_fuchsia_net_interfaces_admin::InterfaceRemovedReason::User
                )
            );

            vec![]
        }
    };

    wait_for_mac_state_eq(
        &mut mac_state_stream,
        fhardware_network::MacFilterMode::MulticastFilter,
        expected_mcast_filters,
    )
    .await
    .expect("cleanup multicast group");
}

#[netstack_test]
async fn fallback_to_multicast_promiscuous(name: &str) {
    fn get_multicast_addr(last: u64) -> MulticastAddr<Ipv6Addr> {
        assert!((Ipv6Addr::BYTES * 8 - Ipv6::MULTICAST_SUBNET.prefix()) as u32 > u8::BITS);
        let mut bytes = Ipv6::MULTICAST_SUBNET.network().ipv6_bytes();
        // Don't use the reserved (0) scope for these addresses.
        bytes[1] = net_types::ip::Ipv6Scope::MULTICAST_SCOPE_ID_GLOBAL;
        bytes[8..].copy_from_slice(&last.to_be_bytes());
        MulticastAddr::new(Ipv6Addr::from_bytes(bytes)).unwrap()
    }

    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm =
        sandbox.create_netstack_realm::<Netstack3, _>(name).expect("failed to create source realm");

    let (tun, netdevice) = netstack_testing_common::devices::create_tun_device();
    let (tun_port, dev_port) = netstack_testing_common::devices::create_eth_tun_port(
        &tun,
        /* port_id */ 1,
        TARGET_MAC_ADDRESS,
    )
    .await;

    let mut mac_state_stream = pin!(get_mac_state_stream(tun_port.clone()));

    let mcast_filters = assert_matches::assert_matches!(
        mac_state_stream.next().await,
        Some(fnet_tun::MacState {
            mode: Some(fhardware_network::MacFilterMode::MulticastFilter),
            multicast_filters: Some(mcast_filters),
            ..
        }) => mcast_filters
    );
    assert_eq!(mcast_filters, vec![]);

    let device_control = netstack_testing_common::devices::install_device(&realm, netdevice);
    let port_id = dev_port.get_info().await.expect("get info").id.expect("missing port id");
    let (control_proxy, server_end) =
        fidl::endpoints::create_proxy::<fnet_interfaces_admin::ControlMarker>();
    device_control
        .create_interface(&port_id, server_end, fnet_interfaces_admin::Options::default())
        .expect("create interface");
    let control = fnet_interfaces_ext::admin::Control::new(control_proxy);

    // Read the interface ID to make sure device install succeeded.
    let id: u64 = control.get_id().await.expect("get id");

    assert_eq!(control.enable().await.expect("can enabled"), Ok(true));
    tun_port.set_online(true).await.expect("can set online");
    let state = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
        .expect("connect to protocol");
    netstack_testing_common::interfaces::wait_for_online(&state, id, true)
        .await
        .expect("waiting interface online");

    let ipv6_ll = netstack_testing_common::interfaces::wait_for_v6_ll(&state, id)
        .await
        .expect("wait LL address");

    let default_mcast_filters = [
        Ipv4::ALL_SYSTEMS_MULTICAST_ADDRESS.into(),
        Ipv6::ALL_NODES_LINK_LOCAL_MULTICAST_ADDRESS.into(),
        ipv6_ll.to_solicited_node_address().into(),
    ]
    .map(|addr: MulticastAddr<Mac>| fidl_fuchsia_net::MacAddress { octets: addr.bytes() });

    // Watch until all the default groups are joined.
    println!("Waiting for default filters");
    wait_for_mac_state_eq(
        &mut mac_state_stream,
        fhardware_network::MacFilterMode::MulticastFilter,
        default_mcast_filters.to_vec(),
    )
    .await
    .expect("installing default filters");

    let sock = fasync::net::UdpSocket::bind_in_realm(
        &realm,
        std::net::SocketAddrV6::new(std::net::Ipv6Addr::UNSPECIFIED, 0, 0, 0).into(),
    )
    .await
    .expect("error creating socket");

    let mut multicast_groups = (default_mcast_filters.len().try_into().unwrap()
        ..fhardware_network_driver::MAX_MAC_FILTER)
        .map(|num| {
            // Shift everything over to avoid colliding with the all
            // nodes address (ff02::1) or any other well known addresses
            // when we narrow to 32 significant bits in the MAC addresses.
            num + 0xff
        })
        .map(|num| get_multicast_addr(num))
        .collect::<Vec<MulticastAddr<Ipv6Addr>>>();

    let mut expected_filters = default_mcast_filters.to_vec();

    for multicast_addr in multicast_groups.iter() {
        let multicast_mac = fidl_fuchsia_net::MacAddress {
            octets: MulticastAddr::<Mac>::from(multicast_addr).bytes(),
        };
        expected_filters.push(multicast_mac);
        println!("Joining {}", **multicast_addr);

        sock.as_ref()
            .join_multicast_v6(&(**multicast_addr).into(), id.try_into().unwrap())
            .expect("error joining multicast group");
    }

    wait_for_mac_state_eq(
        &mut mac_state_stream,
        fhardware_network::MacFilterMode::MulticastFilter,
        expected_filters.to_vec(),
    )
    .await
    .expect("joining the list of filters");

    println!("Joining one more multicast group");
    let multicast_addr = get_multicast_addr(fhardware_network_driver::MAX_MAC_FILTER + 1);
    multicast_groups.push(multicast_addr);
    sock.as_ref()
        .join_multicast_v6(&(*multicast_addr).into(), id.try_into().unwrap())
        .expect("error joining multicast group");

    let mcast_filters = assert_matches::assert_matches!(
        mac_state_stream.next().await,
        Some(fnet_tun::MacState {
            mode: Some(fhardware_network::MacFilterMode::MulticastPromiscuous),
            multicast_filters: Some(mcast_filters),
            ..
        }) => mcast_filters
    );
    assert_eq!(mcast_filters, vec![]);

    for multicast_addr in multicast_groups.iter() {
        sock.as_ref()
            .leave_multicast_v6(&(**multicast_addr).into(), id.try_into().unwrap())
            .expect("error leaving multicast group");
    }

    // It should remain in the promiscuous state and never change until teardown.
    // TODO(https://fxbug.dev/435532334): this behavior is less than ideal,
    // instead we should recover back out of promiscuous mode.
    assert_eq!(
        mac_state_stream
            .next()
            .on_timeout(fasync::MonotonicInstant::after(ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT), || {
                None
            })
            .await,
        None
    );
}

#[netstack_test]
async fn ethernet_frame_zero_padding(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm =
        sandbox.create_netstack_realm::<Netstack3, _>(name).expect("failed to create realm");

    const MIN_TX_LEN: usize = 60;
    let (tun, netdevice) =
        netstack_testing_common::devices::create_tun_device_with(fnet_tun::DeviceConfig {
            base: Some(fnet_tun::BaseDeviceConfig {
                min_tx_buffer_length: Some(MIN_TX_LEN.try_into().unwrap()),
                ..Default::default()
            }),
            blocking: Some(true),
            ..Default::default()
        });
    let (tun_port, dev_port) =
        netstack_testing_common::devices::create_eth_tun_port(&tun, 1, SOURCE_MAC_ADDRESS).await;

    let device_control = netstack_testing_common::devices::install_device(&realm, netdevice);
    let port_id = dev_port.get_info().await.expect("get info").id.expect("missing port id");
    let (control_proxy, server_end) =
        fidl::endpoints::create_proxy::<fnet_interfaces_admin::ControlMarker>();
    device_control
        .create_interface(&port_id, server_end, fnet_interfaces_admin::Options::default())
        .expect("create interface");
    let control = fnet_interfaces_ext::admin::Control::new(control_proxy);

    assert_eq!(control.enable().await.expect("can enable"), Ok(true));
    tun_port.set_online(true).await.expect("can set online");

    let id = control.get_id().await.expect("get ID");
    let state = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
        .expect("connect to protocol");
    netstack_testing_common::interfaces::wait_for_online(&state, id, true)
        .await
        .expect("waiting interface online");

    // Configure IP address.
    let _address_state_provider = netstack_testing_common::interfaces::add_address_wait_assigned(
        &control,
        fnet::Subnet { addr: to_fidl_address(INTERFACE_ADDR), prefix_len: 24 },
        fidl_fuchsia_net_interfaces_admin::AddressParameters {
            perform_dad: Some(false),
            add_subnet_route: Some(true),
            ..Default::default()
        },
    )
    .await
    .expect("add subnet address");

    // Add a static ARP entry for the target.
    let target_ip = net_ip_v4!("192.168.2.2");
    realm
        .add_neighbor_entry(
            id,
            fnet::IpAddress::Ipv4(fnet::Ipv4Address { addr: target_ip.ipv4_bytes() }),
            TARGET_MAC_ADDRESS,
        )
        .await
        .expect("add_neighbor_entry failed");

    // Create a UDP socket and send a small packet.
    let sock = fasync::net::UdpSocket::bind_in_realm(
        &realm,
        std::net::SocketAddrV4::new(std::net::Ipv4Addr::from(INTERFACE_ADDR.ipv4_bytes()), 0)
            .into(),
    )
    .await
    .expect("failed to create socket");

    // Send multiple large packets filled with non-zero bytes to dirty all TX buffers
    // in the netstack's pool.
    let large_msg = vec![0x55; 1000];
    for _ in 0..300 {
        let target_addr =
            std::net::SocketAddrV4::new(std::net::Ipv4Addr::from(target_ip.ipv4_bytes()), 1234);
        let sent = sock.send_to(&large_msg, target_addr.into()).await.expect("failed to send");
        assert_eq!(sent, large_msg.len());
    }
    for _ in 0..300 {
        // Read and discard the frame from tun to complete it and return it to the pool.
        let _frame = tun.read_frame().await.expect("can read").expect("has frame");
    }

    const MSG: &[u8] = b"hello";
    let target_addr =
        std::net::SocketAddrV4::new(std::net::Ipv4Addr::from(target_ip.ipv4_bytes()), 1235);
    let sent = sock.send_to(MSG, target_addr.into()).await.expect("failed to send");
    assert_eq!(sent, MSG.len());

    // Read the frame from tun.
    let full_frame: Vec<u8> = async {
        loop {
            let frame = tun.read_frame().await.expect("can read").expect("has frame").data.unwrap();
            let mut body = &frame[..];

            if let Ok(ethernet) = EthernetFrame::parse(&mut body, EthernetFrameLengthCheck::NoCheck)
            {
                if ethernet.ethertype() == Some(EtherType::Ipv4) {
                    let mut eth_body = ethernet.body();
                    if let Ok(ipv4) = packet_formats::ipv4::Ipv4Packet::parse(&mut eth_body, ()) {
                        let src_ip = ipv4.src_ip();
                        let dst_ip = ipv4.dst_ip();
                        let mut ip_body = ipv4.body();
                        if let Ok(udp) = packet_formats::udp::UdpPacket::parse(
                            &mut ip_body,
                            packet_formats::udp::UdpParseArgs::new(src_ip, dst_ip),
                        ) {
                            if udp.dst_port().get() == 1235 {
                                break frame;
                            }
                        }
                    }
                }
            }
        }
    }
    .on_timeout(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT, || panic!("failed to find sent frame"))
    .await;

    // Verify the frame length is padded to at least MIN_TX_LEN.
    assert!(full_frame.len() >= MIN_TX_LEN, "frame length {} < {}", full_frame.len(), MIN_TX_LEN);

    const MSG_OFFSET: usize = ETHERNET_HDR_LEN_NO_TAG
        + packet_formats::ipv4::HDR_PREFIX_LEN
        + packet_formats::udp::HEADER_BYTES;
    const UNPADDED_LEN: usize = MSG_OFFSET + MSG.len();
    const EXPECTED_PADDING_LEN: usize = MIN_TX_LEN - UNPADDED_LEN;

    // Verify the message content matches.
    assert_eq!(&full_frame[MSG_OFFSET..UNPADDED_LEN], MSG);

    // The frame should be padded with zeros.
    assert!(full_frame.len() >= UNPADDED_LEN);
    let padding = &full_frame[UNPADDED_LEN..];
    assert_eq!(padding, &[0; EXPECTED_PADDING_LEN], "padding is not zero: {:?}", padding);
}

#[netstack_test]
async fn tx_large_buffer_small_payload(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm =
        sandbox.create_netstack_realm::<Netstack3, _>(name).expect("failed to create source realm");

    let (tun, netdevice) =
        netstack_testing_common::devices::create_tun_device_with(fnet_tun::DeviceConfig {
            blocking: Some(true),
            ..Default::default()
        });
    let (tun_port, dev_port) =
        netstack_testing_common::devices::create_eth_tun_port(&tun, 1, SOURCE_MAC_ADDRESS).await;

    let device_control = netstack_testing_common::devices::install_device(&realm, netdevice);
    let port_id = dev_port.get_info().await.expect("get info").id.expect("missing port id");
    let (control_proxy, server_end) =
        fidl::endpoints::create_proxy::<fnet_interfaces_admin::ControlMarker>();
    device_control
        .create_interface(&port_id, server_end, fnet_interfaces_admin::Options::default())
        .expect("create interface");
    let control = fnet_interfaces_ext::admin::Control::new(control_proxy);

    assert_eq!(control.enable().await.expect("can enabled"), Ok(true));
    tun_port.set_online(true).await.expect("can set online");

    let id = control.get_id().await.expect("get ID");

    let state = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
        .expect("connect to protocol");
    netstack_testing_common::interfaces::wait_for_online(&state, id, true)
        .await
        .expect("waiting interface online");

    let source_ip = net_ip_v4!("192.168.2.1");
    let target_ip = net_ip_v4!("192.168.2.2");
    let prefix_len = 24;

    let _address_state_provider = netstack_testing_common::interfaces::add_address_wait_assigned(
        &control,
        fnet::Subnet { addr: to_fidl_address(source_ip), prefix_len },
        fidl_fuchsia_net_interfaces_admin::AddressParameters {
            add_subnet_route: Some(true),
            perform_dad: Some(false),
            ..Default::default()
        },
    )
    .await
    .expect("add subnet address");

    realm
        .add_neighbor_entry(id, to_fidl_address(target_ip), TARGET_MAC_ADDRESS)
        .await
        .expect("add neighbor entry failed");

    let sock = fasync::net::UdpSocket::bind_in_realm(
        &realm,
        std::net::SocketAddr::V4(std::net::SocketAddrV4::new(std::net::Ipv4Addr::UNSPECIFIED, 0)),
    )
    .await
    .expect("error creating socket");

    let payload = b"hello";
    let target_addr = std::net::SocketAddr::V4(std::net::SocketAddrV4::new(target_ip.into(), 1234));

    // Send 3x the FIFO depth to ensure we exercise the vec buffer case.
    let packet_send_count = 3 * usize::from(fidl_fuchsia_net_tun::FIFO_DEPTH);

    for _ in 0..packet_send_count {
        let sent = sock.send_to(payload, target_addr).await.expect("send_to failed");
        assert_eq!(sent, payload.len());
    }

    let mut received_count = 0;
    while received_count < packet_send_count {
        let frame = tun.read_frame().await.expect("can read").expect("has frame");
        let frame_data = frame.data.expect("frame has no data");
        let mut data = &frame_data[..];
        let eth = EthernetFrame::parse(&mut data, EthernetFrameLengthCheck::NoCheck)
            .expect("failed to parse ethernet frame");
        // Note: There are other packets being sent besides the UDP packets,
        // however it is very unlikely that the frame size violation only
        // happens to those packets but not the UDP one, so we ignore those
        // packets.
        match eth.ethertype() {
            Some(EtherType::Ipv4) => {
                let mut body = eth.body();
                let ipv4 = packet_formats::ipv4::Ipv4Packet::parse(&mut body, ())
                    .expect("failed to parse IPv4 packet");
                let meta = packet::ParsablePacket::parse_metadata(&ipv4);
                let expected = ETHERNET_HDR_LEN_NO_TAG + meta.header_len() + meta.body_len();
                if ipv4.proto()
                    == packet_formats::ip::Ipv4Proto::Proto(packet_formats::ip::IpProto::Udp)
                    && ipv4.dst_ip() == target_ip
                {
                    assert_eq!(&ipv4.body()[8..], payload, "payload mismatch");
                    received_count += 1;
                }
                assert_eq!(
                    frame_data.len(),
                    expected,
                    "received_count = {}, frame_data len = {}, expected = {}",
                    received_count,
                    frame_data.len(),
                    expected
                );
            }
            _ => {}
        }
    }
}
