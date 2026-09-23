// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

use std::fmt::Debug;
use std::num::{NonZeroU16, NonZeroU64};
use std::ops::RangeInclusive;
use std::os::fd::AsFd as _;
use std::pin::pin;

use anyhow::Context as _;
use assert_matches::assert_matches;
use fidl_fuchsia_hardware_network as fhardware_network;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_ext::IntoExt as _;
use fidl_fuchsia_net_filter as fnet_filter;
use fidl_fuchsia_net_filter_ext as fnet_filter_ext;
use fidl_fuchsia_net_interfaces as fnet_interfaces;
use fidl_fuchsia_net_interfaces_admin as fnet_interfaces_admin;
use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
use fidl_fuchsia_net_matchers_ext as fnet_matchers_ext;
use fidl_fuchsia_net_tun as fnet_tun;
use fidl_fuchsia_posix_socket as fposix_socket;
use fidl_fuchsia_posix_socket_packet as fpacket;
use fidl_fuchsia_posix_socket_raw as fposix_socket_raw;
use fuchsia_async::net::{DatagramSocket, UdpSocket};
use fuchsia_async::{self as fasync, TimeoutExt as _};
use futures::future::{self, LocalBoxFuture};
use futures::{Future, FutureExt as _, StreamExt as _, TryFutureExt as _, TryStreamExt as _};
use net_declare::{
    fidl_ip_v4, fidl_ip_v6, fidl_mac, fidl_subnet, net_ip_v4, net_ip_v6, std_ip, std_socket_addr,
};
use net_types::Witness as _;
use net_types::ip::{Ip, IpAddress as _, IpInvariant, IpVersion, Ipv4, Ipv4Addr, Ipv6, Ipv6Addr};
use netemul::{TestFakeEndpoint, TestInterface, TestNetwork, TestRealm, TestSandbox};
use netstack_testing_common::interfaces::TestInterfaceExt as _;
use netstack_testing_common::realms::{
    KnownServiceProvider, Netstack, Netstack3, NetstackVersion, TestSandboxExt as _,
};
use netstack_testing_common::{Result, devices};
use netstack_testing_macros::netstack_test;
use packet::{
    NestableSerializer as _, NoOpSerializationContext, ParsablePacket as _, Serializer as _,
};
use packet_formats::icmp::{
    IcmpEchoRequest, IcmpPacketBuilder, IcmpZeroCode, Icmpv4Packet, Icmpv6Packet, MessageBody,
};
use packet_formats::igmp::messages::IgmpPacket;
use packet_formats::ip::{Ipv4Proto, Ipv6Proto};
use packet_formats::ipv4::{Ipv4Header as _, Ipv4Packet, Ipv4PacketBuilder};
use packet_formats::ipv6::{Ipv6Header, Ipv6Packet, Ipv6PacketBuilder};
use sockaddr::{IntoSockAddr as _, PureIpSockaddr, TryToSockaddrLl};
use test_case::{test_case, test_matrix};

mod icmp;
mod raw;
mod tcp;
mod udp;

pub(crate) const CLIENT_SUBNET: fnet::Subnet = fidl_subnet!("192.168.0.2/24");
pub(crate) const SERVER_SUBNET: fnet::Subnet = fidl_subnet!("192.168.0.1/24");
pub(crate) const CLIENT_MAC: fnet::MacAddress = fidl_mac!("02:00:00:00:00:02");
pub(crate) const SERVER_MAC: fnet::MacAddress = fidl_mac!("02:00:00:00:00:01");

async fn run_ip_endpoint_packet_socket_test(
    server: &netemul::TestRealm<'_>,
    server_iface_id: u64,
    client: &netemul::TestRealm<'_>,
    client_iface_id: u64,
    ip_version: IpVersion,
    kind: fpacket::Kind,
) {
    async fn new_packet_socket_in_realm(
        realm: &netemul::TestRealm<'_>,
        addr: PureIpSockaddr,
        kind: fpacket::Kind,
    ) -> Result<fasync::net::DatagramSocket> {
        let socket = realm.packet_socket(kind).await.context("creating packet socket")?;
        let sockaddr = libc::sockaddr_ll::from(addr).into_sockaddr();
        socket.bind(&sockaddr).context("binding packet_socket")?;
        let socket = fasync::net::DatagramSocket::new_from_socket(socket)
            .context("wrapping packet socket in fuchsia-async DatagramSocket")?;
        Ok(socket)
    }

    let client_iface_id = NonZeroU64::new(client_iface_id).expect("client iface id is 0");
    let server_iface_id = NonZeroU64::new(server_iface_id).expect("server iface id is 0");

    let client_sock = new_packet_socket_in_realm(
        client,
        PureIpSockaddr { interface_id: Some(client_iface_id), protocol: ip_version },
        kind,
    )
    .await
    .expect("failed to create client socket");

    let server_sock = new_packet_socket_in_realm(
        server,
        PureIpSockaddr { interface_id: Some(server_iface_id), protocol: ip_version },
        kind,
    )
    .await
    .expect("failed to create server socket");

    const PAYLOAD: &'static str = "Hello World";
    let send_to_addr = libc::sockaddr_ll::from(PureIpSockaddr {
        interface_id: Some(client_iface_id),
        protocol: ip_version,
    })
    .into_sockaddr();
    let r = client_sock.send_to(PAYLOAD.as_bytes(), send_to_addr).await.expect("sendto failed");
    assert_eq!(r, PAYLOAD.as_bytes().len());

    let mut buf = [0u8; 1024];
    // Receive from the socket, ignoring all spurious data that may be observed
    // from the network.
    let (recv_len, from) = {
        loop {
            let (recv_len, from) =
                server_sock.recv_from(&mut buf[..]).await.expect("failed to receive");
            match is_packet_spurious(ip_version, &buf[..recv_len]) {
                // NB: IPv4/IPv6 Parse errors are expected, since we're sending
                // "Hello World" and not a valid packet.
                Err(_) | Ok(false) => break (recv_len, from),
                Ok(true) => continue,
            }
        }
    };
    assert_eq!(recv_len, PAYLOAD.as_bytes().len());
    assert_eq!(&buf[..recv_len], PAYLOAD.as_bytes());
    assert_eq!(i32::from(from.family()), libc::AF_PACKET);
    let from = from.try_to_sockaddr_ll().expect("unexpected peer SockAddress type");
    assert_eq!(from.sll_protocol, sockaddr::sockaddr_ll_ip_protocol(ip_version));
    // As defined by Linux in `if_packet.h``.
    const PACKET_HOST: u8 = 0;
    assert_eq!(from.sll_pkttype, PACKET_HOST);
    // IP endpoints don't have a hardware address.
    assert_eq!(from.sll_halen, 0);
    assert_eq!(from.sll_addr, [0, 0, 0, 0, 0, 0, 0, 0]);
}

pub(crate) trait TestIpExt: packet_formats::ip::IpExt {
    const DOMAIN: fposix_socket::Domain;
    const CLIENT_SUBNET: fnet::Subnet;
    const CLIENT_ADDR: Self::Addr;
    const SERVER_SUBNET: fnet::Subnet;
    const SERVER_ADDR: Self::Addr;
}

impl TestIpExt for Ipv4 {
    const DOMAIN: fposix_socket::Domain = fposix_socket::Domain::Ipv4;
    const CLIENT_SUBNET: fnet::Subnet = fidl_subnet!("192.168.0.2/24");
    const CLIENT_ADDR: Ipv4Addr = net_ip_v4!("192.168.0.2");
    const SERVER_SUBNET: fnet::Subnet = fidl_subnet!("192.168.0.1/24");
    const SERVER_ADDR: Ipv4Addr = net_ip_v4!("192.168.0.1");
}

