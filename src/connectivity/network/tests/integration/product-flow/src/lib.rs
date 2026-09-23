// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_dhcp as fnet_dhcp;
use fidl_fuchsia_net_ext as fnet_ext;
use fidl_fuchsia_net_ext::IntoExt as _;
use fidl_fuchsia_net_filter as fnet_filter;
use fidl_fuchsia_net_filter_ext::{
    Action, Change, Controller, ControllerId, Domain, InstalledIpRoutine, IpHook, MarkAction,
    Matchers, Namespace, NamespaceId, Resource, Routine, RoutineId, RoutineType, Rule, RuleId,
};
use fidl_fuchsia_net_interfaces as fnet_interfaces;
use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
use fidl_fuchsia_net_matchers_ext as fnet_matchers_ext;
use fidl_fuchsia_net_routes_admin as fnet_routes_admin;
use fidl_fuchsia_net_routes_ext as fnet_routes_ext;
use fidl_fuchsia_net_routes_ext::admin::FidlRouteAdminIpExt;
use fidl_fuchsia_net_routes_ext::rules::{
    FidlRuleAdminIpExt, FidlRuleIpExt, RuleAction, RuleIndex, RuleMatcher, RuleSetPriority,
};
use fidl_fuchsia_net_routes_ext::{FidlRouteIpExt, TableId};
use fidl_fuchsia_posix_socket as fposix_socket;
use fuchsia_async::{self as fasync};
use futures::{AsyncReadExt as _, AsyncWriteExt as _, FutureExt as _, StreamExt as _};
use net_declare::{fidl_subnet, std_ip};
use net_types::ip::IpVersion;

use assert_matches::assert_matches;
use netemul::{InStack, RealmTcpListener as _, RealmTcpStream as _, RealmUdpSocket as _};
use netstack_testing_common::constants::ipv6 as ipv6_consts;
use netstack_testing_common::realms::{
    KnownServiceProvider, Netstack, Netstack3, TestSandboxExt as _,
};
use netstack_testing_common::{dhcpv4, interfaces, ndp};
use netstack_testing_macros::netstack_test;
use packet_formats::icmp::ndp::options::{NdpOptionBuilder, PrefixInformation};
use std::pin::pin;
use test_case::test_case;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum IpSupported {
    Ipv4Only,
    Ipv6Only,
    DualStack,
}

