// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

use std::assert_matches;

use fidl::endpoints::Proxy as _;
use fidl_fuchsia_io as _;
use fidl_fuchsia_net as _;
use fidl_fuchsia_net_debug as fnet_debug;
use fidl_fuchsia_net_interfaces_admin as fnet_interfaces_admin;
use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
use fuchsia_async as fasync;
use futures::StreamExt as _;

use net_declare::{fidl_subnet, net_ip_v4, std_ip_v4};
use netemul::RealmUdpSocket as _;
use netstack_testing_common::pcap as pcap_helper;
use netstack_testing_common::realms::{Netstack3, TestRealmExt as _, TestSandboxExt as _};
use netstack_testing_macros::netstack_test;
use packet::ParsablePacket as _;
use test_case::test_case;

async fn setup_interface<'a>(
    realm: &netemul::TestRealm<'a>,
    ep: netemul::TestEndpoint<'a>,
    subnet: fidl_fuchsia_net::Subnet,
) -> netemul::TestInterface<'a> {
    let iface = ep.into_interface_in_realm(realm).await.expect("attach ep");
    // Disable IPv6 to avoid NDP and MLD traffic from being captured.
    let _ = iface
        .control()
        .set_configuration(&fnet_interfaces_admin::Configuration {
            ipv6: Some(fnet_interfaces_admin::Ipv6Configuration {
                enabled: Some(false),
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
        .expect("set_configuration FIDL error")
        .expect("set_configuration error");
    let _ = iface.control().enable().await.expect("enable iface").expect("enable error");
    iface.set_link_up(true).await.expect("set link up");
    iface.add_address_and_subnet_route(subnet).await.expect("configure address");
    iface
}

#[netstack_test]
#[test_case(false; "no_bpf")]
#[test_case(true; "use_bpf")]
async fn rolling_packet_capture_test(name: &str, use_bpf: bool) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    let provider = realm
        .connect_to_protocol::<fnet_debug::PacketCaptureProviderMarker>()
        .expect("connect to PacketCaptureProvider");

    let fnet_interfaces_ext::Properties { id: loopback_id, name: loopback_name, .. } = realm
        .loopback_properties()
        .await
        .expect("failed to get loopback properties")
        .expect("loopback not found");

    let bpf_program = use_bpf.then(|| {
        pcap::compile::compile_filter(
            "ip src 127.0.0.1 and ip dst 127.0.0.1 and udp src port 12345 and udp dst port 12345",
        )
        .expect("failed to compile filter")
    });

    let common_params = fnet_debug::CommonPacketCaptureParams {
        interfaces: Some(fnet_debug::InterfaceSpecifier::InterfaceIds(vec![loopback_id.get()])),
        bpf_program,
        ..Default::default()
    };

    let rolling_params = fnet_debug::RollingPacketCaptureParams::default();

    let rolling_client = provider
        .start_rolling(common_params, &rolling_params)
        .await
        .expect("start_rolling FIDL error")
        .expect("start_rolling error")
        .into_proxy();

    // Bind a UDP socket and send a packet to trigger capture.
    const PORT1: u16 = 12345;
    const PORT2: u16 = 54321;
    const PAYLOAD1: [u8; 4] = [1, 2, 3, 4];
    const PAYLOAD2: [u8; 4] = [5, 6, 7, 8];
    pcap_helper::send_udp_to_self_and_recv(
        &realm,
        (std_ip_v4!("127.0.0.1"), PORT1).into(),
        &PAYLOAD1,
    )
    .await;
    pcap_helper::send_udp_to_self_and_recv(
        &realm,
        (std_ip_v4!("127.0.0.1"), PORT2).into(),
        &PAYLOAD2,
    )
    .await;

    // Stop and download.
    let (file_client, file_server) = fidl::endpoints::create_endpoints();
    rolling_client.stop_and_download(file_server).expect("stop_and_download failed");

    let file_proxy = file_client.into_proxy();
    let bytes = fuchsia_fs::file::read(&file_proxy).await.expect("read file failed");

    // Stop and download again.
    let (file_client2, file_server2) = fidl::endpoints::create_endpoints();
    rolling_client.stop_and_download(file_server2).expect("stop_and_download failed");
    let file_proxy2 = file_client2.into_proxy();
    let bytes2 = fuchsia_fs::file::read(&file_proxy2).await.expect("read file failed");
    assert_eq!(bytes, bytes2);
    let cap = pcap::parse_pcapng(&bytes).unwrap_or_else(|_| {
        panic!("could not parse file with pcap library: {bytes:?}");
    });
    let [interface] = &cap.interfaces[..] else {
        panic!("expected exactly one interface in the capture, got {:?}", cap);
    };
    assert_eq!(
        interface,
        &pcap::ParsedInterfaceDescription {
            link_type: pcap::LinkType::Ethernet,
            snap_len: 0,
            options: pcap::ParsedInterfaceDescriptionOptions {
                if_name: Some(std::borrow::Cow::Borrowed(loopback_name.as_str())),
            },
        }
    );

    if use_bpf {
        pcap_helper::assert_udp_packets(
            cap.packet_blocks(),
            &[pcap_helper::ExpectedUdpPacket {
                src_ip: net_declare::net_ip_v4!("127.0.0.1"),
                dst_ip: net_declare::net_ip_v4!("127.0.0.1"),
                src_port: PORT1,
                dst_port: PORT1,
                payload: &PAYLOAD1,
            }],
            true, /* force_skip_checksum_validation */
        );
    } else {
        pcap_helper::assert_udp_packets(
            cap.packet_blocks(),
            &[
                pcap_helper::ExpectedUdpPacket {
                    src_ip: net_declare::net_ip_v4!("127.0.0.1"),
                    dst_ip: net_declare::net_ip_v4!("127.0.0.1"),
                    src_port: PORT1,
                    dst_port: PORT1,
                    payload: &PAYLOAD1,
                },
                pcap_helper::ExpectedUdpPacket {
                    src_ip: net_declare::net_ip_v4!("127.0.0.1"),
                    dst_ip: net_declare::net_ip_v4!("127.0.0.1"),
                    src_port: PORT2,
                    dst_port: PORT2,
                    payload: &PAYLOAD2,
                },
            ],
            true, /* force_skip_checksum_validation */
        );
    };
}

#[netstack_test]
async fn packet_capture_multiple_interfaces_test(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    // Create two endpoints.
    let ep1 = sandbox.create_endpoint("ep1").await.expect("create endpoint ep1");
    let ep2 = sandbox.create_endpoint("ep2").await.expect("create endpoint ep2");

    let if1 = setup_interface(&realm, ep1, fidl_subnet!("192.0.2.1/24")).await;
    let _if2 = setup_interface(&realm, ep2, fidl_subnet!("192.0.2.2/24")).await;

    // Connect to provider.
    let provider = realm
        .connect_to_protocol::<fnet_debug::PacketCaptureProviderMarker>()
        .expect("connect to PacketCaptureProvider");

    // Capture on interface 1.
    let common_params = fnet_debug::CommonPacketCaptureParams {
        interfaces: Some(fnet_debug::InterfaceSpecifier::InterfaceIds(vec![if1.id()])),
        ..Default::default()
    };
    let rolling_params = fnet_debug::RollingPacketCaptureParams::default();

    let rolling_client = provider
        .start_rolling(common_params, &rolling_params)
        .await
        .expect("start_rolling call fail")
        .expect("start_rolling error")
        .into_proxy();

    // Bind sockets and send packets.
    const PORT: u16 = 9876;

    const PAYLOAD1: [u8; 4] = [1, 2, 3, 4];
    const PAYLOAD2: [u8; 4] = [5, 6, 7, 8];

    pcap_helper::send_udp_to_self_and_recv(
        &realm,
        (std_ip_v4!("192.0.2.1"), PORT).into(),
        &PAYLOAD1,
    )
    .await;
    pcap_helper::send_udp_to_self_and_recv(
        &realm,
        (std_ip_v4!("192.0.2.2"), PORT).into(),
        &PAYLOAD2,
    )
    .await;

    // Stop and download.
    let (file_client, file_server) = fidl::endpoints::create_endpoints();
    rolling_client.stop_and_download(file_server).expect("stop_and_download failed");

    let file_proxy = file_client.into_proxy();
    let bytes = fuchsia_fs::file::read(&file_proxy).await.expect("read file failed");
    let cap = pcap::parse_pcapng(&bytes).expect("could not parse file with pcap library");

    // Verify capture contains PAYLOAD1 but not PAYLOAD2.
    pcap_helper::assert_udp_packets(
        cap.packet_blocks(),
        &[pcap_helper::ExpectedUdpPacket {
            src_ip: net_declare::net_ip_v4!("192.0.2.1"),
            dst_ip: net_declare::net_ip_v4!("192.0.2.1"),
            src_port: PORT,
            dst_port: PORT,
            payload: &PAYLOAD1,
        }],
        true, /* force_skip_checksum_validation */
    );
}

#[netstack_test]
async fn packet_capture_rolling_discard_oldest_test(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    let provider = realm
        .connect_to_protocol::<fnet_debug::PacketCaptureProviderMarker>()
        .expect("connect to PacketCaptureProvider");

    let fnet_interfaces_ext::Properties { id: loopback_id, .. } = realm
        .loopback_properties()
        .await
        .expect("failed to get loopback properties")
        .expect("loopback not found");

    // Capture on loopback with small buffer.
    let common_params = fnet_debug::CommonPacketCaptureParams {
        interfaces: Some(fnet_debug::InterfaceSpecifier::InterfaceIds(vec![loopback_id.get()])),
        ..Default::default()
    };
    // Use MIN_BUFFER_SIZE.
    let rolling_params = fnet_debug::RollingPacketCaptureParams {
        capture_size: Some(fnet_debug::MIN_BUFFER_SIZE),
        ..Default::default()
    };

    let rolling_client = provider
        .start_rolling(common_params, &rolling_params)
        .await
        .expect("start_rolling call fail")
        .expect("start_rolling error")
        .into_proxy();

    // Bind socket and send packets.
    const PORT: u16 = 9877;
    let bind_addr = std::net::SocketAddr::from(([127, 0, 0, 1], PORT));
    let sock =
        fasync::net::UdpSocket::bind_in_realm(&realm, bind_addr).await.expect("create socket");

    // Send enough packets such that not all of them can be stored in
    // MIN_BUFFER_SIZE.
    const PAYLOAD_SIZE: usize = 65000;
    let max_capturable_count = usize::try_from(fnet_debug::MIN_BUFFER_SIZE).unwrap() / PAYLOAD_SIZE;
    let packet_count = max_capturable_count * 2;
    for i in 0..packet_count {
        let payload: Vec<u8> =
            std::iter::once(i as u8).chain((0..=255u8).cycle()).take(PAYLOAD_SIZE).collect();
        let sent = sock.send_to(&payload[..], bind_addr).await.expect("send_to failed");
        assert_eq!(sent, PAYLOAD_SIZE);

        // Actually receive the packets.
        let mut recv_buf = vec![0u8; PAYLOAD_SIZE];
        let (received, from_addr) = sock.recv_from(&mut recv_buf).await.expect("recv_from failed");
        assert_eq!(received, PAYLOAD_SIZE);
        assert_eq!(&recv_buf[..], &payload[..]);
        assert_eq!(from_addr, bind_addr);
    }

    // Stop and download.
    let (file_client, file_server) = fidl::endpoints::create_endpoints();
    rolling_client.stop_and_download(file_server).expect("stop_and_download failed");

    let file_proxy = file_client.into_proxy();
    let bytes = fuchsia_fs::file::read(&file_proxy).await.expect("read file failed");
    let cap = pcap::parse_pcapng(&bytes).expect("could not parse file with pcap library");

    let want_tail_bytes = (0..=255u8).cycle().take(PAYLOAD_SIZE - 1).collect::<Vec<_>>();
    let identifier_bytes = cap
        .packet_blocks()
        .map(|epb| {
            let epb = epb
                .unwrap_or_else(|e| panic!("failed to parse packet block {e:?}\nbytes: {bytes:?}"));
            let buf = &epb.packet_data;
            let (mut body, _src_mac, _dst_mac, src_ip, dst_ip, proto, _ttl) =
                packet_formats::testutil::parse_ip_packet_in_ethernet_frame::<net_types::ip::Ipv4>(
                    &buf,
                    packet_formats::ethernet::EthernetFrameLengthCheck::NoCheck,
                )
                .expect("failed to parse IP packet");

            assert_eq!(
                proto,
                packet_formats::ip::Ipv4Proto::Proto(packet_formats::ip::IpProto::Udp)
            );
            let udp = packet_formats::udp::UdpPacket::parse(
                &mut body,
                packet_formats::udp::UdpParseArgs::with_context(
                    src_ip,
                    dst_ip,
                    // Transport-layer checksums aren't computed for packets
                    // sent over the loopback interface so we skip validation
                    // here.
                    packet_formats::testutil::ForceSkipChecksumValidation(true),
                ),
            )
            .expect("failed to parse UDP packet");

            assert_eq!(udp.src_port().map(|p| p.get()), Some(PORT));
            assert_eq!(udp.dst_port().get(), PORT);
            assert_eq!(src_ip, net_ip_v4!("127.0.0.1"));
            assert_eq!(dst_ip, net_ip_v4!("127.0.0.1"));

            // Compare all bytes of the body except the first byte.
            assert_eq!(body.len(), PAYLOAD_SIZE);
            assert_eq!(body[1..], want_tail_bytes);
            body[0]
        })
        .collect::<Vec<_>>();

    assert!(
        identifier_bytes.len() <= max_capturable_count,
        "got {} packets but expected no more than {}",
        identifier_bytes.len(),
        max_capturable_count,
    );
    let start = u8::try_from(packet_count - identifier_bytes.len()).unwrap();
    let want = (start..u8::try_from(packet_count).unwrap()).collect::<Vec<_>>();
    assert_eq!(identifier_bytes, want);
}

// Tests that the rolling packet capture quota is enforced (only one active capture at a time).
#[netstack_test]
async fn rolling_packet_capture_quota_test(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    let provider = realm
        .connect_to_protocol::<fnet_debug::PacketCaptureProviderMarker>()
        .expect("connect to PacketCaptureProvider");

    let fnet_interfaces_ext::Properties { id: loopback_id, .. } = realm
        .loopback_properties()
        .await
        .expect("failed to get loopback properties")
        .expect("loopback not found");

    let create_common_params = || fnet_debug::CommonPacketCaptureParams {
        interfaces: Some(fnet_debug::InterfaceSpecifier::InterfaceIds(vec![loopback_id.get()])),
        ..Default::default()
    };

    let rolling_params = fnet_debug::RollingPacketCaptureParams::default();

    // First call should succeed.
    let rolling_pcap = provider
        .start_rolling(create_common_params(), &rolling_params)
        .await
        .expect("start_rolling FIDL error 1")
        .expect("start_rolling error 1")
        .into_proxy();

    // Second call should fail with QUOTA_EXCEEDED.
    let res2 = provider
        .start_rolling(create_common_params(), &rolling_params)
        .await
        .expect("start_rolling FIDL error 2");

    assert_eq!(res2, Err(fnet_debug::PacketCaptureStartError::QuotaExceeded));

    let capture_name = "quota_test_capture";
    rolling_pcap.detach(capture_name).await.expect("detach FIDL").expect("detach");
    drop(rolling_pcap);

    // Third call should also fail with QUOTA_EXCEEDED because the capture
    // is detached but still active.
    let res3 = provider
        .start_rolling(create_common_params(), &rolling_params)
        .await
        .expect("start_rolling FIDL error 3");

    assert_eq!(res3, Err(fnet_debug::PacketCaptureStartError::QuotaExceeded));

    let channel = provider
        .reconnect_rolling(capture_name)
        .await
        .expect("reconnect_rolling FIDL error")
        .expect("reconnect_rolling error");
    let rolling_pcap = channel.into_proxy();

    let (file_client, file_server) = fidl::endpoints::create_endpoints();
    rolling_pcap.stop_and_download(file_server).expect("stop_and_download failed");

    let file_proxy = file_client.into_proxy();
    let _bytes = fuchsia_fs::file::read(&file_proxy).await.expect("read file failed");

    rolling_pcap.discard().await.expect("discard failed");
    assert_eq!(rolling_pcap.on_closed().await, Ok(zx::Signals::CHANNEL_PEER_CLOSED));
    assert_eq!(file_proxy.on_closed().await, Ok(zx::Signals::CHANNEL_PEER_CLOSED));

    // Fourth call should succeed because the capture was discarded.
    let _ = provider
        .start_rolling(create_common_params(), &rolling_params)
        .await
        .expect("start_rolling FIDL error 4")
        .expect("start_rolling error 4");
}

// Tests that the quota is not leaked if a request fails with an error (e.g. invalid buffer size).
#[netstack_test]
async fn rolling_packet_capture_quota_leak_test(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    let provider = realm
        .connect_to_protocol::<fnet_debug::PacketCaptureProviderMarker>()
        .expect("connect to PacketCaptureProvider");

    let fnet_interfaces_ext::Properties { id: loopback_id, .. } = realm
        .loopback_properties()
        .await
        .expect("failed to get loopback properties")
        .expect("loopback not found");

    let create_common_params = || fnet_debug::CommonPacketCaptureParams {
        interfaces: Some(fnet_debug::InterfaceSpecifier::InterfaceIds(vec![loopback_id.get()])),
        ..Default::default()
    };

    // Call with invalid buffer size.
    let invalid_rolling_params = fnet_debug::RollingPacketCaptureParams {
        capture_size: Some(fnet_debug::MIN_BUFFER_SIZE / 2),
        ..Default::default()
    };

    let res1 = provider
        .start_rolling(create_common_params(), &invalid_rolling_params)
        .await
        .expect("start_rolling FIDL error 1");

    assert_eq!(res1, Err(fnet_debug::PacketCaptureStartError::InvalidBufferSize));

    // Second call with valid parameters should succeed.
    let rolling_params = fnet_debug::RollingPacketCaptureParams::default();
    let res2 = provider
        .start_rolling(create_common_params(), &rolling_params)
        .await
        .expect("start_rolling FIDL error 2");

    if let Err(e) = res2 {
        panic!("second call to start_rolling should have succeeded, got {e:?}");
    }
}

// Tests that we can detach from a rolling packet capture and reconnect to it.
#[netstack_test]
async fn rolling_packet_capture_detach_reconnect_test(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    let provider = realm
        .connect_to_protocol::<fnet_debug::PacketCaptureProviderMarker>()
        .expect("connect to PacketCaptureProvider");

    let fnet_interfaces_ext::Properties { id: loopback_id, .. } = realm
        .loopback_properties()
        .await
        .expect("failed to get loopback properties")
        .expect("loopback not found");

    let create_common_params = || fnet_debug::CommonPacketCaptureParams {
        interfaces: Some(fnet_debug::InterfaceSpecifier::InterfaceIds(vec![loopback_id.get()])),
        ..Default::default()
    };

    let rolling_params = fnet_debug::RollingPacketCaptureParams::default();

    let capture_name = "test_capture";
    let rolling_proxy = provider
        .start_rolling(create_common_params(), &rolling_params)
        .await
        .expect("start_rolling FIDL error")
        .expect("start_rolling error")
        .into_proxy();

    rolling_proxy.detach(capture_name).await.expect("detach FIDL").expect("detach");

    let payload = b"hello world";
    let bind_addr = std::net::SocketAddr::new(
        std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)),
        12345,
    );
    pcap_helper::send_udp_to_self_and_recv(&realm, bind_addr, payload).await;

    let channel = provider
        .reconnect_rolling(capture_name)
        .await
        .expect("reconnect_rolling FIDL error")
        .expect("reconnect_rolling error");
    let rolling_proxy = channel.into_proxy();

    let (file_client1, file_server) = fidl::endpoints::create_endpoints();
    rolling_proxy.stop_and_download(file_server).expect("stop_and_download failed");
    let file_proxy1 = file_client1.into_proxy();
    // Sync to ensure StopAndDownload was processed by the server.
    let _ = file_proxy1
        .sync()
        .await
        .expect("sync FIDL error")
        .map_err(zx::Status::err_from_raw)
        .expect("sync error");
    // Assert that the OnEnded event was sent.
    let mut event_stream = rolling_proxy.take_event_stream();
    let event = event_stream.next().await.expect("event stream ended").expect("FIDL error");
    assert_matches!(
        event,
        fnet_debug::RollingPacketCaptureEvent::OnEnded {
            reason: fnet_debug::PacketCaptureEndReason::UserRequest
        }
    );

    let channel = provider
        .reconnect_rolling(capture_name)
        .await
        .expect("reconnect_rolling FIDL error")
        .expect("reconnect_rolling error");
    assert_eq!(rolling_proxy.on_closed().await, Ok(zx::Signals::CHANNEL_PEER_CLOSED));
    let rolling_proxy = channel.into_proxy();

    // Since the capture has ended, the reconnect should immediately yield OnEnded.
    let mut event_stream = rolling_proxy.take_event_stream();
    let event = event_stream.next().await.expect("event stream ended").expect("FIDL error");
    let fnet_debug::RollingPacketCaptureEvent::OnEnded { reason } = event;
    assert_eq!(reason, fnet_debug::PacketCaptureEndReason::UserRequest);

    let (file_client2, file_server2) = fidl::endpoints::create_endpoints();
    rolling_proxy.stop_and_download(file_server2).expect("stop_and_download failed");
    let file_proxy2 = file_client2.into_proxy();
    for file_proxy in [&file_proxy1, &file_proxy2] {
        let bytes = fuchsia_fs::file::read(file_proxy).await.expect("read file failed");
        let cap = pcap::parse_pcapng(&bytes).expect("could not parse file with pcap library");

        let expected_packets = [pcap_helper::ExpectedUdpPacket {
            src_ip: net_ip_v4!("127.0.0.1"),
            dst_ip: net_ip_v4!("127.0.0.1"),
            src_port: 12345,
            dst_port: 12345,
            payload: payload.as_slice(),
        }];
        pcap_helper::assert_udp_packets(
            cap.packet_blocks(),
            &expected_packets,
            true, /* force_skip_checksum_validation */
        );
    }

    rolling_proxy.discard().await.expect("discard FIDL");
    assert_eq!(rolling_proxy.on_closed().await, Ok(zx::Signals::CHANNEL_PEER_CLOSED));
    for file_proxy in [file_proxy1, file_proxy2] {
        assert_eq!(file_proxy.on_closed().await, Ok(zx::Signals::CHANNEL_PEER_CLOSED));
    }

    let res =
        provider.reconnect_rolling(capture_name).await.expect("reconnect_rolling FIDL error dup");
    assert_eq!(res, Err(fnet_debug::PacketCaptureReconnectError::NotFound));
}

