// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Tests for the NAT hooks.

use std::marker::PhantomData;
use std::num::{NonZeroU16, NonZeroU64};
use std::ops::RangeInclusive;

use assert_matches::assert_matches;
use fidl_fuchsia_net::MarkDomain;
use fidl_fuchsia_net_ext::{self as fnet_ext, IntoExt as _};
use fidl_fuchsia_net_filter_ext::{
    Action, Change, CommitError, MarkAction, Matchers, NatHook, PortRange, Resource, Rule, RuleId,
};
use fidl_fuchsia_net_matchers_ext as fnet_matchers_ext;
use fidl_fuchsia_net_routes as fnet_routes;
use fuchsia_async as fasync;
use futures::{FutureExt as _, StreamExt as _};
use heck::ToSnakeCase as _;
use net_declare::fidl_mac;
use net_types::Witness as _;
use net_types::ip::{IpAddress as _, Ipv4, Ipv6};
use netemul::RealmTcpStream;
use netstack_testing_macros::netstack_test;
use packet::{NoOpSerializationContext, ParsablePacket, Serializer};
use packet_formats::ethernet::{
    ETHERNET_MIN_BODY_LEN_NO_TAG, EtherType, EthernetFrame, EthernetFrameBuilder,
    EthernetFrameLengthCheck,
};
use packet_formats::icmp::{
    IcmpDestUnreachable, IcmpMessage, IcmpPacketBuilder, IcmpTimeExceeded,
    Icmpv4DestUnreachableCode, Icmpv4ParameterProblem, Icmpv4ParameterProblemCode,
    Icmpv4TimeExceededCode, Icmpv6DestUnreachableCode, Icmpv6ParameterProblem,
    Icmpv6ParameterProblemCode, Icmpv6TimeExceededCode,
};
use packet_formats::ip::{IpPacketBuilder, IpProto, Ipv4Proto, Ipv6Proto};
use test_case::test_case;

use crate::ip_hooks::{
    Addrs, BoundSockets, ExpectedConnectivity, IcmpSocket, LOW_RULE_PRIORITY, MEDIUM_RULE_PRIORITY,
    OriginalDestination, Ports, Realms, RouterTestIpExt, SockAddrs, SocketType, TcpSocket,
    TestIpExt, TestNet, TestRealm, TestRouterNet, UdpSocket,
};

fn local_type_name<T>() -> &'static str {
    std::any::type_name::<T>().split("::").last().unwrap()
}

#[netstack_test]
#[variant(I, Ip)]
#[test_case(PhantomData::<TcpSocket>; "tcp")]
#[test_case(PhantomData::<UdpSocket>; "udp")]
#[test_case(PhantomData::<IcmpSocket>; "icmp")]
async fn redirect_ingress_no_assigned_address<I: TestIpExt, S: SocketType>(
    name: &str,
    _socket_type: PhantomData<S>,
) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let network = sandbox.create_network("net").await.expect("create network");
    let name = format!("{name}_{}", local_type_name::<S>().to_snake_case());

    let mut net = TestNet::new::<I>(
        &sandbox,
        &network,
        &name,
        None, /* ip_hook */
        Some(NatHook::Ingress),
    )
    .await;

    // Send from the client to server and back; assert that we have two-way
    // connectivity when no filtering has been configured.
    //
    // This has the side effect of completing neighbor resolution on the client
    // for the server, so that we can remove the address assigned to the server
    // and still expect traffic from the client to reach it.
    let _handles = net.run_test::<I, S>(ExpectedConnectivity::TwoWay).await;

    // Remove the server's assigned address. Even though we have a Redirect NAT
    // rule installed, the traffic from the client should not reach the server
    // because Redirect drops the packet when there is no address assigned to
    // the incoming interface.
    net.server
        .install_nat_rule::<I>(
            LOW_RULE_PRIORITY,
            S::matcher::<I>(),
            Action::Redirect { dst_port: None },
        )
        .await;
    let ((), _handles) = net
        .run_test_with::<I, S, _, _>(
            ExpectedConnectivity::None,
            |TestNet { client: _, server }, _addrs, ()| {
                Box::pin(async move {
                    let removed = server
                        .interface
                        .del_address_and_subnet_route(I::SERVER_ADDR_WITH_PREFIX)
                        .await
                        .expect("remove address");
                    assert!(removed);
                })
            },
        )
        .await;
}

