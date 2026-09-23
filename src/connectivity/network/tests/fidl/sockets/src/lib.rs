// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

use std::io::{ErrorKind, Read as _, Write as _};
use std::os::fd::AsFd;
use std::time::Duration;

use assert_matches::assert_matches;
use fidl::Error::ClientChannelClosed;
use fidl::endpoints::Proxy as _;
use fidl_fuchsia_net_ext::IntoExt as _;
use fidl_fuchsia_net_sockets::IpSocketState;
use futures::TryStreamExt as _;
use net_declare::std_ip_v6;
use net_types::ip::{Ip, IpAddress, IpVersion};
use netemul::RealmUdpSocket as _;
use netstack_testing_common::realms::{Netstack3, TestSandboxExt as _};
use netstack_testing_macros::netstack_test;
use test_case::test_case;
use test_util::{assert_geq, assert_gt};

use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_matchers as fnet_matchers;
use fidl_fuchsia_net_matchers_ext as fnet_matchers_ext;
use fidl_fuchsia_net_sockets as fnet_sockets;
use fidl_fuchsia_net_sockets_ext as fnet_sockets_ext;
use fidl_fuchsia_net_tcp as fnet_tcp;
use fidl_fuchsia_net_udp as fnet_udp;
use fidl_fuchsia_posix_socket as fposix_socket;
use fuchsia_async as fasync;

const MARK_1: u32 = 100;
const MARK_2: u32 = 200;
const LOCAL_PORT: u16 = 1234;
const IPV6_LOOPBACK: std::net::Ipv6Addr = std_ip_v6!("::1");
const LISTEN_BACKLOG: i32 = 1;

async fn socket_proxy<F: AsFd>(socket: &F) -> fposix_socket::BaseSocketProxy {
    let channel = fdio::clone_channel(socket).expect("failed to clone channel");
    fposix_socket::BaseSocketProxy::new(fidl::AsyncChannel::from_channel(channel))
}

async fn set_marks(proxy: &fposix_socket::BaseSocketProxy) {
    proxy
        .set_mark(fnet::MarkDomain::Mark1, &fposix_socket::OptionalUint32::Value(MARK_1))
        .await
        .expect("fidl error")
        .expect("set mark");
    proxy
        .set_mark(fnet::MarkDomain::Mark2, &fposix_socket::OptionalUint32::Value(MARK_2))
        .await
        .expect("fidl error")
        .expect("set mark");
}

#[netstack_test]
async fn no_results_when_no_sockets(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let diagnostics = realm
        .connect_to_protocol::<fnet_sockets::DiagnosticsMarker>()
        .expect("connect to protocol");

    let (proxy, server_end) = fidl::endpoints::create_proxy::<fnet_sockets::IpIteratorMarker>();
    assert_matches!(
        diagnostics
            .iterate_ip(
                server_end,
                fnet_sockets::Extensions::empty(),
                &[fnet_sockets::IpSocketMatcher::Family(fnet::IpVersion::V4)]
            )
            .await
            .expect("failed to call fidl"),
        fnet_sockets::IterateIpResult::Ok(fnet_sockets::Empty)
    );

    let (sockets, has_more) = proxy.next().await.unwrap();
    assert!(sockets.is_empty());
    assert!(!has_more);
    assert_matches!(proxy.next().await, Err(ClientChannelClosed { .. }));
}

