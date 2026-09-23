// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

use std::fmt::Debug;
use std::marker::PhantomData;
use std::num::NonZeroU16;
use std::os::fd::AsRawFd as _;
use std::time::Duration;

use assert_matches::assert_matches;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_ext as fnet_ext;
use fidl_fuchsia_net_ext::IntoExt as _;
use fidl_fuchsia_net_interfaces_admin as fnet_interfaces_admin;
use fidl_fuchsia_posix_socket as fposix_socket;
use fuchsia_async::{self as fasync, DurationExt, TimeoutExt as _};
use futures::future::{self, LocalBoxFuture};
use futures::io::{AsyncReadExt as _, AsyncWriteExt as _};
use futures::{Future, FutureExt as _, StreamExt as _};
use heck::ToSnakeCase as _;
use net_declare::net_subnet_v4;
use net_types::ip::{Ip, IpAddress as _, IpVersion, Ipv4, Ipv6};
use netemul::{RealmTcpListener as _, RealmTcpStream as _};
use netstack_testing_common::ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT;
use netstack_testing_common::interfaces::TestInterfaceExt as _;
use netstack_testing_common::realms::{Netstack, Netstack3, NetstackVersion, TestSandboxExt as _};
use netstack_testing_macros::netstack_test;
use packet::{
    NestableSerializer as _, NoOpSerializationContext, ParsablePacket as _, Serializer as _,
};
use packet_formats::ethernet::{
    ETHERNET_MIN_BODY_LEN_NO_TAG, EtherType, EthernetFrame, EthernetFrameBuilder,
    EthernetFrameLengthCheck,
};
use packet_formats::icmp::{
    IcmpDestUnreachable, IcmpMessage, IcmpPacketBuilder, IcmpTimeExceeded, IcmpZeroCode,
    Icmpv4DestUnreachableCode, Icmpv4ParameterProblem, Icmpv4ParameterProblemCode,
    Icmpv4TimeExceededCode, Icmpv6DestUnreachableCode, Icmpv6PacketTooBig, Icmpv6ParameterProblem,
    Icmpv6ParameterProblemCode, Icmpv6TimeExceededCode,
};
use packet_formats::ip::{IpPacketBuilder, IpProto, Ipv4Proto, Ipv6Proto};
use packet_formats::tcp::options::TcpOptionsBuilder;
use packet_formats::tcp::{
    TcpParseArgs, TcpSegment, TcpSegmentBuilder, TcpSegmentBuilderWithOptions,
};
use socket2::SockRef;
use test_case::test_case;
use test_util::assert_gt;

use crate::{
    CLIENT_MAC, CLIENT_SUBNET, Interface, MakeSocket as _, MultiNicAndPeerConfig, Network,
    SERVER_MAC, SERVER_SUBNET, TcpSocket, TestIpExt,
};

pub(super) async fn run_tcp_socket_test(
    server: &netemul::TestRealm<'_>,
    server_addr: fnet::IpAddress,
    client: &netemul::TestRealm<'_>,
    client_addr: fnet::IpAddress,
) {
    let fnet_ext::IpAddress(client_addr) = client_addr.into();
    let client_addr = std::net::SocketAddr::new(client_addr, 1234);

    let fnet_ext::IpAddress(server_addr) = server_addr.into();
    let server_addr = std::net::SocketAddr::new(server_addr, 8080);

    // We pick a payload that is small enough to be guaranteed to fit in a TCP segment so both the
    // client and server can read the entire payload in a single `read`.
    const PAYLOAD: &'static str = "Hello World";

    let listener = fasync::net::TcpListener::listen_in_realm(server, server_addr)
        .await
        .expect("failed to create server socket");

    let server_fut = async {
        let (_, mut stream, from) = listener.accept().await.expect("accept failed");

        let mut buf = [0u8; 1024];
        let read_count = stream.read(&mut buf).await.expect("read from tcp server stream failed");

        // Unspecified addresses will use loopback as their source
        if client_addr.ip().is_unspecified() {
            assert!(from.ip().is_loopback())
        } else {
            assert_eq!(from.ip(), client_addr.ip());
        }
        assert_eq!(read_count, PAYLOAD.as_bytes().len());
        assert_eq!(&buf[..read_count], PAYLOAD.as_bytes());

        let write_count =
            stream.write(PAYLOAD.as_bytes()).await.expect("write to tcp server stream failed");
        assert_eq!(write_count, PAYLOAD.as_bytes().len());
    };

    let client_fut = async {
        let mut stream = fasync::net::TcpStream::connect_in_realm(client, server_addr)
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

// Note: This methods returns the two end of the established connection through
// a continuation, this is if we return them directly, the endpoints created
// inside the function will be dropped so no packets can be possibly sent and
// ultimately fail the tests. Using a closure allows us to execute the rest of
// test within the context where the endpoints are still alive.
async fn tcp_socket_accept_cross_ns<
    I: TestIpExt,
    Client: Netstack,
    Server: Netstack,
    Fut: Future,
    F: FnOnce(fasync::net::TcpStream, fasync::net::TcpStream) -> Fut,
>(
    name: &str,
    f: F,
) -> Fut::Output {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let net = sandbox.create_network("net").await.expect("failed to create network");

    let _packet_capture = net.start_capture(name).await.expect("starting packet capture");
    let client = sandbox
        .create_netstack_realm::<Client, _>(format!("{}_client", name))
        .expect("failed to create client realm");
    let client_interface =
        client.join_network(&net, "client-ep").await.expect("failed to join network in realm");
    client_interface
        .add_address_and_subnet_route(I::CLIENT_SUBNET)
        .await
        .expect("configure address");
    client_interface.apply_nud_flake_workaround().await.expect("nud flake workaround");

    let server = sandbox
        .create_netstack_realm::<Server, _>(format!("{}_server", name))
        .expect("failed to create server realm");
    let server_interface =
        server.join_network(&net, "server-ep").await.expect("failed to join network in realm");
    server_interface
        .add_address_and_subnet_route(I::SERVER_SUBNET)
        .await
        .expect("configure address");
    server_interface.apply_nud_flake_workaround().await.expect("nud flake workaround");

    let fnet_ext::IpAddress(client_ip) = I::CLIENT_SUBNET.addr.into();

    let fnet_ext::IpAddress(server_ip) = I::SERVER_SUBNET.addr.into();
    let server_addr = std::net::SocketAddr::new(server_ip, 8080);

    let listener = fasync::net::TcpListener::listen_in_realm(&server, server_addr)
        .await
        .expect("failed to create server socket");

    let client = fasync::net::TcpStream::connect_in_realm(&client, server_addr)
        .await
        .expect("failed to create client socket");

    let (_, accepted, from) = listener.accept().await.expect("accept failed");
    assert_eq!(from.ip(), client_ip);

    f(client, accepted).await
}

#[netstack_test]
#[variant(I, Ip)]
#[variant(Client, Netstack)]
#[variant(Server, Netstack)]
async fn tcp_socket_accept<I: TestIpExt, Client: Netstack, Server: Netstack>(name: &str) {
    tcp_socket_accept_cross_ns::<I, Client, Server, _, _>(name, |_client, _server| async {}).await
}

#[netstack_test]
#[variant(I, Ip)]
#[variant(Client, Netstack)]
#[variant(Server, Netstack)]
async fn tcp_socket_send_recv<I: TestIpExt, Client: Netstack, Server: Netstack>(name: &str) {
    async fn send_recv(mut sender: fasync::net::TcpStream, mut receiver: fasync::net::TcpStream) {
        const PAYLOAD: &'static [u8] = b"Hello World";
        let write_count = sender.write(PAYLOAD).await.expect("write to tcp client stream failed");
        assert_matches!(sender.close().await, Ok(()));

        assert_eq!(write_count, PAYLOAD.len());
        let mut buf = [0u8; 16];
        let read_count = receiver.read(&mut buf).await.expect("read from tcp server stream failed");
        assert_eq!(read_count, write_count);
        assert_eq!(&buf[..read_count], PAYLOAD);

        // Echo the bytes back the already closed sender, the sender is already
        // closed and it should not cause any panic.
        assert_eq!(
            receiver.write(&buf[..read_count]).await.expect("write to tcp server stream failed"),
            read_count
        );
    }
    tcp_socket_accept_cross_ns::<I, Client, Server, _, _>(name, send_recv).await
}

#[netstack_test]
#[variant(I, Ip)]
#[variant(Client, Netstack)]
#[variant(Server, Netstack)]
async fn tcp_socket_shutdown_connection<I: TestIpExt, Client: Netstack, Server: Netstack>(
    name: &str,
) {
    tcp_socket_accept_cross_ns::<I, Client, Server, _, _>(
        name,
        |mut client: fasync::net::TcpStream, mut server: fasync::net::TcpStream| async move {
            client.shutdown(std::net::Shutdown::Both).expect("failed to shutdown the client");
            assert_eq!(
                client.write(b"Hello").await.map_err(|e| e.kind()),
                Err(std::io::ErrorKind::BrokenPipe)
            );
            assert_matches!(server.read_to_end(&mut Vec::new()).await, Ok(0));
            server.shutdown(std::net::Shutdown::Both).expect("failed to shutdown the server");
            assert_eq!(
                server.write(b"Hello").await.map_err(|e| e.kind()),
                Err(std::io::ErrorKind::BrokenPipe)
            );
            assert_matches!(client.read_to_end(&mut Vec::new()).await, Ok(0));
        },
    )
    .await
}

// Shutting down one end of the socket in both directions should cause writes to fail on the
// other end. Same applies when closing the socket, (`close()` implies `shutdown(RDWR)`).
#[netstack_test]
#[variant(I, Ip)]
#[variant(Client, Netstack)]
#[variant(Server, Netstack)]
#[test_case(false; "shutdown")]
#[test_case(true; "close")]
async fn tcp_socket_send_after_shutdown<I: TestIpExt, Client: Netstack, Server: Netstack>(
    name: &str,
    close: bool,
) {
    tcp_socket_accept_cross_ns::<I, Client, Server, _, _>(
        name,
        |mut client: fasync::net::TcpStream, server: fasync::net::TcpStream| async move {
            // Either close or shutdown the server end of the socket.
            let _server = if close {
                std::mem::drop(server);
                None
            } else {
                server.shutdown(std::net::Shutdown::Both).expect("Failed to shutdown TCP read");
                Some(server)
            };

            async {
                // Keep writing until we get an error.
                loop {
                    if let Err(e) = client.write(b"Hello").await {
                        // NS2 returns EPIPE, which is incorrect. Check the error only with NS3.
                        if !matches!(
                            Client::VERSION,
                            NetstackVersion::Netstack2 { .. } | NetstackVersion::ProdNetstack2
                        ) {
                            assert_eq!(e.kind(), std::io::ErrorKind::ConnectionReset);
                        }
                        break;
                    }
                }
            }
            .on_timeout(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT.after_now(), || {
                panic!("timed out waiting for error from send()")
            })
            .await;
        },
    )
    .await
}

#[netstack_test]
#[variant(I, Ip)]
#[variant(N, Netstack)]
async fn tcp_socket_shutdown_listener<I: TestIpExt, N: Netstack>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let net = sandbox.create_network("net").await.expect("failed to create network");

    let client = sandbox
        .create_netstack_realm::<N, _>(format!("{}_client", name))
        .expect("failed to create client realm");
    let client_interface =
        client.join_network(&net, "client-ep").await.expect("failed to join network in realm");
    client_interface
        .add_address_and_subnet_route(I::CLIENT_SUBNET)
        .await
        .expect("configure address");
    client_interface.apply_nud_flake_workaround().await.expect("nud flake workaround");

    let server = sandbox
        .create_netstack_realm::<N, _>(format!("{}_server", name))
        .expect("failed to create server realm");
    let server_interface =
        server.join_network(&net, "server-ep").await.expect("failed to join network in realm");
    server_interface
        .add_address_and_subnet_route(I::SERVER_SUBNET)
        .await
        .expect("configure address");
    server_interface.apply_nud_flake_workaround().await.expect("nud flake workaround");

    let fnet_ext::IpAddress(client_ip) = I::CLIENT_SUBNET.addr.into();
    let fnet_ext::IpAddress(server_ip) = I::SERVER_SUBNET.addr.into();
    let client_addr = std::net::SocketAddr::new(client_ip, 8080);
    let server_addr = std::net::SocketAddr::new(server_ip, 8080);

    // Create listener sockets on both netstacks and shut them down.
    let client = socket2::Socket::from(
        std::net::TcpListener::listen_in_realm(&client, client_addr)
            .await
            .expect("failed to create the client socket"),
    );
    assert_matches!(client.shutdown(std::net::Shutdown::Both), Ok(()));

    let server = socket2::Socket::from(
        std::net::TcpListener::listen_in_realm(&server, server_addr)
            .await
            .expect("failed to create the server socket"),
    );

    assert_matches!(server.shutdown(std::net::Shutdown::Both), Ok(()));

    // Listen again on the server socket.
    assert_matches!(server.listen(1), Ok(()));
    let server = fasync::net::TcpListener::from_std(server.into()).unwrap();

    // Call connect on the client socket.
    let _client = fasync::net::TcpStream::connect_from_raw(client, server_addr)
        .expect("failed to connect client socket")
        .await;

    // Both should succeed and we have an established connection.
    let (_, _accepted, from) = server.accept().await.expect("accept failed");
    let fnet_ext::IpAddress(client_ip) = I::CLIENT_SUBNET.addr.into();
    assert_eq!(from.ip(), client_ip);
}

#[netstack_test]
#[variant(N, Netstack)]
async fn tcpv4_tcpv6_listeners_coexist<N: Netstack>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let net = sandbox.create_network("net").await.expect("failed to create network");

    let host = sandbox.create_netstack_realm::<N, _>(name).expect("failed to create server realm");
    let interface =
        host.join_network(&net, "server-ep").await.expect("failed to join network in realm");
    interface
        .add_address_and_subnet_route(Ipv4::SERVER_SUBNET)
        .await
        .expect("failed to add v4 addr");
    interface
        .add_address_and_subnet_route(Ipv6::SERVER_SUBNET)
        .await
        .expect("failed to add v6 addr");

    let fnet_ext::IpAddress(v4_addr) = Ipv4::SERVER_SUBNET.addr.into();
    let fnet_ext::IpAddress(v6_addr) = Ipv6::SERVER_SUBNET.addr.into();
    let v4_addr = std::net::SocketAddr::new(v4_addr, 8080);
    let v6_addr = std::net::SocketAddr::new(v6_addr, 8080);
    let _listener_v4 = fasync::net::TcpListener::listen_in_realm(&host, v4_addr)
        .await
        .expect("failed to create v4 socket");
    let _listener_v6 = fasync::net::TcpListener::listen_in_realm(&host, v6_addr)
        .await
        .expect("failed to create v6 socket");
}