#[netstack_test]
#[variant(I, Ip)]
#[test_case(PhantomData::<TcpSocket>; "tcp")]
#[test_case(PhantomData::<UdpSocket>; "udp")]
#[test_case(PhantomData::<IcmpSocket>; "icmp")]
async fn masquerade_egress_no_assigned_address<I: TestIpExt, S: SocketType>(
    name: &str,
    _socket_type: PhantomData<S>,
) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let network = sandbox.create_network("net").await.expect("create network");
    let name = format!("{name}_{}", local_type_name::<S>().to_snake_case());

    let mut net = TestNet::new::<I>(
        &sandbox,
        &network,
        &name,
        None, /* ip_hook */
        Some(NatHook::Egress),
    )
    .await;

    // Send from the client to server and back; assert that we have two-way
    // connectivity when no filtering has been configured.
    //
    // This has the side effect of completing neighbor resolution on the client
    // for the server, so that we can remove the address assigned to the server
    // and still expect traffic from the client to reach it.
    let _handles = net.run_test::<I, S>(ExpectedConnectivity::TwoWay).await;

    // Remove the server's assigned address. Even though we have a Masquerade NAT
    // rule installed, the response traffic from the server should not reach the
    // client because Masquerade drops the outgoing packet when there is no address
    // assigned to the outgoing interface.
    net.server
        .install_nat_rule::<I>(
            LOW_RULE_PRIORITY,
            S::matcher::<I>(),
            Action::Masquerade { src_port: None },
        )
        .await;
    let ((), _handles) = net
        .run_test_with::<I, S, _, _>(
            ExpectedConnectivity::None,
            |TestNet { client: _, server }, _addrs, ()| {
                Box::pin(async move {
                    let removed = server
                        .interface
                        .del_address_and_subnet_route(I::SERVER_ADDR_WITH_PREFIX)
                        .await
                        .expect("remove address");
                    assert!(removed);
                })
            },
        )
        .await;
}

#[netstack_test]
#[variant(I, Ip)]
async fn masquerade_remove_and_add_address<I: RouterTestIpExt>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");

    // Set up a network with two hosts (client and server) and a router. The client
    // and server are both link-layer neighbors with the router but on isolated L2
    // networks.
    let mut net =
        TestRouterNet::<I>::new(&sandbox, &name, None /* ip_hook */, Some(NatHook::Egress)).await;

    // Install a rule on the egress hook of the router that masquerades outgoing
    // traffic behind its IP address.
    net.install_nat_rule(
        Matchers {
            out_interface: Some(fnet_matchers_ext::Interface::Id(
                NonZeroU64::new(net.router_server_interface.id()).unwrap(),
            )),
            ..Default::default()
        },
        Action::Masquerade { src_port: None },
    )
    .await;

    // The traffic should look to the server like it is originating from the router,
    // and the NATing should be transparent to the client.
    let (BoundSockets { client, mut server }, sock_addrs) =
        UdpSocket::bind_sockets(net.realms(), TestRouterNet::<I>::addrs()).await;
    let fnet_ext::IpAddress(router_addr) = I::ROUTER_SERVER_ADDR_WITH_PREFIX.addr.into();
    let sock_addrs = SockAddrs {
        client: std::net::SocketAddr::new(router_addr, sock_addrs.client.port()),
        server: sock_addrs.server,
    };
    let mut client_and_server =
        UdpSocket::connect(client, &mut server, sock_addrs, ExpectedConnectivity::TwoWay, None)
            .await
            .into_connected()
            .expect("connect client to server");
    UdpSocket::send_and_recv::<I>(
        net.realms(),
        &mut client_and_server,
        sock_addrs,
        ExpectedConnectivity::TwoWay,
    )
    .await;

    // Remove the router's assigned address (but *not* its subnet route -- we want
    // the traffic to still be routable but to be dropped by the filtering engine
    // when it attempts to masquerade and sees that the address it was using is no
    // longer valid).
    //
    // The traffic from the client to the server should now be dropped by the router
    // because there is no address that can be used to masquerade the forwarded
    // traffic.
    let removed = net
        .router_server_interface
        .del_address_and_subnet_route(I::ROUTER_SERVER_ADDR_WITH_PREFIX)
        .await
        .expect("remove address");
    assert!(removed);
    let added = net
        .router_server_interface
        .add_route_either(
            fnet_ext::apply_subnet_mask(I::ROUTER_SERVER_ADDR_WITH_PREFIX),
            None, /* gateway */
            fnet_routes::SpecifiedMetric::InheritedFromInterface(fnet_routes::Empty),
        )
        .await
        .expect("re-add subnet route");
    assert!(added);
    UdpSocket::send_and_recv::<I>(
        net.realms(),
        &mut client_and_server,
        sock_addrs,
        ExpectedConnectivity::None,
    )
    .await;

    // But if we re-add the address, the masqueraded traffic should observe that the
    // original address used to masquerade the flow was re-acquired and successfully
    // resume masquerading.
    //
    // NB: we only exercise UDP here because TCP implements reliable delivery, so
    // the client will attempt to retransmit the message that was dropped by NAT
    // when the address was removed. It's easier to test this behavior if a packet
    // being dropped actually means it will never reach its destination.
    net.router_server_interface
        .add_address_and_subnet_route(I::ROUTER_SERVER_ADDR_WITH_PREFIX)
        .await
        .expect("re-add address");
    UdpSocket::send_and_recv::<I>(
        net.realms(),
        &mut client_and_server,
        sock_addrs,
        ExpectedConnectivity::TwoWay,
    )
    .await;
}