#[netstack_test]
async fn invalid_matcher(name: &str) {
    let good_matcher = fnet_sockets::IpSocketMatcher::Family(fnet::IpVersion::V4);
    let invalid_matcher = fnet_sockets::IpSocketMatcher::BoundInterface(
        fnet_matchers::BoundInterface::Bound(fnet_matchers::Interface::Id(0)),
    );

    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let diagnostics = realm
        .connect_to_protocol::<fnet_sockets::DiagnosticsMarker>()
        .expect("connect to protocol");

    let (proxy, server_end) = fidl::endpoints::create_proxy::<fnet_sockets::IpIteratorMarker>();
    assert_matches!(
        diagnostics
            .iterate_ip(
                server_end,
                fnet_sockets::Extensions::empty(),
                &[invalid_matcher.clone(), good_matcher.clone(), good_matcher.clone(),]
            )
            .await
            .expect("failed to call fidl"),
        fnet_sockets::IterateIpResult::InvalidMatcher(fnet_sockets::InvalidMatcher { index: 0 })
    );
    assert_matches!(proxy.next().await, Err(ClientChannelClosed { .. }));

    let (proxy, server_end) = fidl::endpoints::create_proxy::<fnet_sockets::IpIteratorMarker>();
    assert_matches!(
        diagnostics
            .iterate_ip(
                server_end,
                fnet_sockets::Extensions::empty(),
                &[good_matcher.clone(), good_matcher, invalid_matcher,]
            )
            .await
            .expect("failed to call fidl"),
        fnet_sockets::IterateIpResult::InvalidMatcher(fnet_sockets::InvalidMatcher { index: 2 })
    );
    assert_matches!(proxy.next().await, Err(ClientChannelClosed { .. }));
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum Protocol {
    Udp,
    Tcp,
}

#[derive(Debug, Copy, Clone)]
enum SocketState {
    Listen,
    Connected,
}

#[netstack_test]
#[variant(I, Ip)]
#[test_case(Protocol::Udp, SocketState::Listen)]
#[test_case(Protocol::Udp, SocketState::Connected)]
#[test_case(Protocol::Tcp, SocketState::Listen)]
#[test_case(Protocol::Tcp, SocketState::Connected)]
async fn test_sockets<I: Ip>(name: &str, protocol: Protocol, state: SocketState) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    let local_addr = std::net::SocketAddr::new(I::LOOPBACK_ADDRESS.to_ip_addr().into(), LOCAL_PORT);
    let domain = match I::VERSION {
        IpVersion::V4 => fposix_socket::Domain::Ipv4,
        IpVersion::V6 => fposix_socket::Domain::Ipv6,
    };

    let socket = match protocol {
        Protocol::Udp => {
            let socket = realm
                .datagram_socket(domain, fposix_socket::DatagramSocketProtocol::Udp)
                .await
                .unwrap();
            socket.bind(&local_addr.into()).unwrap();
            if let SocketState::Connected = state {
                socket.connect(&local_addr.into()).unwrap();
            }
            socket_proxy(&socket).await
        }
        Protocol::Tcp => {
            let socket = realm
                .stream_socket(domain, fposix_socket::StreamSocketProtocol::Tcp)
                .await
                .unwrap();
            socket.bind(&local_addr.into()).unwrap();
            match state {
                SocketState::Listen => socket.listen(LISTEN_BACKLOG).unwrap(),
                // NOTE: This is a self-connected socket so we can avoid
                // creating other TCP sockets that we'd just have to filter
                // out.
                SocketState::Connected => socket.connect(&local_addr.into()).expect("connect"),
            }
            socket_proxy(&socket).await
        }
    };

    set_marks(&socket).await;

    let diagnostics = realm
        .connect_to_protocol::<fnet_sockets::DiagnosticsMarker>()
        .expect("connect to protocol");

    let (proxy, server_end) = fidl::endpoints::create_proxy::<fnet_sockets::IpIteratorMarker>();
    let matchers = [
        fnet_sockets_ext::IpSocketMatcher::Family(I::VERSION),
        fnet_sockets_ext::IpSocketMatcher::Proto(match protocol {
            Protocol::Udp => {
                fnet_matchers_ext::SocketTransportProtocol::Udp(fnet_matchers_ext::UdpSocket::Empty)
            }
            Protocol::Tcp => {
                fnet_matchers_ext::SocketTransportProtocol::Tcp(fnet_matchers_ext::TcpSocket::Empty)
            }
        }),
    ]
    .into_iter()
    .map(Into::into)
    .collect::<Vec<_>>();
    assert_matches!(
        diagnostics
            .iterate_ip(server_end, fnet_sockets::Extensions::empty(), &matchers)
            .await
            .expect("failed to call fidl"),
        fnet_sockets::IterateIpResult::Ok(_)
    );

    let expected_src_addr = I::LOOPBACK_ADDRESS.to_ip_addr().into_ext();
    let expected_cookie = socket.get_cookie().await.expect("failed to get cookie").unwrap();
    let (expected_dst_addr, expected_dst_port) = match state {
        SocketState::Listen => (None, None),
        SocketState::Connected => (Some(expected_src_addr), Some(LOCAL_PORT)),
    };

    let expected_transport = match protocol {
        Protocol::Udp => {
            fnet_sockets::IpSocketTransportState::Udp(fnet_sockets::IpSocketUdpState {
                src_port: Some(LOCAL_PORT),
                dst_port: expected_dst_port,
                state: Some(match state {
                    SocketState::Listen => fnet_udp::State::Bound,
                    SocketState::Connected => fnet_udp::State::Connected,
                }),
                __source_breaking: fidl::marker::SourceBreaking,
            })
        }
        Protocol::Tcp => {
            fnet_sockets::IpSocketTransportState::Tcp(fnet_sockets::IpSocketTcpState {
                src_port: Some(LOCAL_PORT),
                dst_port: expected_dst_port,
                state: Some(match state {
                    SocketState::Listen => fnet_tcp::State::Listen,
                    SocketState::Connected => fnet_tcp::State::Established,
                }),
                tcp_info: None,
                __source_breaking: fidl::marker::SourceBreaking,
            })
        }
    };

    let expected_socket = fnet_sockets::IpSocketState {
        family: Some(I::VERSION.into_ext()),
        src_addr: Some(expected_src_addr),
        dst_addr: expected_dst_addr,
        cookie: Some(expected_cookie),
        marks: Some(fnet::Marks {
            mark_1: Some(MARK_1),
            mark_2: Some(MARK_2),
            __source_breaking: fidl::marker::SourceBreaking,
        }),
        transport: Some(expected_transport),
        __source_breaking: fidl::marker::SourceBreaking,
    };

    let (batch, has_more) = proxy.next().await.unwrap();
    assert!(!has_more);
    assert_eq!(batch, vec![expected_socket]);
}

