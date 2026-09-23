// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_debug as fnet_debug;
use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
use test_case::test_case;

use fuchsia_async as fasync;
use futures::{AsyncReadExt as _, AsyncWriteExt as _};
use net_declare::{fidl_subnet, std_ip};
use net_types::ip::{IpAddress, IpVersionMarker, Ipv4, Ipv6};
use netemul::{RealmTcpListener as _, RealmTcpStream as _, RealmUdpSocket as _};
use netstack_testing_common::realms::{Netstack3, TestRealmExt as _, TestSandboxExt as _};
use netstack_testing_macros::netstack_test;
use packet::{
    FromRaw as _, InnerPacketBuilder as _, NestablePacketBuilder as _, ParsablePacket as _,
    Serializer as _,
};
use packet_formats::TransportChecksumAction;
use packet_formats::ethernet::EthernetFrameLengthCheck;
use packet_formats::ip::{IpExt, IpProto};
use packet_formats::tcp::{
    CHECKSUM_OFFSET as TCP_CHECKSUM_OFFSET, TcpParseArgs, TcpSegment, TcpSegmentRaw,
};
use packet_formats::testutil::{
    ForceChecksumAction, ForceSkipChecksumValidation, parse_ip_packet_in_ethernet_frame,
};
use packet_formats::udp::{
    CHECKSUM_OFFSET as UDP_CHECKSUM_OFFSET, UdpPacket, UdpPacketRaw, UdpParseArgs,
};

const PORT: u16 = 9875;
const PAYLOAD: [u8; 4] = [1, 2, 3, 4];

const CLIENT_MAC: fnet::MacAddress = fnet::MacAddress { octets: [2, 0, 0, 0, 0, 1] };
const SERVER_MAC: fnet::MacAddress = fnet::MacAddress { octets: [2, 0, 0, 0, 0, 2] };

async fn send_and_recv_udp(
    realm: &netemul::TestRealm<'_>,
    bind_addr: std::net::SocketAddr,
    payload: &[u8],
) {
    let sock =
        fasync::net::UdpSocket::bind_in_realm(realm, bind_addr).await.expect("create socket");
    let sent = sock.send_to(payload, bind_addr).await.expect("send_to failed");
    assert_eq!(sent, payload.len());

    let mut recv_buf = vec![0u8; payload.len()];
    let (received, from_addr) = sock.recv_from(&mut recv_buf).await.expect("recv_from failed");
    assert_eq!(received, payload.len());
    assert_eq!(recv_buf.as_slice(), payload);
    assert_eq!(from_addr, bind_addr);
}

fn parse_udp_packet_raw<'a, I: IpExt>(
    buf: &'a [u8],
) -> Option<(UdpPacketRaw<&'a [u8]>, I::Addr, I::Addr)> {
    let (mut body, _, _, src_ip, dst_ip, proto, _) =
        parse_ip_packet_in_ethernet_frame::<I>(buf, EthernetFrameLengthCheck::NoCheck).ok()?;
    if proto != IpProto::Udp.into() {
        return None;
    }
    let udp = UdpPacketRaw::parse(&mut body, IpVersionMarker::<I>::default()).ok()?;
    Some((udp, src_ip, dst_ip))
}

fn verify_udp_raw_packet<A: IpAddress>(
    udp: UdpPacketRaw<&[u8]>,
    src_ip: A,
    dst_ip: A,
    expect_valid_checksum: bool,
) {
    let packet = UdpPacket::try_from_raw_with(
        udp,
        UdpParseArgs::with_context(src_ip, dst_ip, ForceSkipChecksumValidation(true)),
    )
    .unwrap();

    let header_bytes = packet.as_bytes()[0];
    let actual_checksum = &header_bytes[UDP_CHECKSUM_OFFSET..UDP_CHECKSUM_OFFSET + 2];

    let action = if expect_valid_checksum {
        TransportChecksumAction::ComputeFull
    } else {
        TransportChecksumAction::ComputePartial
    };

    let serializer = packet.builder(src_ip, dst_ip).wrap_body(packet.body().into_serializer());
    let new_buf = serializer.serialize_vec_outer(&mut ForceChecksumAction(action)).unwrap();
    let expected_checksum = &new_buf.as_ref()[UDP_CHECKSUM_OFFSET..UDP_CHECKSUM_OFFSET + 2];

    assert_eq!(actual_checksum, expected_checksum);
}