fn different_ephemeral_port(port: u16) -> u16 {
    // Grab an arbitrary different port by incrementing the current one.
    let new_port = port.wrapping_add(1);

    // Now map that new port into the ephemeral range.
    const EPHEMERAL_RANGE: RangeInclusive<u16> = 49152..=65535;
    let len = u16::try_from(EPHEMERAL_RANGE.len()).unwrap();
    EPHEMERAL_RANGE.start() + (new_port % len)
}

#[netstack_test]
#[variant(I, Ip)]
#[test_case(PhantomData::<TcpSocket>, false; "tcp")]
#[test_case(PhantomData::<UdpSocket>, false; "udp")]
#[test_case(PhantomData::<TcpSocket>, true; "tcp to local port")]
#[test_case(PhantomData::<UdpSocket>, true; "udp to local port")]
// NB: ICMP does not allow selecting the redirect port range.
#[test_case(PhantomData::<IcmpSocket>, false; "icmp")]
async fn redirect_ingress<I: TestIpExt, S: SocketType>(
    name: &str,
    _socket_type: PhantomData<S>,
    change_dst_port: bool,
) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let network = sandbox.create_network("net").await.expect("create network");
    let name = format!("{name}_{}", local_type_name::<S>().to_snake_case());

    let mut net = TestNet::new::<I>(
        &sandbox,
        &network,
        &name,
        None, /* ip_hook */
        Some(NatHook::Ingress),
    )
    .await;

    // Install a rule to redirect incoming traffic to the primary address of the
    // incoming interface. This should have no effect on connectivity because
    // the incoming traffic is already destined to this address.
    net.server
        .install_nat_rule::<I>(
            LOW_RULE_PRIORITY,
            S::matcher::<I>(),
            Action::Redirect { dst_port: None },
        )
        .await;
    let _handles = net.run_test::<I, S>(ExpectedConnectivity::TwoWay).await;

    // Now run a similar test, but instead of sending from the client socket to
    // the address the server socket is bound to, send to some other address
    // that is not assigned to the server, and optionally send to a different
    // port than the server's socket is bound to as well.
    //
    // This traffic should be redirected to the server's assigned address (and
    // optionally the local port) and received on the server socket, and the
    // traffic should be NATed such that this is transparent to the client.
    let (sockets, sock_addrs) = S::bind_sockets(net.realms(), net.addrs()).await;

    // Replace the existing subnet route on the client's interface with a
    // default route through the server to ensure the traffic will be routed
    // there, even though we are sending to a different address.
    net.client
        .interface
        .del_subnet_route(I::CLIENT_ADDR_WITH_PREFIX)
        .await
        .expect("remove subnet route");
    net.client
        .interface
        .add_default_route(I::SERVER_ADDR_WITH_PREFIX.addr)
        .await
        .expect("add default route through server");

    let original_dst = {
        let fnet_ext::IpAddress(addr) = I::OTHER_ADDR_WITH_PREFIX.addr.into();
        let port = if change_dst_port {
            different_ephemeral_port(sock_addrs.server.port())
        } else {
            sock_addrs.server.port()
        };
        std::net::SocketAddr::new(addr, port)
    };
    let dst_port = change_dst_port.then(|| {
        let server_port = NonZeroU16::new(sock_addrs.server.port()).unwrap();
        PortRange(server_port..=server_port)
    });
    net.server
        .install_nat_rule::<I>(
            MEDIUM_RULE_PRIORITY,
            S::matcher::<I>(),
            Action::Redirect { dst_port },
        )
        .await;

    let _handles = S::run_test::<I>(
        net.realms(),
        sockets,
        SockAddrs { client: sock_addrs.client, server: original_dst },
        ExpectedConnectivity::TwoWay,
        Some(OriginalDestination {
            dst: original_dst,
            // The NAT redirection is done on the server netstack, so the client
            // is unable to observe the original destination of the connection.
            known_to_client: false,
            known_to_server: true,
        }),
    )
    .await;
}