#[netstack_test]
async fn paginated_results(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    const BASE_PORT: u16 = 20000;
    let mut sockets = Vec::new();
    let mut expected_ports = std::collections::HashSet::new();
    for i in 0..(fnet_sockets::MAX_IP_SOCKET_BATCH_SIZE + 1) {
        let port = BASE_PORT + (i as u16);
        let socket = fasync::net::UdpSocket::bind_in_realm(
            &realm,
            std::net::SocketAddr::from((IPV6_LOOPBACK, port)),
        )
        .await
        .expect("failed to create socket");
        sockets.push(socket);
        assert!(expected_ports.insert(port));
    }

    let diagnostics = realm
        .connect_to_protocol::<fnet_sockets::DiagnosticsMarker>()
        .expect("connect to protocol");

    let (proxy, server_end) = fidl::endpoints::create_proxy::<fnet_sockets::IpIteratorMarker>();
    assert_matches!(
        diagnostics
            .iterate_ip(server_end, fnet_sockets::Extensions::empty(), &[])
            .await
            .expect("failed to call fidl"),
        fnet_sockets::IterateIpResult::Ok(fnet_sockets::Empty)
    );

    let mut observed_ports = std::collections::HashSet::new();
    let mut process_batch = |batch: &[IpSocketState]| {
        for socket in batch {
            let src_port = assert_matches!(
                &socket.transport,
                Some(fnet_sockets::IpSocketTransportState::Udp(udp)) => {
                    assert_matches!(udp.src_port, Some(port) => port)
                }
            );
            assert!(observed_ports.insert(src_port), "duplicate port {} returned", src_port);
        }
    };

    let (batch, has_more) = proxy.next().await.expect("failed to get first batch");
    assert_eq!(batch.len(), fnet_sockets::MAX_IP_SOCKET_BATCH_SIZE as usize);
    assert!(has_more);
    process_batch(&batch);

    let (batch, has_more) = proxy.next().await.expect("failed to get second batch");
    assert_eq!(batch.len(), 1);
    assert!(!has_more);
    process_batch(&batch);

    assert_eq!(observed_ports, expected_ports);

    assert_matches!(proxy.next().await, Err(ClientChannelClosed { .. }));
}

#[netstack_test]
async fn control_disconnect_ip_unconstrained_matchers_error(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let control =
        realm.connect_to_protocol::<fnet_sockets::ControlMarker>().expect("connect to protocol");

    let result = control
        .disconnect_ip(&fnet_sockets::ControlDisconnectIpRequest::default())
        .await
        .expect("call disconnect_ip");
    assert_eq!(
        result,
        fnet_sockets::DisconnectIpResult::UnconstrainedMatchers(fnet_sockets::Empty)
    );

    // Receiving an error should not close the channel.
    let result = control
        .disconnect_ip(&fnet_sockets::ControlDisconnectIpRequest {
            matchers: Some(vec![]),
            ..Default::default()
        })
        .await
        .expect("call disconnect_ip");
    assert_eq!(
        result,
        fnet_sockets::DisconnectIpResult::UnconstrainedMatchers(fnet_sockets::Empty)
    );
}

#[netstack_test]
async fn control_disconnect_ip_matcher_error(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let control =
        realm.connect_to_protocol::<fnet_sockets::ControlMarker>().expect("connect to protocol");

    let invalid_matcher = fnet_sockets::IpSocketMatcher::BoundInterface(
        fnet_matchers::BoundInterface::Bound(fnet_matchers::Interface::Id(0)),
    );

    let result = control
        .disconnect_ip(&fnet_sockets::ControlDisconnectIpRequest {
            matchers: Some(vec![invalid_matcher]),
            ..Default::default()
        })
        .await
        .expect("call disconnect_ip");
    assert_eq!(
        result,
        fnet_sockets::DisconnectIpResult::InvalidMatcher(fnet_sockets::InvalidMatcher { index: 0 })
    );
}