#[netstack_test]
#[variant(N, Netstack)]
#[variant(I, Ip)]
#[test_case(100; "large positive")]
#[test_case(1; "min positive")]
#[test_case(0; "zero")]
#[test_case(-1; "negative")]
async fn tcp_socket_listen<N: Netstack, I: TestIpExt>(name: &str, backlog: i16) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");

    let host = sandbox
        .create_netstack_realm::<N, _>(format!("{}host", name))
        .expect("failed to create realm");

    const PORT: u16 = 8080;

    let listener = {
        let socket = host
            .stream_socket(I::DOMAIN, fposix_socket::StreamSocketProtocol::Tcp)
            .await
            .expect("create TCP socket");
        socket
            .bind(&std::net::SocketAddr::from((I::UNSPECIFIED_ADDRESS.to_ip_addr(), PORT)).into())
            .expect("no conflict");

        // Listen with the provided backlog value.
        socket
            .listen(backlog.into())
            .unwrap_or_else(|_| panic!("backlog of {} is accepted", backlog));
        fasync::net::TcpListener::from_std(socket.into()).expect("is TCP listener")
    };

    let mut conn = fasync::net::TcpStream::connect_in_realm(
        &host,
        (I::LOOPBACK_ADDRESS.to_ip_addr(), PORT).into(),
    )
    .await
    .expect("should be accepted");

    let (_, mut served, _): (fasync::net::TcpListener, _, std::net::SocketAddr) =
        listener.accept().await.expect("connection waiting");

    // Confirm that the connection is working.
    const NUM_BYTES: u8 = 10;
    let written = Vec::from_iter(0..NUM_BYTES);
    served.write_all(written.as_slice()).await.expect("write succeeds");
    let mut read = [0; NUM_BYTES as usize];
    conn.read_exact(&mut read).await.expect("read finished");
    assert_eq!(&read, written.as_slice());
}

#[netstack_test]
#[variant(N, Netstack)]
async fn tcp_socket<N: Netstack>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let net = sandbox.create_network("net").await.expect("failed to create network");

    let client = sandbox
        .create_netstack_realm::<N, _>(format!("{}_client", name))
        .expect("failed to create client realm");
    let client_ep = client
        .join_network_with(
            &net,
            "client",
            netemul::new_endpoint_config(netemul::DEFAULT_MTU, Some(CLIENT_MAC)),
            Default::default(),
        )
        .await
        .expect("client failed to join network");
    client_ep.add_address_and_subnet_route(CLIENT_SUBNET).await.expect("configure address");
    client_ep.apply_nud_flake_workaround().await.expect("nud flake workaround");

    let server = sandbox
        .create_netstack_realm::<N, _>(format!("{}_server", name))
        .expect("failed to create server realm");
    let server_ep = server
        .join_network_with(
            &net,
            "server",
            netemul::new_endpoint_config(netemul::DEFAULT_MTU, Some(SERVER_MAC)),
            Default::default(),
        )
        .await
        .expect("server failed to join network");
    server_ep.add_address_and_subnet_route(SERVER_SUBNET).await.expect("configure address");
    server_ep.apply_nud_flake_workaround().await.expect("nud flake workaround");

    run_tcp_socket_test(&server, SERVER_SUBNET.addr, &client, CLIENT_SUBNET.addr).await
}