async fn send_and_recv_tcp(
    realm: &netemul::TestRealm<'_>,
    addr: std::net::SocketAddr,
    payload: &[u8],
) {
    let listener =
        fasync::net::TcpListener::listen_in_realm(realm, addr).await.expect("listen failed");

    let client_fut = async {
        let mut stream =
            fasync::net::TcpStream::connect_in_realm(realm, addr).await.expect("connect failed");
        stream.write_all(payload).await.expect("write failed");
        stream.flush().await.expect("flush failed");
    };

    let server_fut = async {
        let (_, mut stream, _) = listener.accept().await.expect("accept failed");
        let mut recv_buf = vec![0u8; payload.len()];
        stream.read_exact(&mut recv_buf).await.expect("read failed");
        assert_eq!(recv_buf.as_slice(), payload);
    };

    futures::join!(client_fut, server_fut);
}

fn parse_tcp_segment_raw<'a, I: IpExt>(
    buf: &'a [u8],
) -> Option<(TcpSegmentRaw<&'a [u8]>, I::Addr, I::Addr)> {
    let (mut body, _, _, src_ip, dst_ip, proto, _) =
        parse_ip_packet_in_ethernet_frame::<I>(buf, EthernetFrameLengthCheck::NoCheck).ok()?;
    if proto != IpProto::Tcp.into() {
        return None;
    }
    let tcp = TcpSegmentRaw::parse(&mut body, ()).ok()?;
    Some((tcp, src_ip, dst_ip))
}

fn verify_tcp_raw_segment<A: IpAddress>(
    tcp: TcpSegmentRaw<&[u8]>,
    src_ip: A,
    dst_ip: A,
    expect_valid_checksum: bool,
) {
    let segment = TcpSegment::try_from_raw_with(
        tcp,
        TcpParseArgs::with_context(src_ip, dst_ip, ForceSkipChecksumValidation(true)),
    )
    .unwrap();

    let header_bytes = segment.as_bytes()[0];
    let actual_checksum = &header_bytes[TCP_CHECKSUM_OFFSET..TCP_CHECKSUM_OFFSET + 2];

    let action = if expect_valid_checksum {
        TransportChecksumAction::ComputeFull
    } else {
        TransportChecksumAction::ComputePartial
    };

    let serializer = segment.builder(src_ip, dst_ip).wrap_body(segment.body().into_serializer());
    let new_buf = serializer.serialize_vec_outer(&mut ForceChecksumAction(action)).unwrap();
    let expected_checksum = &new_buf.as_ref()[TCP_CHECKSUM_OFFSET..TCP_CHECKSUM_OFFSET + 2];

    assert_eq!(actual_checksum, expected_checksum);
}

/// Helper function to capture and verify UDP packets on an interface.
/// `generate_traffic` is expected to send exactly one UDP packet over the
/// interface, and `expect_valid_checksum` controls whether the test expects the
/// packet to have a valid checksum.
async fn capture_and_verify_udp<I: IpExt>(
    realm: &netemul::TestRealm<'_>,
    interface_id: u64,
    expect_valid_checksum: bool,
    generate_traffic: impl Future<Output = ()>,
) {
    let provider = realm
        .connect_to_protocol::<fnet_debug::PacketCaptureProviderMarker>()
        .expect("connect to PacketCaptureProvider");

    let common_params = fnet_debug::CommonPacketCaptureParams {
        interfaces: Some(fnet_debug::InterfaceSpecifier::InterfaceIds(vec![interface_id])),
        ..Default::default()
    };

    let rolling_params = fnet_debug::RollingPacketCaptureParams::default();

    let rolling_client = provider
        .start_rolling(common_params, &rolling_params)
        .await
        .expect("start_rolling FIDL error")
        .expect("start_rolling error")
        .into_proxy();

    generate_traffic.await;

    // Stop and download.
    let (file_client, file_server) = fidl::endpoints::create_endpoints();
    rolling_client.stop_and_download(file_server).expect("stop_and_download failed");

    let file_proxy = file_client.into_proxy();
    let bytes = fuchsia_fs::file::read(&file_proxy).await.expect("read file failed");
    let cap = pcap::parse_pcapng(&bytes).unwrap();

    let packets: Vec<_> =
        cap.packet_blocks().collect::<Result<Vec<_>, _>>().expect("EPB parse error");

    let mut udp_packets_found = 0;
    for epb in packets {
        let buf = &epb.packet_data;
        if let Some((udp, src_ip, dst_ip)) = parse_udp_packet_raw::<I>(buf) {
            if udp.dst_port().map(|p| p.get()) == Some(PORT) {
                verify_udp_raw_packet(udp, src_ip, dst_ip, expect_valid_checksum);
                udp_packets_found += 1;
            }
        }
    }
    assert_eq!(udp_packets_found, 1, "Expected to find exactly 1 UDP packet with port {}", PORT);
}