#[netstack_test]
async fn control_disconnect_ip_no_sockets_success(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let control =
        realm.connect_to_protocol::<fnet_sockets::ControlMarker>().expect("connect to protocol");

    let result = control
        .disconnect_ip(&fnet_sockets::ControlDisconnectIpRequest {
            matchers: Some(vec![fnet_sockets::IpSocketMatcher::Family(fnet::IpVersion::V4)]),
            ..Default::default()
        })
        .await
        .expect("call disconnect_ip");
    assert_eq!(
        result,
        fnet_sockets::DisconnectIpResult::Ok(fnet_sockets::DisconnectIpResponse {
            disconnected: 0
        })
    );
}

#[netstack_test]
#[variant(I, Ip)]
async fn control_disconnect_ip_tcp_listener<I: Ip>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let control =
        realm.connect_to_protocol::<fnet_sockets::ControlMarker>().expect("connect to protocol");

    let local_addr = std::net::SocketAddr::new(I::LOOPBACK_ADDRESS.to_ip_addr().into(), LOCAL_PORT);
    let domain = match I::VERSION {
        IpVersion::V4 => fposix_socket::Domain::Ipv4,
        IpVersion::V6 => fposix_socket::Domain::Ipv6,
    };

    let listener =
        realm.stream_socket(domain, fposix_socket::StreamSocketProtocol::Tcp).await.unwrap();
    listener.bind(&local_addr.into()).unwrap();
    listener.listen(LISTEN_BACKLOG).expect("listen");

    let client =
        realm.stream_socket(domain, fposix_socket::StreamSocketProtocol::Tcp).await.unwrap();
    client.connect(&local_addr.into()).expect("connect");

    let result = control
        .disconnect_ip(&fnet_sockets::ControlDisconnectIpRequest {
            matchers: Some(vec![fnet_sockets::IpSocketMatcher::Proto(
                fnet_matchers::SocketTransportProtocol::Tcp(fnet_matchers::TcpSocket::States(
                    fnet_matchers::TcpState::LISTEN,
                )),
            )]),
            ..Default::default()
        })
        .await
        .expect("call disconnect_ip");

    assert_matches!(
        result,
        fnet_sockets::DisconnectIpResult::Ok(fnet_sockets::DisconnectIpResponse {
            disconnected: 1
        })
    );

    // We can't connect anymore :-(
    let client =
        realm.stream_socket(domain, fposix_socket::StreamSocketProtocol::Tcp).await.unwrap();
    assert_matches!(
        client.connect(&local_addr.into()),
        Err(err) => assert_eq!(err.raw_os_error(), Some(libc::ECONNREFUSED))
    );

    // We're able to "resurrect" a listener by calling listen on it again.
    listener.listen(LISTEN_BACKLOG).expect("listen");
    let client =
        realm.stream_socket(domain, fposix_socket::StreamSocketProtocol::Tcp).await.unwrap();
    assert_matches!(client.connect(&local_addr.into()), Ok(_));
    let _ = listener.accept().expect("accept");
}

#[netstack_test]
#[variant(I, Ip)]
async fn control_disconnect_ip_tcp_listener_with_pending<I: Ip>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let control =
        realm.connect_to_protocol::<fnet_sockets::ControlMarker>().expect("connect to protocol");

    let local_addr = std::net::SocketAddr::new(I::LOOPBACK_ADDRESS.to_ip_addr().into(), LOCAL_PORT);
    let domain = match I::VERSION {
        IpVersion::V4 => fposix_socket::Domain::Ipv4,
        IpVersion::V6 => fposix_socket::Domain::Ipv6,
    };

    let listener =
        realm.stream_socket(domain, fposix_socket::StreamSocketProtocol::Tcp).await.unwrap();
    listener.bind(&local_addr.into()).unwrap();
    listener.listen(LISTEN_BACKLOG).expect("listen");

    // Connect a client but DO NOT accept to leave it in the accept queue.
    let client =
        realm.stream_socket(domain, fposix_socket::StreamSocketProtocol::Tcp).await.unwrap();
    client.connect(&local_addr.into()).expect("connect");

    let result = control
        .disconnect_ip(&fnet_sockets::ControlDisconnectIpRequest {
            matchers: Some(vec![fnet_sockets::IpSocketMatcher::Proto(
                fnet_matchers::SocketTransportProtocol::Tcp(fnet_matchers::TcpSocket::States(
                    fnet_matchers::TcpState::LISTEN,
                )),
            )]),
            ..Default::default()
        })
        .await
        .expect("call disconnect_ip");

    assert_matches!(
        result,
        fnet_sockets::DisconnectIpResult::Ok(fnet_sockets::DisconnectIpResponse {
            disconnected: 1
        })
    );

    let mut client = std::net::TcpStream::from(client);
    let mut buf = [0u8; 1];
    assert_matches!(
        client.read(&mut buf),
        Err(err) => assert_eq!(err.kind(), ErrorKind::ConnectionReset)
    );
}