#[netstack_test]
#[test_case(true; "disconnect_before_removal")]
#[test_case(false; "keep_connected_during_removal")]
async fn packet_capture_interface_removed_test(name: &str, disconnect_before_removal: bool) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    // Create an endpoint and attach it as an interface.
    let ep = sandbox.create_endpoint("ep").await.expect("create endpoint");
    let iface = setup_interface(&realm, ep, fidl_subnet!("192.0.2.1/24")).await;

    // Connect to provider.
    let provider = realm
        .connect_to_protocol::<fnet_debug::PacketCaptureProviderMarker>()
        .expect("connect to PacketCaptureProvider");

    // Capture on the interface.
    let common_params = fnet_debug::CommonPacketCaptureParams {
        interfaces: Some(fnet_debug::InterfaceSpecifier::InterfaceIds(vec![iface.id()])),
        ..Default::default()
    };
    let rolling_params = fnet_debug::RollingPacketCaptureParams::default();

    let rolling_proxy = provider
        .start_rolling(common_params, &rolling_params)
        .await
        .expect("start_rolling call fail")
        .expect("start_rolling error")
        .into_proxy();

    // Bind socket and send a packet.
    const PORT: u16 = 9876;
    const PAYLOAD: [u8; 4] = [1, 2, 3, 4];
    pcap_helper::send_udp_to_self_and_recv(
        &realm,
        (std_ip_v4!("192.0.2.1"), PORT).into(),
        &PAYLOAD,
    )
    .await;

    let capture_name = "interface_removed_test";

    let rolling_proxy = if disconnect_before_removal {
        // Detach the packet capture and close the client.
        rolling_proxy.detach(capture_name).await.expect("detach FIDL").expect("detach");
        std::mem::drop(rolling_proxy);

        // Remove the interface!
        let control = iface.control().clone();
        control.remove().await.expect("remove call failed").expect("remove failed");

        // Reconnect to the packet capture.
        let channel = provider
            .reconnect_rolling(capture_name)
            .await
            .expect("reconnect_rolling FIDL error")
            .expect("reconnect_rolling error");
        channel.into_proxy()
    } else {
        // Remove the interface while capture is still going and client is connected.
        let control = iface.control().clone();
        control.remove().await.expect("remove call failed").expect("remove failed");
        rolling_proxy
    };

    // Assert that the channel yields the OnEnded event.
    let mut event_stream = rolling_proxy.take_event_stream();
    let event = event_stream.next().await.expect("event stream ended").expect("FIDL error");
    let fnet_debug::RollingPacketCaptureEvent::OnEnded { reason } = event;
    assert_eq!(reason, fnet_debug::PacketCaptureEndReason::InterfaceRemoved);

    // Assert that we can still successfully call StopAndDownload to download the pcap.
    let (file_client, file_server) = fidl::endpoints::create_endpoints();
    rolling_proxy.stop_and_download(file_server).expect("stop_and_download failed");

    let file_proxy = file_client.into_proxy();
    let bytes = fuchsia_fs::file::read(&file_proxy).await.expect("read file failed");
    let cap = pcap::parse_pcapng(&bytes).expect("could not parse file with pcap library");

    // Verify capture contains the UDP packet.
    pcap_helper::assert_udp_packets(
        cap.packet_blocks(),
        &[pcap_helper::ExpectedUdpPacket {
            src_ip: net_declare::net_ip_v4!("192.0.2.1"),
            dst_ip: net_declare::net_ip_v4!("192.0.2.1"),
            src_port: PORT,
            dst_port: PORT,
            payload: &PAYLOAD,
        }],
        true, /* force_skip_checksum_validation */
    );

    // Discard and clean up.
    rolling_proxy.discard().await.expect("discard failed");
}

