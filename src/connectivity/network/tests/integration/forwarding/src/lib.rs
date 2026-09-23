// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#![cfg(test)]

use assert_matches::assert_matches;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_filter as fnet_filter;
use fidl_fuchsia_net_filter_ext as fnet_filter_ext;
use fidl_fuchsia_net_interfaces_admin as fnet_interfaces_admin;
use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
use fidl_fuchsia_net_matchers_ext as fnet_matchers_ext;
use fidl_fuchsia_netemul_network as fnetemul_network;
use fidl_fuchsia_posix_socket as fposix_socket;
use fidl_fuchsia_posix_socket_packet as fposix_socket_packet;
use fidl_fuchsia_posix_socket_raw as fposix_socket_raw;
use fuchsia_async::{DurationExt, MonotonicDuration, TimeoutExt};
use futures_util::{AsyncReadExt as _, AsyncWriteExt as _, FutureExt, SinkExt, StreamExt};
use net_declare::{fidl_ip, fidl_subnet};
use net_types::ip::{Ip, IpVersion, Ipv4, Ipv6};
use netemul::{RealmTcpListener as _, RealmTcpStream as _, RealmUdpSocket};
use netstack_testing_common::interfaces::TestInterfaceExt;
use netstack_testing_common::realms::{Netstack, Netstack3, TestSandboxExt as _};
use netstack_testing_common::{
    ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT, ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT,
};
use netstack_testing_macros::netstack_test;
use packet::{
    InnerPacketBuilder as _, NestablePacketBuilder as _, NoOpSerializationContext,
    ParsablePacket as _, Serializer as _,
};
use packet_formats::ethernet::ETHERNET_HDR_LEN_NO_TAG;
use packet_formats::icmp::{IcmpParseArgs, Icmpv6Packet};
use packet_formats::ip::{FragmentOffset, IpProto, Ipv4Proto, Ipv6Proto};
use packet_formats::ipv4::{Ipv4Header as _, Ipv4Packet, Ipv4PacketBuilder};
use packet_formats::ipv6::ext_hdrs::Ipv6ExtensionHeader;
use packet_formats::ipv6::{
    IPV6_FIXED_HDR_LEN, Ipv6Packet, Ipv6PacketBuilder, Ipv6PacketBuilderWithFragmentHeader,
};
use packet_formats::udp::HEADER_BYTES as UDP_HDR_LEN;
use ping::PingError;
use sockaddr::{EthernetSockaddr, IntoSockAddr as _};
use std::num::NonZeroU64;
use test_case::test_case;

enum ForwardingConfig {
    BothDisabled,
    ClientSideEnabled,
    ServerSideEnabled,
    BothEnabled,
}

impl ForwardingConfig {
    fn is_client_enabled(&self) -> bool {
        match self {
            ForwardingConfig::BothDisabled => false,
            ForwardingConfig::ClientSideEnabled => true,
            ForwardingConfig::ServerSideEnabled => false,
            ForwardingConfig::BothEnabled => true,
        }
    }

    fn is_server_enabled(&self) -> bool {
        match self {
            ForwardingConfig::BothDisabled => false,
            ForwardingConfig::ClientSideEnabled => false,
            ForwardingConfig::ServerSideEnabled => true,
            ForwardingConfig::BothEnabled => true,
        }
    }
}

struct SetupConfig {
    client_subnet: fnet::Subnet,
    client_gateway: fnet::IpAddress,
    server_subnet: fnet::Subnet,
    server_gateway: fnet::IpAddress,
    router_client_ip: fnet::Subnet,
    router_server_ip: fnet::Subnet,
    router_client_if_config: fnet_interfaces_admin::Configuration,
    router_server_if_config: fnet_interfaces_admin::Configuration,
    router_client_ep_config: fnetemul_network::EndpointConfig,
    router_server_ep_config: fnetemul_network::EndpointConfig,
}