#[netstack_test]
#[variant(I, Ip)]
async fn control_disconnect_ip_tcp_connected<I: Ip>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let control =
        realm.connect_to_protocol::<fnet_sockets::ControlMarker>().expect("connect to protocol");

    let local_addr = std::net::SocketAddr::new(I::LOOPBACK_ADDRESS.to_ip_addr().into(), LOCAL_PORT);
    let domain = match I::VERSION {
        IpVersion::V4 => fposix_socket::Domain::Ipv4,
        IpVersion::V6 => fposix_socket::Domain::Ipv6,
    };

    let listener =
        realm.stream_socket(domain, fposix_socket::StreamSocketProtocol::Tcp).await.unwrap();
    listener.bind(&local_addr.into()).unwrap();
    listener.listen(LISTEN_BACKLOG).expect("listen");

    let client =
        realm.stream_socket(domain, fposix_socket::StreamSocketProtocol::Tcp).await.unwrap();
    client.connect(&local_addr.into()).expect("connect");
    let _server_socket = listener.accept().expect("accept");

    let result = control
        .disconnect_ip(&fnet_sockets::ControlDisconnectIpRequest {
            matchers: Some(vec![
                fnet_sockets::IpSocketMatcher::Proto(fnet_matchers::SocketTransportProtocol::Tcp(
                    fnet_matchers::TcpSocket::SrcPort(fnet_matchers::BoundPort::Bound(
                        fnet_matchers::Port { start: LOCAL_PORT, end: LOCAL_PORT, invert: false },
                    )),
                )),
                fnet_sockets::IpSocketMatcher::Proto(fnet_matchers::SocketTransportProtocol::Tcp(
                    fnet_matchers::TcpSocket::States(fnet_matchers::TcpState::ESTABLISHED),
                )),
            ]),
            ..Default::default()
        })
        .await
        .expect("call disconnect_ip");
    assert_matches!(
        result,
        fnet_sockets::DisconnectIpResult::Ok(fnet_sockets::DisconnectIpResponse {
            disconnected: 1
        })
    );

    let mut client = std::net::TcpStream::from(client);
    let mut buf = [0u8; 1];
    assert_matches!(
        client.read(&mut buf),
        Err(err) => assert_eq!(err.kind(), ErrorKind::ConnectionReset)
    );
}

#[netstack_test]
#[variant(I, Ip)]
async fn control_disconnect_ip_udp_connected<I: Ip>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let control =
        realm.connect_to_protocol::<fnet_sockets::ControlMarker>().expect("connect to protocol");

    let local_addr = std::net::SocketAddr::new(I::LOOPBACK_ADDRESS.to_ip_addr().into(), LOCAL_PORT);
    let domain = match I::VERSION {
        IpVersion::V4 => fposix_socket::Domain::Ipv4,
        IpVersion::V6 => fposix_socket::Domain::Ipv6,
    };

    let mut socket =
        realm.datagram_socket(domain, fposix_socket::DatagramSocketProtocol::Udp).await.unwrap();
    socket.bind(&local_addr.into()).unwrap();
    socket.connect(&local_addr.into()).unwrap();

    let result = control
        .disconnect_ip(&fnet_sockets::ControlDisconnectIpRequest {
            matchers: Some(vec![fnet_sockets::IpSocketMatcher::Proto(
                fnet_matchers::SocketTransportProtocol::Udp(fnet_matchers::UdpSocket::SrcPort(
                    fnet_matchers::BoundPort::Bound(fnet_matchers::Port {
                        start: LOCAL_PORT,
                        end: LOCAL_PORT,
                        invert: false,
                    }),
                )),
            )]),
            ..Default::default()
        })
        .await
        .expect("call disconnect_ip");

    assert_matches!(
        result,
        fnet_sockets::DisconnectIpResult::Ok(fnet_sockets::DisconnectIpResponse {
            disconnected: 1
        })
    );

    let mut buf = [0; 1];
    let res = socket.read(&mut buf);
    assert!(res.is_err());
    assert_eq!(res.unwrap_err().raw_os_error(), Some(libc::ECONNABORTED));

    let err = socket.take_error().unwrap();
    assert!(err.is_none());

    let buf = [0u8; 1];
    let res = socket.send(&buf);
    assert!(res.is_err());
    assert_eq!(res.unwrap_err().raw_os_error(), Some(libc::EDESTADDRREQ));
}

