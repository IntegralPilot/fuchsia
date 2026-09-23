// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ffx_e2e_emu::IsolatedEmulator;
use ffx_target_net::{Bidirectional, Counters, PortForwarder, SocketProvider, TargetTcpStream};
use fho::TryFromEnv as _;
use futures::{AsyncReadExt as _, AsyncWriteExt as _, FutureExt as _, StreamExt as _};
use log::info;
use net_declare::std_socket_addr;
use std::net::SocketAddr;
use std::pin::pin;
use std::time::Duration;
use target_holders::RemoteControlProxyHolder;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(120);

const LOCALHOST_UNSPECIFIED_PORT: SocketAddr = std_socket_addr!("127.0.0.1:0");

#[fuchsia::test(logging = true)]
async fn ffx_target_net_test() {
    info!("starting emulator...");

    // IsolatedEmulator provides isolation by using unique paths for its environment
    // (data dir, config, etc.), so we don't need to include the PID or a counter
    // in the name to prevent collisions between parallel test runs.
    let name = "ffx-target-net-test";
    let emu = IsolatedEmulator::start(name).await.unwrap();
    let fho_env = emu.fho_env();

    // Manually resolve the target address for the emulator we just started. This prevents
    // falling back to default discovery, which could be ambiguous if other emulators are running.
    emu.setup_fake_direct_connector(&fho_env).await.unwrap();

    let rcs = RemoteControlProxyHolder::try_from_env(&fho_env).await.expect("connect to rcs");

    let socket_provider =
        SocketProvider::new_with_rcs(CONNECT_TIMEOUT, &*rcs).await.expect("create socket provider");
    // To avoid the overhead of creating many emulator instances, just do
    // all of our tests in a single case.
    test_target_tcp_sockets(&socket_provider).await;
    test_forwarding(socket_provider.clone()).await;
    test_reverse(socket_provider).await;
}

async fn test_target_tcp_sockets(socket_provider: &SocketProvider) {
    info!("=== test_target_tcp_sockets ===");
    let listener = socket_provider.listen(LOCALHOST_UNSPECIFIED_PORT, None).await.expect("listen");
    let mut connected = socket_provider.connect(listener.local_addr()).await.expect("connect");
    let mut accepted = listener.accept().await.expect("accept");
    assert_eq!(accepted.local_addr(), connected.peer_addr());
    assert_eq!(accepted.peer_addr(), connected.local_addr());
    assert_eq!(accepted.local_addr(), listener.local_addr());
    info!("accepted socket");
    let msg = b"hello world";
    connected.write_all(msg).await.expect("write message");
    let mut buff = vec![0u8; msg.len()];
    accepted.read_exact(&mut buff[..]).await.expect("read all");
    info!("exchanged message");
    assert_eq!(buff, msg);
    connected.close().await.expect("close");
    info!("waiting for hangup");
    let read = accepted.read(&mut buff[..]).await.expect("read final");
    assert_eq!(read, 0);
    info!("hangup finished");
}

async fn assert_working_connection(
    host: &mut tokio::net::TcpStream,
    target: &mut TargetTcpStream,
    bytes_to_send: usize,
) {
    let send_buffer =
        (bytes_to_send..bytes_to_send * 2).into_iter().map(|b| b as u8).collect::<Vec<_>>();
    let mut recv_buffer = vec![0u8; bytes_to_send];
    futures::future::try_join(host.write_all(&send_buffer), target.read_exact(&mut recv_buffer))
        .await
        .expect("host => target");
    assert_eq!(recv_buffer, send_buffer);
    recv_buffer.fill(0);
    futures::future::try_join(target.write_all(&send_buffer), host.read_exact(&mut recv_buffer))
        .await
        .expect("target => host");
    assert_eq!(recv_buffer, send_buffer);
}