/// Helper function to capture and verify TCP segments on an interface.
/// `generate_traffic` is expected to send at least one TCP segment over the
/// interface, and `expect_valid_checksum` controls whether the test expects the
/// segments to have valid checksums.
async fn capture_and_verify_tcp<I: IpExt>(
    realm: &netemul::TestRealm<'_>,
    interface_id: u64,
    expect_valid_checksum: bool,
    generate_traffic: impl Future<Output = ()>,
) {
    let provider = realm
        .connect_to_protocol::<fnet_debug::PacketCaptureProviderMarker>()
        .expect("connect to PacketCaptureProvider");

    let common_params = fnet_debug::CommonPacketCaptureParams {
        interfaces: Some(fnet_debug::InterfaceSpecifier::InterfaceIds(vec![interface_id])),
        ..Default::default()
    };

    let rolling_params = fnet_debug::RollingPacketCaptureParams::default();

    let rolling_client = provider
        .start_rolling(common_params, &rolling_params)
        .await
        .expect("start_rolling FIDL error")
        .expect("start_rolling error")
        .into_proxy();

    generate_traffic.await;

    // Stop and download.
    let (file_client, file_server) = fidl::endpoints::create_endpoints();
    rolling_client.stop_and_download(file_server).expect("stop_and_download failed");

    let file_proxy = file_client.into_proxy();
    let bytes = fuchsia_fs::file::read(&file_proxy).await.expect("read file failed");
    let cap = pcap::parse_pcapng(&bytes).unwrap();

    let packets: Vec<_> =
        cap.packet_blocks().collect::<Result<Vec<_>, _>>().expect("EPB parse error");

    let mut tcp_packets_found = 0;
    for epb in packets {
        let buf = &epb.packet_data;
        if let Some((tcp, src_ip, dst_ip)) = parse_tcp_segment_raw::<I>(buf) {
            if tcp.flow_header().src_dst().1 == PORT {
                verify_tcp_raw_segment(tcp, src_ip, dst_ip, expect_valid_checksum);
                tcp_packets_found += 1;
            }
        }
    }
    assert!(tcp_packets_found > 0, "Expected to find TCP packets with port {}", PORT);
}

#[netstack_test]
#[variant(I, Ip)]
async fn test_loopback_checksum_skipped_udp<I: IpExt>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    let fnet_interfaces_ext::Properties { id: loopback_id, .. } = realm
        .loopback_properties()
        .await
        .expect("failed to get loopback properties")
        .expect("loopback not found");

    let loopback_ip = match I::VERSION {
        net_types::ip::IpVersion::V4 => std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        net_types::ip::IpVersion::V6 => std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
    };

    capture_and_verify_udp::<I>(
        &realm,
        loopback_id.get(),
        // We only compute a partial checksum over the loopback interface, so
        // checksum validation should fail.
        false,
        send_and_recv_udp(&realm, (loopback_ip, PORT).into(), &PAYLOAD),
    )
    .await;
}

#[netstack_test]
#[variant(I, Ip)]
async fn test_loopback_checksum_skipped_tcp<I: IpExt>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    let fnet_interfaces_ext::Properties { id: loopback_id, .. } = realm
        .loopback_properties()
        .await
        .expect("failed to get loopback properties")
        .expect("loopback not found");

    let loopback_ip = match I::VERSION {
        net_types::ip::IpVersion::V4 => std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        net_types::ip::IpVersion::V6 => std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
    };

    capture_and_verify_tcp::<I>(
        &realm,
        loopback_id.get(),
        // We only compute a partial checksum over the loopback interface, so
        // checksum validation should fail.
        false,
        send_and_recv_tcp(&realm, (loopback_ip, PORT).into(), &PAYLOAD),
    )
    .await;
}

trait TestIpExt: IpExt {
    const LOCAL_ADDR: std::net::IpAddr;
    const LOCAL_SUBNET: fidl_fuchsia_net::Subnet;
    const REMOTE_ADDR: std::net::IpAddr;
    const REMOTE_SUBNET: fidl_fuchsia_net::Subnet;
}