#[netstack_test]
#[variant(I, Ip)]
async fn diagnostics_tcp_info<I: Ip>(name: &str) {
    const SLEEP_MS: u64 = 100;

    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    let local_addr = std::net::SocketAddr::new(I::LOOPBACK_ADDRESS.to_ip_addr().into(), 0);
    let domain = match I::VERSION {
        IpVersion::V4 => fposix_socket::Domain::Ipv4,
        IpVersion::V6 => fposix_socket::Domain::Ipv6,
    };

    let listener =
        realm.stream_socket(domain, fposix_socket::StreamSocketProtocol::Tcp).await.unwrap();
    listener.bind(&local_addr.into()).expect("bind");
    let listener_addr = listener.local_addr().expect("local_addr");
    listener.listen(LISTEN_BACKLOG).expect("listen");

    let mut client =
        realm.stream_socket(domain, fposix_socket::StreamSocketProtocol::Tcp).await.unwrap();
    client.connect(&listener_addr.into()).expect("connect");
    let client_addr = client.local_addr().expect("local_addr");

    let (mut accepted, _accepted_addr) = listener.accept().expect("accept");

    let payload = b"hello";
    client.write_all(payload).expect("write_all");
    let mut buf = [0u8; 5];
    accepted.read_exact(&mut buf).expect("read_exact");
    assert_eq!(&buf, payload);

    // Sending data allows us to wait for the network to quiesce.
    accepted.write_all(payload).expect("write_all server");
    client.read_exact(&mut buf).expect("read_exact client");
    assert_eq!(&buf, payload);

    let diagnostics = realm
        .connect_to_protocol::<fnet_sockets::DiagnosticsMarker>()
        .expect("connect to protocol");

    let client_addr = client_addr.as_socket().expect("as_socket");
    let port = client_addr.port();
    let addr = net_types::ip::IpAddr::from(client_addr.ip());
    let prefix_len = I::Addr::BYTES * 8;

    let matchers = [
        fnet_sockets_ext::IpSocketMatcher::Family(I::VERSION),
        fnet_sockets_ext::IpSocketMatcher::Proto(fnet_matchers_ext::SocketTransportProtocol::Tcp(
            fnet_matchers_ext::TcpSocket::SrcPort(fnet_matchers_ext::BoundPort::Bound(
                fnet_matchers_ext::Port::new(port, port, false).unwrap(),
            )),
        )),
        fnet_sockets_ext::IpSocketMatcher::SrcAddr(fnet_matchers_ext::BoundAddress::Bound(
            fnet_matchers_ext::Address {
                matcher: fnet_matchers_ext::AddressMatcherType::Subnet(
                    fnet_matchers_ext::Subnet::try_from(fnet::Subnet {
                        addr: addr.into_ext(),
                        prefix_len,
                    })
                    .unwrap(),
                ),
                invert: false,
            },
        )),
    ];

    let sockets: Vec<fnet_sockets_ext::IpSocketState> = fnet_sockets_ext::iterate_ip(
        &diagnostics,
        fnet_sockets::Extensions::TCP_INFO,
        matchers.clone(),
    )
    .await
    .unwrap()
    .try_collect()
    .await
    .unwrap();

    // Capture last_data_sent and last_ack_recv for comparison after a sleep.
    let (last_data_sent, last_ack_recv) = {
        let socket = assert_matches!(&sockets[..], [socket] => socket.clone());
        let fnet_sockets_ext::TcpInfo {
            state,
            ca_state,
            rto_usec,
            tcpi_last_data_sent_msec,
            tcpi_last_ack_recv_msec,
            rtt_usec,
            rtt_var_usec,
            snd_ssthresh,
            snd_cwnd,
            // NB: There's no valuable check to perform against the number of
            // retransmits. Retransmissions may or may not have occurred during
            // the handshake.
            tcpi_total_retrans: _,
            tcpi_segs_out,
            tcpi_segs_in,
            reorder_seen,
            tcpi_snd_mss,
            tcpi_rcv_mss,
        } = {
            let transport = match socket {
                fnet_sockets_ext::IpSocketState::V4(state) => {
                    assert_matches!(
                        state.transport,
                        fnet_sockets_ext::IpSocketTransportState::Tcp(transport)
                            => transport
                    )
                }
                fnet_sockets_ext::IpSocketState::V6(state) => {
                    assert_matches!(
                        state.transport,
                        fnet_sockets_ext::IpSocketTransportState::Tcp(transport)
                            => transport
                    )
                }
            };

            assert_matches!(
                transport,
                fnet_sockets_ext::IpSocketTcpState { tcp_info: Some(tcp_info), .. }
                    => tcp_info
            )
        };

        // We can't verify exact values for RTT/variances, but we can at least
        // verify that they're present.
        assert_matches!(rto_usec, Some(rto_usec) => assert_gt!(rto_usec, 0));
        assert_matches!(rtt_usec, Some(_));
        assert_matches!(rtt_var_usec, Some(_));
        assert_gt!(snd_ssthresh, 0);
        assert_gt!(snd_cwnd, 0);
        assert_gt!(tcpi_segs_out, 0);
        assert_gt!(tcpi_segs_in, 0);
        assert_eq!(state, fnet_tcp::State::Established);
        assert_eq!(ca_state, fnet_tcp::CongestionControlState::Open);
        assert_eq!(reorder_seen, false);
        assert_matches!(tcpi_snd_mss, Some(tcpi_snd_mss) => assert_gt!(tcpi_snd_mss, 0));
        assert_matches!(tcpi_rcv_mss, Some(tcpi_rcv_mss) => assert_gt!(tcpi_rcv_mss, 0));
        (
            assert_matches!(
                tcpi_last_data_sent_msec,
                Some(tcpi_last_data_sent_msec)
                    => tcpi_last_data_sent_msec
            ),
            assert_matches!(
                tcpi_last_ack_recv_msec,
                Some(tcpi_last_ack_recv_msec)
                    => tcpi_last_ack_recv_msec
            ),
        )
    };

    // Sleep for a bit to ensure the timestamps increase. SLEEP_MS is arbitrary
    // but plenty long enough to be measurable (1ms granularity).
    fasync::Timer::new(Duration::from_millis(SLEEP_MS)).await;

    // We don't send any more data, the "time since last X" metrics will increase
    // by at least the sleep duration.

    let sockets: Vec<fnet_sockets_ext::IpSocketState> =
        fnet_sockets_ext::iterate_ip(&diagnostics, fnet_sockets::Extensions::TCP_INFO, matchers)
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap();

    let socket = assert_matches!(&sockets[..], [socket] => socket.clone());
    let fnet_sockets_ext::TcpInfo { tcpi_last_data_sent_msec, tcpi_last_ack_recv_msec, .. } = {
        let transport = match socket {
            fnet_sockets_ext::IpSocketState::V4(state) => {
                assert_matches!(
                    state.transport,
                    fnet_sockets_ext::IpSocketTransportState::Tcp(transport)
                        => transport
                )
            }
            fnet_sockets_ext::IpSocketState::V6(state) => {
                assert_matches!(
                    state.transport,
                    fnet_sockets_ext::IpSocketTransportState::Tcp(transport)
                        => transport
                )
            }
        };

        assert_matches!(
            transport,
            fnet_sockets_ext::IpSocketTcpState { tcp_info: Some(tcp_info), .. }
                => tcp_info
        )
    };

    assert_matches!(
        tcpi_last_data_sent_msec,
        Some(tcpi_last_data_sent_msec)
            => assert_geq!(
                tcpi_last_data_sent_msec,
                last_data_sent + u32::try_from(SLEEP_MS).unwrap()
    ));
    assert_matches!(
        tcpi_last_ack_recv_msec,
        Some(tcpi_last_ack_recv_msec)
            => assert_geq!(
                tcpi_last_ack_recv_msec,
                last_ack_recv + u32::try_from(SLEEP_MS).unwrap()
    ));
}