impl TestIpExt for Ipv6 {
    const DOMAIN: fposix_socket::Domain = fposix_socket::Domain::Ipv6;
    const CLIENT_SUBNET: fnet::Subnet = fidl_subnet!("2001:0db8:85a3::8a2e:0370:7334/64");
    const CLIENT_ADDR: Ipv6Addr = net_ip_v6!("2001:0db8:85a3::8a2e:0370:7334");
    const SERVER_SUBNET: fnet::Subnet = fidl_subnet!("2001:0db8:85a3::8a2e:0370:7335/64");
    const SERVER_ADDR: Ipv6Addr = net_ip_v6!("2001:0db8:85a3::8a2e:0370:7335");
}

// Helper function to add ip device to stack.
async fn install_ip_device(
    realm: &netemul::TestRealm<'_>,
    port: fhardware_network::PortProxy,
    addrs: impl IntoIterator<Item = fnet::Subnet>,
) -> (u64, fnet_interfaces_ext::admin::Control, fnet_interfaces_admin::DeviceControlProxy) {
    let installer = realm.connect_to_protocol::<fnet_interfaces_admin::InstallerMarker>().unwrap();

    let port_id = port.get_info().await.expect("get port info").id.expect("missing port id");
    let device = {
        let (device, server_end) =
            fidl::endpoints::create_endpoints::<fhardware_network::DeviceMarker>();
        port.get_device(server_end).expect("get device");
        device
    };
    let device_control = {
        let (control, server_end) =
            fidl::endpoints::create_proxy::<fnet_interfaces_admin::DeviceControlMarker>();
        installer.install_device(device, server_end).expect("install device");
        control
    };
    let control = {
        let (control, server_end) =
            fnet_interfaces_ext::admin::Control::create_endpoints().expect("create endpoints");
        device_control
            .create_interface(&port_id, server_end, fnet_interfaces_admin::Options::default())
            .expect("create interface");
        control
    };
    assert!(control.enable().await.expect("enable interface").expect("failed to enable interface"));

    let id = control.get_id().await.expect("get id");

    futures::stream::iter(addrs.into_iter())
        .for_each_concurrent(None, |subnet| {
            let (address_state_provider, server_end) = fidl::endpoints::create_proxy::<
                fnet_interfaces_admin::AddressStateProviderMarker,
            >();

            // We're not interested in maintaining the address' lifecycle through
            // the proxy.
            address_state_provider.detach().expect("detach");
            control
                .add_address(
                    &subnet,
                    &fnet_interfaces_admin::AddressParameters {
                        add_subnet_route: Some(true),
                        ..Default::default()
                    },
                    server_end,
                )
                .expect("add address");

            // Wait for the address to be assigned.
            fnet_interfaces_ext::admin::wait_assignment_state(
                fnet_interfaces_ext::admin::assignment_state_stream(address_state_provider),
                fnet_interfaces::AddressAssignmentState::Assigned,
            )
            .map(|r| r.expect("wait assignment state"))
        })
        .await;
    (id, control, device_control)
}

/// Creates default base config for an IP tun device.
fn base_ip_device_port_config() -> fnet_tun::BasePortConfig {
    fnet_tun::BasePortConfig {
        id: Some(devices::TUN_DEFAULT_PORT_ID),
        mtu: Some(netemul::DEFAULT_MTU.into()),
        rx_types: Some(vec![
            fhardware_network::FrameType::Ipv4,
            fhardware_network::FrameType::Ipv6,
        ]),
        tx_types: Some(vec![
            fhardware_network::FrameTypeSupport {
                type_: fhardware_network::FrameType::Ipv4,
                features: fhardware_network::FRAME_FEATURES_RAW,
                supported_flags: fhardware_network::TxFlags::empty(),
            },
            fhardware_network::FrameTypeSupport {
                type_: fhardware_network::FrameType::Ipv6,
                features: fhardware_network::FRAME_FEATURES_RAW,
                supported_flags: fhardware_network::TxFlags::empty(),
            },
        ]),
        ..Default::default()
    }
}

enum IpEndpointsSocketTestCase {
    Udp,
    Tcp,
    Packet(fpacket::Kind),
}

#[netstack_test]
#[variant(N, Netstack)]
#[variant(I, Ip)]
#[test_case(IpEndpointsSocketTestCase::Udp; "udp_socket")]
#[test_case(IpEndpointsSocketTestCase::Tcp; "tcp_socket")]
#[test_case(IpEndpointsSocketTestCase::Packet(fpacket::Kind::Network); "packet_dgram_socket")]
#[test_case(IpEndpointsSocketTestCase::Packet(fpacket::Kind::Link); "packet_raw_socket")]
async fn ip_endpoints_socket<N: Netstack, I: Ip>(
    name: &str,
    socket_type: IpEndpointsSocketTestCase,
) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let client = sandbox
        .create_netstack_realm::<N, _>(format!("{}_client", name))
        .expect("failed to create client realm");
    let server = sandbox
        .create_netstack_realm::<N, _>(format!("{}_server", name))
        .expect("failed to create server realm");

    let (_tun_pair, client_port, server_port) = devices::create_tun_pair_with(
        fnet_tun::DevicePairConfig::default(),
        fnet_tun::DevicePairPortConfig {
            base: Some(base_ip_device_port_config()),
            // No MAC, this is a pure IP device.
            mac_left: None,
            mac_right: None,
            ..Default::default()
        },
    )
    .await;

    // Addresses must be in the same subnet.
    let (client_addr, server_addr) = match I::VERSION {
        IpVersion::V4 => (fidl_subnet!("192.168.0.1/24"), fidl_subnet!("192.168.0.2/24")),
        IpVersion::V6 => (fidl_subnet!("2001::1/120"), fidl_subnet!("2001::2/120")),
    };

    // We install both devices in parallel because a DevicePair will only have
    // its link signal set to up once both sides have sessions attached. This
    // way both devices will be configured "at the same time" and DAD will be
    // able to complete for IPv6 addresses.
    let (
        (client_id, _client_control, _client_device_control),
        (server_id, _server_control, _server_device_control),
    ) = futures::future::join(
        install_ip_device(&client, client_port, [client_addr]),
        install_ip_device(&server, server_port, [server_addr]),
    )
    .await;

    match socket_type {
        IpEndpointsSocketTestCase::Udp => {
            udp::run_udp_socket_test(&server, server_addr.addr, &client, client_addr.addr).await
        }
        IpEndpointsSocketTestCase::Tcp => {
            tcp::run_tcp_socket_test(&server, server_addr.addr, &client, client_addr.addr).await
        }
        IpEndpointsSocketTestCase::Packet(kind) => {
            run_ip_endpoint_packet_socket_test(
                &server,
                server_id,
                &client,
                client_id,
                I::VERSION,
                kind,
            )
            .await
        }
    }
}

/// Returns true if the packet is one of is IGMP, MLD, or NDP.