#[netstack_test]
#[variant(I, Ip)]
#[test_case(PhantomData::<TcpSocket>, false; "tcp")]
#[test_case(PhantomData::<UdpSocket>, false; "udp")]
#[test_case(PhantomData::<TcpSocket>, true; "tcp to local port")]
#[test_case(PhantomData::<UdpSocket>, true; "udp to local port")]
// NB: ICMP does not allow selecting the redirect port range.
#[test_case(PhantomData::<IcmpSocket>, false; "icmp")]
async fn redirect_local_egress<I: TestIpExt, S: SocketType>(
    name: &str,
    _socket_type: PhantomData<S>,
    change_dst_port: bool,
) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let network = sandbox.create_network("net").await.expect("create network");
    let name = format!("{name}_{}", local_type_name::<S>().to_snake_case());

    let mut netstack = TestRealm::new::<I>(
        &sandbox,
        &network,
        None, /* ip_hook */
        Some(NatHook::LocalEgress),
        name,
        I::CLIENT_ADDR_WITH_PREFIX,
        I::OTHER_ADDR_WITH_PREFIX,
    )
    .await;

    // Create two sockets, a client socket bound to an address assigned on an
    // interface, and a server socket bound to the loopback address. Send from
    // the client to a different, routable address, and optionally to a
    // different port than the server socket is bound to.
    let (sockets, sock_addrs) = S::bind_sockets(
        Realms { client: &netstack.realm, server: &netstack.realm },
        Addrs {
            client: I::CLIENT_ADDR_WITH_PREFIX.addr,
            server: I::LOOPBACK_ADDRESS.get().to_ip_addr().into_ext(),
        },
    )
    .await;

    // Install a rule to redirect outgoing traffic to the loopback address (and
    // optional the server socket's local port).
    //
    // This traffic should be redirected to the loopback address and received by
    // the server, and the traffic should be NATed such that this is transparent
    // to the client.
    let original_dst = {
        let fnet_ext::IpAddress(addr) = I::OTHER_ADDR_WITH_PREFIX.addr.into();
        let port = if change_dst_port {
            different_ephemeral_port(sock_addrs.server.port())
        } else {
            sock_addrs.server.port()
        };
        std::net::SocketAddr::new(addr, port)
    };

    let dst_port = change_dst_port.then(|| {
        let server_port = NonZeroU16::new(sock_addrs.server.port()).unwrap();
        PortRange(server_port..=server_port)
    });
    netstack
        .install_nat_rule::<I>(LOW_RULE_PRIORITY, S::matcher::<I>(), Action::Redirect { dst_port })
        .await;

    let _handles = S::run_test::<I>(
        Realms { client: &netstack.realm, server: &netstack.realm },
        sockets,
        SockAddrs { client: sock_addrs.client, server: original_dst },
        ExpectedConnectivity::TwoWay,
        Some(OriginalDestination {
            dst: original_dst,
            // The client and server sockets are on the same netstack, so they
            // can both observe the original destination of the connection.
            known_to_client: true,
            known_to_server: true,
        }),
    )
    .await;
}