#[netstack_test]
#[variant(I, Ip)]
#[test_case(Protocol::Udp)]
#[test_case(Protocol::Tcp)]
async fn diagnostics_recent_destructions<I: Ip>(name: &str, protocol: Protocol) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    let local_addr = std::net::SocketAddr::new(I::LOOPBACK_ADDRESS.to_ip_addr().into(), LOCAL_PORT);
    let domain = match I::VERSION {
        IpVersion::V4 => fposix_socket::Domain::Ipv4,
        IpVersion::V6 => fposix_socket::Domain::Ipv6,
    };

    let diagnostics = realm
        .connect_to_protocol::<fnet_sockets::DiagnosticsMarker>()
        .expect("connect to protocol");
    let (watcher, server_end) =
        fidl::endpoints::create_proxy::<fnet_sockets::DestructionWatcherMarker>();
    diagnostics
        .get_destruction_watcher(server_end)
        .await
        .expect("get_destruction_watcher should succeed");

    // Create an unbound socket and immediately close/drop it. We don't expect
    // to get a notification for this.
    {
        let _unbound = match protocol {
            Protocol::Udp => realm
                .datagram_socket(domain, fposix_socket::DatagramSocketProtocol::Udp)
                .await
                .expect("UDP socket creation should succeed"),
            Protocol::Tcp => realm
                .stream_socket(domain, fposix_socket::StreamSocketProtocol::Tcp)
                .await
                .expect("TCP socket creation should succeed"),
        };
    }

    let cookie = {
        let socket = match protocol {
            Protocol::Udp => {
                let socket = realm
                    .datagram_socket(domain, fposix_socket::DatagramSocketProtocol::Udp)
                    .await
                    .expect("UDP socket creation should succeed");
                socket.bind(&local_addr.into()).expect("bind should succeed");
                socket
            }
            Protocol::Tcp => {
                let socket = realm
                    .stream_socket(domain, fposix_socket::StreamSocketProtocol::Tcp)
                    .await
                    .expect("TCP socket creation should succeed");
                socket.bind(&local_addr.into()).expect("bind should succeed");
                socket.listen(LISTEN_BACKLOG).expect("listen should succeed");
                socket
            }
        };
        let proxy = socket_proxy(&socket).await;
        proxy.get_cookie().await.expect("failed to get cookie").expect("get_cookie should succeed")
    };
    // Socket is dropped here.

    let (sockets, dropped_events) = watcher.watch().await.expect("watch should succeed");

    assert_eq!(sockets.len(), 1);
    assert_eq!(dropped_events, 0);
    let socket = &sockets[0];
    assert_eq!(socket.cookie, Some(cookie));

    let transport = socket.transport.as_ref().expect("transport is missing");
    match transport {
        fnet_sockets::IpSocketTransportState::Tcp(tcp) => {
            assert_eq!(protocol, Protocol::Tcp);
            let tcp_info = tcp.tcp_info.as_ref().expect("tcp_info is missing");
            assert_eq!(tcp_info.state, Some(fnet_tcp::State::Close));
        }
        fnet_sockets::IpSocketTransportState::Udp(udp) => {
            assert_eq!(protocol, Protocol::Udp);
            assert_eq!(udp.state, Some(fnet_udp::State::Bound));
        }
        _ => panic!("unexpected transport state: {transport:?}"),
    }
}