/// This traffic implicitly exists on the network, and may be unexpectedly
/// received during tests who interact directly with the underlying device (e.g.
/// via packet sockets, or via the netdevice APIs).
///
/// Returns `Err` if the packet cannot be parsed.
fn is_packet_spurious(ip_version: IpVersion, mut body: &[u8]) -> Result<bool> {
    match ip_version {
        IpVersion::V6 => {
            let ipv6 = Ipv6Packet::parse(&mut body, ())
                .with_context(|| format!("failed to parse IPv6 packet {:?}", body))?;
            if ipv6.proto() == Ipv6Proto::Icmpv6 {
                let parse_args =
                    packet_formats::icmp::IcmpParseArgs::new(ipv6.src_ip(), ipv6.dst_ip());
                match Icmpv6Packet::parse(&mut body, parse_args)
                    .context("failed to parse ICMP packet")?
                {
                    Icmpv6Packet::Ndp(p) => {
                        println!("ignoring NDP packet {:?}", p);
                        Ok(true)
                    }
                    Icmpv6Packet::Mld(p) => {
                        println!("ignoring MLD packet {:?}", p);
                        Ok(true)
                    }
                    Icmpv6Packet::DestUnreachable(_)
                    | Icmpv6Packet::PacketTooBig(_)
                    | Icmpv6Packet::TimeExceeded(_)
                    | Icmpv6Packet::ParameterProblem(_)
                    | Icmpv6Packet::EchoRequest(_)
                    | Icmpv6Packet::EchoReply(_) => Ok(false),
                }
            } else {
                Ok(false)
            }
        }
        IpVersion::V4 => {
            let ipv4 = Ipv4Packet::parse(&mut body, ())
                .with_context(|| format!("failed to parse IPv4 packet {:?}", body))?;
            if ipv4.proto() == Ipv4Proto::Igmp {
                let p = IgmpPacket::parse(&mut body, ()).context("failed to parse IGMP packet")?;
                println!("ignoring IGMP packet {:?}", p);
                Ok(true)
            } else {
                Ok(false)
            }
        }
    }
}

#[netstack_test]
#[variant(N, Netstack)]
async fn ip_endpoint_packets<N: Netstack>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm = sandbox.create_netstack_realm::<N, _>(name).expect("failed to create client realm");

    let tun = fuchsia_component::client::connect_to_protocol::<fnet_tun::ControlMarker>()
        .expect("failed to connect to tun protocol");

    let (tun_dev, req) = fidl::endpoints::create_proxy::<fnet_tun::DeviceMarker>();
    tun.create_device(
        &fnet_tun::DeviceConfig { base: None, blocking: Some(true), ..Default::default() },
        req,
    )
    .expect("failed to create tun pair");

    let (_tun_port, port) = {
        let (tun_port, server_end) = fidl::endpoints::create_proxy::<fnet_tun::PortMarker>();
        tun_dev
            .add_port(
                &fnet_tun::DevicePortConfig {
                    base: Some(base_ip_device_port_config()),
                    online: Some(true),
                    // No MAC, this is a pure IP device.
                    mac: None,
                    ..Default::default()
                },
                server_end,
            )
            .expect("add_port failed");

        let (port, server_end) = fidl::endpoints::create_proxy::<fhardware_network::PortMarker>();
        tun_port.get_port(server_end).expect("get_port failed");
        (tun_port, port)
    };

    // Declare addresses in the same subnet. Alice is Netstack, and Bob is our
    // end of the tun device that we'll use to inject frames.
    const PREFIX_V4: u8 = 24;
    const PREFIX_V6: u8 = 120;
    const ALICE_ADDR_V4: fnet::Ipv4Address = fidl_ip_v4!("192.168.0.1");
    const ALICE_ADDR_V6: fnet::Ipv6Address = fidl_ip_v6!("2001::1");
    const BOB_ADDR_V4: fnet::Ipv4Address = fidl_ip_v4!("192.168.0.2");
    const BOB_ADDR_V6: fnet::Ipv6Address = fidl_ip_v6!("2001::2");

    let (_id, _control, _device_control) = install_ip_device(
        &realm,
        port,
        [
            fnet::Subnet { addr: fnet::IpAddress::Ipv4(ALICE_ADDR_V4), prefix_len: PREFIX_V4 },
            fnet::Subnet { addr: fnet::IpAddress::Ipv6(ALICE_ADDR_V6), prefix_len: PREFIX_V6 },
        ],
    )
    .await;

    let read_frame = futures::stream::try_unfold(tun_dev.clone(), |tun_dev| async move {
        let frame = tun_dev
            .read_frame()
            .await
            .context("read_frame_failed")?
            .map_err(zx::Status::err_from_raw)
            .context("read_frame returned error")?;
        Ok(Some((frame, tun_dev)))
    })
    .try_filter_map(|frame| async move {
        let frame_type = frame.frame_type.context("missing frame type in frame")?;
        let frame_data = frame.data.context("missing data in frame")?;
        let is_spurious = match frame_type {
            fhardware_network::FrameType::Ipv6 => {
                is_packet_spurious(IpVersion::V6, &frame_data[..])
            }
            fhardware_network::FrameType::Ipv4 => {
                is_packet_spurious(IpVersion::V4, &frame_data[..])
            }
            fhardware_network::FrameType::Ethernet => Ok(false),
            fhardware_network::FrameType::__SourceBreaking { unknown_ordinal } => {
                panic!("unknown frame type {unknown_ordinal}")
            }
        }?;
        Ok((!is_spurious).then_some((frame_type, frame_data)))
    });
    let mut read_frame = pin!(read_frame);

    async fn write_frame_and_read_with_timeout<S>(
        tun_dev: &fnet_tun::DeviceProxy,
        frame: fnet_tun::Frame,
        read_frame: &mut S,
    ) -> Result<Option<S::Ok>>
    where
        S: futures::stream::TryStream<Error = anyhow::Error> + std::marker::Unpin,
    {
        tun_dev
            .write_frame(&frame)
            .await
            .context("write_frame failed")?
            .map_err(zx::Status::err_from_raw)
            .context("write_frame returned error")?;
        Ok(read_frame
            .try_next()
            .and_then(|f| {
                futures::future::ready(f.context("frame stream ended unexpectedly").map(Some))
            })
            .on_timeout(
                fasync::MonotonicInstant::after(zx::MonotonicDuration::from_millis(50)),
                || Ok(None),
            )
            .await
            .context("failed to read frame")?)
    }

    const ICMP_ID: u16 = 10;
    const SEQ_NUM: u16 = 1;
    let mut payload = [1u8, 2, 3, 4];

    // Manually build a ping frame and see it come back out of the stack.
    let src_ip = Ipv4Addr::new(BOB_ADDR_V4.addr);
    let dst_ip = Ipv4Addr::new(ALICE_ADDR_V4.addr);
    let packet = packet::Buf::new(&mut payload[..], ..)
        .wrap_in(IcmpPacketBuilder::<Ipv4, _>::new(
            src_ip,
            dst_ip,
            IcmpZeroCode,
            IcmpEchoRequest::new(ICMP_ID, SEQ_NUM),
        ))
        .wrap_in(Ipv4PacketBuilder::new(src_ip, dst_ip, 1, Ipv4Proto::Icmp))
        .serialize_vec_outer(&mut NoOpSerializationContext)
        .expect("serialization failed")
        .as_ref()
        .to_vec();

    // Send v4 ping request.
    tun_dev
        .write_frame(&fnet_tun::Frame {
            port: Some(devices::TUN_DEFAULT_PORT_ID),
            frame_type: Some(fhardware_network::FrameType::Ipv4),
            data: Some(packet.clone()),
            meta: None,
            ..Default::default()
        })
        .await
        .expect("write_frame failed")
        .map_err(zx::Status::err_from_raw)
        .expect("write_frame returned error");

    // Read ping response.
    let (frame_type, data) = read_frame
        .try_next()
        .await
        .expect("failed to read ping response")
        .expect("frame stream ended unexpectedly");
    assert_eq!(frame_type, fhardware_network::FrameType::Ipv4);
    let mut bv = &data[..];
    let ipv4_packet = Ipv4Packet::parse(&mut bv, ()).expect("failed to parse IPv4 packet");
    assert_eq!(ipv4_packet.src_ip(), dst_ip);
    assert_eq!(ipv4_packet.dst_ip(), src_ip);
    assert_eq!(ipv4_packet.proto(), packet_formats::ip::Ipv4Proto::Icmp);

    let parse_args =
        packet_formats::icmp::IcmpParseArgs::new(ipv4_packet.src_ip(), ipv4_packet.dst_ip());
    let icmp_packet =
        match Icmpv4Packet::parse(&mut bv, parse_args).expect("failed to parse ICMP packet") {
            Icmpv4Packet::EchoReply(reply) => reply,
            p => panic!("got ICMP packet {:?}, want EchoReply", p),
        };
    assert_eq!(icmp_packet.message().id(), ICMP_ID);
    assert_eq!(icmp_packet.message().seq(), SEQ_NUM);

    let (inner_header, inner_body) = icmp_packet.body().bytes();
    assert!(inner_body.is_none());
    assert_eq!(inner_header, &payload[..]);

    // Send the same data again, but with an IPv6 frame type, expect that it'll
    // fail parsing and no response will be generated.
    assert_matches!(
        write_frame_and_read_with_timeout(
            &tun_dev,
            fnet_tun::Frame {
                port: Some(devices::TUN_DEFAULT_PORT_ID),
                frame_type: Some(fhardware_network::FrameType::Ipv6),
                data: Some(packet),
                meta: None,
                ..Default::default()
            },
            &mut read_frame,
        )
        .await,
        Ok(None)
    );

    // Manually build a V6 ping frame and see it come back out of the stack.
    let src_ip = Ipv6Addr::from_bytes(BOB_ADDR_V6.addr);
    let dst_ip = Ipv6Addr::from_bytes(ALICE_ADDR_V6.addr);
    let packet = packet::Buf::new(&mut payload[..], ..)
        .wrap_in(IcmpPacketBuilder::<Ipv6, _>::new(
            src_ip,
            dst_ip,
            IcmpZeroCode,
            IcmpEchoRequest::new(ICMP_ID, SEQ_NUM),
        ))
        .wrap_in(Ipv6PacketBuilder::new(src_ip, dst_ip, 1, Ipv6Proto::Icmpv6))
        .serialize_vec_outer(&mut NoOpSerializationContext)
        .expect("serialization failed")
        .as_ref()
        .to_vec();

    // Send v6 ping request.
    tun_dev
        .write_frame(&fnet_tun::Frame {
            port: Some(devices::TUN_DEFAULT_PORT_ID),
            frame_type: Some(fhardware_network::FrameType::Ipv6),
            data: Some(packet.clone()),
            meta: None,
            ..Default::default()
        })
        .await
        .expect("write_frame failed")
        .map_err(zx::Status::err_from_raw)
        .expect("write_frame returned error");

    // Read ping response.
    let (frame_type, data) = read_frame
        .try_next()
        .await
        .expect("failed to read ping response")
        .expect("frame stream ended unexpectedly");
    assert_eq!(frame_type, fhardware_network::FrameType::Ipv6);
    let mut bv = &data[..];
    let ipv6_packet = Ipv6Packet::parse(&mut bv, ()).expect("failed to parse IPv6 packet");
    assert_eq!(ipv6_packet.src_ip(), dst_ip);
    assert_eq!(ipv6_packet.dst_ip(), src_ip);
    assert_eq!(ipv6_packet.proto(), packet_formats::ip::Ipv6Proto::Icmpv6);

    let parse_args =
        packet_formats::icmp::IcmpParseArgs::new(ipv6_packet.src_ip(), ipv6_packet.dst_ip());
    let icmp_packet =
        match Icmpv6Packet::parse(&mut bv, parse_args).expect("failed to parse ICMPv6 packet") {
            Icmpv6Packet::EchoReply(reply) => reply,
            p => panic!("got ICMPv6 packet {:?}, want EchoReply", p),
        };
    assert_eq!(icmp_packet.message().id(), ICMP_ID);
    assert_eq!(icmp_packet.message().seq(), SEQ_NUM);

    let (inner_header, inner_body) = icmp_packet.body().bytes();
    assert!(inner_body.is_none());
    assert_eq!(inner_header, &payload[..]);

    // Send the same data again, but with an IPv4 frame type, expect that it'll
    // fail parsing and no response will be generated.
    assert_matches!(
        write_frame_and_read_with_timeout(
            &tun_dev,
            fnet_tun::Frame {
                port: Some(devices::TUN_DEFAULT_PORT_ID),
                frame_type: Some(fhardware_network::FrameType::Ipv4),
                data: Some(packet),
                meta: None,
                ..Default::default()
            },
            &mut read_frame,
        )
        .await,
        Ok(None)
    );
}