// This is a regression test for https://fxbug.dev/361402347.
#[netstack_test]
#[variant(I, Ip)]
#[variant(N, Netstack)]
async fn tcp_bind_listen_on_same_port_different_address<I: TestIpExt, N: Netstack>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let net = sandbox.create_network("net").await.expect("create network");
    let netstack =
        sandbox.create_netstack_realm::<N, _>(format!("{}", name)).expect("create netstack realm");
    let interface = netstack.join_network(&net, "ep").await.expect("join network");
    interface.add_address_and_subnet_route(I::CLIENT_SUBNET).await.expect("configure address");
    interface.add_address_and_subnet_route(I::SERVER_SUBNET).await.expect("configure address");

    const PORT: u16 = 80;

    let first = TcpSocket::new_in_realm::<I>(&netstack).await.expect("create TCP socket");
    let fnet_ext::IpAddress(addr) = I::CLIENT_SUBNET.addr.into();
    first.bind(&std::net::SocketAddr::new(addr, PORT).into()).expect("no conflict");
    first.listen(0).expect("no conflict");

    let second = TcpSocket::new_in_realm::<I>(&netstack).await.expect("create TCP socket");
    let fnet_ext::IpAddress(addr) = I::SERVER_SUBNET.addr.into();
    second.bind(&std::net::SocketAddr::new(addr, PORT).into()).expect("no conflict");
    second.listen(0).expect("no conflict");
}

enum WhichEnd {
    Send,
    Receive,
}

#[netstack_test]
#[variant(I, Ip)]
#[variant(N, Netstack)]
#[test_case(WhichEnd::Send; "send buffer")]
#[test_case(WhichEnd::Receive; "receive buffer")]
async fn tcp_buffer_size<I: TestIpExt, N: Netstack>(name: &str, which: WhichEnd) {
    tcp_socket_accept_cross_ns::<I, N, N, _, _>(name, |mut sender, mut receiver| async move {
        // Set either the sender SO_SNDBUF or receiver SO_RECVBUF so that a
        // large amount of data can be buffered even if the receiver isn't
        // reading.
        let set_size;
        let size = match which {
            WhichEnd::Send => {
                const SEND_BUFFER_SIZE: usize = 1024 * 1024;
                set_size = SEND_BUFFER_SIZE;
                let sender_ref = SockRef::from(sender.std());
                sender_ref.set_send_buffer_size(SEND_BUFFER_SIZE).expect("set size is infallible");
                sender_ref.send_buffer_size().expect("get size is infallible")
            }
            WhichEnd::Receive => {
                const RECEIVE_BUFFER_SIZE: usize = 128 * 1024;
                let receiver_ref = SockRef::from(receiver.std());
                set_size = RECEIVE_BUFFER_SIZE;
                receiver_ref
                    .set_recv_buffer_size(RECEIVE_BUFFER_SIZE)
                    .expect("set size is infallible");
                receiver_ref.recv_buffer_size().expect("get size is infallible")
            }
        };
        assert!(size >= set_size, "{} >= {}", size, set_size);

        let data = Vec::from_iter((0..set_size).map(|i| i as u8));
        sender.write_all(data.as_slice()).await.expect("all written");
        sender.close().await.expect("close succeeds");

        let mut buf = Vec::with_capacity(set_size);
        let read = receiver.read_to_end(&mut buf).await.expect("all bytes read");
        assert_eq!(read, set_size);
    })
    .await
}

#[netstack_test]
#[variant(I, Ip)]
#[variant(N, Netstack)]
async fn decrease_tcp_sendbuf_size<I: TestIpExt, N: Netstack>(name: &str) {
    // This is a regression test for https://fxbug.dev/42072897. With Netstack3,
    // if a TCP socket had a full send buffer and a decrease of the send buffer
    // size was requested, the new size would not take effect immediately as
    // expected.  Instead, the apparent size (visible via POSIX `getsockopt`
    // with `SO_SNDBUF`) would decrease linearly as data was transferred. This
    // test verifies that this is no longer the case by filling up the send
    // buffer for a TCP socket, requesting a smaller size, then observing the
    // size as the buffer is drained (by transferring to the receiver).
    tcp_socket_accept_cross_ns::<I, N, N, _, _>(name, |mut sender, mut receiver| async move {
        // Fill up the sender and receiver buffers by writing a lot of data.
        const LARGE_BUFFER_SIZE: usize = 1024 * 1024;
        SockRef::from(sender.std()).set_send_buffer_size(LARGE_BUFFER_SIZE).expect("can set");

        let data = vec![b'x'; LARGE_BUFFER_SIZE];
        // Fill up the sending socket's send buffer. Since we can't prevent it
        // from sending data to the receiver, this will also fill up the
        // receiver's receive buffer, which is fine. We do this by writing as
        // much as possible while giving time for the sender to transfer the
        // bytes to the receiver.
        let mut written = 0;
        while sender
            .write_all(data.as_slice())
            .map(|r| {
                r.unwrap();
                true
            })
            .on_timeout(zx::MonotonicDuration::from_seconds(2), || false)
            .await
        {
            written += data.len();
        }

        // Now reduce the size of the send buffer. The apparent size of the send
        // buffer should decrease immediately.
        let sender_ref = SockRef::from(sender.std());
        let size_before = sender_ref.send_buffer_size().unwrap();
        sender_ref.set_send_buffer_size(0).expect("can set");
        let size_after = sender_ref.send_buffer_size().unwrap();
        assert!(size_before > size_after, "{} > {}", size_before, size_after);

        // Read data from the socket so that the the sender can send more.
        // This won't finish until the entire transfer has been received.
        let mut buf = vec![0; LARGE_BUFFER_SIZE];
        let mut read = 0;
        while read < written {
            read += receiver.read(&mut buf).await.expect("can read");
        }

        let sender = SockRef::from(sender.std());
        // Draining all the data from the sender into the receiver shouldn't
        // decrease the sender's apparent send buffer size.
        assert_eq!(sender.send_buffer_size().unwrap(), size_after);
        // Now that the sender's buffer is empty, try setting the size again.
        // This should have no effect!
        sender.set_send_buffer_size(0).expect("can set");
        assert_eq!(sender.send_buffer_size().unwrap(), size_after);
    })
    .await
}

#[netstack_test]
#[variant(N, Netstack)]
async fn tcp_connect_bound_to_device<N: Netstack>(name: &str) {
    const NUM_PEERS: u8 = 2;
    const PORT: u16 = 90;

    async fn connect_to_peer(
        config: MultiNicAndPeerConfig<TcpSocket>,
    ) -> MultiNicAndPeerConfig<fasync::net::TcpStream> {
        let MultiNicAndPeerConfig { multinic_ip, multinic_socket, peer_ip, peer_socket } = config;
        let (TcpSocket(peer_socket), TcpSocket(multinic_socket)) = (peer_socket, multinic_socket);
        peer_socket.listen(1).expect("listen on bound socket");
        let peer_socket =
            fasync::net::TcpListener::from_std(std::net::TcpListener::from(peer_socket))
                .expect("convert socket");

        let multinic_socket =
            fasync::net::TcpStream::connect_from_raw(multinic_socket, (peer_ip, PORT).into())
                .expect("start connect failed")
                .await
                .expect("connect failed");

        let (_peer_listener, peer_socket, ip): (fasync::net::TcpListener, _, _) =
            peer_socket.accept().await.expect("accept failed");

        assert_eq!(ip, (multinic_ip, PORT).into());
        MultiNicAndPeerConfig { multinic_ip, multinic_socket, peer_ip, peer_socket }
    }

    crate::with_multinic_and_peers::<N, TcpSocket, Ipv4, _, _>(
        name,
        NUM_PEERS,
        net_subnet_v4!("192.168.0.0/16").into(),
        PORT,
        |configs| async move {
            let connected_configs = futures::stream::iter(configs)
                .map(connect_to_peer)
                .buffer_unordered(usize::MAX)
                .collect::<Vec<_>>()
                .await;

            futures::stream::iter(connected_configs)
                .enumerate()
                .for_each_concurrent(
                    None,
                    |(
                        i,
                        MultiNicAndPeerConfig {
                            multinic_ip: _,
                            mut multinic_socket,
                            peer_ip: _,
                            mut peer_socket,
                        },
                    )| async move {
                        let message = format!("send number {}", i);
                        futures::stream::iter([&mut multinic_socket, &mut peer_socket])
                            .for_each_concurrent(None, |socket| async {
                                assert_eq!(
                                    socket
                                        .write(message.as_bytes())
                                        .await
                                        .expect("host write succeeds"),
                                    message.len()
                                );

                                let mut buf = vec![0; message.len()];
                                socket.read_exact(&mut buf).await.expect("host read succeeds");
                                assert_eq!(&buf, message.as_bytes());
                            })
                            .await;
                    },
                )
                .await
        },
    )
    .await
}