impl SetupConfig {
    fn ipv4(forwarding: ForwardingConfig) -> SetupConfig {
        let disabled_config = || fnet_interfaces_admin::Configuration::default();
        let enabled_config = || fnet_interfaces_admin::Configuration {
            ipv4: Some(fnet_interfaces_admin::Ipv4Configuration {
                unicast_forwarding: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        };
        let router_client_if_config = forwarding
            .is_client_enabled()
            .then_some(enabled_config())
            .unwrap_or_else(|| disabled_config());
        let router_server_if_config = forwarding
            .is_server_enabled()
            .then_some(enabled_config())
            .unwrap_or_else(|| disabled_config());

        SetupConfig {
            client_subnet: fidl_subnet!("192.168.1.2/24"),
            client_gateway: fidl_ip!("192.168.1.1"),
            server_subnet: fidl_subnet!("192.168.0.2/24"),
            server_gateway: fidl_ip!("192.168.0.1"),
            router_client_ip: fidl_subnet!("192.168.1.1/24"),
            router_server_ip: fidl_subnet!("192.168.0.1/24"),
            router_client_if_config,
            router_server_if_config,
            router_client_ep_config: netemul::new_endpoint_config(netemul::DEFAULT_MTU, None),
            router_server_ep_config: netemul::new_endpoint_config(netemul::DEFAULT_MTU, None),
        }
    }

    fn ipv6(forwarding: ForwardingConfig) -> SetupConfig {
        let disabled_config = || fnet_interfaces_admin::Configuration::default();
        let enabled_config = || fnet_interfaces_admin::Configuration {
            ipv6: Some(fnet_interfaces_admin::Ipv6Configuration {
                unicast_forwarding: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        };
        let router_client_if_config = forwarding
            .is_client_enabled()
            .then_some(enabled_config())
            .unwrap_or_else(|| disabled_config());
        let router_server_if_config = forwarding
            .is_server_enabled()
            .then_some(enabled_config())
            .unwrap_or_else(|| disabled_config());

        SetupConfig {
            client_subnet: fidl_subnet!("fd00:0:0:1::2/64"),
            client_gateway: fidl_ip!("fd00:0:0:1::1"),
            server_subnet: fidl_subnet!("fd00:0:0:2::2/64"),
            server_gateway: fidl_ip!("fd00:0:0:2::1"),
            router_client_ip: fidl_subnet!("fd00:0:0:1::1/64"),
            router_server_ip: fidl_subnet!("fd00:0:0:2::1/64"),
            router_client_if_config,
            router_server_if_config,
            router_client_ep_config: netemul::new_endpoint_config(netemul::DEFAULT_MTU, None),
            router_server_ep_config: netemul::new_endpoint_config(netemul::DEFAULT_MTU, None),
        }
    }

    // Set up two networks, connected by a router.
    async fn build<'a, N: Netstack>(
        self,
        name: &str,
        sandbox: &'a netemul::TestSandbox,
    ) -> Setup<'a> {
        let SetupConfig {
            client_subnet,
            client_gateway,
            server_subnet,
            server_gateway,
            router_client_ip,
            router_server_ip,
            router_client_if_config,
            router_server_if_config,
            router_client_ep_config,
            router_server_ep_config,
        } = self;

        let client_net = sandbox.create_network("client").await.expect("create network");
        let server_net = sandbox.create_network("server").await.expect("create network");
        let client = sandbox
            .create_netstack_realm::<N, _>(format!("{}_client", name))
            .expect("create realm");
        let server = sandbox
            .create_netstack_realm::<N, _>(format!("{}_server", name))
            .expect("create realm");
        let router = sandbox
            .create_netstack_realm::<N, _>(format!("{}_router", name))
            .expect("create realm");

        let client_iface = client
            .join_network(&client_net, "client-ep")
            .await
            .expect("install interface in client netstack");
        client_iface.add_address_and_subnet_route(client_subnet).await.expect("configure address");
        client_iface.apply_nud_flake_workaround().await.expect("nud flake workaround");
        let server_iface = server
            .join_network(&server_net, "server-ep")
            .await
            .expect("install interface in server netstack");
        server_iface.add_address_and_subnet_route(server_subnet).await.expect("configure address");
        server_iface.apply_nud_flake_workaround().await.expect("nud flake workaround");
        let router_client_iface = router
            .join_network_with(
                &client_net,
                "router-client-ep",
                router_client_ep_config,
                Default::default(),
            )
            .await
            .expect("install interface in router netstack");
        router_client_iface
            .add_address_and_subnet_route(router_client_ip)
            .await
            .expect("configure address");
        router_client_iface.apply_nud_flake_workaround().await.expect("nud flake workaround");
        let router_server_iface = router
            .join_network_with(
                &server_net,
                "router-server-ep",
                router_server_ep_config,
                Default::default(),
            )
            .await
            .expect("install interface in router netstack");
        router_server_iface
            .add_address_and_subnet_route(router_server_ip)
            .await
            .expect("configure address");
        router_server_iface.apply_nud_flake_workaround().await.expect("nud flake workaround");

        client_iface.add_default_route(client_gateway).await.expect("add default route");
        server_iface.add_default_route(server_gateway).await.expect("add default route");

        async fn configure_forwarding(
            interface: &fnet_interfaces_ext::admin::Control,
            config: &fnet_interfaces_admin::Configuration,
        ) {
            let _prev_config: fnet_interfaces_admin::Configuration = interface
                .set_configuration(config)
                .await
                .expect("call set configuration")
                .expect("set interface configuration");
        }
        configure_forwarding(router_client_iface.control(), &router_client_if_config).await;
        configure_forwarding(router_server_iface.control(), &router_server_if_config).await;
        Setup {
            _client_net: client_net,
            _server_net: server_net,
            client,
            server,
            router,
            client_iface,
            server_iface,
            router_client_iface,
            router_server_iface,
        }
    }
}

/// An instantiated test setup based on [`SetupConfig`].
struct Setup<'a> {
    _client_net: netemul::TestNetwork<'a>,
    client: netemul::TestRealm<'a>,
    client_iface: netemul::TestInterface<'a>,
    _server_net: netemul::TestNetwork<'a>,
    server: netemul::TestRealm<'a>,
    server_iface: netemul::TestInterface<'a>,
    router: netemul::TestRealm<'a>,
    router_client_iface: netemul::TestInterface<'a>,
    router_server_iface: netemul::TestInterface<'a>,
}

const PORT: u16 = 8080;
const REQUEST: &str = "hello from client";
const RESPONSE: &str = "hello from server";

#[netstack_test]
#[variant(N, Netstack)]
#[test_case(SetupConfig::ipv4(ForwardingConfig::BothEnabled); "ipv4")]
#[test_case(SetupConfig::ipv6(ForwardingConfig::BothEnabled); "ipv6")]
async fn forwarding<N: Netstack>(name: &str, setup_config: SetupConfig) {
    let server_ip = fidl_fuchsia_net_ext::IpAddress::from(setup_config.server_subnet.addr).0;
    let client_ip = fidl_fuchsia_net_ext::IpAddress::from(setup_config.client_subnet.addr).0;

    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let setup = setup_config.build::<N>(name, &sandbox).await;

    let sockaddr = std::net::SocketAddr::from((server_ip, PORT));

    let client = async {
        let mut stream = fuchsia_async::net::TcpStream::connect_in_realm(&setup.client, sockaddr)
            .await
            .expect("connect to server");
        let request = REQUEST.as_bytes();
        assert_eq!(stream.write(request).await.expect("write to stream"), request.len());
        stream.flush().await.expect("flush stream");

        let mut buffer = [0; 512];
        let read = stream.read(&mut buffer).await.expect("read from stream");
        let response = String::from_utf8_lossy(&buffer[0..read]);
        assert_eq!(response, RESPONSE, "got unexpected response from server: {}", response);
    };

    let listener = fuchsia_async::net::TcpListener::listen_in_realm(&setup.server, sockaddr)
        .await
        .expect("bind to address");
    let server = async {
        let (_listener, mut stream, remote) =
            listener.accept().await.expect("accept incoming connection");
        assert_eq!(remote.ip(), client_ip);
        let mut buffer = [0; 512];
        let read = stream.read(&mut buffer).await.expect("read from stream");
        let request = String::from_utf8_lossy(&buffer[0..read]);
        assert_eq!(request, REQUEST, "got unexpected request from client: {}", request);

        let response = RESPONSE.as_bytes();
        assert_eq!(stream.write(response).await.expect("write to stream"), response.len());
        stream.flush().await.expect("flush stream");
    };

    futures_util::future::join(client, server).await;
}

async fn send_ping_and_wait_response(
    source_realm: &netemul::TestRealm<'_>,
    addr: fnet::IpAddress,
    timeout: MonotonicDuration,
) -> Option<Result<(), PingError>> {
    async fn inner<I: ping::FuchsiaIpExt>(
        source_realm: &netemul::TestRealm<'_>,
        addr: I::SockAddr,
        timeout: MonotonicDuration,
    ) -> Option<Result<(), PingError>> {
        const PAYLOAD: [u8; 10] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        const SEQ: u16 = 1;

        let icmp_sock =
            source_realm.icmp_socket::<I>().await.expect("failed to create ICMP socket");
        let (mut sink, mut stream) = ping::new_unicast_sink_and_stream::<I, _, { u16::MAX as usize }>(
            &icmp_sock, &addr, &PAYLOAD,
        );
        sink.send(SEQ).await.expect("failed to send ping");
        stream
            .next()
            .on_timeout(timeout.after_now(), || None)
            .await
            .map(|r| r.map(|seq| assert_eq!(seq, SEQ)))
    }

    const PING_PORT: u16 = 0;
    let sockaddr =
        std::net::SocketAddr::from((fidl_fuchsia_net_ext::IpAddress::from(addr).0, PING_PORT));
    match sockaddr {
        std::net::SocketAddr::V4(a) => Box::pin(inner::<Ipv4>(source_realm, a, timeout)).await,
        std::net::SocketAddr::V6(a) => Box::pin(inner::<Ipv6>(source_realm, a, timeout)).await,
    }
}

/// Sends a single ICMP echo request from `source_realm` to `addr`, expecting it
/// to succeed.
async fn expect_successful_ping(
    source_realm: &netemul::TestRealm<'_>,
    addr: fnet::IpAddress,
    msg: &str,
) {
    assert_matches!(
        send_ping_and_wait_response(source_realm, addr, ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT).await,
        Some(Ok(())),
        "{msg}",
    );
}

/// Sends a single ICMP echo request from `source_realm` to `addr`, expecting it
/// to fail.
async fn expect_failed_ping(
    source_realm: &netemul::TestRealm<'_>,
    addr: fnet::IpAddress,
    msg: &str,
) {
    assert_matches!(
        send_ping_and_wait_response(source_realm, addr, ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT,).await,
        None,
        "{msg}",
    );
}

#[netstack_test]
#[variant(N, Netstack)]
#[test_case(SetupConfig::ipv4(ForwardingConfig::BothEnabled), true; "ipv4_with_forwarding")]
#[test_case(SetupConfig::ipv4(ForwardingConfig::BothDisabled), false; "ipv4_without_forwarding")]
#[test_case(SetupConfig::ipv6(ForwardingConfig::BothEnabled), true; "ipv6_with_forwarding")]
#[test_case(SetupConfig::ipv6(ForwardingConfig::BothDisabled), false; "ipv6_without_forwarding")]
async fn ping_other_router_addr<N: Netstack>(
    name: &str,
    setup_config: SetupConfig,
    expect_success: bool,
) {
    let router_client_ip = setup_config.router_client_ip;
    let router_server_ip = setup_config.router_server_ip;

    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let setup = setup_config.build::<N>(name, &sandbox).await;

    // Each side should be able to ping the router IP in its own network,
    // regardless of if forwarding is enabled.
    expect_successful_ping(
        &setup.client,
        router_client_ip.addr,
        "client ping router's client interface",
    )
    .await;
    expect_successful_ping(
        &setup.server,
        router_server_ip.addr,
        "server ping router's server interface",
    )
    .await;

    // Each side should be able to ping the router IP in the other network, only
    // if forwarding is enabled.
    //
    // Essentially, this verifies that the netstack operates as a weak host when
    // forwarding is enabled, and a strong host otherwise. See
    // https://en.wikipedia.org/wiki/Host_model
    if expect_success {
        expect_successful_ping(
            &setup.client,
            router_server_ip.addr,
            "client ping router's server interface",
        )
        .await;
        expect_successful_ping(
            &setup.server,
            router_client_ip.addr,
            "server ping router's client interface",
        )
        .await;
    } else {
        expect_failed_ping(
            &setup.client,
            router_server_ip.addr,
            "client ping router's server interface",
        )
        .await;
        expect_failed_ping(
            &setup.server,
            router_client_ip.addr,
            "server ping router's client interface",
        )
        .await;
    }
}

#[derive(Debug)]
enum ProbeError {
    SendFailed { _err: std::io::Error },
    RecvTimedOut,
}

/// Returns Ok(() if the sender is able to send a UDP packet to the receiver
/// within the given timeout.
async fn probe_connectivity_with_udp(
    sender: &netemul::TestRealm<'_>,
    send_addr: &fnet::Subnet,
    receiver: &netemul::TestRealm<'_>,
    receive_addr: &fnet::Subnet,
    timeout: MonotonicDuration,
) -> Result<(), ProbeError> {
    const PORT: u16 = 12345;

    // Create a pair of sender/receiver sockets.
    let send_addr =
        std::net::SocketAddr::from((fidl_fuchsia_net_ext::IpAddress::from(send_addr.addr).0, PORT));
    let send_sock = fuchsia_async::net::UdpSocket::bind_in_realm(sender, send_addr)
        .await
        .expect("bind send sock");
    let receive_addr = std::net::SocketAddr::from((
        fidl_fuchsia_net_ext::IpAddress::from(receive_addr.addr).0,
        PORT,
    ));
    let receive_sock = fuchsia_async::net::UdpSocket::bind_in_realm(receiver, receive_addr)
        .await
        .expect("bind receive sock");

    // Send data and wait for a response.
    const PAYLOAD: [u8; 10] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
    match send_sock.send_to(&PAYLOAD, receive_addr).await {
        Ok(written) => assert_eq!(written, PAYLOAD.len()),
        Err(e) => return Err(ProbeError::SendFailed { _err: e }),
    };

    let mut buf = [0u8; 10];
    let result = receive_sock
        .recv_from(&mut buf[..])
        .map(Option::Some)
        .on_timeout(timeout.after_now(), || None)
        .await;
    let (read, from) = match result {
        None => return Err(ProbeError::RecvTimedOut),
        Some(result) => result.expect("receive should succeed"),
    };
    assert_eq!(read, PAYLOAD.len());
    assert_eq!(&buf[..read], &PAYLOAD);
    assert_eq!(from, send_addr);
    Ok(())
}

/// Install a drop filter on the forwarding hook.
async fn install_forwarding_filter(
    controller: &mut fnet_filter_ext::Controller,
    ingress_if: &netemul::TestInterface<'_>,
    egress_if: &netemul::TestInterface<'_>,
    name: &str,
) {
    let namespace = fnet_filter_ext::NamespaceId(name.to_owned());
    let routine =
        fnet_filter_ext::RoutineId { namespace: namespace.clone(), name: name.to_owned() };

    let matchers = fnet_filter_ext::Matchers {
        in_interface: Some(fnet_matchers_ext::Interface::Id(
            NonZeroU64::new(ingress_if.id()).unwrap(),
        )),
        out_interface: Some(fnet_matchers_ext::Interface::Id(
            NonZeroU64::new(egress_if.id()).unwrap(),
        )),
        ..Default::default()
    };

    controller
        .push_changes(vec![
            fnet_filter_ext::Change::Create(fnet_filter_ext::Resource::Namespace(
                fnet_filter_ext::Namespace {
                    id: namespace.clone(),
                    domain: fnet_filter_ext::Domain::AllIp,
                },
            )),
            fnet_filter_ext::Change::Create(fnet_filter_ext::Resource::Routine(
                fnet_filter_ext::Routine {
                    id: routine.clone(),
                    routine_type: fnet_filter_ext::RoutineType::Ip(Some(
                        fnet_filter_ext::InstalledIpRoutine {
                            hook: fnet_filter_ext::IpHook::Forwarding,
                            priority: 0,
                        },
                    )),
                },
            )),
            fnet_filter_ext::Change::Create(fnet_filter_ext::Resource::Rule(
                fnet_filter_ext::Rule {
                    id: fnet_filter_ext::RuleId { routine: routine.clone(), index: 0 },
                    matchers,
                    action: fnet_filter_ext::Action::Drop,
                },
            )),
        ])
        .await
        .expect("push changes");
    controller.commit().await.expect("commit changes");
}

/// Verify the Weak Host "internal forwarding" behavior for traffic ingressing
/// the netstack.
#[test_case(SetupConfig::ipv4(ForwardingConfig::ClientSideEnabled); "ipv4")]
#[test_case(SetupConfig::ipv6(ForwardingConfig::ClientSideEnabled); "ipv6")]
#[fuchsia::test]
async fn internal_forwarding_ingress(setup_config: SetupConfig) {
    let client_ip = setup_config.client_subnet;
    let server_ip = setup_config.server_subnet;
    let router_client_ip = setup_config.router_client_ip;
    let router_server_ip = setup_config.router_server_ip;

    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    // NB: This test relies on fuchsia.net.filter, which is only implemented
    // for Netstack3.
    let setup = setup_config.build::<Netstack3>("internal_forwarding_ingress", &sandbox).await;

    // The client should be able to send traffic to the router's server side IP
    // but the server should not be able to send traffic to the router's client
    // side IP. This is because only the client side interface has forwarding
    // enabled.
    probe_connectivity_with_udp(
        &setup.client,
        &client_ip,
        &setup.router,
        &router_server_ip,
        ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT,
    )
    .await
    .expect("probe should succeed");
    assert_matches!(
        probe_connectivity_with_udp(
            &setup.server,
            &server_ip,
            &setup.router,
            &router_client_ip,
            ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT
        )
        .await,
        Err(ProbeError::RecvTimedOut)
    );

    // A filter dropping traffic forwarded from `router_server_if` to
    // `router_client_if` should have no effect.
    let control = setup
        .router
        .connect_to_protocol::<fnet_filter::ControlMarker>()
        .expect("connect to protocol");
    let mut controller = fnet_filter_ext::Controller::new(
        &control,
        &fnet_filter_ext::ControllerId("internal_forwarding_ingress".to_owned()),
    )
    .await
    .expect("create controller");
    install_forwarding_filter(
        &mut controller,
        &setup.router_server_iface,
        &setup.router_client_iface,
        "wrong_filter",
    )
    .await;
    probe_connectivity_with_udp(
        &setup.client,
        &client_ip,
        &setup.router,
        &router_server_ip,
        ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT,
    )
    .await
    .expect("probe should succeed");

    // A filter dropping traffic forwarded from `router_client_if` to
    // `router_server_if` should cause the router to drop the traffic.
    install_forwarding_filter(
        &mut controller,
        &setup.router_client_iface,
        &setup.router_server_iface,
        "right_filter",
    )
    .await;
    assert_matches!(
        probe_connectivity_with_udp(
            &setup.client,
            &client_ip,
            &setup.router,
            &router_server_ip,
            ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT
        )
        .await,
        Err(ProbeError::RecvTimedOut)
    );
}

/// Verify the Weak Host "internal forwarding" behavior for traffic egressing
/// the netstack.
#[test_case(SetupConfig::ipv4(ForwardingConfig::ServerSideEnabled); "ipv4")]
#[test_case(SetupConfig::ipv6(ForwardingConfig::ServerSideEnabled); "ipv6")]
#[fuchsia::test]
async fn internal_forwarding_egress(setup_config: SetupConfig) {
    let client_ip = setup_config.client_subnet;
    let server_ip = setup_config.server_subnet;
    let router_client_ip = setup_config.router_client_ip;
    let router_server_ip = setup_config.router_server_ip;

    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    // NB: This test relies on fuchsia.net.filter, which is only implemented
    // for Netstack3.
    let setup = setup_config.build::<Netstack3>("internal_forwarding_egress", &sandbox).await;

    // The router should be able to send traffic from its server side IP to the
    // client, but the router should not be able to send traffic from its
    // client side IP to the server. This is because only the server side
    // interface has forwarding enabled.
    probe_connectivity_with_udp(
        &setup.router,
        &router_server_ip,
        &setup.client,
        &client_ip,
        ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT,
    )
    .await
    .expect("probe should succeed");
    assert_matches!(
        probe_connectivity_with_udp(
            &setup.router,
            &router_client_ip,
            &setup.server,
            &server_ip,
            ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT,
        )
        .await,
        Err(ProbeError::SendFailed { _err: _ })
    );

    // A filter dropping traffic forwarded from `router_client_if` to
    // `router_server_if` should have no effect.
    let control = setup
        .router
        .connect_to_protocol::<fnet_filter::ControlMarker>()
        .expect("connect to protocol");
    let mut controller = fnet_filter_ext::Controller::new(
        &control,
        &fnet_filter_ext::ControllerId("internal_forwarding_egress".to_owned()),
    )
    .await
    .expect("create controller");
    install_forwarding_filter(
        &mut controller,
        &setup.router_client_iface,
        &setup.router_server_iface,
        "wrong_filter",
    )
    .await;
    probe_connectivity_with_udp(
        &setup.router,
        &router_server_ip,
        &setup.client,
        &client_ip,
        ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT,
    )
    .await
    .expect("probe should succeed");

    // A filter dropping traffic forwarded from `router_server_if` to
    // `router_client_if` should cause the router to drop the traffic.
    install_forwarding_filter(
        &mut controller,
        &setup.router_server_iface,
        &setup.router_client_iface,
        "right_filter",
    )
    .await;
    assert_matches!(
        probe_connectivity_with_udp(
            &setup.router,
            &router_server_ip,
            &setup.client,
            &client_ip,
            ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT,
        )
        .await,
        Err(ProbeError::RecvTimedOut)
    );
}

async fn create_icmpv6_raw_socket(
    realm: &netemul::TestRealm<'_>,
) -> fuchsia_async::net::DatagramSocket {
    let recv_socket = realm
        .raw_socket(
            fposix_socket::Domain::Ipv6,
            fposix_socket_raw::ProtocolAssociation::Associated(Ipv6Proto::Icmpv6.into()),
        )
        .await
        .expect("create raw socket");
    fuchsia_async::net::DatagramSocket::new_from_socket(recv_socket)
        .expect("create async datagram socket")
}

async fn recv_icmpv6_packet_too_big(
    recv_socket: &fuchsia_async::net::DatagramSocket,
    router_ip: net_types::ip::Ipv6Addr,
    client_ip: net_types::ip::Ipv6Addr,
) -> u32 {
    let mut buf = [0u8; 2048];
    async {
        loop {
            let (read, _from) = recv_socket.recv_from(&mut buf).await.expect("recv_from");
            let mut bv = &buf[..read];
            let parse_args = IcmpParseArgs::new(router_ip, client_ip);
            if let Ok(icmp_packet) = Icmpv6Packet::parse(&mut bv, parse_args) {
                if let Icmpv6Packet::PacketTooBig(packet_too_big) = icmp_packet {
                    return packet_too_big.message().mtu();
                }
            }
        }
    }
    .on_timeout(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT.after_now(), || {
        panic!("timed out waiting for PacketTooBig ICMPv6 error");
    })
    .await
}

/// The Netstack should generate an ICMPv6 PacketTooBig error when asked to
/// forward a IPv6 packet that would exceed the egress interface's MTU.
#[netstack_test]
async fn forwarding_packet_too_big(name: &str) {
    const INGRESS_MTU: u16 = 1500;
    const EGRESS_MTU: u16 = 1400;

    let setup_config = SetupConfig {
        router_client_ep_config: netemul::new_endpoint_config(INGRESS_MTU, None),
        router_server_ep_config: netemul::new_endpoint_config(EGRESS_MTU, None),
        ..SetupConfig::ipv6(ForwardingConfig::BothEnabled)
    };

    let client_sockaddr = std::net::SocketAddr::from((
        fidl_fuchsia_net_ext::IpAddress::from(setup_config.client_subnet.addr).0,
        PORT,
    ));
    let server_sockaddr = std::net::SocketAddr::from((
        fidl_fuchsia_net_ext::IpAddress::from(setup_config.server_subnet.addr).0,
        PORT,
    ));

    let client_ipv6 = match setup_config.client_subnet.addr {
        fnet::IpAddress::Ipv6(addr) => net_types::ip::Ipv6Addr::from_bytes(addr.addr),
        fnet::IpAddress::Ipv4(_) => unreachable!(),
    };
    let router_client_ipv6 = match setup_config.router_client_ip.addr {
        fnet::IpAddress::Ipv6(addr) => net_types::ip::Ipv6Addr::from_bytes(addr.addr),
        fnet::IpAddress::Ipv4(_) => unreachable!(),
    };

    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let setup = setup_config.build::<Netstack3>(name, &sandbox).await;

    let recv_socket = create_icmpv6_raw_socket(&setup.client).await;
    let send_socket = fuchsia_async::net::UdpSocket::bind_in_realm(&setup.client, client_sockaddr)
        .await
        .expect("bind send sock");

    // Construct a UDP payload such that the total IPv6 packet length equals
    // INGRESS_MTU, which exceeds the router's egress interface MTU, EGRESS_MTU.
    const UDP_PAYLOAD_LEN: usize =
        (INGRESS_MTU as usize) - ETHERNET_HDR_LEN_NO_TAG - IPV6_FIXED_HDR_LEN - UDP_HDR_LEN;
    let payload = [0u8; UDP_PAYLOAD_LEN];

    let sent = send_socket.send_to(&payload, server_sockaddr.into()).await.expect("send_to failed");
    assert_eq!(sent, payload.len());

    let mtu = recv_icmpv6_packet_too_big(&recv_socket, router_client_ipv6, client_ipv6).await;
    assert_eq!(mtu as usize, EGRESS_MTU as usize - ETHERNET_HDR_LEN_NO_TAG);
}

// Verify that UDP datagrams requiring fragmentation can be forwarded successfully.
#[netstack_test]
#[variant(N, Netstack)]
#[test_case(SetupConfig::ipv4(ForwardingConfig::BothEnabled); "ipv4")]
#[test_case(SetupConfig::ipv6(ForwardingConfig::BothEnabled); "ipv6")]
async fn forwarding_fragmented_udp_datagram<N: Netstack>(name: &str, setup_config: SetupConfig) {
    let client_sockaddr = std::net::SocketAddr::from((
        fidl_fuchsia_net_ext::IpAddress::from(setup_config.client_subnet.addr).0,
        PORT,
    ));
    let server_sockaddr = std::net::SocketAddr::from((
        fidl_fuchsia_net_ext::IpAddress::from(setup_config.server_subnet.addr).0,
        PORT,
    ));

    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let setup = setup_config.build::<N>(name, &sandbox).await;

    let send_socket = fuchsia_async::net::UdpSocket::bind_in_realm(&setup.client, client_sockaddr)
        .await
        .expect("bind client socket");
    let recv_socket = fuchsia_async::net::UdpSocket::bind_in_realm(&setup.server, server_sockaddr)
        .await
        .expect("bind server socket");

    // Generate a datagram that's large enough to require IP fragmentation
    // (will exceed MTU once the ETH + IP + UDP headers are applied).
    let payload: Vec<u8> = (0..netemul::DEFAULT_MTU).map(|i| i as u8).collect();
    let sent = send_socket.send_to(&payload, server_sockaddr).await.expect("send_to failed");
    assert_eq!(sent, payload.len());

    let mut buf = vec![0u8; 2048];
    let (read, from) = recv_socket
        .recv_from(&mut buf)
        .on_timeout(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT.after_now(), || {
            panic!("timed out waiting for fragmented UDP datagram on server");
        })
        .await
        .expect("recv_from failed");

    assert_eq!(read, payload.len());
    assert_eq!(&buf[..read], &payload[..]);
    assert_eq!(from, client_sockaddr);
}

async fn create_packet_socket<I: Ip>(
    realm: &netemul::TestRealm<'_>,
    iface_id: u64,
) -> fuchsia_async::net::DatagramSocket {
    let sock = realm
        .packet_socket(fposix_socket_packet::Kind::Network)
        .await
        .expect("create packet socket");
    let protocol = match I::VERSION {
        IpVersion::V4 => packet_formats::ethernet::EtherType::Ipv4,
        IpVersion::V6 => packet_formats::ethernet::EtherType::Ipv6,
    };
    let bind_addr = libc::sockaddr_ll::from(EthernetSockaddr {
        interface_id: NonZeroU64::new(iface_id),
        addr: net_types::ethernet::Mac::UNSPECIFIED,
        protocol,
    })
    .into_sockaddr();
    sock.bind(&bind_addr).expect("bind packet socket");
    fuchsia_async::net::DatagramSocket::new_from_socket(sock).expect("create async datagram socket")
}

// Verify that if the netstack receives IPv4 fragments that are too large for
// the egress interface, it will refragment and forward.
#[netstack_test]
async fn ipv4_will_refragment(name: &str) {
    const INGRESS_MTU: u16 = 1500;
    const EGRESS_MTU: u16 = 1400;
    let setup_config = SetupConfig {
        router_client_ep_config: netemul::new_endpoint_config(INGRESS_MTU, None),
        router_server_ep_config: netemul::new_endpoint_config(EGRESS_MTU, None),
        ..SetupConfig::ipv4(ForwardingConfig::BothEnabled)
    };

    let client_ipv4 = match setup_config.client_subnet.addr {
        fnet::IpAddress::Ipv4(addr) => net_types::ip::Ipv4Addr::new(addr.addr),
        fnet::IpAddress::Ipv6(_) => unreachable!(),
    };
    let server_ipv4 = match setup_config.server_subnet.addr {
        fnet::IpAddress::Ipv4(addr) => net_types::ip::Ipv4Addr::new(addr.addr),
        fnet::IpAddress::Ipv6(_) => unreachable!(),
    };

    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let setup = setup_config.build::<Netstack3>(name, &sandbox).await;

    let router_client_mac = setup.router_client_iface.mac().await;
    let client_send_to_addr = libc::sockaddr_ll::from(EthernetSockaddr {
        interface_id: Some(NonZeroU64::new(setup.client_iface.id()).unwrap()),
        addr: net_types::ethernet::Mac::new(router_client_mac.octets),
        protocol: packet_formats::ethernet::EtherType::Ipv4,
    })
    .into_sockaddr();

    // Client will send IP fragments on a packet socket. Server will receive
    // IP fragments on a packet socket.
    let client_send_sock =
        create_packet_socket::<Ipv4>(&setup.client, setup.client_iface.id()).await;
    let server_recv_sock =
        create_packet_socket::<Ipv4>(&setup.server, setup.server_iface.id()).await;

    // Setup and send two IPv4 fragments.
    const TTL: u8 = 64;
    const PROTOCOL: Ipv4Proto = Ipv4Proto::Proto(IpProto::Reserved);
    const ID: u16 = 12345;
    // NB: Set the fragment size such that, after applying the IPv4 & Ethernet
    // headers, the packet size is < INGRESS MTU, but greater than EGRESS_MTU.
    const FRAGMENT_SIZE: u16 = EGRESS_MTU;
    let mut fragment1 = Ipv4PacketBuilder::new(client_ipv4, server_ipv4, TTL, PROTOCOL);
    fragment1.id(ID);
    fragment1.mf_flag(true);
    fragment1.fragment_offset(FragmentOffset::ZERO);
    let mut fragment2 = Ipv4PacketBuilder::new(client_ipv4, server_ipv4, TTL, PROTOCOL);
    fragment2.id(ID);
    fragment2.mf_flag(false);
    fragment2.fragment_offset(
        FragmentOffset::new_with_bytes(FRAGMENT_SIZE)
            .expect("FRAGMENT_SIZE should be a multiple of 8"),
    );
    let fragment_body: Vec<u8> = (0..FRAGMENT_SIZE).map(|i| i as u8).collect();
    let fragment1 = fragment1
        .wrap_body((&fragment_body[..]).into_serializer())
        .serialize_vec_outer(&mut NoOpSerializationContext)
        .expect("serialization should succeed")
        .unwrap_b()
        .into_inner();
    let fragment2 = fragment2
        .wrap_body((&fragment_body[..]).into_serializer())
        .serialize_vec_outer(&mut NoOpSerializationContext)
        .expect("serialization should succeed")
        .unwrap_b()
        .into_inner();
    assert_eq!(fragment1.len(), fragment2.len());
    let len = fragment1.len();
    assert!(len < usize::from(INGRESS_MTU) && len > usize::from(EGRESS_MTU));
    assert_matches!(
        client_send_sock.send_to(&fragment1, client_send_to_addr.clone()).await,
        Ok(l) if l == len
    );
    assert_matches!(
        client_send_sock.send_to(&fragment2, client_send_to_addr.clone()).await,
        Ok(l) if l == len
    );

    // The server should receive the fragments. They should be resized to fit
    // EGRESS_MTU.
    let mut expected_body = Vec::with_capacity(usize::from(FRAGMENT_SIZE) * 2);
    expected_body.extend_from_slice(&fragment_body);
    expected_body.extend_from_slice(&fragment_body);

    let reassembled_body = async {
        let mut reassembled_body = vec![0u8; expected_body.len()];
        let mut received_bytes = 0;
        let mut seen_last_fragment = false;
        let mut buf = [0u8; 2048];

        while !seen_last_fragment || received_bytes < expected_body.len() {
            let (len, _from) =
                server_recv_sock.recv_from(&mut buf[..]).await.expect("recv_from shouldn't fail");
            let mut data = &buf[..len];
            let packet = Ipv4Packet::parse(&mut data, ()).expect("failed to parse IPv4 packet");
            let offset = usize::from(packet.fragment_offset().into_bytes());
            let body = packet.body();
            let end = offset + body.len();

            assert_eq!(packet.src_ip(), client_ipv4);
            assert_eq!(packet.dst_ip(), server_ipv4);
            assert_eq!(packet.proto(), PROTOCOL);
            assert_eq!(packet.ttl(), TTL - 1);
            assert!(len <= usize::from(EGRESS_MTU));
            assert!(end <= expected_body.len());

            reassembled_body[offset..end].copy_from_slice(body);
            received_bytes += body.len();

            if !packet.mf_flag() && end == expected_body.len() {
                seen_last_fragment = true;
            }
        }

        reassembled_body
    }
    .on_timeout(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT.after_now(), || {
        panic!("timed out waiting for IPv4 fragments on server");
    })
    .await;
    assert_eq!(reassembled_body, expected_body);
}

// Verify that if the netstack receives IPv6 fragments that are too large for
// the egress interface, it will not refragment and will instead respond with
// an ICMPv6 PacketTooBig error.
#[netstack_test]
async fn ipv6_will_not_refragment(name: &str) {
    const INGRESS_MTU: u16 = 1500;
    const EGRESS_MTU: u16 = 1400;
    let setup_config = SetupConfig {
        router_client_ep_config: netemul::new_endpoint_config(INGRESS_MTU, None),
        router_server_ep_config: netemul::new_endpoint_config(EGRESS_MTU, None),
        ..SetupConfig::ipv6(ForwardingConfig::BothEnabled)
    };

    let client_ipv6 = match setup_config.client_subnet.addr {
        fnet::IpAddress::Ipv6(addr) => net_types::ip::Ipv6Addr::from_bytes(addr.addr),
        fnet::IpAddress::Ipv4(_) => unreachable!(),
    };
    let server_ipv6 = match setup_config.server_subnet.addr {
        fnet::IpAddress::Ipv6(addr) => net_types::ip::Ipv6Addr::from_bytes(addr.addr),
        fnet::IpAddress::Ipv4(_) => unreachable!(),
    };
    let router_client_ipv6 = match setup_config.router_client_ip.addr {
        fnet::IpAddress::Ipv6(addr) => net_types::ip::Ipv6Addr::from_bytes(addr.addr),
        fnet::IpAddress::Ipv4(_) => unreachable!(),
    };

    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let setup = setup_config.build::<Netstack3>(name, &sandbox).await;

    let router_client_mac = setup.router_client_iface.mac().await;
    let client_send_to_addr = libc::sockaddr_ll::from(EthernetSockaddr {
        interface_id: Some(NonZeroU64::new(setup.client_iface.id()).unwrap()),
        addr: net_types::ethernet::Mac::new(router_client_mac.octets),
        protocol: packet_formats::ethernet::EtherType::Ipv6,
    })
    .into_sockaddr();

    // Client will send IP fragments on a packet socket, and receive
    // PacketTooBig on an raw socket.
    let client_send_sock =
        create_packet_socket::<Ipv6>(&setup.client, setup.client_iface.id()).await;
    let server_recv_sock =
        create_packet_socket::<Ipv6>(&setup.server, setup.server_iface.id()).await;
    let client_recv_sock = create_icmpv6_raw_socket(&setup.client).await;

    // Setup and send two IPv6 fragments.
    const HOP_LIMIT: u8 = 64;
    const PROTOCOL: Ipv6Proto = Ipv6Proto::NoNextHeader;
    const ID: u32 = 12345;
    // NB: Set the fragment size such that, after applying the IPv6 & Ethernet
    // headers, the packet size is < INGRESS MTU, but greater than EGRESS_MTU.
    const FRAGMENT_SIZE: u16 = EGRESS_MTU;
    let builder = Ipv6PacketBuilder::new(client_ipv6, server_ipv6, HOP_LIMIT, PROTOCOL);
    let fragment1 =
        Ipv6PacketBuilderWithFragmentHeader::new(builder.clone(), FragmentOffset::ZERO, true, ID);
    let offset = FragmentOffset::new_with_bytes(FRAGMENT_SIZE)
        .expect("FRAGMENT_SIZE should be a multiple of 8");
    let fragment2 = Ipv6PacketBuilderWithFragmentHeader::new(builder, offset, false, ID);
    let fragment_body: Vec<u8> = (0..FRAGMENT_SIZE).map(|i| i as u8).collect();
    let fragment1 = fragment1
        .wrap_body((&fragment_body[..]).into_serializer())
        .serialize_vec_outer(&mut NoOpSerializationContext)
        .expect("serialization should succeed")
        .unwrap_b()
        .into_inner();
    let fragment2 = fragment2
        .wrap_body((&fragment_body[..]).into_serializer())
        .serialize_vec_outer(&mut NoOpSerializationContext)
        .expect("serialization should succeed")
        .unwrap_b()
        .into_inner();
    assert_eq!(fragment1.len(), fragment2.len());
    let len = fragment1.len();
    assert!(len < usize::from(INGRESS_MTU) && len > usize::from(EGRESS_MTU));
    assert_matches!(
        client_send_sock.send_to(&fragment1, client_send_to_addr.clone()).await,
        Ok(l) if l == len
    );
    assert_matches!(
        client_send_sock.send_to(&fragment2, client_send_to_addr.clone()).await,
        Ok(l) if l == len
    );

    // The server should not receive the fragments, as they exceed `EGRESS_MTU`.
    let received_fragment = futures_util::stream::unfold(vec![0u8; 2048], |mut buf| async {
        let (_len, _from) =
            server_recv_sock.recv_from(&mut buf[..]).await.expect("recv_from shouldn't fail");
        let mut data = &buf[..len];
        let matches_fragment = match Ipv6Packet::parse(&mut data, ()) {
            Err(_) => false,
            Ok(packet) => packet
                .iter_extension_hdrs()
                .any(|hdr| matches!(hdr, Ipv6ExtensionHeader::Fragment { .. })),
        };
        Some((matches_fragment, buf))
    })
    .any(|matches_fragment| futures_util::future::ready(matches_fragment))
    .on_timeout(ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT.after_now(), || false)
    .await;
    assert!(!received_fragment);

    // The client should receive a PacketTooBig error.
    let mtu = recv_icmpv6_packet_too_big(&client_recv_sock, router_client_ipv6, client_ipv6).await;
    assert_eq!(mtu as usize, EGRESS_MTU as usize - ETHERNET_HDR_LEN_NO_TAG);
}