enum SocketType {
    Udp,
    Tcp,
}

#[netstack_test]
#[variant(N, Netstack)]
#[variant(I, Ip)]
#[test_case(SocketType::Udp, true; "UDP specified")]
#[test_case(SocketType::Udp, false; "UDP unspecified")]
#[test_case(SocketType::Tcp, true; "TCP specified")]
#[test_case(SocketType::Tcp, false; "TCP unspecified")]
// Verify socket connectivity over loopback.
// The Netstack is expected to treat the unspecified address as loopback.
async fn socket_loopback_test<N: Netstack, I: Ip>(
    name: &str,
    socket_type: SocketType,
    specified: bool,
) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm = sandbox.create_netstack_realm::<N, _>(name).expect("failed to create realm");
    let address = specified
        .then_some(I::LOOPBACK_ADDRESS.get())
        .unwrap_or(I::UNSPECIFIED_ADDRESS)
        .to_ip_addr()
        .into_ext();

    match socket_type {
        SocketType::Udp => udp::run_udp_socket_test(&realm, address, &realm, address).await,
        SocketType::Tcp => tcp::run_tcp_socket_test(&realm, address, &realm, address).await,
    }
}

#[netstack_test]
#[variant(N, Netstack)]
#[test_case(SocketType::Udp)]
#[test_case(SocketType::Tcp)]
async fn socket_clone_bind<N: Netstack>(name: &str, socket_type: SocketType) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let network = sandbox.create_network("net").await.expect("failed to create network");
    let realm = sandbox.create_netstack_realm::<N, _>(name).expect("create realm");
    let interface = realm.join_network(&network, "stack").await.expect("join network failed");
    interface
        .add_address_and_subnet_route(fidl_subnet!("192.168.1.10/16"))
        .await
        .expect("configure address");

    let socket = match socket_type {
        SocketType::Udp => realm
            .datagram_socket(
                fposix_socket::Domain::Ipv4,
                fposix_socket::DatagramSocketProtocol::Udp,
            )
            .await
            .expect("create UDP datagram socket"),
        SocketType::Tcp => realm
            .stream_socket(fposix_socket::Domain::Ipv4, fposix_socket::StreamSocketProtocol::Tcp)
            .await
            .expect("create UDP datagram socket"),
    };

    // Call `Clone` on the FIDL channel to get a new socket backed by a new
    // handle. Just cloning the Socket isn't sufficient since that calls the
    // POSIX `dup()` which is handled completely within FDIO. Instead we
    // explicitly clone the underlying FD to get a new handle and transmogrify
    // that into a new Socket.
    let other_socket: socket2::Socket =
        fdio::create_fd(fdio::clone_fd(socket.as_fd()).expect("clone_fd failed"))
            .expect("create_fd failed")
            .into();

    // Since both sockets refer to the same resource, binding one will affect
    // the other's bound address.

    let bind_addr = std_socket_addr!("127.0.0.1:2048");
    socket.bind(&bind_addr.clone().into()).expect("bind should succeed");

    let local_addr = other_socket.local_addr().expect("local addr exists");
    assert_eq!(bind_addr, local_addr.as_socket().unwrap());
}