/// Test that network is usable after an interface has its link go down and
/// then back up. The client netstack acquires an IPv4 address via DHCPv4
/// client, and an IPv6 global address via SLAAC in response to injected
/// Router Advertisements; both of these closely resembles production
/// environments. These tests exercise both UDP and TCP sockets, as well as the
/// interface watcher API, and guard against regressions in netstack's ability
/// to recover connectivity (or in the case of interface watcher API,
/// misleading clients of the API into thinking connectivity has not
/// been restored when it has).
#[netstack_test]
#[variant(N, Netstack)]
#[test_case(IpSupported::Ipv6Only; "ipv6_only")]
#[test_case(IpSupported::Ipv4Only; "ipv4_only")]
#[test_case(IpSupported::DualStack; "dual_stack")]
async fn interface_disruption<N: Netstack>(name: &str, ip_supported: IpSupported) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");

    let client_realm = sandbox
        .create_netstack_realm::<N, _>(format!("{}_client", name))
        .expect("failed to create client netstack realm");
    let server_realm = sandbox
        .create_netstack_realm_with::<N, _, _>(
            format!("{}_server", name),
            &[
                KnownServiceProvider::DhcpServer { persistent: false },
                KnownServiceProvider::DnsResolver,
                KnownServiceProvider::FakeClock,
                KnownServiceProvider::SecureStash,
            ],
        )
        .expect("failed to create server netstack realm");

    let net = sandbox.create_network("net").await.expect("failed to create network");

    let client_if = client_realm
        .join_network(&net, "client_ep")
        .await
        .expect("failed to join network in client realm");
    const SERVER_IF_NAME: &str = "server_if";
    let server_if = server_realm
        .join_network_with_if_config(
            &net,
            "server_ep",
            netemul::InterfaceConfig { name: Some(SERVER_IF_NAME.into()), ..Default::default() },
        )
        .await
        .expect("failed to join network in server realm");

    let fake_ep = net.create_fake_endpoint().expect("create fake endpoint");

    let client_interfaces_state = client_realm
        .connect_to_protocol::<fnet_interfaces::StateMarker>()
        .expect("connect to protocol");
    let server_interfaces_state = server_realm
        .connect_to_protocol::<fnet_interfaces::StateMarker>()
        .expect("connect to protocol");

    let server_link_local = interfaces::wait_for_v6_ll(&server_interfaces_state, server_if.id())
        .await
        .expect("wait for link-local address in server realm");

    let send_ra_and_wait_for_addr = || async {
        let mut send_ra_fut = pin!(
            fasync::Interval::new(zx::MonotonicDuration::from_seconds(4)).for_each(|()| async {
                let options = [NdpOptionBuilder::PrefixInformation(PrefixInformation::new(
                    ipv6_consts::GLOBAL_PREFIX.prefix(),  /* prefix_length */
                    false,                                /* on_link_flag */
                    true,  /* autonomous_address_configuration_flag */
                    99999, /* valid_lifetime */
                    99999, /* preferred_lifetime */
                    ipv6_consts::GLOBAL_PREFIX.network(), /* prefix */
                ))];
                ndp::send_ra_with_router_lifetime(&fake_ep, 9999, &options, server_link_local)
                    .await
                    .expect("failed to send router advertisement")
            })
        );
        let mut wait_addr_fut = pin!(
            interfaces::wait_for_addresses(&client_interfaces_state, client_if.id(), |addresses| {
                addresses.iter().find_map(
                    |&fnet_interfaces_ext::Address {
                         addr: fnet::Subnet { addr, prefix_len: _ },
                         assignment_state,
                         ..
                     }| {
                        assert_eq!(
                            assignment_state,
                            fnet_interfaces::AddressAssignmentState::Assigned
                        );
                        match addr {
                            fnet::IpAddress::Ipv4(fnet::Ipv4Address { addr: _ }) => None,
                            fnet::IpAddress::Ipv6(fnet::Ipv6Address { addr }) => {
                                let addr = net_types::ip::Ipv6Addr::from_bytes(addr);
                                ipv6_consts::GLOBAL_PREFIX.contains(&addr).then_some(addr)
                            }
                        }
                    },
                )
            })
            .map(|r| r.expect("wait for SLAAC IPv6 address to appear"))
        );
        let addr = futures::select! {
            () = send_ra_fut => unreachable!("sending Router Advertisements should never stop"),
            addr = wait_addr_fut => addr
        };
        net_types::ip::IpAddr::V6(addr)
    };
    const SERVER_V6: net_types::ip::Ipv6Addr = ipv6_consts::GLOBAL_ADDR;
    let client_v6 = match ip_supported {
        IpSupported::Ipv4Only => None,
        IpSupported::Ipv6Only | IpSupported::DualStack => {
            server_if
                .add_address_and_subnet_route(fnet::Subnet {
                    addr: fnet::IpAddress::Ipv6(fnet::Ipv6Address { addr: SERVER_V6.ipv6_bytes() }),
                    prefix_len: ipv6_consts::GLOBAL_PREFIX.prefix(),
                })
                .await
                .expect("add IPv6 address and subnet route to server");

            // Allow the client realm to generate global IPv6 addresses.
            Some(send_ra_and_wait_for_addr().await)
        }
    };

    client_if.start_dhcp::<InStack>().await.expect("start DHCPv4 client");

    let server_v4 = {
        let dhcpv4::TestConfig { server_addr: fnet::Ipv4Address { addr }, managed_addrs: _ } =
            dhcpv4::DEFAULT_TEST_CONFIG;
        net_types::ip::Ipv4Addr::from(addr)
    };
    // Set up DHCPv4 server on server.
    match ip_supported {
        IpSupported::Ipv6Only => {}
        IpSupported::Ipv4Only | IpSupported::DualStack => {
            server_if
                .add_address_and_subnet_route(
                    dhcpv4::DEFAULT_TEST_CONFIG.server_addr_with_prefix().into_ext(),
                )
                .await
                .expect("add IPv4 address and subnet route to server");

            let dhcp_server = server_realm
                .connect_to_protocol::<fnet_dhcp::Server_Marker>()
                .expect("failed to connect to DHCP server");
            let param_iter =
                dhcpv4::DEFAULT_TEST_CONFIG.dhcp_parameters().into_iter().chain(std::iter::once(
                    fnet_dhcp::Parameter::BoundDeviceNames(vec![SERVER_IF_NAME.to_string()]),
                ));
            dhcpv4::set_server_settings(
                &dhcp_server,
                param_iter,
                std::iter::once(fnet_dhcp::Option_::Router(vec![
                    dhcpv4::DEFAULT_TEST_CONFIG.server_addr,
                ])),
            )
            .await;

            dhcp_server
                .start_serving()
                .await
                .expect("failed to call dhcp/Server.StartServing")
                .map_err(zx::Status::err_from_raw)
                .expect("dhcp/Server.StartServing returned error");
        }
    }
    let wait_for_dhcpv4 = || async {
        let mut state =
            fidl_fuchsia_net_interfaces_ext::InterfaceState::<(), _>::Unknown(client_if.id());
        let fnet::Subnet { addr, prefix_len: _ } = fnet_interfaces_ext::wait_interface_with_id(
            fnet_interfaces_ext::event_stream_from_state::<fnet_interfaces_ext::DefaultInterest>(
                &client_interfaces_state,
                Default::default(),
            )
            .expect("get interface event stream"),
            &mut state,
            |fnet_interfaces_ext::PropertiesAndState {
                 properties:
                     fnet_interfaces_ext::Properties {
                         addresses,
                         has_default_ipv4_route,
                         id: _,
                         name: _,
                         port_class: _,
                         online: _,
                         has_default_ipv6_route: _,
                         port_identity_koid: _,
                     },
                 state: _,
             }| {
                if !has_default_ipv4_route {
                    return None;
                }
                addresses.iter().find_map(
                    |&fnet_interfaces_ext::Address { addr, assignment_state, .. }| {
                        assert_eq!(
                            assignment_state,
                            fnet_interfaces::AddressAssignmentState::Assigned
                        );
                        (addr == dhcpv4::DEFAULT_TEST_CONFIG.expected_acquired()).then_some(addr)
                    },
                )
            },
        )
        .await
        .expect("wait for DHCPv4 address and default route");

        let addr = assert_matches!(addr, fnet::IpAddress::Ipv4(fnet::Ipv4Address { addr }) => addr);
        net_types::ip::IpAddr::V4(net_types::ip::Ipv4Addr::new(addr))
    };
    let client_v4 = match ip_supported {
        IpSupported::Ipv6Only => None,
        IpSupported::Ipv4Only | IpSupported::DualStack => Some(wait_for_dhcpv4().await),
    };

    const CLIENT_PORT: u16 = 12345;
    const SERVER_PORT: u16 = 54321;
    let into_sockaddr =
        |addr: net_types::ip::IpAddr<net_types::ip::Ipv4Addr, net_types::ip::Ipv6Addr>, port| {
            match addr {
                net_types::ip::IpAddr::V4(addr_v4) => {
                    std::net::SocketAddr::from((addr_v4.ipv4_bytes(), port))
                }
                net_types::ip::IpAddr::V6(addr_v6) => {
                    std::net::SocketAddr::from((addr_v6.ipv6_bytes(), port))
                }
            }
        };
    let bind_udp = |realm, sockaddr| async move {
        let sock =
            fasync::net::UdpSocket::bind_in_realm(realm, sockaddr).await.unwrap_or_else(|e| {
                panic!("failed to bind UDP socket to {} in server realm: {}", sockaddr, e)
            });
        sock
    };
    let setup = |client_realm,
                 server_realm,
                 client_addr: Option<
        net_types::ip::IpAddr<net_types::ip::Ipv4Addr, net_types::ip::Ipv6Addr>,
    >,
                 server_addr| async move {
        if let Some(client_addr) = client_addr {
            let server_sockaddr = into_sockaddr(server_addr, SERVER_PORT);
            let client_sockaddr = into_sockaddr(client_addr.into(), CLIENT_PORT);

            let server_udp = bind_udp(server_realm, server_sockaddr).await;
            let client_udp = bind_udp(client_realm, client_sockaddr).await;

            let listener = fasync::net::TcpListener::listen_in_realm(server_realm, server_sockaddr)
                .await
                .expect("server realm listen");
            let connect_fut = fasync::net::TcpStream::bind_and_connect_in_realm(
                client_realm,
                client_sockaddr,
                server_sockaddr,
            )
            .map(|r| r.expect("bind and connect"));
            let accept_fut = listener.accept().map(|r| r.expect("accept"));
            let (client_tcp_stream, (_, server_tcp_stream, _)): (
                _,
                (fasync::net::TcpListener, _, std::net::SocketAddr),
            ) = futures::future::join(connect_fut, accept_fut).await;
            Some((client_udp, server_udp, client_tcp_stream, server_tcp_stream))
        } else {
            None
        }
    };
    let (mut v6, mut v4) = futures::future::join(
        setup(&client_realm, &server_realm, client_v6, SERVER_V6.into()),
        setup(&client_realm, &server_realm, client_v4, server_v4.into()),
    )
    .await;

    let mut message_id = 0;
    let mut next_message_id = || {
        message_id += 1;
        message_id
    };
    async fn udp_send_and_recv(
        message_id: u8,
        client_sock: &fasync::net::UdpSocket,
        server_sock: &fasync::net::UdpSocket,
    ) {
        let message = format!("Message {}", message_id);
        let server_sockaddr = server_sock.local_addr().expect("server socket local addr");
        let got =
            client_sock.send_to(message.as_bytes(), server_sockaddr).await.expect("send failed");
        assert_eq!(got, message.len());

        let mut buf: [u8; 32] = [0; 32];
        let (got_len, got_sockaddr) =
            server_sock.recv_from(&mut buf).await.expect("server receive failed");
        assert_eq!(got_len, message.len());
        assert_eq!(&buf[..got_len], message.as_bytes());
        assert_eq!(got_sockaddr, client_sock.local_addr().expect("client socket local addr"));
    }
    async fn tcp_send_and_recv(
        message_id: u8,
        client: &mut fasync::net::TcpStream,
        server: &mut fasync::net::TcpStream,
    ) {
        let message = format!("Message {}", message_id);
        let mut buf = vec![0; message.len()];
        client.write_all(message.as_bytes()).await.expect("write TCP bytes");
        server.read_exact(&mut buf).await.expect("read TCP");
        assert_eq!(&buf, message.as_bytes());
    }
    futures::stream::iter([v4.as_mut(), v6.as_mut()].into_iter())
        .for_each_concurrent(None, |sockets| {
            futures::future::OptionFuture::from(sockets.map(
                |(client_udp, server_udp, client_tcp, server_tcp)| {
                    let (tcp_message_id, udp_message_id) = (next_message_id(), next_message_id());
                    futures::future::join(
                        udp_send_and_recv(udp_message_id, client_udp, server_udp),
                        tcp_send_and_recv(tcp_message_id, client_tcp, server_tcp),
                    )
                },
            ))
            .map(|_: Option<((), ())>| ())
        })
        .await;

    client_if.set_link_up(false).await.expect("failed to set client interface link down");
    // Waiting for interface watcher to return that the interface is offline
    // enables the test to immediately assert sending on sockets to fail.
    interfaces::wait_for_online(&client_interfaces_state, client_if.id(), false)
        .await
        .expect("wait for client interface to go offline");

    async fn send_while_interface_offline(
        (client_udp, server_udp, client_tcp, _): &mut (
            fasync::net::UdpSocket,
            fasync::net::UdpSocket,
            fasync::net::TcpStream,
            fasync::net::TcpStream,
        ),
        udp_message_id: u8,
        tcp_message_id: u8,
    ) -> u8 {
        let server_sockaddr = server_udp.local_addr().expect("server get local sockaddr");
        futures::future::join(
            async {
                let message = format!("Message {}", udp_message_id);
                let got = client_udp.send_to(message.as_bytes(), server_sockaddr).await;

                assert_matches!(got, Err(e) => {
                    // TODO(https://github.com/rust-lang/rust/issues/86442): once
                    // std::io::ErrorKind::HostUnreachable is stable, we should use that instead.
                    assert_eq!(e.raw_os_error(), Some(libc::EHOSTUNREACH));
                });
            },
            async {
                let message = format!("Message {}", tcp_message_id);
                client_tcp.write_all(message.as_bytes()).await.expect("TCP write failed");
            },
        )
        .await;
        tcp_message_id
    }
    let (tcp_v6_message_id, tcp_v4_message_id): (Option<u8>, Option<u8>) = futures::future::join(
        futures::future::OptionFuture::from(v6.as_mut().map(|sockets| {
            send_while_interface_offline(sockets, next_message_id(), next_message_id())
        })),
        futures::future::OptionFuture::from(v4.as_mut().map(|sockets| {
            send_while_interface_offline(sockets, next_message_id(), next_message_id())
        })),
    )
    .await;

    client_if.set_link_up(true).await.expect("failed to set client interface link up");
    interfaces::wait_for_online(&client_interfaces_state, client_if.id(), true)
        .await
        .expect("wait for client interface to go offline");

    // Induce the IPv6 default route to be re-added.
    match ip_supported {
        IpSupported::Ipv4Only => {}
        IpSupported::Ipv6Only | IpSupported::DualStack => {
            let _: net_types::ip::IpAddr = send_ra_and_wait_for_addr().await;
        }
    };
    match ip_supported {
        IpSupported::Ipv6Only => {}
        IpSupported::Ipv4Only | IpSupported::DualStack => {
            let _: net_types::ip::IpAddr = wait_for_dhcpv4().await;
        }
    }

    futures::stream::iter(
        [(v4.as_mut(), tcp_v4_message_id), (v6.as_mut(), tcp_v6_message_id)].into_iter(),
    )
    .for_each_concurrent(None, |(sockets, want_message_id)| {
        futures::future::OptionFuture::from(sockets.map(
            |(client_udp, server_udp, client_tcp, server_tcp)| {
                let udp_message_id = next_message_id();
                let tcp_message_id = next_message_id();
                async move {
                    udp_send_and_recv(udp_message_id, client_udp, server_udp).await;

                    // Read the TCP message sent while interface was offline.
                    let message =
                        format!("Message {}", want_message_id.expect("message ID must be present"));
                    let mut buf = vec![0; message.len()];
                    server_tcp.read_exact(&mut buf).await.expect("read TCP");
                    assert_eq!(&buf, message.as_bytes());

                    tcp_send_and_recv(tcp_message_id, client_tcp, server_tcp).await;
                }
            },
        ))
        .map(|_: Option<()>| ())
    })
    .await;
}