async fn tcp_communicate_with_remote_with_zone<
    N: Netstack,
    M: for<'a, 's> Fn(
        &'s netemul::TestRealm<'a>,
        &'s Interface<'a, net_types::ip::Ipv6Addr>,
        net_types::ip::Ipv6Addr,
    ) -> LocalBoxFuture<'s, fasync::net::TcpStream>,
>(
    name: &str,
    make_multinic_conn: M,
) {
    const PORT: u16 = 80;
    const NUM_BYTES: usize = 10;

    let make_multinic_conn = &make_multinic_conn;
    crate::with_multinic_and_peer_networks::<N, net_types::ip::Ipv6, _>(
        name,
        2,
        net_types::ip::Ipv6::LINK_LOCAL_UNICAST_SUBNET,
        |networks, multinic, ()| {
            Box::pin(async move {
                let interfaces_and_listeners =
                    future::join_all(networks.iter().map(|network| async move {
                        let Network { peer_realm, peer_interface, _network, multinic_interface } =
                            network;
                        let Interface { iface: _, ip: peer_ip } = peer_interface;
                        let peer_listener = fasync::net::TcpListener::listen_in_realm(
                            peer_realm,
                            (std::net::Ipv6Addr::UNSPECIFIED, PORT).into(),
                        )
                        .await
                        .expect("can listen");
                        (multinic_interface, (peer_listener, *peer_ip))
                    }))
                    .await;

                let _: Vec<()> = future::join_all(interfaces_and_listeners.into_iter().map(
                    |(multinic_interface, (peer_listener, peer_ip))| async move {
                        let mut host_conn =
                            make_multinic_conn(multinic, multinic_interface, peer_ip).await;
                        let id: u8 = multinic_interface.iface.id().try_into().unwrap();
                        let data = [id; NUM_BYTES];
                        let (_peer_listener, mut peer_conn, _) =
                            peer_listener.accept().await.expect("receive connection");
                        host_conn.write_all(&data).await.expect("can send");
                        host_conn.close().await.expect("can close");

                        let mut buf = Vec::with_capacity(data.len());
                        assert_eq!(
                            peer_conn.read_to_end(&mut buf).await.expect("can read"),
                            data.len()
                        );
                        assert_eq!(&buf, &data);
                    },
                ))
                .await;
            })
        },
    )
    .await
}

#[netstack_test]
#[variant(N, Netstack)]
async fn tcp_connect_to_remote_with_zone<N: Netstack>(name: &str) {
    const PORT: u16 = 80;

    tcp_communicate_with_remote_with_zone::<N, _>(name, |realm, interface, peer_ip| {
        Box::pin(async move {
            let Interface { iface: interface, ip: _ } = interface;
            let id: u8 = interface.id().try_into().unwrap();
            fasync::net::TcpStream::connect_in_realm(
                realm,
                std::net::SocketAddrV6::new(peer_ip.clone().into(), PORT, 0, id.into()).into(),
            )
            .await
            .expect("can connect")
        })
    })
    .await
}

#[netstack_test]
#[variant(N, Netstack)]
async fn tcp_bind_with_zone_connect_unzoned<N: Netstack>(name: &str) {
    const PORT: u16 = 80;

    tcp_communicate_with_remote_with_zone::<N, _>(name, |realm, interface, peer_ip| {
        Box::pin(async move {
            let Interface { iface: interface, ip } = interface;
            let id: u8 = interface.id().try_into().unwrap();
            let socket = TcpSocket::new_in_realm::<Ipv6>(realm).await.expect("create TCP socket");
            socket
                .bind(&std::net::SocketAddrV6::new(ip.clone().into(), PORT, 0, id.into()).into())
                .expect("no conflict");
            let remote_addr = std::net::SocketAddrV6::new(peer_ip.clone().into(), PORT, 0, 0);
            fasync::net::TcpStream::connect_from_raw(socket, remote_addr.into())
                .expect("is connected")
                .await
                .expect("connected")
        })
    })
    .await
}