trait MakeSocket: Sized {
    async fn new_in_realm<I: TestIpExt>(t: &netemul::TestRealm<'_>) -> Result<socket2::Socket>;

    fn from_socket(s: socket2::Socket) -> Result<Self>;
}

impl MakeSocket for UdpSocket {
    async fn new_in_realm<I: TestIpExt>(t: &netemul::TestRealm<'_>) -> Result<socket2::Socket> {
        t.datagram_socket(I::DOMAIN, fposix_socket::DatagramSocketProtocol::Udp).await
    }

    fn from_socket(s: socket2::Socket) -> Result<Self> {
        UdpSocket::from_datagram(DatagramSocket::new_from_socket(s)?).map_err(Into::into)
    }
}

struct TcpSocket(socket2::Socket);

impl MakeSocket for TcpSocket {
    async fn new_in_realm<I: TestIpExt>(t: &netemul::TestRealm<'_>) -> Result<socket2::Socket> {
        t.stream_socket(I::DOMAIN, fposix_socket::StreamSocketProtocol::Tcp).await
    }

    fn from_socket(s: socket2::Socket) -> Result<Self> {
        Ok(Self(s))
    }
}

#[derive(Debug)]
struct Interface<'a, A> {
    iface: TestInterface<'a>,
    ip: A,
}

#[derive(Debug)]
struct Network<'a, A> {
    peer_realm: netemul::TestRealm<'a>,
    peer_interface: Interface<'a, A>,
    _network: netemul::TestNetwork<'a>,
    multinic_interface: Interface<'a, A>,
}

/// Sets up [`num_peers`]+1 realms: `num_peers` peers and 1 multi-nic host. Each
/// peer is connected to the multi-nic host via a different network. Once the
/// hosts are set up and sockets initialized, the provided callback is called.
///
/// When `call_with_sockets` is invoked, all of these sockets are provided as
/// arguments. The first argument contains the sockets in the multi-NIC realm,
/// and the second argument is the socket in the peer realm.
///
/// NB: in order for callers to provide a `call_with_networks` that captures
/// its environment, we need to constrain the HRTB lifetime `'a` with
/// `'params: 'a`, i.e. "`'params`' outlives `'a`". Since "where" clauses are
/// unsupported for HRTB, the only way to do this is with an implied bound.
/// The type `&'a &'params ()` is only well-formed if `'params: 'a`, so adding
/// an argument of that type implies the bound.
/// See https://stackoverflow.com/a/72673740 for a more thorough explanation.
async fn with_multinic_and_peer_networks<
    'params,
    N: Netstack,
    I: TestIpExt,
    F: for<'a> FnOnce(
        Vec<Network<'a, I::Addr>>,
        &'a netemul::TestRealm<'a>,
        &'a &'params (),
    ) -> LocalBoxFuture<'a, ()>,
>(
    name: &str,
    num_peers: u8,
    subnet: net_types::ip::Subnet<I::Addr>,
    call_with_networks: F,
) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let sandbox = &sandbox;

    let multinic =
        sandbox.create_netstack_realm::<N, _>(format!("{name}_multinic")).expect("create realm");
    let multinic = &multinic;

    let networks: Vec<_> = future::join_all((0..num_peers).map(|i| async move {
        // Put all addresses in a single subnet, where the mult-nic host's
        // interface will have an address with a final octet of 1, and the peer
        // a final octet of 2.
        let ip = |host| -> I::Addr {
            I::map_ip(
                (subnet.network(), IpInvariant(host)),
                |(v4, IpInvariant(host))| {
                    let mut addr = v4.ipv4_bytes();
                    *addr.last_mut().unwrap() = host;
                    Ipv4Addr::new(addr)
                },
                |(v6, IpInvariant(host))| {
                    let mut addr = v6.ipv6_bytes();
                    *addr.last_mut().unwrap() = host;
                    net_types::ip::Ipv6Addr::from_bytes(addr)
                },
            )
        };
        let multinic_ip = ip(1);
        let peer_ip = ip(2);

        let network = sandbox.create_network(format!("net_{i}")).await.expect("create network");
        let (peer_realm, peer_interface) = {
            let peer = sandbox
                .create_netstack_realm::<N, _>(format!("{name}_peer_{i}"))
                .expect("create realm");
            let peer_iface = peer
                .join_network(&network, format!("peer-{i}-ep"))
                .await
                .expect("install interface in peer netstack");
            peer_iface
                .add_address_and_subnet_route(fnet::Subnet {
                    addr: peer_ip.to_ip_addr().into_ext(),
                    prefix_len: subnet.prefix(),
                })
                .await
                .expect("configure address");
            peer_iface.apply_nud_flake_workaround().await.expect("nud flake workaround");
            (peer, Interface { iface: peer_iface, ip: peer_ip.into() })
        };
        let multinic_interface = {
            let name = format!("multinic-ep-{i}");
            let multinic_iface =
                multinic.join_network(&network, name).await.expect("adding interface failed");
            multinic_iface
                .add_address_and_subnet_route(fnet::Subnet {
                    addr: multinic_ip.to_ip_addr().into_ext(),
                    prefix_len: subnet.prefix(),
                })
                .await
                .expect("configure address");
            multinic_iface.apply_nud_flake_workaround().await.expect("nud flake workaround");
            Interface { iface: multinic_iface, ip: multinic_ip.into() }
        };
        Network { peer_realm, peer_interface, _network: network, multinic_interface }
    }))
    .await;

    call_with_networks(networks, multinic, &&()).await
}

async fn with_multinic_and_peers<
    N: Netstack,
    S: MakeSocket,
    I: TestIpExt,
    F: FnOnce(Vec<MultiNicAndPeerConfig<S>>) -> R,
    R: Future<Output = ()>,
>(
    name: &str,
    num_peers: u8,
    subnet: net_types::ip::Subnet<I::Addr>,
    port: u16,
    call_with_sockets: F,
) {
    with_multinic_and_peer_networks::<N, I, _>(name, num_peers, subnet, |networks, multinic, ()| {
        Box::pin(async move {
            let config = future::join_all(networks.iter().map(
                |Network {
                     peer_realm,
                     peer_interface: Interface { iface: _, ip: peer_ip },
                     multinic_interface: Interface { iface: multinic_iface, ip: multinic_ip },
                     _network,
                 }| async move {
                    let multinic_socket = {
                        let socket = S::new_in_realm::<I>(multinic).await.expect("creating socket");

                        socket
                            .bind_device(Some(
                                multinic_iface
                                    .get_interface_name()
                                    .await
                                    .expect("get_name failed")
                                    .as_bytes(),
                            ))
                            .and_then(|()| {
                                socket.bind(
                                    &std::net::SocketAddr::from((
                                        std::net::Ipv4Addr::UNSPECIFIED,
                                        port,
                                    ))
                                    .into(),
                                )
                            })
                            .expect("failed to bind device");
                        S::from_socket(socket).expect("failed to create server socket")
                    };
                    let peer_socket = S::new_in_realm::<I>(&peer_realm)
                        .await
                        .and_then(|s| {
                            s.bind(
                                &std::net::SocketAddr::from((
                                    std::net::Ipv4Addr::UNSPECIFIED,
                                    port,
                                ))
                                .into(),
                            )?;
                            S::from_socket(s)
                        })
                        .expect("bind failed");
                    MultiNicAndPeerConfig {
                        multinic_socket,
                        multinic_ip: multinic_ip.clone().into(),
                        peer_socket,
                        peer_ip: peer_ip.clone().into(),
                    }
                },
            ))
            .await;

            call_with_sockets(config).await
        })
    })
    .await
}