impl TestIpExt for Ipv4 {
    const LOCAL_ADDR: std::net::IpAddr = std_ip!("192.0.2.1");
    const LOCAL_SUBNET: fidl_fuchsia_net::Subnet = fidl_subnet!("192.0.2.1/24");
    const REMOTE_ADDR: std::net::IpAddr = std_ip!("192.0.2.2");
    const REMOTE_SUBNET: fidl_fuchsia_net::Subnet = fidl_subnet!("192.0.2.2/24");
}

impl TestIpExt for Ipv6 {
    const LOCAL_ADDR: std::net::IpAddr = std_ip!("2001:db8::1");
    const LOCAL_SUBNET: fidl_fuchsia_net::Subnet = fidl_subnet!("2001:db8::1/64");
    const REMOTE_ADDR: std::net::IpAddr = std_ip!("2001:db8::2");
    const REMOTE_SUBNET: fidl_fuchsia_net::Subnet = fidl_subnet!("2001:db8::2/64");
}

#[netstack_test]
#[variant(I, Ip)]
async fn test_local_delivery_checksum_skipped_udp<I: TestIpExt>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    let ep = sandbox.create_endpoint("ep").await.expect("create endpoint");
    let iface = ep.into_interface_in_realm(&realm).await.expect("attach ep");
    let _ = iface.control().enable().await.expect("enable iface").expect("enable error");
    iface.set_link_up(true).await.expect("set link up");
    iface.add_address_and_subnet_route(I::LOCAL_SUBNET).await.expect("configure address");

    capture_and_verify_udp::<I>(
        &realm,
        iface.id(),
        // Packets originating and destined locally are delivered over the
        // loopback interface. We only compute a partial checksum over the
        // loopback interface, so checksum validation should fail.
        false,
        send_and_recv_udp(&realm, (I::LOCAL_ADDR, PORT).into(), &PAYLOAD),
    )
    .await;
}

#[netstack_test]
#[variant(I, Ip)]
async fn test_local_delivery_checksum_skipped_tcp<I: TestIpExt>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    let ep = sandbox.create_endpoint("ep").await.expect("create endpoint");
    let iface = ep.into_interface_in_realm(&realm).await.expect("attach ep");
    let _ = iface.control().enable().await.expect("enable iface").expect("enable error");
    iface.set_link_up(true).await.expect("set link up");
    iface.add_address_and_subnet_route(I::LOCAL_SUBNET).await.expect("configure address");

    capture_and_verify_tcp::<I>(
        &realm,
        iface.id(),
        // Packets originating and destined locally are delivered over the
        // loopback interface. We only compute a partial checksum over the
        // loopback interface, so checksum validation should fail.
        false,
        send_and_recv_tcp(&realm, (I::LOCAL_ADDR, PORT).into(), &PAYLOAD),
    )
    .await;
}

#[netstack_test]
#[variant(I, Ip)]
#[test_case(false; "computed")]
#[test_case(true; "offloaded")]
async fn test_remote_delivery_checksum_udp<I: TestIpExt>(name: &str, checksum_offload: bool) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let net = sandbox.create_network("net").await.expect("create network");

    let client_realm = sandbox
        .create_netstack_realm::<Netstack3, _>(format!("{}_client", name))
        .expect("failed to create client realm");
    let server_realm = sandbox
        .create_netstack_realm::<Netstack3, _>(format!("{}_server", name))
        .expect("failed to create server realm");

    let mut client_config = netemul::new_endpoint_config(netemul::DEFAULT_MTU, Some(CLIENT_MAC));
    client_config.checksum_offload = checksum_offload;
    let client_iface = client_realm
        .join_network_with(&net, "client", client_config, Default::default())
        .await
        .expect("client failed to join network");
    client_iface.add_address_and_subnet_route(I::LOCAL_SUBNET).await.expect("configure address");

    let mut server_config = netemul::new_endpoint_config(netemul::DEFAULT_MTU, Some(SERVER_MAC));
    server_config.checksum_offload = checksum_offload;
    let server_iface = server_realm
        .join_network_with(&net, "server", server_config, Default::default())
        .await
        .expect("server failed to join network");
    server_iface.add_address_and_subnet_route(I::REMOTE_SUBNET).await.expect("configure address");

    // Skip NUD because packets queued during address resolution bypass checksum
    // offloading.
    client_realm
        .add_neighbor_entry(client_iface.id(), I::REMOTE_SUBNET.addr, SERVER_MAC)
        .await
        .expect("add neighbor entry");
    server_realm
        .add_neighbor_entry(server_iface.id(), I::LOCAL_SUBNET.addr, CLIENT_MAC)
        .await
        .expect("add neighbor entry");

    let traffic_fut = async {
        let server_sock =
            fasync::net::UdpSocket::bind_in_realm(&server_realm, (I::REMOTE_ADDR, PORT).into())
                .await
                .expect("create server socket");

        let client_sock =
            fasync::net::UdpSocket::bind_in_realm(&client_realm, (I::LOCAL_ADDR, 0).into())
                .await
                .expect("create client socket");

        let sent = client_sock
            .send_to(&PAYLOAD, (I::REMOTE_ADDR, PORT).into())
            .await
            .expect("send_to failed");
        assert_eq!(sent, PAYLOAD.len());

        let mut recv_buf = vec![0u8; PAYLOAD.len()];
        let (received, from_addr) =
            server_sock.recv_from(&mut recv_buf).await.expect("recv_from failed");
        assert_eq!(received, PAYLOAD.len());
        assert_eq!(recv_buf.as_slice(), &PAYLOAD);
        assert_eq!(from_addr.ip(), I::LOCAL_ADDR);
    };

    capture_and_verify_udp::<I>(
        &client_realm,
        client_iface.id(),
        // If checksum offloading is enabled, the packet should be sent with a
        // partial checksum and checksum validation should fail. Otherwise, the
        // full checksum should be computed and validation should pass.
        !checksum_offload,
        traffic_fut,
    )
    .await;
}