#[netstack_test]
#[variant(I, Ip)]
#[test_case(PhantomData::<TcpSocket>; "tcp")]
#[test_case(PhantomData::<UdpSocket>; "udp")]
#[test_case(PhantomData::<IcmpSocket>; "icmp")]
async fn masquerade<I: RouterTestIpExt, S: SocketType>(name: &str, _socket_type: PhantomData<S>) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let name = format!("{name}_{}", local_type_name::<S>().to_snake_case());

    // Set up a network with two hosts (client and server) and a router. The client
    // and server are both link-layer neighbors with the router but on isolated L2
    // networks.
    let mut net =
        TestRouterNet::<I>::new(&sandbox, &name, None /* ip_hook */, Some(NatHook::Egress)).await;

    // Install a rule on the egress hook of the router that masquerades outgoing
    // traffic behind its IP address.
    net.install_nat_rule(
        Matchers {
            out_interface: Some(fnet_matchers_ext::Interface::Id(
                NonZeroU64::new(net.router_server_interface.id()).unwrap(),
            )),
            ..Default::default()
        },
        Action::Masquerade { src_port: None },
    )
    .await;

    // The traffic should look to the server like it is originating from the router,
    // and the NATing should be transparent to the client.
    let (sockets, sock_addrs) = S::bind_sockets(net.realms(), TestRouterNet::<I>::addrs()).await;
    let fnet_ext::IpAddress(router_addr) = I::ROUTER_SERVER_ADDR_WITH_PREFIX.addr.into();
    let _handles = S::run_test::<I>(
        net.realms(),
        sockets,
        SockAddrs {
            client: std::net::SocketAddr::new(router_addr, sock_addrs.client.port()),
            server: sock_addrs.server,
        },
        ExpectedConnectivity::TwoWay,
        None,
    )
    .await;
}

#[netstack_test]
#[test_case(
    PhantomData::<Ipv4>, IcmpDestUnreachable::default(),
    Icmpv4DestUnreachableCode::DestNetworkUnreachable => libc::ENETUNREACH
)]
#[test_case(
    PhantomData::<Ipv4>, Icmpv4ParameterProblem::new(0),
    Icmpv4ParameterProblemCode::PointerIndicatesError => libc::EPROTO
)]
#[test_case(
    PhantomData::<Ipv4>, IcmpTimeExceeded::default(),
    Icmpv4TimeExceededCode::TtlExpired => libc::EHOSTUNREACH
)]
#[test_case(
    PhantomData::<Ipv6>, IcmpDestUnreachable::default(),
    Icmpv6DestUnreachableCode::NoRoute => libc::ENETUNREACH
)]
#[test_case(
    PhantomData::<Ipv6>, Icmpv6ParameterProblem::new(0),
    Icmpv6ParameterProblemCode::ErroneousHeaderField => libc::EPROTO
)]
#[test_case(
    PhantomData::<Ipv6>, IcmpTimeExceeded::default(),
    Icmpv6TimeExceededCode::HopLimitExceeded => libc::EHOSTUNREACH
)]
async fn masquerade_icmp_error<
    I: RouterTestIpExt + packet_formats::ip::IpExt,
    M: IcmpMessage<I> + std::fmt::Debug,