struct MultiNicAndPeerConfig<S> {
    multinic_ip: net_types::ip::IpAddr,
    multinic_socket: S,
    peer_ip: net_types::ip::IpAddr,
    peer_socket: S,
}

#[derive(PartialEq)]
enum ProtocolWithZirconSocket {
    Tcp,
    FastUdp,
}

#[netstack_test]
#[variant(N, Netstack)]
#[test_case(ProtocolWithZirconSocket::Tcp)]
#[test_case(ProtocolWithZirconSocket::FastUdp)]
async fn zx_socket_rights<N: Netstack>(name: &str, protocol: ProtocolWithZirconSocket) {
    // TODO(https://fxbug.dev/42182397): Remove this test when Fast UDP is
    // supported by Netstack3.
    if matches!(N::VERSION, NetstackVersion::Netstack3 | NetstackVersion::ProdNetstack3)
        && protocol == ProtocolWithZirconSocket::FastUdp
    {
        return;
    }

    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let netstack = match N::VERSION {
        NetstackVersion::Netstack2 { tracing: false, fast_udp: false } => sandbox
            .create_realm(
                format!("{}", name),
                [KnownServiceProvider::Netstack(NetstackVersion::Netstack2 {
                    fast_udp: true,
                    tracing: false,
                })],
            )
            .expect("create realm"),
        NetstackVersion::Netstack3 => {
            sandbox.create_netstack_realm::<N, _>(format!("{}", name)).expect("create realm")
        }
        v @ (NetstackVersion::Netstack2 { tracing: _, fast_udp: _ }
        | NetstackVersion::ProdNetstack2
        | NetstackVersion::ProdNetstack3) => panic!(
            "netstack_test should only be parameterized with Netstack2 or Netstack3: got {:?}",
            v
        ),
    };

    let provider = netstack
        .connect_to_protocol::<fposix_socket::ProviderMarker>()
        .expect("connect to socket provider");
    let socket = match protocol {
        ProtocolWithZirconSocket::Tcp => {
            let socket = provider
                .stream_socket(
                    fposix_socket::Domain::Ipv4,
                    fposix_socket::StreamSocketProtocol::Tcp,
                )
                .await
                .expect("call stream socket")
                .expect("request stream socket");
            let fposix_socket::StreamSocketDescribeResponse { socket, .. } =
                socket.into_proxy().describe().await.expect("call describe");
            socket
        }
        ProtocolWithZirconSocket::FastUdp => {
            let response = provider
                .datagram_socket(
                    fposix_socket::Domain::Ipv4,
                    fposix_socket::DatagramSocketProtocol::Udp,
                )
                .await
                .expect("call datagram socket")
                .expect("request datagram socket");
            let socket = match response {
                fposix_socket::ProviderDatagramSocketResponse::SynchronousDatagramSocket(_) => {
                    panic!("expected fast udp socket, got sync udp")
                }
                fposix_socket::ProviderDatagramSocketResponse::DatagramSocket(socket) => socket,
            };
            let fposix_socket::DatagramSocketDescribeResponse { socket, .. } =
                socket.into_proxy().describe().await.expect("call describe");
            socket
        }
    };

    let zx::HandleBasicInfo { rights, .. } = socket
        .expect("zircon socket returned by describe")
        .basic_info()
        .expect("get socket basic info");
    assert_eq!(
        rights.bits(),
        zx::sys::ZX_RIGHT_TRANSFER
            | zx::sys::ZX_RIGHT_WAIT
            | zx::sys::ZX_RIGHT_INSPECT
            | zx::sys::ZX_RIGHT_WRITE
            | zx::sys::ZX_RIGHT_READ
            // TODO(https://fxbug.dev/417777189): Remove signal rights when no
            // longer necessary for ffx support.
            | zx::sys::ZX_RIGHT_SIGNAL
    );
}

trait MulticastTestIpExt:
    packet_formats::ip::IpExt
    + packet_formats::ip::IpProtoExt
    + packet_formats::icmp::IcmpIpExt
    + TestIpExt
{
    const NETWORKS: [fnet::Subnet; 2];
    const MCAST_ADDR: std::net::SocketAddr;

    fn iface_ip(index: usize) -> std::net::IpAddr {
        match Self::NETWORKS[index].addr {
            fnet::IpAddress::Ipv4(addr) => std::net::IpAddr::V4(addr.addr.into()),
            fnet::IpAddress::Ipv6(addr) => std::net::IpAddr::V6(addr.addr.into()),
        }
    }
}

impl MulticastTestIpExt for Ipv4 {
    const NETWORKS: [fnet::Subnet; 2] =
        [fidl_subnet!("50.28.45.23/24"), fidl_subnet!("10.0.0.1/24")];
    const MCAST_ADDR: std::net::SocketAddr = std_socket_addr!("224.0.0.5:3513");
}

impl MulticastTestIpExt for Ipv6 {
    const NETWORKS: [fnet::Subnet; 2] =
        [fidl_subnet!("2001:db8::1/64"), fidl_subnet!("2001:ac::1/64")];

    // Use site-local address to ensure that a global address is picked for
    // the connection and not a link-local one.
    const MCAST_ADDR: std::net::SocketAddr = std_socket_addr!("[FF05::1:2]:3513");
}

struct MulticastTestNetwork<'a> {
    _net: TestNetwork<'a>,
    iface: TestInterface<'a>,
    receiver: TestFakeEndpoint<'a>,
}

async fn init_multicast_test_networks<'a, I: MulticastTestIpExt>(
    sandbox: &'a netemul::TestSandbox,
    client: &netemul::TestRealm<'a>,
) -> Vec<MulticastTestNetwork<'a>> {
    future::join_all(I::NETWORKS.iter().enumerate().map(|(i, subnet)| async move {
        let net =
            sandbox.create_network(format!("net{i}")).await.expect("failed to create network");
        let iface =
            client.join_network(&net, format!("if{i}")).await.expect("failed to join network");
        iface.add_address_and_subnet_route(subnet.clone()).await.expect("failed to set ip");
        let receiver = net.create_fake_endpoint().expect("failed to create endpoint");
        MulticastTestNetwork { _net: net, iface, receiver }
    }))
    .await
}

trait RedirectTestIpExt: Ip {
    const SUBNET: fnet::Subnet;
    const ADDR: std::net::IpAddr;
}

impl RedirectTestIpExt for Ipv4 {
    const SUBNET: fnet::Subnet = fidl_subnet!("192.0.2.1/24");
    const ADDR: std::net::IpAddr = std_ip!("192.0.2.1");
}

impl RedirectTestIpExt for Ipv6 {
    const SUBNET: fnet::Subnet = fidl_subnet!("2001:db8::1/64");
    const ADDR: std::net::IpAddr = std_ip!("2001:db8::1");
}

struct RedirectTestSetup<'a> {
    netstack: TestRealm<'a>,
    _network: TestNetwork<'a>,
    _interface: TestInterface<'a>,
    _control: fnet_filter::ControlProxy,
    _controller: fnet_filter_ext::Controller,
}