#[netstack_test]
#[variant(I, Ip)]
async fn mark_on_incoming_syn<
    I: FidlRuleAdminIpExt + FidlRuleIpExt + FidlRouteAdminIpExt + FidlRouteIpExt,
>(
    name: &str,
) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let server = sandbox
        .create_netstack_realm::<Netstack3, _>(format!("{name}_server"))
        .expect("create netstack");
    let client = sandbox
        .create_netstack_realm::<Netstack3, _>(format!("{name}_client"))
        .expect("create netstack");
    let network = sandbox.create_network("net").await.expect("create network");
    let server_ep = server.join_network(&network, "server_ep").await.expect("join network");
    let client_ep = client.join_network(&network, "client_ep").await.expect("join network");

    let (server_subnet, client_subnet) = match I::VERSION {
        IpVersion::V4 => (fidl_subnet!("192.168.0.2/24"), fidl_subnet!("192.168.0.1/24")),
        IpVersion::V6 => (
            fidl_subnet!("2001:0db8:85a3::8a2e:0370:7334/64"),
            fidl_subnet!("2001:0db8:85a3::8a2e:0370:7335/64"),
        ),
    };

    server_ep.add_address_and_subnet_route(server_subnet).await.expect("set ip");
    client_ep.add_address_and_subnet_route(client_subnet).await.expect("set ip");

    let control =
        server.connect_to_protocol::<fnet_filter::ControlMarker>().expect("connect to protocol");
    let mut controller = Controller::new(&control, &ControllerId(String::from("mangle")))
        .await
        .expect("create controller");
    let namespace_id = NamespaceId(String::from("namespace"));
    let routine_id = RoutineId { namespace: namespace_id.clone(), name: String::from("routine") };
    const MARK: u32 = 100;
    const DOMAIN: fnet::MarkDomain = fnet::MarkDomain::Mark1;
    let resources = [
        Resource::Namespace(Namespace { id: namespace_id.clone(), domain: Domain::AllIp }),
        Resource::Routine(Routine {
            id: routine_id.clone(),
            routine_type: RoutineType::Ip(Some(InstalledIpRoutine {
                hook: IpHook::Ingress,
                priority: 0,
            })),
        }),
        Resource::Rule(Rule {
            id: RuleId { routine: routine_id.clone(), index: 0 },
            matchers: Matchers::default(),
            action: Action::Mark {
                domain: DOMAIN,
                action: MarkAction::SetMark { clearing_mask: 0, mark: MARK },
            },
        }),
    ];
    controller
        .push_changes(resources.iter().cloned().map(Change::Create).collect())
        .await
        .expect("push changes");
    controller.commit().await.expect("commit pending changes");

    let rule_table =
        server.connect_to_protocol::<I::RuleTableMarker>().expect("connect to rule table");
    let priority = RuleSetPriority::from(0);
    let rule_set =
        fnet_routes_ext::rules::new_rule_set::<I>(&rule_table, priority).expect("fidl error");

    const RULE_INDEX_0: RuleIndex = RuleIndex::new(0);
    const RULE_INDEX_1: RuleIndex = RuleIndex::new(1);

    let main_table = server
        .connect_to_protocol::<I::RouteTableMarker>()
        .expect("connect to route table provider");

    let fnet_routes_admin::GrantForRouteTableAuthorization { table_id: main_table_id, token } =
        fnet_routes_ext::admin::get_authorization_for_route_table::<I>(&main_table)
            .await
            .expect("fidl error");

    fnet_routes_ext::rules::authenticate_for_route_table::<I>(&rule_set, main_table_id, token)
        .await
        .expect("fidl error")
        .expect("failed to authenticate");
    fnet_routes_ext::rules::add_rule::<I>(
        &rule_set,
        RULE_INDEX_0,
        RuleMatcher {
            mark_1: Some(fnet_matchers_ext::Mark::Marked {
                mask: !0,
                between: MARK..=MARK,
                invert: false,
            }),
            ..Default::default()
        },
        RuleAction::Lookup(TableId::new(main_table_id)),
    )
    .await
    .expect("fidl error")
    .expect("failed to add a new rule");

    fnet_routes_ext::rules::add_rule::<I>(
        &rule_set,
        RULE_INDEX_1,
        RuleMatcher::default(),
        RuleAction::Unreachable,
    )
    .await
    .expect("fidl error")
    .expect("failed to add a new rule");

    let fnet_ext::IpAddress(client_addr) = client_subnet.addr.into();
    let client_addr = std::net::SocketAddr::new(client_addr, 1234);

    let fnet_ext::IpAddress(server_addr) = server_subnet.addr.into();
    let server_addr = std::net::SocketAddr::new(server_addr, 8080);

    // We pick a payload that is small enough to be guaranteed to fit in a TCP segment so both the
    // client and server can read the entire payload in a single `read`.
    const PAYLOAD: &'static str = "Hello World";

    let listener = fasync::net::TcpListener::listen_in_realm(&server, server_addr)
        .await
        .expect("failed to create server socket");

    let server_fut = async {
        let (_, mut stream, from) = listener.accept().await.expect("accept failed");

        let mut buf = [0u8; 1024];
        let read_count = stream.read(&mut buf).await.expect("read from tcp server stream failed");
        assert_eq!(from.ip(), client_addr.ip());

        assert_eq!(read_count, PAYLOAD.as_bytes().len());
        assert_eq!(&buf[..read_count], PAYLOAD.as_bytes());

        let write_count =
            stream.write(PAYLOAD.as_bytes()).await.expect("write to tcp server stream failed");
        assert_eq!(write_count, PAYLOAD.as_bytes().len());

        // The accepted socket should bear the mark that we set by filter rules.
        let channel = fdio::clone_channel(stream.std()).expect("failed to clone channel");
        let proxy = fposix_socket::BaseSocketProxy::new(fidl::AsyncChannel::from_channel(channel));
        assert_eq!(
            proxy.get_mark(DOMAIN).await.expect("fidl error").expect("get mark"),
            fposix_socket::OptionalUint32::Value(MARK),
        );
    };

    let client_fut = async {
        let mut stream = fasync::net::TcpStream::connect_in_realm(&client, server_addr)
            .await
            .expect("failed to create client socket");

        let write_count =
            stream.write(PAYLOAD.as_bytes()).await.expect("write to tcp client stream failed");

        assert_eq!(write_count, PAYLOAD.as_bytes().len());

        let mut buf = [0u8; 1024];
        let read_count = stream.read(&mut buf).await.expect("read from tcp client stream failed");

        assert_eq!(read_count, PAYLOAD.as_bytes().len());
        assert_eq!(&buf[..read_count], PAYLOAD.as_bytes());
    };

    let ((), ()) = futures::future::join(client_fut, server_fut).await;
}