>(
    name: &str,
    _ip_version: PhantomData<I>,
    message: M,
    code: M::Code,
) -> i32 {
    use packet::NestableSerializer as _;
    use packet_formats::ip::IpPacket as _;

    let sandbox = netemul::TestSandbox::new().expect("create sandbox");

    // Set up a network with two hosts (client and server) and a router. The
    // client and server are both link-layer neighbors with the router but on
    // isolated L2 networks.
    let mut net =
        TestRouterNet::<I>::new(&sandbox, &name, None /* ip_hook */, Some(NatHook::Egress)).await;

    // By shutting down the server, we ensure the fake endpoint will be the only
    // thing in the server network that's able to receive or send traffic.
    net.server.shutdown().await.expect("shut down server realm");
    let fake_ep =
        net.router_server_net.create_fake_endpoint().expect("failed to create fake endpoint");
    let fake_ep = &fake_ep;

    // Add an arbitrary neighbor entry for the server address so the router will
    // route packets onto the server network for our fake endpoint to snoop on.
    net.router
        .add_neighbor_entry(
            net.router_server_interface.id(),
            I::SERVER_ADDR_WITH_PREFIX.addr,
            fidl_mac!("02:00:00:00:00:01"),
        )
        .await
        .expect("add_neighbor_entry");

    // Install a rule on the egress hook of the router that masquerades outgoing
    // traffic behind its IP address.
    net.install_nat_rule(
        Matchers {
            out_interface: Some(fnet_matchers_ext::Interface::Id(
                NonZeroU64::new(net.router_server_interface.id()).unwrap(),
            )),
            ..Default::default()
        },
        Action::Masquerade { src_port: None },
    )
    .await;

    let fake_ep_loop = async move {
        fake_ep
            .frame_stream()
            .map(|r| r.expect("failed to read frame"))
            .for_each(|(frame, dropped)| async move {
                assert_eq!(dropped, 0);

                let eth = EthernetFrame::parse(&mut &frame[..], EthernetFrameLengthCheck::NoCheck)
                    .expect("valid ethernet frame");
                let Ok(ip) = I::Packet::parse(&mut eth.body(), ()) else {
                    return;
                };
                if ip.proto() != IpProto::Tcp.into() {
                    return;
                }

                // Ensure that we're receiving a NATed packet, because otherwise
                // this test is meaningless.
                let src_ip: net_types::ip::IpAddr = ip.src_ip().into();
                let expected: net_types::ip::IpAddr =
                    I::ROUTER_SERVER_ADDR_WITH_PREFIX.addr.into_ext();
                assert_eq!(src_ip, expected);

                let icmp_error = packet::Buf::new(&mut eth.body().to_vec(), ..)
                    .wrap_in(IcmpPacketBuilder::<I, _>::new(
                        ip.dst_ip(),
                        ip.src_ip(),
                        code,
                        message,
                    ))
                    .wrap_in(I::PacketBuilder::new(
                        ip.dst_ip(),
                        ip.src_ip(),
                        u8::MAX,
                        I::map_ip_out((), |()| Ipv4Proto::Icmp, |()| Ipv6Proto::Icmpv6),
                    ))
                    .wrap_in(EthernetFrameBuilder::new(
                        eth.dst_mac(),
                        eth.src_mac(),
                        EtherType::from_ip_version(I::VERSION),
                        ETHERNET_MIN_BODY_LEN_NO_TAG,
                    ))
                    .serialize_vec_outer(&mut NoOpSerializationContext)
                    .expect("failed to serialize ICMP error")
                    .unwrap_b();
                fake_ep.write(icmp_error.as_ref()).await.expect("failed to write ICMP error");
            })
            .await;
    };

    let server_addr: net_types::ip::IpAddr = I::SERVER_ADDR_WITH_PREFIX.addr.into_ext();
    let server_addr = std::net::SocketAddr::new(server_addr.into(), 8080);

    let Realms { client, server: _ } = net.realms();

    let connect = async move {
        let error = fasync::net::TcpStream::connect_in_realm(client, server_addr)
            .await
            .expect_err("connect should fail");
        let error = error.downcast::<std::io::Error>().expect("failed to cast to std::io::Result");
        error.raw_os_error()
    };

    futures::select! {
        () = fake_ep_loop.fuse() => unreachable!("should never finish"),
        errno = connect.fuse() => return errno.expect("must have an errno"),
    }
}