async fn setup_redirect_test<'a>(
    name: &str,
    sandbox: &'a TestSandbox,
    subnet: fnet::Subnet,
    matcher: fnet_matchers_ext::TransportProtocol,
    redirect: Option<RangeInclusive<NonZeroU16>>,
) -> RedirectTestSetup<'a> {
    use fnet_filter_ext::{
        Action, Change, Controller, ControllerId, Domain, InstalledNatRoutine, Matchers, Namespace,
        NamespaceId, NatHook, Resource, Routine, RoutineId, RoutineType, Rule, RuleId,
    };

    let netstack =
        sandbox.create_netstack_realm::<Netstack3, _>(name.to_owned()).expect("create netstack");
    let network = sandbox.create_network("net").await.expect("create network");
    let interface = netstack.join_network(&network, "interface").await.expect("join network");
    interface.add_address_and_subnet_route(subnet).await.expect("set ip");

    let control =
        netstack.connect_to_protocol::<fnet_filter::ControlMarker>().expect("connect to protocol");
    let mut controller = Controller::new(&control, &ControllerId(String::from("redirect")))
        .await
        .expect("create controller");
    let namespace_id = NamespaceId(String::from("namespace"));
    let routine_id = RoutineId { namespace: namespace_id.clone(), name: String::from("routine") };
    let resources = [
        Resource::Namespace(Namespace { id: namespace_id.clone(), domain: Domain::AllIp }),
        Resource::Routine(Routine {
            id: routine_id.clone(),
            routine_type: RoutineType::Nat(Some(InstalledNatRoutine {
                hook: NatHook::LocalEgress,
                priority: 0,
            })),
        }),
        Resource::Rule(Rule {
            id: RuleId { routine: routine_id.clone(), index: 0 },
            matchers: Matchers { transport_protocol: Some(matcher), ..Default::default() },
            action: Action::Redirect { dst_port: redirect.map(fnet_filter_ext::PortRange) },
        }),
    ];
    controller
        .push_changes(resources.iter().cloned().map(Change::Create).collect())
        .await
        .expect("push changes");
    controller.commit().await.expect("commit pending changes");

    RedirectTestSetup {
        netstack,
        _network: network,
        _interface: interface,
        _control: control,
        _controller: controller,
    }
}

const LISTEN_PORT: NonZeroU16 = NonZeroU16::new(11111).unwrap();

struct TestCaseV4 {
    original_dst: std::net::SocketAddr,
    matcher: fnet_matchers_ext::TransportProtocol,
    redirect_dst: Option<RangeInclusive<NonZeroU16>>,
    expect_redirect: bool,
}

#[netstack_test]
#[test_case(
    TestCaseV4 {
        original_dst: std::net::SocketAddr::new(Ipv4::ADDR, LISTEN_PORT.get()),
        matcher: fnet_matchers_ext::TransportProtocol::Tcp { src_port: None, dst_port: None },
        redirect_dst: None,
        expect_redirect: true,
    };
    "redirect to localhost"
)]
#[test_case(
    TestCaseV4 {
        original_dst: std::net::SocketAddr::new(Ipv4::ADDR, 22222),
        matcher: fnet_matchers_ext::TransportProtocol::Tcp { src_port: None, dst_port: None },
        redirect_dst: Some(LISTEN_PORT..=LISTEN_PORT),
        expect_redirect: true,
    };
    "redirect to localhost port 11111"
)]
#[test_case(
    TestCaseV4 {
        original_dst: std::net::SocketAddr::new(std_ip!("127.0.0.1"), LISTEN_PORT.get()),
        matcher: fnet_matchers_ext::TransportProtocol::Udp { src_port: None, dst_port: None },
        redirect_dst: None,
        expect_redirect: false,
    };
    "no redirect"
)]
async fn redirect_original_destination_v4(name: &str, test_case: TestCaseV4) {
    let TestCaseV4 { original_dst, matcher, redirect_dst, expect_redirect } = test_case;

    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let setup = setup_redirect_test(name, &sandbox, Ipv4::SUBNET, matcher, redirect_dst).await;

    let server = setup
        .netstack
        .stream_socket(Ipv4::DOMAIN, fposix_socket::StreamSocketProtocol::Tcp)
        .await
        .expect("create socket");
    server
        .bind(
            &std::net::SocketAddr::from((Ipv4::LOOPBACK_ADDRESS.to_ip_addr(), LISTEN_PORT.get()))
                .into(),
        )
        .expect("no conflict");
    server.listen(1).expect("listen on server socket");

    let client = setup
        .netstack
        .stream_socket(Ipv4::DOMAIN, fposix_socket::StreamSocketProtocol::Tcp)
        .await
        .expect("create socket");
    client.connect(&original_dst.into()).expect("connect to server");

    let (server, _addr) = server.accept().expect("accept incoming connection");

    // The original destination should be observable on both the client and server sockets.
    let verify_original_dst = |socket: &socket2::Socket| {
        let result = socket.original_dst_v4();
        if expect_redirect {
            assert_eq!(
                result
                    .expect("get original destination of connection")
                    .as_socket()
                    .expect("should be valid socket addr"),
                original_dst
            );
        } else {
            let error = result.expect_err("socket should have no original destination").kind();
            assert_eq!(error, std::io::ErrorKind::NotFound);
        }
    };
    verify_original_dst(&client);
    verify_original_dst(&server);
}

struct TestCaseV6 {
    original_dst: std::net::SocketAddr,
    matcher: fnet_matchers_ext::TransportProtocol,
    redirect_dst: Option<RangeInclusive<NonZeroU16>>,
}

#[netstack_test]
#[test_case(
    TestCaseV6 {
        original_dst: std::net::SocketAddr::new(Ipv6::ADDR, LISTEN_PORT.get()),
        matcher: fnet_matchers_ext::TransportProtocol::Tcp { src_port: None, dst_port: None },
        redirect_dst: None,
    };
    "redirect to localhost"
)]
#[test_case(
    TestCaseV6 {
        original_dst: std::net::SocketAddr::new(Ipv6::ADDR, 22222),
        matcher: fnet_matchers_ext::TransportProtocol::Tcp { src_port: None, dst_port: None },
        redirect_dst: Some(LISTEN_PORT..=LISTEN_PORT),
    };
    "redirect to localhost port 11111"
)]
#[test_case(
    TestCaseV6 {
        original_dst: std::net::SocketAddr::new(std_ip!("::1"), LISTEN_PORT.get()),
        matcher: fnet_matchers_ext::TransportProtocol::Udp { src_port: None, dst_port: None },
        redirect_dst: None,
    };
    "no redirect"
)]
async fn redirect_original_destination_v6(name: &str, test_case: TestCaseV6) {
    let TestCaseV6 { original_dst, matcher, redirect_dst } = test_case;

    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let setup = setup_redirect_test(name, &sandbox, Ipv6::SUBNET, matcher, redirect_dst).await;

    let server = setup
        .netstack
        .stream_socket(Ipv6::DOMAIN, fposix_socket::StreamSocketProtocol::Tcp)
        .await
        .expect("create socket");
    server
        .bind(
            &std::net::SocketAddr::from((Ipv6::LOOPBACK_ADDRESS.to_ip_addr(), LISTEN_PORT.get()))
                .into(),
        )
        .expect("no conflict");
    server.listen(1).expect("listen on server socket");

    let client = setup
        .netstack
        .stream_socket(Ipv6::DOMAIN, fposix_socket::StreamSocketProtocol::Tcp)
        .await
        .expect("create socket");
    client.connect(&original_dst.into()).expect("connect to server");

    let (server, _addr) = server.accept().expect("accept incoming connection");

    // Although this connection was redirected, SO_ORIGINAL_DST should return
    // ENOENT because the original destination was not an IPv4 address.
    let verify_original_dst = |socket: &socket2::Socket| {
        let result = socket.original_dst_v4();
        let error = result.expect_err("socket should have no original destination").kind();
        assert_eq!(error, std::io::ErrorKind::NotFound);
    };
    verify_original_dst(&client);
    verify_original_dst(&server);

    // TODO(https://fxbug.dev/345465222): exercise SOL_IPV6 - IP6T_SO_ORIGINAL_DST
    // when it is available and implemented.
}