#[netstack_test]
#[variant(I, Ip)]
#[test_case(false; "computed")]
#[test_case(true; "offloaded")]
async fn test_remote_delivery_checksum_tcp<I: TestIpExt>(name: &str, checksum_offload: bool) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let net = sandbox.create_network("net").await.expect("create network");

    let client_realm = sandbox
        .create_netstack_realm::<Netstack3, _>(format!("{}_client", name))
        .expect("failed to create client realm");
    let server_realm = sandbox
        .create_netstack_realm::<Netstack3, _>(format!("{}_server", name))
        .expect("failed to create server realm");

    let mut client_config = netemul::new_endpoint_config(netemul::DEFAULT_MTU, Some(CLIENT_MAC));
    client_config.checksum_offload = checksum_offload;
    let client_iface = client_realm
        .join_network_with(&net, "client", client_config, Default::default())
        .await
        .expect("client failed to join network");
    client_iface.add_address_and_subnet_route(I::LOCAL_SUBNET).await.expect("configure address");

    let mut server_config = netemul::new_endpoint_config(netemul::DEFAULT_MTU, Some(SERVER_MAC));
    server_config.checksum_offload = checksum_offload;
    let server_iface = server_realm
        .join_network_with(&net, "server", server_config, Default::default())
        .await
        .expect("server failed to join network");
    server_iface.add_address_and_subnet_route(I::REMOTE_SUBNET).await.expect("configure address");

    // Skip NUD because packets queued during address resolution bypass checksum
    // offloading.
    client_realm
        .add_neighbor_entry(client_iface.id(), I::REMOTE_SUBNET.addr, SERVER_MAC)
        .await
        .expect("add neighbor entry");
    server_realm
        .add_neighbor_entry(server_iface.id(), I::LOCAL_SUBNET.addr, CLIENT_MAC)
        .await
        .expect("add neighbor entry");

    let traffic_fut = async {
        let listener =
            fasync::net::TcpListener::listen_in_realm(&server_realm, (I::REMOTE_ADDR, PORT).into())
                .await
                .expect("listen failed");

        let client_fut = async {
            let mut stream = fasync::net::TcpStream::connect_in_realm(
                &client_realm,
                (I::REMOTE_ADDR, PORT).into(),
            )
            .await
            .expect("connect failed");
            stream.write_all(&PAYLOAD).await.expect("write failed");
            stream.flush().await.expect("flush failed");
        };

        let server_fut = async {
            let (_, mut stream, _) = listener.accept().await.expect("accept failed");
            let mut recv_buf = vec![0u8; PAYLOAD.len()];
            stream.read_exact(&mut recv_buf).await.expect("read failed");
            assert_eq!(recv_buf.as_slice(), &PAYLOAD);
        };

        futures::join!(client_fut, server_fut);
    };

    capture_and_verify_tcp::<I>(
        &client_realm,
        client_iface.id(),
        // If checksum offloading is enabled, the packet should be sent with a
        // partial checksum and checksum validation should fail. Otherwise, the
        // full checksum should be computed and validation should pass.
        !checksum_offload,
        traffic_fut,
    )
    .await;
}
