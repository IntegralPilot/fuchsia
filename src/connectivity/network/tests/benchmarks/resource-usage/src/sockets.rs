// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_async::net::{TcpListener, UdpSocket};
use futures::{AsyncReadExt as _, AsyncWriteExt as _, StreamExt as _};
use netemul::{RealmTcpListener as _, RealmTcpStream as _, RealmUdpSocket as _};
use std::net::SocketAddr;

const UNSPECIFIED: std::net::SocketAddr =
    std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 0);

const PERF_TEST_MODE_SOCKET_PAIRS: usize = 100;
const PERF_TEST_MODE_ITERATIONS: usize = 100;

const UNIT_TEST_MODE_SOCKET_PAIRS: usize = 2;
const UNIT_TEST_MODE_ITERATIONS: usize = 1;

pub struct TcpSockets;

impl crate::Workload for TcpSockets {
    const NAME: &'static str = "Sockets/TCP";

    async fn create(_netstack: &netemul::TestRealm<'_>) -> Self {
        Self
    }

    async fn run(&self, netstack: &netemul::TestRealm<'_>, perftest_mode: bool) {
        let pairs =
            if perftest_mode { PERF_TEST_MODE_SOCKET_PAIRS } else { UNIT_TEST_MODE_SOCKET_PAIRS };
        futures::stream::repeat(())
            .take(pairs)
            .for_each_concurrent(None, |()| async {
                let listener =
                    TcpListener::listen_in_realm(netstack, UNSPECIFIED).await.expect("bind");
                let mut connected = fuchsia_async::net::TcpStream::connect_in_realm(
                    netstack,
                    listener.local_addr().expect("get addr of listener"),
                )
                .await
                .expect("connect");
                let (_, mut connection, _): (TcpListener, _, SocketAddr) =
                    listener.accept().await.expect("accept connection");

                let iterations = if perftest_mode {
                    PERF_TEST_MODE_ITERATIONS
                } else {
                    UNIT_TEST_MODE_ITERATIONS
                };
                for _ in 0..iterations {
                    const MSG_SIZE: usize = 100_000;
                    const DATA: [u8; MSG_SIZE] = [0xff; MSG_SIZE];
                    connected.write_all(&DATA).await.expect("write");
                    let mut recv = [0u8; MSG_SIZE];
                    connection.read_exact(&mut recv).await.expect("read");
                }
            })
            .await;
    }
}

pub struct UdpSockets;

impl crate::Workload for UdpSockets {
    const NAME: &'static str = "Sockets/UDP";

    async fn create(_netstack: &netemul::TestRealm<'_>) -> Self {
        Self
    }

    async fn run(&self, netstack: &netemul::TestRealm<'_>, perftest_mode: bool) {
        let pairs =
            if perftest_mode { PERF_TEST_MODE_SOCKET_PAIRS } else { UNIT_TEST_MODE_SOCKET_PAIRS };
        futures::stream::repeat(())
            .take(pairs)
            .for_each_concurrent(None, |()| async {
                let client = UdpSocket::bind_in_realm(netstack, UNSPECIFIED).await.expect("bind");
                let server = UdpSocket::bind_in_realm(netstack, UNSPECIFIED).await.expect("bind");
                let server_addr = server.local_addr().expect("get local addr");

                let iterations = if perftest_mode {
                    PERF_TEST_MODE_ITERATIONS
                } else {
                    UNIT_TEST_MODE_ITERATIONS
                };
                for _ in 0..iterations {
                    const MSG_SIZE: usize = 60_000;
                    const DATA: [u8; MSG_SIZE] = [0xff; MSG_SIZE];
                    assert_eq!(
                        client.send_to(&DATA, server_addr).await.expect("send to"),
                        DATA.len()
                    );
                    let mut recv = [0u8; MSG_SIZE];
                    let (read, _): (_, SocketAddr) =
                        server.recv_from(&mut recv).await.expect("recv from");
                    assert_eq!(read, recv.len());
                    assert_eq!(DATA, recv);
                }
            })
            .await;
    }
}