#[netstack_test]
#[test_case(
    TestCaseV4 {
        original_dst: std::net::SocketAddr::new(Ipv4::ADDR, LISTEN_PORT.get()),
        matcher: fnet_matchers_ext::TransportProtocol::Tcp { src_port: None, dst_port: None },
        redirect_dst: None,
        expect_redirect: true,
    };
    "redirect to localhost"
)]
#[test_case(
    TestCaseV4 {
        original_dst: std::net::SocketAddr::new(Ipv4::ADDR, 22222),
        matcher: fnet_matchers_ext::TransportProtocol::Tcp { src_port: None, dst_port: None },
        redirect_dst: Some(LISTEN_PORT..=LISTEN_PORT),
        expect_redirect: true,
    };
    "redirect to localhost port 11111"
)]
#[test_case(
    TestCaseV4 {
        original_dst: std::net::SocketAddr::new(std_ip!("127.0.0.1"), LISTEN_PORT.get()),
        matcher: fnet_matchers_ext::TransportProtocol::Udp { src_port: None, dst_port: None },
        redirect_dst: None,
        expect_redirect: false,
    };
    "no redirect"
)]
async fn redirect_original_destination_dual_stack(name: &str, test_case: TestCaseV4) {
    let TestCaseV4 { original_dst, matcher, redirect_dst, expect_redirect } = test_case;

    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let setup = setup_redirect_test(name, &sandbox, Ipv4::SUBNET, matcher, redirect_dst).await;

    let server = setup
        .netstack
        .stream_socket(Ipv6::DOMAIN, fposix_socket::StreamSocketProtocol::Tcp)
        .await
        .expect("create socket");
    server
        .bind(
            &std::net::SocketAddr::from((
                Ipv6::UNSPECIFIED_ADDRESS.to_ip_addr(),
                LISTEN_PORT.get(),
            ))
            .into(),
        )
        .expect("no conflict");
    server.listen(1).expect("listen on server socket");

    let client = setup
        .netstack
        .stream_socket(Ipv4::DOMAIN, fposix_socket::StreamSocketProtocol::Tcp)
        .await
        .expect("create socket");
    client.connect(&original_dst.into()).expect("connect to server");

    let (server, _addr) = server.accept().expect("accept incoming connection");

    // The original destination should be observable on both the client and server sockets.
    let verify_original_dst = |socket: &socket2::Socket| {
        let result = socket.original_dst_v4();
        if expect_redirect {
            assert_eq!(
                result
                    .expect("get original destination of connection")
                    .as_socket()
                    .expect("should be valid socket addr"),
                original_dst
            );
        } else {
            let error = result.expect_err("socket should have no original destination").kind();
            assert_eq!(error, std::io::ErrorKind::NotFound);
        }
    };
    verify_original_dst(&client);
    verify_original_dst(&server);
}

#[netstack_test]
#[test_matrix(
    [fposix_socket::Domain::Ipv4, fposix_socket::Domain::Ipv6],
    [fposix_socket::DatagramSocketProtocol::Udp, fposix_socket::DatagramSocketProtocol::IcmpEcho],
    [fnet::MarkDomain::Mark1, fnet::MarkDomain::Mark2],
    [
        fposix_socket::OptionalUint32::Unset(fposix_socket::Empty),
        fposix_socket::OptionalUint32::Value(0)
    ]
)]
async fn datagram_socket_mark(
    name: &str,
    domain: fposix_socket::Domain,
    proto: fposix_socket::DatagramSocketProtocol,
    mark_domain: fnet::MarkDomain,
    mark: fposix_socket::OptionalUint32,
) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm =
        sandbox.create_netstack_realm::<Netstack3, _>(name).expect("failed to create client realm");
    let sock =
        realm.datagram_socket(domain, proto).await.expect("failed to create datagram socket");
    let channel = fdio::clone_channel(sock).expect("failed to clone channel");
    let proxy = fposix_socket::BaseSocketProxy::new(fidl::AsyncChannel::from_channel(channel));
    proxy.set_mark(mark_domain, &mark).await.expect("fidl error").expect("set mark");
    assert_eq!(proxy.get_mark(mark_domain).await.expect("fidl error").expect("get mark"), mark);
}

#[netstack_test]
#[test_matrix(
    [fposix_socket::Domain::Ipv4, fposix_socket::Domain::Ipv6],
    [fnet::MarkDomain::Mark1, fnet::MarkDomain::Mark2],
    [
        fposix_socket::OptionalUint32::Unset(fposix_socket::Empty),
        fposix_socket::OptionalUint32::Value(0)
    ]
)]
async fn stream_socket_mark(
    name: &str,
    domain: fposix_socket::Domain,
    mark_domain: fnet::MarkDomain,
    mark: fposix_socket::OptionalUint32,
) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm =
        sandbox.create_netstack_realm::<Netstack3, _>(name).expect("failed to create client realm");
    let sock = realm
        .stream_socket(domain, fposix_socket::StreamSocketProtocol::Tcp)
        .await
        .expect("failed to create datagram socket");
    let channel = fdio::clone_channel(sock).expect("failed to clone channel");
    let proxy = fposix_socket::BaseSocketProxy::new(fidl::AsyncChannel::from_channel(channel));
    proxy.set_mark(mark_domain, &mark).await.expect("fidl error").expect("set mark");
    assert_eq!(proxy.get_mark(mark_domain).await.expect("fidl error").expect("get mark"), mark);
}

#[netstack_test]
#[test_matrix(
    [fposix_socket::Domain::Ipv4, fposix_socket::Domain::Ipv6],
    [
        fposix_socket_raw::ProtocolAssociation::Unassociated(fposix_socket_raw::Empty),
        fposix_socket_raw::ProtocolAssociation::Associated(0)
    ],
    [fnet::MarkDomain::Mark1, fnet::MarkDomain::Mark2],
    [
        fposix_socket::OptionalUint32::Unset(fposix_socket::Empty),
        fposix_socket::OptionalUint32::Value(0)
    ]
)]
async fn raw_socket_mark(
    name: &str,
    domain: fposix_socket::Domain,
    proto: fposix_socket_raw::ProtocolAssociation,
    mark_domain: fnet::MarkDomain,
    mark: fposix_socket::OptionalUint32,
) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm =
        sandbox.create_netstack_realm::<Netstack3, _>(name).expect("failed to create client realm");
    let sock = realm.raw_socket(domain, proto).await.expect("failed to create datagram socket");
    let channel = fdio::clone_channel(sock).expect("failed to clone channel");
    let proxy = fposix_socket::BaseSocketProxy::new(fidl::AsyncChannel::from_channel(channel));
    proxy.set_mark(mark_domain, &mark).await.expect("fidl error").expect("set mark");
    assert_eq!(proxy.get_mark(mark_domain).await.expect("fidl error").expect("get mark"), mark);
}