#[netstack_test]
#[variant(N, Netstack)]
#[test_case(
    PhantomData::<Ipv4>, IcmpDestUnreachable::default(),
    Icmpv4DestUnreachableCode::DestNetworkUnreachable => libc::ENETUNREACH
)]
#[test_case(
    PhantomData::<Ipv4>, IcmpDestUnreachable::default(),
    Icmpv4DestUnreachableCode::DestHostUnreachable => libc::EHOSTUNREACH
)]
#[test_case(
    PhantomData::<Ipv4>, IcmpDestUnreachable::default(),
    Icmpv4DestUnreachableCode::DestProtocolUnreachable => libc::ENOPROTOOPT
)]
#[test_case(
    PhantomData::<Ipv4>, IcmpDestUnreachable::default(),
    Icmpv4DestUnreachableCode::DestPortUnreachable => libc::ECONNREFUSED
)]
#[test_case(
    PhantomData::<Ipv4>, IcmpDestUnreachable::default(),
    Icmpv4DestUnreachableCode::SourceRouteFailed => libc::EOPNOTSUPP
)]
#[test_case(
    PhantomData::<Ipv4>, IcmpDestUnreachable::default(),
    Icmpv4DestUnreachableCode::DestNetworkUnknown => libc::ENETUNREACH
)]
#[test_case(
    PhantomData::<Ipv4>, IcmpDestUnreachable::default(),
    Icmpv4DestUnreachableCode::DestHostUnknown => libc::EHOSTDOWN
)]
#[test_case(
    PhantomData::<Ipv4>, IcmpDestUnreachable::default(),
    Icmpv4DestUnreachableCode::SourceHostIsolated => libc::ENONET
)]
#[test_case(
    PhantomData::<Ipv4>, IcmpDestUnreachable::default(),
    Icmpv4DestUnreachableCode::NetworkAdministrativelyProhibited => libc::ENETUNREACH
)]
#[test_case(
    PhantomData::<Ipv4>, IcmpDestUnreachable::default(),
    Icmpv4DestUnreachableCode::HostAdministrativelyProhibited => libc::EHOSTUNREACH
)]
#[test_case(
    PhantomData::<Ipv4>, IcmpDestUnreachable::default(),
    Icmpv4DestUnreachableCode::NetworkUnreachableForToS => libc::ENETUNREACH
)]
#[test_case(
    PhantomData::<Ipv4>, IcmpDestUnreachable::default(),
    Icmpv4DestUnreachableCode::HostUnreachableForToS => libc::EHOSTUNREACH
)]
#[test_case(
    PhantomData::<Ipv4>, IcmpDestUnreachable::default(),
    Icmpv4DestUnreachableCode::CommAdministrativelyProhibited => libc::EHOSTUNREACH
)]
#[test_case(
    PhantomData::<Ipv4>, IcmpDestUnreachable::default(),
    Icmpv4DestUnreachableCode::HostPrecedenceViolation => libc::EHOSTUNREACH
)]
#[test_case(
    PhantomData::<Ipv4>, IcmpDestUnreachable::default(),
    Icmpv4DestUnreachableCode::PrecedenceCutoffInEffect => libc::EHOSTUNREACH
)]
#[test_case(
    PhantomData::<Ipv4>, Icmpv4ParameterProblem::new(0),
    Icmpv4ParameterProblemCode::PointerIndicatesError => libc::EPROTO
)]
#[test_case(
    PhantomData::<Ipv4>, Icmpv4ParameterProblem::new(0),
    Icmpv4ParameterProblemCode::MissingRequiredOption => libc::EPROTO
)]
#[test_case(
    PhantomData::<Ipv4>, Icmpv4ParameterProblem::new(0),
    Icmpv4ParameterProblemCode::BadLength => libc::EPROTO
)]
#[test_case(
    PhantomData::<Ipv4>, IcmpTimeExceeded::default(),
    Icmpv4TimeExceededCode::TtlExpired => libc::EHOSTUNREACH
)]
#[test_case(
    PhantomData::<Ipv4>, IcmpTimeExceeded::default(),
    Icmpv4TimeExceededCode::FragmentReassemblyTimeExceeded => libc::ETIMEDOUT
)]
#[test_case(
    PhantomData::<Ipv6>, IcmpDestUnreachable::default(),
    Icmpv6DestUnreachableCode::NoRoute => libc::ENETUNREACH
)]
#[test_case(
    PhantomData::<Ipv6>, IcmpDestUnreachable::default(),
    Icmpv6DestUnreachableCode::CommAdministrativelyProhibited => libc::EACCES
)]
#[test_case(
    PhantomData::<Ipv6>, IcmpDestUnreachable::default(),
    Icmpv6DestUnreachableCode::BeyondScope => libc::EHOSTUNREACH
)]
#[test_case(
    PhantomData::<Ipv6>, IcmpDestUnreachable::default(),
    Icmpv6DestUnreachableCode::AddrUnreachable => libc::EHOSTUNREACH
)]
#[test_case(
    PhantomData::<Ipv6>, IcmpDestUnreachable::default(),
    Icmpv6DestUnreachableCode::PortUnreachable => libc::ECONNREFUSED
)]
#[test_case(
    PhantomData::<Ipv6>, IcmpDestUnreachable::default(),
    Icmpv6DestUnreachableCode::SrcAddrFailedPolicy => libc::EACCES
)]
#[test_case(
    PhantomData::<Ipv6>, IcmpDestUnreachable::default(),
    Icmpv6DestUnreachableCode::RejectRoute => libc::EACCES
)]
#[test_case(
    PhantomData::<Ipv6>, Icmpv6ParameterProblem::new(0),
    Icmpv6ParameterProblemCode::ErroneousHeaderField => libc::EPROTO
)]
#[test_case(
    PhantomData::<Ipv6>, Icmpv6ParameterProblem::new(0),
    Icmpv6ParameterProblemCode::UnrecognizedNextHeaderType => libc::EPROTO
)]
#[test_case(
    PhantomData::<Ipv6>, Icmpv6ParameterProblem::new(0),
    Icmpv6ParameterProblemCode::UnrecognizedIpv6Option => libc::EPROTO
)]
#[test_case(
    PhantomData::<Ipv6>, IcmpTimeExceeded::default(),
    Icmpv6TimeExceededCode::HopLimitExceeded => libc::EHOSTUNREACH
)]
#[test_case(
    PhantomData::<Ipv6>, IcmpTimeExceeded::default(),
    Icmpv6TimeExceededCode::FragmentReassemblyTimeExceeded => libc::EHOSTUNREACH
)]
async fn tcp_connect_icmp_error<N: Netstack, I: TestIpExt, M: IcmpMessage<I> + Debug>(
    name: &str,
    _ip_version: PhantomData<I>,
    message: M,
    code: M::Code,
) -> i32 {
    use packet::NestableSerializer as _;
    use packet_formats::ip::IpPacket as _;

    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let net = sandbox.create_network("net").await.expect("failed to create network");
    let fake_ep = net.create_fake_endpoint().expect("failed to create fake endpoint");
    let fake_ep = &fake_ep;

    let client = sandbox
        .create_netstack_realm::<N, _>(format!("{name}_{}", format!("{code:?}").to_snake_case()))
        .expect("failed to create client realm");
    let client_interface =
        client.join_network(&net, "client-ep").await.expect("failed to join network in realm");
    client_interface
        .add_address_and_subnet_route(I::CLIENT_SUBNET)
        .await
        .expect("configure address");
    client
        .add_neighbor_entry(client_interface.id(), I::SERVER_SUBNET.addr, SERVER_MAC)
        .await
        .expect("add_neighbor_entry");

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

    let server_addr: net_types::ip::IpAddr = I::SERVER_ADDR.into();
    let server_addr = std::net::SocketAddr::new(server_addr.into(), 8080);

    let connect = async move {
        let error = fasync::net::TcpStream::connect_in_realm(&client, server_addr)
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

// Regression test for https://fxbug.dev/468040882.
//
// Typically we'd write these as a syscall test, but having a TCP connection
// timeout over loopback (the available interface) is not well supported to
// compare with linux. Also there's a slight difference between how
// fuchsia-async and fdio report errors, so it's good to show both here.
#[netstack_test]
#[variant(N, Netstack)]
#[variant(I, Ip)]
#[test_case(true; "sync-block")]
#[test_case(false; "async-nonblock")]
async fn tcp_connect_timeout<N: Netstack, I: TestIpExt>(name: &str, sync: bool) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let net = sandbox.create_network("net").await.expect("failed to create network");
    let realm = sandbox.create_netstack_realm::<N, _>(name).expect("failed to create client realm");
    let client_interface =
        realm.join_network(&net, "client-ep").await.expect("failed to join network in realm");
    client_interface
        .add_address_and_subnet_route(I::CLIENT_SUBNET)
        .await
        .expect("configure address");
    let server_addr: net_types::ip::IpAddr = I::SERVER_ADDR.into();
    let server_addr = std::net::SocketAddr::new(server_addr.into(), 8080);

    let timeout = Duration::from_millis(100);

    // This connect should timeout at neighbor resolution.
    let error = if sync {
        let socket = TcpSocket::new_in_realm::<I>(&realm).await.expect("open socket");
        socket.set_tcp_user_timeout(Some(timeout)).expect("set user timeout");
        socket.connect(&server_addr.into()).expect_err("connect should fail")
    } else {
        fasync::net::TcpStream::connect_in_realm_with_sock(&realm, server_addr, |socket| {
            socket.set_tcp_user_timeout(Some(timeout)).expect("set user timeout");
            Ok(())
        })
        .await
        .expect_err("connect should fail")
        .downcast::<std::io::Error>()
        .expect("failed to cast to std::io::Result")
    };

    // NS2 returns the wrong error here. We don't have a linux syscall to prove
    // it, but this is what sockscripter says, showing that ETIMEDOUT is the
    // right value:
    //
    // $ fx sockscripter -C tcp set-tcp-user-timeout 200 connect '192.168.4.2:8080'
    // sockscripter.cc[477]:Opened IPv4-STREAM socket (proto:TCP) fd:3
    // sockscripter.cc[557]:Set IPPROTO_TCP:TCP_USER_TIMEOUT = 200
    // sockscripter.cc[1140]:Connect(fd:3) to 192.168.4.2:8080
    // sockscripter.cc[1145]:Error-Connect(fd:3) failed-[110]Connection timed out
    let expect_error = match N::VERSION {
        NetstackVersion::Netstack2 { .. } => libc::EHOSTUNREACH,
        NetstackVersion::Netstack3 => libc::ETIMEDOUT,
        v => panic!("unexpected netstack version {v:?}"),
    };
    assert_eq!(error.raw_os_error(), Some(expect_error));
}

fn try_parse_frame_as_tcp<I: TestIpExt>(
    frame: Vec<u8>,
) -> Option<(
    EthernetFrameBuilder,
    impl IpPacketBuilder<NoOpSerializationContext, I>,
    TcpSegmentBuilder<I::Addr>,
    Vec<u8>,
)> {
    use packet_formats::ip::IpPacket as _;
    let eth = EthernetFrame::parse(&mut &frame[..], EthernetFrameLengthCheck::NoCheck)
        .expect("valid ethernet frame");

    if eth.ethertype() != Some(EtherType::from_ip_version(I::VERSION)) {
        return None;
    }
    let ip = I::Packet::parse(&mut eth.body(), ()).ok()?;
    if ip.proto() != IpProto::Tcp.into() {
        return None;
    }
    let tcp =
        TcpSegment::parse(&mut ip.body(), TcpParseArgs::new(ip.src_ip(), ip.dst_ip())).ok()?;

    let eth_builder = eth.builder();
    let mut ip_builder = <I::PacketBuilder<NoOpSerializationContext> as IpPacketBuilder<
        NoOpSerializationContext,
        I,
    >>::new(ip.src_ip(), ip.dst_ip(), ip.ttl(), ip.proto());
    ip_builder.set_dscp_and_ecn(ip.dscp_and_ecn());
    let tcp_builder = tcp.builder(ip.src_ip(), ip.dst_ip()).prefix_builder().clone();
    drop(ip);

    Some((eth_builder, ip_builder, tcp_builder, frame))
}

#[netstack_test]
#[variant(N, Netstack)]
#[test_case(
    PhantomData::<Ipv4>, IcmpDestUnreachable::default(),
    Icmpv4DestUnreachableCode::DestNetworkUnreachable => libc::ENETUNREACH
)]
#[test_case(
    PhantomData::<Ipv4>, IcmpDestUnreachable::default(),
    Icmpv4DestUnreachableCode::DestHostUnreachable => libc::EHOSTUNREACH
)]
#[test_case(
    PhantomData::<Ipv4>, IcmpDestUnreachable::default(),
    Icmpv4DestUnreachableCode::DestProtocolUnreachable => libc::ENOPROTOOPT
)]
#[test_case(
    PhantomData::<Ipv4>, IcmpDestUnreachable::default(),
    Icmpv4DestUnreachableCode::DestPortUnreachable => libc::ECONNREFUSED
)]
#[test_case(
    PhantomData::<Ipv4>, IcmpDestUnreachable::default(),
    Icmpv4DestUnreachableCode::SourceRouteFailed => libc::EOPNOTSUPP
)]
#[test_case(
    PhantomData::<Ipv4>, IcmpDestUnreachable::default(),
    Icmpv4DestUnreachableCode::DestNetworkUnknown => libc::ENETUNREACH
)]
#[test_case(
    PhantomData::<Ipv4>, IcmpDestUnreachable::default(),
    Icmpv4DestUnreachableCode::DestHostUnknown => libc::EHOSTDOWN
)]
#[test_case(
    PhantomData::<Ipv4>, IcmpDestUnreachable::default(),
    Icmpv4DestUnreachableCode::SourceHostIsolated => libc::ENONET
)]
#[test_case(
    PhantomData::<Ipv4>, IcmpDestUnreachable::default(),
    Icmpv4DestUnreachableCode::NetworkAdministrativelyProhibited => libc::ENETUNREACH
)]
#[test_case(
    PhantomData::<Ipv4>, IcmpDestUnreachable::default(),
    Icmpv4DestUnreachableCode::HostAdministrativelyProhibited => libc::EHOSTUNREACH
)]
#[test_case(
    PhantomData::<Ipv4>, IcmpDestUnreachable::default(),
    Icmpv4DestUnreachableCode::NetworkUnreachableForToS => libc::ENETUNREACH
)]
#[test_case(
    PhantomData::<Ipv4>, IcmpDestUnreachable::default(),
    Icmpv4DestUnreachableCode::HostUnreachableForToS => libc::EHOSTUNREACH
)]
#[test_case(
    PhantomData::<Ipv4>, IcmpDestUnreachable::default(),
    Icmpv4DestUnreachableCode::CommAdministrativelyProhibited => libc::EHOSTUNREACH
)]
#[test_case(
    PhantomData::<Ipv4>, IcmpDestUnreachable::default(),
    Icmpv4DestUnreachableCode::HostPrecedenceViolation => libc::EHOSTUNREACH
)]
#[test_case(
    PhantomData::<Ipv4>, IcmpDestUnreachable::default(),
    Icmpv4DestUnreachableCode::PrecedenceCutoffInEffect => libc::EHOSTUNREACH
)]
#[test_case(
    PhantomData::<Ipv4>, Icmpv4ParameterProblem::new(0),
    Icmpv4ParameterProblemCode::PointerIndicatesError => libc::EPROTO
)]
#[test_case(
    PhantomData::<Ipv4>, Icmpv4ParameterProblem::new(0),
    Icmpv4ParameterProblemCode::MissingRequiredOption => libc::EPROTO
)]
#[test_case(
    PhantomData::<Ipv4>, Icmpv4ParameterProblem::new(0),
    Icmpv4ParameterProblemCode::BadLength => libc::EPROTO
)]
#[test_case(
    PhantomData::<Ipv4>, IcmpTimeExceeded::default(),
    Icmpv4TimeExceededCode::TtlExpired => libc::EHOSTUNREACH
)]
#[test_case(
    PhantomData::<Ipv4>, IcmpTimeExceeded::default(),
    Icmpv4TimeExceededCode::FragmentReassemblyTimeExceeded => libc::ETIMEDOUT
)]
#[test_case(
    PhantomData::<Ipv6>, IcmpDestUnreachable::default(),
    Icmpv6DestUnreachableCode::NoRoute => libc::ENETUNREACH
)]
#[test_case(
    PhantomData::<Ipv6>, IcmpDestUnreachable::default(),
    Icmpv6DestUnreachableCode::CommAdministrativelyProhibited => libc::EACCES
)]
#[test_case(
    PhantomData::<Ipv6>, IcmpDestUnreachable::default(),
    Icmpv6DestUnreachableCode::BeyondScope => libc::EHOSTUNREACH
)]
#[test_case(
    PhantomData::<Ipv6>, IcmpDestUnreachable::default(),
    Icmpv6DestUnreachableCode::AddrUnreachable => libc::EHOSTUNREACH
)]
#[test_case(
    PhantomData::<Ipv6>, IcmpDestUnreachable::default(),
    Icmpv6DestUnreachableCode::PortUnreachable => libc::ECONNREFUSED
)]
#[test_case(
    PhantomData::<Ipv6>, IcmpDestUnreachable::default(),
    Icmpv6DestUnreachableCode::SrcAddrFailedPolicy => libc::EACCES
)]
#[test_case(
    PhantomData::<Ipv6>, IcmpDestUnreachable::default(),
    Icmpv6DestUnreachableCode::RejectRoute => libc::EACCES
)]
#[test_case(
    PhantomData::<Ipv6>, Icmpv6ParameterProblem::new(0),
    Icmpv6ParameterProblemCode::ErroneousHeaderField => libc::EPROTO
)]
#[test_case(
    PhantomData::<Ipv6>, Icmpv6ParameterProblem::new(0),
    Icmpv6ParameterProblemCode::UnrecognizedNextHeaderType => libc::EPROTO
)]
#[test_case(
    PhantomData::<Ipv6>, Icmpv6ParameterProblem::new(0),
    Icmpv6ParameterProblemCode::UnrecognizedIpv6Option => libc::EPROTO
)]
#[test_case(
    PhantomData::<Ipv6>, IcmpTimeExceeded::default(),
    Icmpv6TimeExceededCode::HopLimitExceeded => libc::EHOSTUNREACH
)]
#[test_case(
    PhantomData::<Ipv6>, IcmpTimeExceeded::default(),
    Icmpv6TimeExceededCode::FragmentReassemblyTimeExceeded => libc::EHOSTUNREACH
)]
async fn tcp_established_icmp_error<N: Netstack, I: TestIpExt, M: IcmpMessage<I> + Debug>(
    name: &str,
    _ip_version: PhantomData<I>,
    message: M,
    code: M::Code,
) -> i32 {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let net = sandbox.create_network("net").await.expect("failed to create network");
    let fake_ep = net.create_fake_endpoint().expect("failed to create fake endpoint");

    let client = sandbox
        .create_netstack_realm::<N, _>(format!("{name}_{}", format!("{code:?}").to_snake_case()))
        .expect("failed to create client realm");
    let client_interface =
        client.join_network(&net, "client-ep").await.expect("failed to join network in realm");
    client_interface
        .add_address_and_subnet_route(I::CLIENT_SUBNET)
        .await
        .expect("configure address");
    client
        .add_neighbor_entry(client_interface.id(), I::SERVER_SUBNET.addr, SERVER_MAC)
        .await
        .expect("add_neighbor_entry");

    // Filter frames observed on the fake endpoint to just those containing a TCP
    // segment in an IP packet.
    let fake_ep = &fake_ep;
    let mut frames = fake_ep.frame_stream().filter_map(|result| {
        Box::pin(async {
            let (frame, dropped) = result.unwrap();
            assert_eq!(dropped, 0);
            try_parse_frame_as_tcp::<I>(frame)
        })
    });

    let server = async {
        // Wait for an incoming TCP connection.
        let (eth, ip, tcp, _frame) = frames.next().await.unwrap();
        assert!(tcp.syn_set());

        // Send a SYN/ACK in response.
        let ethernet_builder = EthernetFrameBuilder::new(
            eth.dst_mac(),
            eth.src_mac(),
            EtherType::from_ip_version(I::VERSION),
            ETHERNET_MIN_BODY_LEN_NO_TAG,
        );
        let mut syn_ack = TcpSegmentBuilder::new(
            ip.dst_ip(),
            ip.src_ip(),
            tcp.dst_port().unwrap(),
            tcp.src_port().unwrap(),
            tcp.seq_num(),
            Some(tcp.seq_num() + 1),
            tcp.window_size(),
        );
        syn_ack.syn(true);
        let frame = packet::Buf::new([], ..)
            .wrap_in(syn_ack)
            .wrap_in(I::PacketBuilder::new(ip.dst_ip(), ip.src_ip(), u8::MAX, IpProto::Tcp.into()))
            .wrap_in(ethernet_builder.clone())
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .expect("serialize SYN/ACK")
            .unwrap_b();
        fake_ep.write(frame.as_ref()).await.expect("write SYN/ACK");

        // Wait for the ACK response, skipping any other packets (such as
        // retransmitted SYNs).
        loop {
            let (_eth, _ip, tcp, _frame) = frames.next().await.unwrap();
            if tcp.ack_num().is_some() {
                break;
            }
        }

        // Now that the connection is established, respond to the next packet with an
        // ICMP error to cause a soft error on the connection.
        let (_eth, ip, tcp, _frame) = frames.next().await.unwrap();
        let icmp_error = packet::Buf::new([], ..)
            .wrap_in(tcp)
            .wrap_in(ip.clone())
            .wrap_in(IcmpPacketBuilder::<I, _>::new(ip.dst_ip(), ip.src_ip(), code, message))
            .wrap_in(I::PacketBuilder::new(
                ip.dst_ip(),
                ip.src_ip(),
                u8::MAX,
                I::map_ip_out((), |()| Ipv4Proto::Icmp, |()| Ipv6Proto::Icmpv6),
            ))
            .wrap_in(ethernet_builder)
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .expect("serialize ICMP error")
            .unwrap_b();
        fake_ep.write(icmp_error.as_ref()).await.expect("write ICMP error");
    };

    let client = async {
        let server_addr: net_types::ip::IpAddr = I::SERVER_ADDR.into();
        let server_addr = std::net::SocketAddr::new(server_addr.into(), 8080);
        let mut socket = fasync::net::TcpStream::connect_in_realm(&client, server_addr)
            .await
            .expect("connect to server");
        socket.write_all(b"hello").await.unwrap();

        // We have to check SO_ERROR in a retry loop because there is no mechanism to
        // subscribe to be notified when a soft error occurs on a socket; they are not
        // signaled the way hard errors are.
        loop {
            fasync::Timer::new(std::time::Duration::from_millis(50)).await;

            // SAFETY: `getsockopt` does not retain memory passed to it.
            let mut value = 0i32;
            let mut value_size = std::mem::size_of_val(&value) as libc::socklen_t;
            let result = unsafe {
                libc::getsockopt(
                    socket.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_ERROR,
                    &mut value as *mut _ as *mut libc::c_void,
                    &mut value_size,
                )
            };
            assert_eq!(result, 0);
            if value != 0 {
                break value;
            }
        }
    };

    let (error, ()) = future::join(client, server).await;
    error
}

