// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

use fidl_fuchsia_posix_socket as fposix_socket;
use fidl_fuchsia_posix_socket_raw as fposix_socket_raw;
use fuchsia_async::net::DatagramSocket;
use fuchsia_async::{self as fasync, TimeoutExt as _};
use futures::FutureExt as _;
use net_types::ip::{Ip, IpAddress as _, IpVersion};
use netstack_testing_common::realms::{Netstack, TestSandboxExt as _};
use netstack_testing_common::{
    ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT, ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT,
};
use netstack_testing_macros::netstack_test;
use packet::ParsablePacket as _;
use packet_formats::ip::IpProto;
use packet_formats::ipv4::Ipv4Packet;
use socket2::InterfaceIndexOrAddress;
use test_case::test_case;

use crate::{MulticastTestIpExt, TestIpExt};

// A helper to receive a message and the control data from a raw socket.
async fn recv_msg_and_control(
    proxy: &fposix_socket_raw::SocketProxy,
) -> (Vec<u8>, fposix_socket::NetworkSocketRecvControlData) {
    let describe_info = proxy.describe().await.expect("describe should succeed");
    let event = describe_info.event.expect("info should have an event");
    let incoming_signal = zx::Signals::from_bits(fposix_socket::SIGNAL_DATAGRAM_INCOMING).unwrap();
    let _signals = fasync::OnSignals::new(event, incoming_signal)
        .await
        .expect("waiting for signals should succeed");
    let (_addr, data, control, _truncated) = proxy
        .recv_msg(
            false,           // want_addr
            u16::MAX.into(), // data_len
            true,            // want_control
            fposix_socket::RecvMsgFlags::empty(),
        )
        .await
        .expect("sending request should succeed")
        .expect("RecvMsg request should succeed");
    (data, control)
}

#[track_caller]
fn verify_packet_body<I: Ip>(buf: &[u8], expected_body: &[u8]) {
    match I::VERSION {
        // NB: Raw IPv4 Sockets receive the full IP Header
        IpVersion::V4 => {
            let buffer = packet::Buf::new(buf, ..);
            let packet = Ipv4Packet::parse(&mut buffer.as_ref(), ()).expect("parse should succeed");
            assert_eq!(packet.body(), &expected_body[..]);
        }
        IpVersion::V6 => assert_eq!(&buf[..], &expected_body[..]),
    }
}

#[netstack_test]
#[variant(N, Netstack)]
#[variant(I, Ip)]
#[test_case(None, true; "default_should_loop")]
#[test_case(Some(true), true; "enabled_should_loop")]
#[test_case(Some(false), false; "disabled_shouldnt_loop")]
async fn multicast_loop_on_raw_ip_socket<N: Netstack, I: MulticastTestIpExt>(
    name: &str,
    multicast_loop_value: Option<bool>,
    should_receive: bool,
) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let client = sandbox
        .create_netstack_realm::<N, _>(format!("{name}_client"))
        .expect("failed to create client realm");
    let networks = crate::init_multicast_test_networks::<I>(&sandbox, &client).await;

    // NB: Ensure we send the packet over a non-loopback interface, as that
    // would defeat the purpose of the multicast_loop test.
    let iface = &networks[0].iface;

    let send_socket = client
        .raw_socket(
            I::DOMAIN,
            fposix_socket_raw::ProtocolAssociation::Associated(IpProto::Udp.into()),
        )
        .await
        .expect("failed to create socket");
    send_socket
        .bind_device(Some(
            iface.get_interface_name().await.expect("get_interface_name failed").as_bytes(),
        ))
        .expect("failed to bind socket to an interface");

    if let Some(multicast_loop) = multicast_loop_value {
        match I::VERSION {
            IpVersion::V4 => send_socket.set_multicast_loop_v4(multicast_loop),
            IpVersion::V6 => send_socket.set_multicast_loop_v6(multicast_loop),
        }
        .expect("failed to set IPV6_MULTICAST_LOOP");
    }

    let recv_socket = client
        .raw_socket(
            I::DOMAIN,
            fposix_socket_raw::ProtocolAssociation::Associated(IpProto::Udp.into()),
        )
        .await
        .expect("failed to create socket");
    let recv_socket = DatagramSocket::new_from_socket(recv_socket).unwrap();

    // NB: Multicast traffic is dropped before being delivered to raw IP sockets
    // if we don't have any interest in the packet. Register a UDP socket
    // with interest.
    let _multicast_interested_sock = {
        let socket = client
            .datagram_socket(I::DOMAIN, fposix_socket::DatagramSocketProtocol::Udp)
            .await
            .expect("failed to create socket");
        let iface_id = u32::try_from(iface.id()).unwrap();
        match I::MCAST_ADDR.ip() {
            std::net::IpAddr::V4(addr_v4) => socket
                .join_multicast_v4_n(&addr_v4.into(), &InterfaceIndexOrAddress::Index(iface_id))
                .expect("failed to join multicast group"),
            std::net::IpAddr::V6(addr_v6) => socket
                .join_multicast_v6(&addr_v6.into(), iface_id)
                .expect("failed to join multicast group"),
        }
        socket
    };

    let data = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
    assert_eq!(
        send_socket.send_to(&data, &I::MCAST_ADDR.into()).expect("failed to send multicast packet"),
        data.len()
    );

    let mut buf = [0u8; 200];
    let recv_fut = recv_socket.recv_from(&mut buf);
    if should_receive {
        let (size, _addr) = recv_fut
            .on_timeout(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT, || {
                Err(std::io::ErrorKind::TimedOut.into())
            })
            .await
            .expect("recv_from failed");
        verify_packet_body::<I>(&buf[..size], &data);
    } else {
        recv_fut
            .map(|output| panic!("unexpected received packet {output:?}"))
            .on_timeout(ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT, || ())
            .await;
    }
}