#[netstack_test]
#[variant(I, Ip)]
async fn bind_to_device_skips_unreachable_rule<
    I: FidlRuleAdminIpExt + FidlRuleIpExt + FidlRouteAdminIpExt + FidlRouteIpExt,
>(
    name: &str,
) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let client = sandbox
        .create_netstack_realm::<Netstack3, _>(format!("{name}_client"))
        .expect("create netstack");
    let network = sandbox.create_network("net").await.expect("create network");
    let client_ep = client.join_network(&network, "client_ep").await.expect("join network");

    let (subnet, connect_to) = match I::VERSION {
        IpVersion::V4 => (fidl_subnet!("192.168.0.1/24"), std_ip!("192.168.0.2")),
        IpVersion::V6 => (
            fidl_subnet!("2001:0db8:85a3::8a2e:0370:7334/64"),
            std_ip!("2001:0db8:85a3::8a2e:0370:7335"),
        ),
    };

    client_ep.add_address_and_subnet_route(subnet).await.expect("set ip");

    let rule_table =
        client.connect_to_protocol::<I::RuleTableMarker>().expect("connect to rule table");
    let priority = RuleSetPriority::from(0);
    let rule_set =
        fnet_routes_ext::rules::new_rule_set::<I>(&rule_table, priority).expect("fidl error");

    const RULE_INDEX_0: RuleIndex = RuleIndex::new(0);

    fnet_routes_ext::rules::add_rule::<I>(
        &rule_set,
        RULE_INDEX_0,
        RuleMatcher {
            bound_device: Some(fnet_matchers_ext::BoundInterface::Unbound),
            ..Default::default()
        },
        RuleAction::Unreachable,
    )
    .await
    .expect("fidl error")
    .expect("failed to add a new rule");

    let fnet_ext::IpAddress(client_addr) = subnet.addr.into();
    let client_addr = std::net::SocketAddr::new(client_addr, 1234);

    let connect_addr = std::net::SocketAddr::new(connect_to, 4321);

    let udp_connector = socket2::Socket::from(
        std::net::UdpSocket::bind_in_realm(&client, client_addr)
            .await
            .expect("failed to create a udp socket"),
    );

    assert_matches!(
        udp_connector.connect(&connect_addr.into()), Err(io_err)
            => assert_eq!(io_err.kind(), std::io::ErrorKind::NetworkUnreachable)
    );
    udp_connector
        .bind_device(Some(
            client_ep.get_interface_name().await.expect("failed to get interface name").as_bytes(),
        ))
        .expect("SO_BINDTODEVICE");
    assert_matches!(udp_connector.connect(&connect_addr.into()), Ok(()));
}