trait TestPmtuIpExt: TestIpExt {
    type Message: IcmpMessage<Self> + Debug;

    fn packet_too_big(
        lowered_mtu: NonZeroU16,
    ) -> (Self::Message, <Self::Message as IcmpMessage<Self>>::Code);
}

impl TestPmtuIpExt for Ipv4 {
    type Message = IcmpDestUnreachable;

    fn packet_too_big(lowered_mtu: NonZeroU16) -> (IcmpDestUnreachable, Icmpv4DestUnreachableCode) {
        (
            IcmpDestUnreachable::new_for_frag_req(lowered_mtu),
            Icmpv4DestUnreachableCode::FragmentationRequired,
        )
    }
}

impl TestPmtuIpExt for Ipv6 {
    type Message = Icmpv6PacketTooBig;

    fn packet_too_big(lowered_mtu: NonZeroU16) -> (Icmpv6PacketTooBig, IcmpZeroCode) {
        (Icmpv6PacketTooBig::new(u32::from(lowered_mtu.get())), IcmpZeroCode)
    }
}

#[netstack_test]
#[variant(N, Netstack)]
#[variant(I, Ip)]
#[test_case(true; "start by priming cache")]
#[test_case(false; "start with empty cache")]
async fn tcp_update_mss_from_pmtu<N: Netstack, I: TestPmtuIpExt>(name: &str, prime_cache: bool) {
    use packet::NestableSerializer as _;
    use packet_formats::ip::IpPacket as _;

    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let net = sandbox.create_network("net").await.expect("failed to create network");
    let fake_ep = net.create_fake_endpoint().expect("failed to create fake endpoint");
    let client =
        sandbox.create_netstack_realm::<N, _>(name).expect("failed to create client realm");
    let client_interface =
        client.join_network(&net, "ep").await.expect("failed to join network in realm");
    client_interface
        .add_address_and_subnet_route(I::CLIENT_SUBNET)
        .await
        .expect("configure address");
    client
        .add_neighbor_entry(client_interface.id(), I::SERVER_SUBNET.addr, SERVER_MAC)
        .await
        .expect("add_neighbor_entry");

    // NS3 enforces a minimum TCP MSS of 216. Select a new MTU value that is
    // large enough to accommodate this, while also being larger than the IP
    // version link minimum MTU.
    let new_mtu = match I::VERSION {
        // NB: IPv4's `MINIMUM_LINK_MTU` (68) is too small for TCP on NS3.
        // 256 allows for MSS (216) + TCP Header (20) + IPv4 Header (20).
        IpVersion::V4 => 256,
        // NB: IPv6's `MINIMUM_LINK_MTU` (1280) is sufficient for TCP on NS3.
        IpVersion::V6 => u16::try_from(Ipv6::MINIMUM_LINK_MTU.get()).unwrap(),
    };
    let new_mtu = NonZeroU16::new(new_mtu).unwrap();

    if prime_cache {
        // First, prime the PMTU cache with the value we will send later, as a
        // regression test for https://fxbug.dev/435260334.
        let (message, code) = I::packet_too_big(new_mtu);
        let icmp_error = packet::Buf::new([], ..)
            .wrap_in(IcmpPacketBuilder::<I, _>::new(I::SERVER_ADDR, I::CLIENT_ADDR, code, message))
            .wrap_in(I::PacketBuilder::new(
                I::SERVER_ADDR,
                I::CLIENT_ADDR,
                u8::MAX,
                I::map_ip_out((), |()| Ipv4Proto::Icmp, |()| Ipv6Proto::Icmpv6),
            ))
            .wrap_in(EthernetFrameBuilder::new(
                SERVER_MAC.into_ext(),
                client_interface.mac().await.into_ext(),
                EtherType::from_ip_version(I::VERSION),
                ETHERNET_MIN_BODY_LEN_NO_TAG,
            ))
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .expect("serialize ICMP error")
            .unwrap_b();
        fake_ep.write(icmp_error.as_ref()).await.expect("write ICMP error");
    }

    // Filter frames observed on the fake endpoint to just those containing a TCP
    // segment in an IP packet.
    let fake_ep = &fake_ep;
    let mut frames = fake_ep.frame_stream().filter_map(|result| {
        Box::pin(async {
            let (frame, dropped) = result.unwrap();
            assert_eq!(dropped, 0);
            try_parse_frame_as_tcp::<I>(frame)
        })
    });

    let server = async {
        // Wait for an incoming TCP connection.
        let (eth, ip, tcp, _frame) = frames.next().await.unwrap();
        assert!(tcp.syn_set());

        // Send a SYN/ACK in response.
        let ethernet_builder = EthernetFrameBuilder::new(
            eth.dst_mac(),
            eth.src_mac(),
            EtherType::from_ip_version(I::VERSION),
            ETHERNET_MIN_BODY_LEN_NO_TAG,
        );

        let mut syn_ack = TcpSegmentBuilder::new(
            ip.dst_ip(),
            ip.src_ip(),
            tcp.dst_port().unwrap(),
            tcp.src_port().unwrap(),
            tcp.seq_num(),
            Some(tcp.seq_num() + 1),
            tcp.window_size(),
        );
        syn_ack.syn(true);
        let frame = packet::Buf::new([], ..)
            .wrap_in(
                // Advertise an initial MSS that is large enough to fit the sender's payload in
                // a single segment.
                TcpSegmentBuilderWithOptions::new(
                    syn_ack,
                    TcpOptionsBuilder { mss: Some(1500), ..Default::default() },
                )
                .unwrap(),
            )
            .wrap_in(I::PacketBuilder::new(ip.dst_ip(), ip.src_ip(), u8::MAX, IpProto::Tcp.into()))
            .wrap_in(ethernet_builder.clone())
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .expect("serialize SYN/ACK")
            .unwrap_b();
        fake_ep.write(frame.as_ref()).await.expect("write SYN/ACK");

        // Wait for the ACK response, skipping any other packets (such as retransmitted
        // SYNs).
        loop {
            let (_eth, _ip, tcp, _frame) = frames.next().await.unwrap();
            if tcp.ack_num().is_some() {
                break;
            }
        }

        // Now that the connection is established, respond to the next packet with an
        // ICMP error indicating the packet was too big and providing a lower MTU.
        let (_eth, ip, tcp, too_large_frame) = frames.next().await.unwrap();

        // Ensure that the initial frame sent was larger than the updated PMTU we
        // provided, so that we can be sure we're actually exercising a reduction in the
        // PMTU.
        let too_large_frame =
            EthernetFrame::parse(&mut &too_large_frame[..], EthernetFrameLengthCheck::NoCheck)
                .expect("valid ethernet frame");
        assert_gt!(too_large_frame.body().len(), usize::try_from(new_mtu.get()).unwrap());

        let (message, code) = I::packet_too_big(new_mtu);
        let icmp_error = packet::Buf::new([], ..)
            .wrap_in(tcp)
            .wrap_in(ip.clone())
            .wrap_in(IcmpPacketBuilder::<I, _>::new(ip.dst_ip(), ip.src_ip(), code, message))
            .wrap_in(I::PacketBuilder::new(
                ip.dst_ip(),
                ip.src_ip(),
                u8::MAX,
                I::map_ip_out((), |()| Ipv4Proto::Icmp, |()| Ipv6Proto::Icmpv6),
            ))
            .wrap_in(ethernet_builder)
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .expect("serialize ICMP error")
            .unwrap_b();
        fake_ep.write(icmp_error.as_ref()).await.expect("write ICMP error");

        // The initial segment should be retransmitted in smaller pieces, respecting the
        // reduced PMTU.
        let retransmitted_segment = async {
            loop {
                let (_eth, _ip, _tcp, frame) = frames.next().await.unwrap();
                let eth = EthernetFrame::parse(&mut &frame[..], EthernetFrameLengthCheck::NoCheck)
                    .expect("valid ethernet frame");

                // It's possible the PMTU update wasn't processed by the netstack before the
                // retransmission timer fired, in which case we'd see the original segment
                // again.
                if eth.body().len() != usize::try_from(new_mtu.get()).unwrap() {
                    continue;
                }

                let ip = I::Packet::parse(&mut eth.body(), ()).expect("valid IP packet");
                let tcp =
                    TcpSegment::parse(&mut ip.body(), TcpParseArgs::new(ip.src_ip(), ip.dst_ip()))
                        .expect("valid TCP segment");
                break tcp.body().to_vec();
            }
        }
        .on_timeout(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT.after_now(), || {
            panic!("timed out waiting to observe segment with reduced MSS")
        })
        .await;

        let ip = I::Packet::parse(&mut too_large_frame.body(), ()).expect("valid IP packet");
        let too_large_segment =
            TcpSegment::parse(&mut ip.body(), TcpParseArgs::new(ip.src_ip(), ip.dst_ip()))
                .expect("valid TCP segment");
        assert_eq!(retransmitted_segment, &too_large_segment.body()[..retransmitted_segment.len()]);
    };

    let client = async {
        let server_addr: net_types::ip::IpAddr = I::SERVER_ADDR.into();
        let server_addr = std::net::SocketAddr::new(server_addr.into(), 8080);
        let mut socket = fasync::net::TcpStream::connect_in_realm(&client, server_addr)
            .await
            .expect("connect to server");

        // Send a payload that will not fit in a single segment. (The PMTU is updated to
        // `new_mtu`, which is too small due to the need to also fit the TCP
        // and IP headers).
        let len = usize::try_from(new_mtu.get()).unwrap();
        let payload = vec![0xFF; len];
        socket.write_all(&payload[..]).await.unwrap();
        socket
    };

    let (_socket, ()) = future::join(client, server).await;
}