#[netstack_test]
#[variant(N, Netstack)]
#[variant(I, Ip)]
async fn raw_ip_socket_recv_hop_limit<N: Netstack, I: TestIpExt>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let client = sandbox
        .create_netstack_realm::<N, _>(format!("{name}_client"))
        .expect("creating client realm should succeed");

    let socket = client
        .raw_socket(
            I::DOMAIN,
            fposix_socket_raw::ProtocolAssociation::Associated(IpProto::Udp.into()),
        )
        .await
        .expect("creating socket should succeed");

    let socket_channel = fdio::clone_channel(&socket).expect("cloning channel should succeed");
    let socket_proxy =
        fposix_socket_raw::SocketProxy::new(fidl::AsyncChannel::from_channel(socket_channel));

    // Set `IP_TTL` or `IPV6_HOPLIMIT` so that the packet has a known hop limit.
    const EXPECTED_TTL: u8 = 73;
    match I::VERSION {
        IpVersion::V4 => {
            socket.set_ttl_v4(EXPECTED_TTL.into()).expect("setting IP_TTL should succeed")
        }
        IpVersion::V6 => socket
            .set_unicast_hops_v6(EXPECTED_TTL.into())
            .expect("setting IPV6_UNICAST_HOPS should succeed"),
    }

    fn get_hop_limit<I: Ip>(control: &fposix_socket::NetworkSocketRecvControlData) -> Option<u8> {
        match I::VERSION {
            IpVersion::V4 => control.ip.as_ref().map(|c| c.ttl).flatten(),
            IpVersion::V6 => control.ipv6.as_ref().map(|c| c.hoplimit).flatten(),
        }
    }

    // Send a packet and verify that the TTL/HopLimit is not present.
    let loopback_addr = std::net::SocketAddr::new(I::LOOPBACK_ADDRESS.to_ip_addr().into(), 0);
    let dest_addr = socket2::SockAddr::from(loopback_addr);
    let data = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
    assert_eq!(socket.send_to(&data, &dest_addr).expect("failed to send packet"), data.len());
    let (msg, control) = recv_msg_and_control(&socket_proxy).await;
    verify_packet_body::<I>(msg.as_slice(), &data);
    assert_eq!(get_hop_limit::<I>(&control), None);

    // Now, set `IP_RECVTTL` or `IPV6_RECVHOPLIMIT` and verify that the
    // TTL/HopLimit is present.
    // NB: socket2 doesn't support these options directly, so use the FIDL API
    // instead.
    match I::VERSION {
        IpVersion::V4 => socket_proxy
            .set_ip_receive_ttl(true)
            .await
            .expect("sending the request should succeed")
            .expect("setting IP_RECVTTL should succeed"),
        IpVersion::V6 => socket_proxy
            .set_ipv6_receive_hop_limit(true)
            .await
            .expect("sending the request should succeed")
            .expect("setting IPV6_RECVHOPLIMIT should succeed"),
    }
    assert_eq!(socket.send_to(&data, &dest_addr).expect("failed to send packet"), data.len());
    let (msg, control) = recv_msg_and_control(&socket_proxy).await;
    verify_packet_body::<I>(msg.as_slice(), &data);
    assert_eq!(get_hop_limit::<I>(&control), Some(EXPECTED_TTL));
}