#[netstack_test]
async fn diagnostics_recent_destructions_concurrent_watch(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    let diagnostics = realm
        .connect_to_protocol::<fnet_sockets::DiagnosticsMarker>()
        .expect("connect to protocol");
    let (watcher, server_end) =
        fidl::endpoints::create_proxy::<fnet_sockets::DestructionWatcherMarker>();
    diagnostics
        .get_destruction_watcher(server_end)
        .await
        .expect("get_destruction_watcher should succeed");

    let first_watch = watcher.watch();
    let second_watch = watcher.watch();

    assert_matches!(
        futures::join!(first_watch, second_watch),
        (
            Err(fidl::Error::ClientChannelClosed { epitaph: epitaph1, .. }),
            Err(fidl::Error::ClientChannelClosed { epitaph: epitaph2, .. }),
        ) if epitaph1 == zx::Status::ALREADY_EXISTS && epitaph2 == zx::Status::ALREADY_EXISTS
    );

    assert_eq!(watcher.on_closed().await, Ok(zx::Signals::CHANNEL_PEER_CLOSED));
}

#[netstack_test]
#[variant(I, Ip)]
async fn diagnostics_recent_destructions_queue_limit<I: Ip>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    let local_addr = std::net::SocketAddr::new(I::LOOPBACK_ADDRESS.to_ip_addr().into(), 0);
    let domain = match I::VERSION {
        IpVersion::V4 => fposix_socket::Domain::Ipv4,
        IpVersion::V6 => fposix_socket::Domain::Ipv6,
    };

    let diagnostics = realm
        .connect_to_protocol::<fnet_sockets::DiagnosticsMarker>()
        .expect("connect to protocol");
    let (watcher, server_end) =
        fidl::endpoints::create_proxy::<fnet_sockets::DestructionWatcherMarker>();
    diagnostics
        .get_destruction_watcher(server_end)
        .await
        .expect("get_destruction_watcher should succeed");

    let queue_capacity = usize::try_from(fnet_sockets::MAX_IP_SOCKET_BATCH_SIZE).unwrap() * 5;
    // Overflow by a couple to make sure we know how to count properly.
    for _ in 0..(queue_capacity + 2) {
        let socket = realm
            .datagram_socket(domain, fposix_socket::DatagramSocketProtocol::Udp)
            .await
            .expect("UDP socket creation should succeed");
        socket.bind(&local_addr.into()).expect("bind should succeed");
    }

    // We get a full batch, but are also signalled of dropped events.
    let (sockets, dropped_events) = watcher.watch().await.expect("watch should succeed");
    assert_eq!(sockets.len(), usize::try_from(fnet_sockets::MAX_IP_SOCKET_BATCH_SIZE).unwrap());
    assert_eq!(dropped_events, 2);

    // Still another full batch, but the dropped signal is cleared.
    let (sockets, dropped_events) = watcher.watch().await.expect("watch should succeed");
    assert_eq!(sockets.len(), usize::try_from(fnet_sockets::MAX_IP_SOCKET_BATCH_SIZE).unwrap());
    assert_eq!(dropped_events, 0);
}