/// Tests that a connection pending in an accept queue can be accepted and
/// returns the expected scope id even if the device the scope id matches has
/// been removed from the stack.
#[netstack_test]
#[variant(N, Netstack)]
async fn tcp_accept_with_removed_device_scope<N: Netstack>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let net = sandbox.create_network("net").await.expect("failed to create network");

    let client = sandbox
        .create_netstack_realm::<N, _>(format!("{name}_client"))
        .expect("failed to create client realm");

    let server = sandbox
        .create_netstack_realm::<N, _>(format!("{name}_server"))
        .expect("failed to create client realm");

    let client_iface =
        client.join_network(&net, "client-ep").await.expect("failed to join network");
    let server_iface =
        server.join_network(&net, "server-ep").await.expect("failed to join network");

    async fn get_ll_addr(
        realm: &netemul::TestRealm<'_>,
        ep: &netemul::TestInterface<'_>,
    ) -> std::net::Ipv6Addr {
        let interfaces_state = realm
            .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
            .expect("connect to protocol");
        netstack_testing_common::interfaces::wait_for_v6_ll(&interfaces_state, ep.id())
            .await
            .expect("wait LL address")
            .into()
    }

    let server_addr = get_ll_addr(&server, &server_iface).await;
    let client_addr = get_ll_addr(&client, &client_iface).await;

    const PORT: u16 = 8080;
    let server_sock = fasync::net::TcpListener::listen_in_realm(
        &server,
        std::net::SocketAddrV6::new(std::net::Ipv6Addr::UNSPECIFIED, PORT, 0, 0).into(),
    )
    .await
    .expect("listen in realm");

    // We need to notify that we want readable so that fuchsia_async clears the
    // cached readable signals within _before_ we actually start the connection
    // process so we can wait for readable later with a clean slate.
    futures::future::poll_fn(|cx| {
        server_sock.need_read(cx);
        futures::task::Poll::Ready(())
    })
    .await;

    let client_sock = fasync::net::TcpStream::connect_in_realm(
        &client,
        std::net::SocketAddrV6::new(
            server_addr.into(),
            PORT,
            0,
            client_iface.id().try_into().unwrap(),
        )
        .into(),
    )
    .await
    .expect("connect");

    let client_port = client_sock.std().local_addr().expect("local addr").port();

    let server_scope: u32 = server_iface.id().try_into().unwrap();

    // Ensure that the connection is ready to be accepted, the server socket
    // must be readable.
    futures::future::poll_fn(|cx| server_sock.poll_readable(cx))
        .await
        .expect("polling server socket");

    server_iface
        .control()
        .remove()
        .await
        .expect("requesting removal")
        .expect("failed to request removal");
    assert_eq!(
        server_iface.wait_removal().await.expect("waiting removal"),
        fnet_interfaces_admin::InterfaceRemovedReason::User
    );

    let (_server_sock, _connection, from) = server_sock.accept().await.expect("accept failed");
    let v6_addr = assert_matches!(from, std::net::SocketAddr::V6(v6) => v6);
    assert_eq!(v6_addr.ip(), &client_addr);
    assert_eq!(v6_addr.port(), client_port);
    assert_eq!(v6_addr.scope_id(), server_scope);
}