async fn test_forwarding(socket_provider: SocketProvider) {
    info!("=== test_forwarding ===");
    let target_listener =
        socket_provider.listen(LOCALHOST_UNSPECIFIED_PORT, None).await.expect("target listen");
    let target_listener = &target_listener;
    let target_addr = target_listener.local_addr();
    info!("target server listening on {target_addr}");
    let forwarder = PortForwarder::new(socket_provider);

    let host_listener =
        tokio::net::TcpListener::bind(LOCALHOST_UNSPECIFIED_PORT).await.expect("bind listener");
    let host_addr = host_listener.local_addr().expect("get local addr");
    info!("host server listening on {host_addr}");

    const CONNECTIONS: usize = 3;
    let tests = futures::stream::iter(1..=CONNECTIONS)
        .then(|i| async move {
            // Connect and accept in order so we know these are two sides of a
            // forwarded connection.
            let conn = tokio::net::TcpStream::connect(host_addr).await.expect("connect local");
            let accepted = target_listener.accept().await.expect("accept");
            info!(
                "connected through forwarding tunnel {i} ({}, {}) => ({}, {})",
                conn.local_addr().unwrap(),
                conn.peer_addr().unwrap(),
                accepted.local_addr(),
                accepted.peer_addr(),
            );
            (accepted, conn, i)
        })
        .collect::<Vec<_>>()
        .then(|tests| {
            // Verify all connections in parallel once they're all established.
            futures::future::join_all(tests.into_iter().map(
                |(mut accepted, mut conn, i)| async move {
                    let bytes = i * 100;
                    assert_working_connection(&mut conn, &mut accepted, bytes).await;
                    (accepted, conn, bytes)
                },
            ))
            .map(|sockets| {
                let Counters { active_connections, total_bytes } = forwarder.read_counters();
                let expect_bytes = sockets.iter().map(|(_, _, bytes)| *bytes).sum();
                assert_eq!(
                    active_connections,
                    Bidirectional { host_to_target: CONNECTIONS, target_to_host: 0 }
                );
                assert_eq!(
                    total_bytes,
                    Bidirectional { host_to_target: expect_bytes, target_to_host: expect_bytes }
                );
            })
        });

    {
        let mut tests = pin!(tests.fuse());
        let mut forward_fut = pin!(forwarder.forward(host_listener, target_addr).fuse());
        futures::select! {
            r = forward_fut => panic!("should not finish {r:?}"),
            () = tests => {},
        }
    }
    assert_eq!(forwarder.read_counters().active_connections, Bidirectional::default());
}

async fn test_reverse(socket_provider: SocketProvider) {
    info!("=== test_reverse ===");
    let socket_provider = &socket_provider;
    let target_listener =
        socket_provider.listen(LOCALHOST_UNSPECIFIED_PORT, None).await.expect("target listen");
    let target_addr = target_listener.local_addr();
    info!("target server listening on {target_addr}");
    let forwarder = PortForwarder::new(socket_provider.clone());

    let host_listener =
        tokio::net::TcpListener::bind(LOCALHOST_UNSPECIFIED_PORT).await.expect("bind listener");
    let host_listener = &host_listener;
    let host_addr = host_listener.local_addr().expect("get local addr");
    info!("host server listening on {host_addr}");

    const CONNECTIONS: usize = 3;
    let tests = futures::stream::iter(1..=CONNECTIONS)
        .then(|i| async move {
            // Connect and accept in order so we know these are two sides of a
            // forwarded connection.
            let conn = socket_provider.connect(target_addr).await.expect("connect target");
            let (accepted, _) = host_listener.accept().await.expect("accept");
            info!(
                "connected through reverse forwarding tunnel {i} ({}, {}) => ({}, {})",
                conn.local_addr(),
                conn.peer_addr(),
                accepted.local_addr().unwrap(),
                accepted.peer_addr().unwrap(),
            );
            (accepted, conn, i)
        })
        .collect::<Vec<_>>()
        .then(|tests| {
            // Verify all connections in parallel once they're all established.
            futures::future::join_all(tests.into_iter().map(
                |(mut accepted, mut conn, i)| async move {
                    let bytes = i * 100;
                    assert_working_connection(&mut accepted, &mut conn, bytes).await;
                    (accepted, conn, bytes)
                },
            ))
            .map(|sockets| {
                let Counters { active_connections, total_bytes } = forwarder.read_counters();
                let expect_bytes = sockets.iter().map(|(_, _, bytes)| *bytes).sum();
                assert_eq!(
                    active_connections,
                    Bidirectional { host_to_target: 0, target_to_host: CONNECTIONS }
                );
                assert_eq!(
                    total_bytes,
                    Bidirectional { host_to_target: expect_bytes, target_to_host: expect_bytes }
                );
            })
        });
    {
        let mut tests = pin!(tests.fuse());
        let mut reverse_fut = pin!(forwarder.reverse(target_listener, host_addr).fuse());
        futures::select! {
            r = reverse_fut => panic!("should not finish {r:?}"),
            () = tests => {},
        }
    }
    assert_eq!(forwarder.read_counters().active_connections, Bidirectional::default());
}