#[netstack_test]
#[variant(I, Ip)]
#[test_case(PhantomData::<TcpSocket>, false; "tcp")]
#[test_case(PhantomData::<UdpSocket>, false; "udp")]
#[test_case(PhantomData::<UdpSocket>, true; "udp restrict to occupied port")]
#[test_case(PhantomData::<TcpSocket>, true; "tcp restrict to occupied port")]
// NB: Restricting the source port of ICMP sockets is not allowed, but we can
// still run the test to ensure two way connectivity is ensured.
#[test_case(PhantomData::<IcmpSocket>, false; "icmp")]
async fn masquerade_rewrite_src_port<I: RouterTestIpExt, S: SocketType>(
    name: &str,
    _socket_type: PhantomData<S>,
    rewrite_to_conflicting_port: bool,
) {
    if rewrite_to_conflicting_port {
        assert!(S::SUPPORTS_NAT_PORT_RANGE, "socket type doesn't support test set up");
    }

    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let name = format!("{name}_{}", local_type_name::<S>().to_snake_case());

    // Set up a network with two hosts (client and server) and a router. The client
    // and server are both link-layer neighbors with the router but on isolated L2
    // networks.
    let mut net =
        TestRouterNet::<I>::new(&sandbox, &name, None /* ip_hook */, Some(NatHook::Egress)).await;

    // Bind one socket on the router and one on the server and send some data back
    // and forth, so that the connection is tracked by the router netstack.
    let realms = Realms { client: &net.router, server: &net.server };
    let (BoundSockets { client, mut server }, sock_addrs) = S::bind_sockets(
        realms,
        Addrs {
            client: I::ROUTER_SERVER_ADDR_WITH_PREFIX.addr,
            server: I::SERVER_ADDR_WITH_PREFIX.addr,
        },
    )
    .await;
    let client_port = NonZeroU16::new(sock_addrs.client.port()).unwrap();
    let mut router_and_server =
        S::connect(client, &mut server, sock_addrs, ExpectedConnectivity::TwoWay, None)
            .await
            .into_connected()
            .expect("router can connect to server");
    let _handles = S::send_and_recv::<I>(
        realms,
        &mut router_and_server,
        sock_addrs,
        ExpectedConnectivity::TwoWay,
    )
    .await;

    // Install a rule on the egress hook of the router that masquerades outgoing
    // traffic behind its IP address, and restrict the range of ports within which
    // the source port can be rewritten.
    let masquerade_src_port = if rewrite_to_conflicting_port {
        // Restrict the source port rewrite range such that either the only allowed port
        // is the one that's already bound on the router, causing a conflict.
        client_port
    } else {
        // Specify the source port rewrite range such that the port should be available
        // on the router.
        NonZeroU16::new(different_ephemeral_port(client_port.get())).unwrap()
    };
    net.install_nat_rule(
        Matchers {
            out_interface: Some(fnet_matchers_ext::Interface::Id(
                NonZeroU64::new(net.router_server_interface.id()).unwrap(),
            )),
            ..S::matcher::<I>()
        },
        Action::Masquerade {
            src_port: S::SUPPORTS_NAT_PORT_RANGE
                .then_some(PortRange(masquerade_src_port..=masquerade_src_port)),
        },
    )
    .await;

    // Now bind a new socket on the client to the same port used by the router, and
    // send to the same server as before, so that the there is a conflict. The
    // router should attempt to rewrite the source port of the client's traffic.
    //
    // If the source port range of the Masquerade rule includes a port that does not
    // conflict with an existing tuple, the client's source port should be mapped to
    // an unoccupied one as part of the NAT configuration, and communication with
    // the server should succeed.
    //
    // If it was restricted to only the port that's already occupied, the traffic
    // from the client will be dropped because there is no viable option to remap
    // its source port while avoiding a conflict.
    let (BoundSockets { client: new_client, server: _ }, _sock_addrs) = S::bind_sockets_to_ports(
        net.realms(),
        TestRouterNet::<I>::addrs(),
        Ports { src: client_port.get(), dst: 0 /* we're not using the server socket */ },
    )
    .await;
    let fnet_ext::IpAddress(router_addr) = I::ROUTER_SERVER_ADDR_WITH_PREFIX.addr.into();
    let _handles = S::run_test::<I>(
        net.realms(),
        BoundSockets { client: new_client, server },
        SockAddrs {
            client: std::net::SocketAddr::new(router_addr, masquerade_src_port.get()),
            server: sock_addrs.server,
        },
        if rewrite_to_conflicting_port {
            ExpectedConnectivity::None
        } else {
            ExpectedConnectivity::TwoWay
        },
        None,
    )
    .await;
}