struct PeerNetwork<'a> {
    client_iface: netemul::TestInterface<'a>,
    peer_realm: netemul::TestRealm<'a>,
    _peer_iface: netemul::TestInterface<'a>,
    _net: netemul::TestNetwork<'a>,
    peer_addr: std::net::SocketAddr,
}

async fn setup_peer_network<'a>(
    sandbox: &'a netemul::TestSandbox,
    client: &netemul::TestRealm<'a>,
    name: &str,
    net_num: u8,
) -> PeerNetwork<'a> {
    let net = sandbox.create_network(format!("net{net_num}")).await.expect("create net");
    let client_iface =
        client.join_network(&net, format!("client-ep-{net_num}")).await.expect("join client");
    client_iface
        .add_address_and_subnet_route(fnet::Subnet {
            addr: fnet::IpAddress::Ipv4(fnet::Ipv4Address { addr: [192, 168, net_num, 1] }),
            prefix_len: 24,
        })
        .await
        .expect("configure client addr");
    client_iface.apply_nud_flake_workaround().await.expect("nud workaround");

    let peer_realm = sandbox
        .create_netstack_realm::<Netstack3, _>(format!("{name}_peer{net_num}"))
        .expect("create peer realm");
    let peer_iface =
        peer_realm.join_network(&net, format!("peer-ep-{net_num}")).await.expect("join peer");
    peer_iface
        .add_address_and_subnet_route(fnet::Subnet {
            addr: fnet::IpAddress::Ipv4(fnet::Ipv4Address { addr: [192, 168, net_num, 2] }),
            prefix_len: 24,
        })
        .await
        .expect("configure peer addr");
    peer_iface.apply_nud_flake_workaround().await.expect("nud workaround");

    let peer_addr = std::net::SocketAddr::from(([192, 168, net_num, 2], 8080));
    PeerNetwork { client_iface, peer_realm, _peer_iface: peer_iface, _net: net, peer_addr }
}

#[netstack_test]
async fn tcp_connect_with_bound_device(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let client = sandbox
        .create_netstack_realm::<Netstack3, _>(format!("{name}_client"))
        .expect("create client realm");

    let peer1 = setup_peer_network(&sandbox, &client, name, 1).await;
    let peer2 = setup_peer_network(&sandbox, &client, name, 2).await;

    let iface1_name = peer1.client_iface.get_interface_name().await.expect("get iface1 name");

    let listener1 = fasync::net::TcpListener::listen_in_realm(&peer1.peer_realm, peer1.peer_addr)
        .await
        .expect("listen on peer1");

    // Connecting to a peer reachable via the bound interface succeeds.
    let _stream =
        fasync::net::TcpStream::connect_in_realm_with_sock(&client, peer1.peer_addr, |socket| {
            socket.bind_device(Some(iface1_name.as_bytes())).expect("bind_device to iface1");
            Ok(())
        })
        .await
        .expect("connect to peer1 on bound iface1 should succeed");

    let (_server_sock1, _conn1, _from1) = listener1.accept().await.expect("accept on peer1");

    // Connecting to a destination on a different subnet reachable only through
    // an unbound interface fails because route resolution is constrained to iface1.
    let err =
        fasync::net::TcpStream::connect_in_realm_with_sock(&client, peer2.peer_addr, |socket| {
            socket.bind_device(Some(iface1_name.as_bytes())).expect("bind_device to iface1");
            Ok(())
        })
        .await
        .expect_err("connect to peer2 with socket bound to iface1 must fail")
        .downcast::<std::io::Error>()
        .expect("downcast to std::io::Error");

    assert_eq!(
        err.raw_os_error(),
        Some(libc::ENETUNREACH),
        "expected ENETUNREACH when connecting to disjoint subnet with bound device"
    );
}