#[netstack_test]
async fn packet_capture_interface_removed_after_stop_and_download_test(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    let ep = sandbox.create_endpoint("ep").await.expect("create endpoint");
    let iface = setup_interface(&realm, ep, fidl_subnet!("192.0.2.1/24")).await;

    let provider = realm
        .connect_to_protocol::<fnet_debug::PacketCaptureProviderMarker>()
        .expect("connect to PacketCaptureProvider");

    let common_params = fnet_debug::CommonPacketCaptureParams {
        interfaces: Some(fnet_debug::InterfaceSpecifier::InterfaceIds(vec![iface.id()])),
        ..Default::default()
    };
    let rolling_params = fnet_debug::RollingPacketCaptureParams::default();

    let rolling_proxy = provider
        .start_rolling(common_params, &rolling_params)
        .await
        .expect("start_rolling call fail")
        .expect("start_rolling error")
        .into_proxy();

    const PORT: u16 = 9876;
    const PAYLOAD: [u8; 4] = [1, 2, 3, 4];
    pcap_helper::send_udp_to_self_and_recv(
        &realm,
        (std_ip_v4!("192.0.2.1"), PORT).into(),
        &PAYLOAD,
    )
    .await;

    let (file_client, file_server) = fidl::endpoints::create_endpoints();
    rolling_proxy.stop_and_download(file_server).expect("stop_and_download failed");

    let mut event_stream = rolling_proxy.take_event_stream();
    let event = event_stream.next().await.expect("event stream ended").expect("FIDL error");
    assert_matches!(
        event,
        fnet_debug::RollingPacketCaptureEvent::OnEnded {
            reason: fnet_debug::PacketCaptureEndReason::UserRequest
        }
    );

    iface.control().remove().await.expect("remove call failed").expect("remove failed");

    let file_proxy = file_client.into_proxy();
    let bytes = fuchsia_fs::file::read(&file_proxy).await.expect("read file failed");
    let cap = pcap::parse_pcapng(&bytes).expect("could not parse file with pcap library");

    pcap_helper::assert_udp_packets(
        cap.packet_blocks(),
        &[pcap_helper::ExpectedUdpPacket {
            src_ip: net_declare::net_ip_v4!("192.0.2.1"),
            dst_ip: net_declare::net_ip_v4!("192.0.2.1"),
            src_port: PORT,
            dst_port: PORT,
            payload: &PAYLOAD,
        }],
        true, /* force_skip_checksum_validation */
    );

    rolling_proxy.discard().await.expect("discard failed");
}