#[netstack_test]
#[variant(I, Ip)]
#[test_case(PhantomData::<TcpSocket>; "tcp")]
#[test_case(PhantomData::<UdpSocket>; "udp")]
#[test_case(PhantomData::<IcmpSocket>; "icmp")]
async fn implicit_snat_ports_of_locally_generated_traffic<I: RouterTestIpExt, S: SocketType>(
    name: &str,
    _socket_type: PhantomData<S>,
) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let name = format!("{name}_{}", local_type_name::<S>().to_snake_case());

    // Set up a network with two hosts (client and server) and a router. The client
    // and server are both link-layer neighbors with the router but on isolated L2
    // networks.
    //
    // Install a rule on the egress hook of the router that masquerades outgoing
    // traffic from the client behind its IP address.
    let mut net =
        TestRouterNet::<I>::new(&sandbox, &name, None /* ip_hook */, Some(NatHook::Egress)).await;
    net.install_nat_rule(
        Matchers {
            src_addr: Some(fnet_matchers_ext::Address {
                matcher: fnet_matchers_ext::AddressMatcherType::Subnet(
                    I::ROUTER_CLIENT_SUBNET.try_into().unwrap(),
                ),
                invert: false,
            }),
            out_interface: Some(fnet_matchers_ext::Interface::Id(
                NonZeroU64::new(net.router_server_interface.id()).unwrap(),
            )),
            ..S::matcher::<I>()
        },
        Action::Masquerade { src_port: None },
    )
    .await;

    // Send traffic from the client to the server and back. This has the side effect
    // that the router is tracking this connection.
    let (BoundSockets { client, mut server }, sock_addrs) =
        S::bind_sockets(net.realms(), TestRouterNet::<I>::addrs()).await;
    let client_port = NonZeroU16::new(sock_addrs.client.port()).unwrap();
    let fnet_ext::IpAddress(router_addr) = I::ROUTER_SERVER_ADDR_WITH_PREFIX.addr.into();
    let sock_addrs = SockAddrs {
        client: std::net::SocketAddr::new(router_addr, client_port.get()),
        server: sock_addrs.server,
    };
    let mut client_and_server =
        S::connect(client, &mut server, sock_addrs, ExpectedConnectivity::TwoWay, None)
            .await
            .into_connected()
            .expect("client can connect to server");
    S::send_and_recv::<I>(
        net.realms(),
        &mut client_and_server,
        sock_addrs,
        ExpectedConnectivity::TwoWay,
    )
    .await;

    // Bind a socket on the router to the *same* source port that was used by the
    // client and then allocated by the router for the above NATed connection.
    let realms = Realms { client: &net.router, server: &net.server };
    let (BoundSockets { client: new_client, server: _ }, _sock_addrs) = S::bind_sockets_to_ports(
        realms,
        Addrs {
            client: I::ROUTER_SERVER_ADDR_WITH_PREFIX.addr,
            server: I::SERVER_ADDR_WITH_PREFIX.addr,
        },
        Ports { src: client_port.get(), dst: 0 /* we're not using the server socket */ },
    )
    .await;

    // Send traffic from this socket on the router to the server and back. The
    // router should perform source port remapping for the locally-generated traffic
    // (even though it did not match any NAT rules) to ensure that it does not
    // conflict with any existing tracked connections.
    {
        let sock_addrs = SockAddrs {
            // Don't assert on the client port, because we expect it to be rewritten.
            client: std::net::SocketAddr::new(router_addr, 0),
            server: sock_addrs.server,
        };
        let mut router_and_server =
            S::connect(new_client, &mut server, sock_addrs, ExpectedConnectivity::TwoWay, None)
                .await
                .into_connected()
                .expect("client can connect to server");
        let _handles = S::send_and_recv::<I>(
            realms,
            &mut router_and_server,
            sock_addrs,
            ExpectedConnectivity::TwoWay,
        )
        .await;
    }

    // Make sure we can still send traffic from the original client address and port
    // to the server (and back) through the NAT gateway.
    S::send_and_recv::<I>(
        net.realms(),
        &mut client_and_server,
        sock_addrs,
        ExpectedConnectivity::TwoWay,
    )
    .await;
}

#[netstack_test]
#[variant(I, Ip)]
#[test_case(NatHook::Ingress)]
#[test_case(NatHook::LocalIngress)]
#[test_case(NatHook::LocalEgress)]
#[test_case(NatHook::Egress)]
async fn invalid_mark_action<I: TestIpExt>(name: &str, nat_hook: NatHook) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let network = sandbox.create_network("net").await.expect("create network");
    let name = format!("{name}_{nat_hook:?}");

    let mut net =
        TestNet::new::<I>(&sandbox, &network, &name, None /* ip_hook */, Some(nat_hook)).await;

    let id = RuleId {
        routine: net.server.nat_routine.clone().expect("NAT routine should be installed"),
        index: LOW_RULE_PRIORITY,
    };
    net.server
        .controller
        .push_changes(vec![Change::Create(Resource::Rule(Rule {
            id: id.clone(),
            matchers: Matchers::default(),
            action: Action::Mark {
                domain: MarkDomain::Mark1,
                action: MarkAction::SetMark { clearing_mask: 0, mark: 0 },
            },
        }))])
        .await
        .expect("push changes");

    assert_matches!(
        net.server.controller.commit().await,
        Err(CommitError::RuleWithInvalidAction(got)) => assert_eq!(got, id)
    );
}
