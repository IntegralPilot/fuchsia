// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::marker::PhantomData;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use component_events::events::{EventStream, ExitStatus, Stopped, StoppedPayload};
use component_events::matcher::EventMatcher;
use fasync::TimeoutExt as _;
use fidl_fuchsia_posix_socket as fposix_socket;
use fidl_fuchsia_posix_socket_ext as fposix_socket_ext;
use fidl_fuchsia_starnix_runner as fstarnix_runner;
use fuchsia_async as fasync;
use fuchsia_component_test::{RealmBuilder, RealmBuilderParams};
use futures::{AsyncReadExt as _, AsyncWriteExt as _, FutureExt as _};
use test_case::test_case;

const AWAKE_SIGNAL: zx::Signals = zx::Signals::USER_0;
const ASLEEP_SIGNAL: zx::Signals = zx::Signals::USER_1;
const SERVER_PORT: u16 = 33333;
const SERVER_ADDR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), SERVER_PORT);
const PAYLOAD: &[u8] = b"Hello, world!";

trait WakeupSocket {
    const NAME: &str;

    async fn connect(provider: &fposix_socket::ProviderProxy) -> Self;
    async fn write(&mut self);
    async fn read(&mut self);
}

struct Tcp(fasync::net::TcpStream);

impl WakeupSocket for Tcp {
    const NAME: &str = "tcp";

    async fn connect(provider: &fposix_socket::ProviderProxy) -> Self {
        for _ in 0..5 {
            let socket = provider
                .stream_socket(
                    fposix_socket::Domain::Ipv4,
                    fposix_socket::StreamSocketProtocol::Tcp,
                )
                .await
                .expect("call stream_socket")
                .expect("create stream socket");
            let socket: socket2::Socket = fdio::create_fd(socket.into()).expect("create fd").into();
            let connector = fasync::net::TcpStream::connect_from_raw(socket, SERVER_ADDR)
                .expect("create connector");
            match connector.await {
                Ok(stream) => return Self(stream),
                Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                    fasync::Timer::new(std::time::Duration::from_secs(1)).await;
                    continue;
                }
                Err(e) => panic!("unexpected connect error: {e:?}"),
            }
        }

        panic!("failed to connect after several attempts");
    }

    async fn write(&mut self) {
        let Self(socket) = self;

        socket.write_all(PAYLOAD).await.expect("write to server");
    }

    async fn read(&mut self) {
        let Self(socket) = self;

        let mut buf = Vec::new();
        let bytes = socket.read_to_end(&mut buf).await.expect("read response from server");
        assert_eq!(bytes, PAYLOAD.len());
        assert_eq!(buf, PAYLOAD);
    }
}

struct Udp(fasync::net::UdpSocket);

impl WakeupSocket for Udp {
    const NAME: &str = "udp";

    async fn connect(provider: &fposix_socket::ProviderProxy) -> Self {
        let socket = fposix_socket_ext::datagram_socket(
            &provider,
            fposix_socket::Domain::Ipv4,
            fposix_socket::DatagramSocketProtocol::Udp,
        )
        .await
        .expect("call datagram_socket")
        .expect("create datagram socket");
        let bind_addr = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
        socket.bind(&bind_addr.into()).expect("bind UDP socket");
        let socket = fasync::net::UdpSocket::from_socket(socket.into()).expect("into async socket");

        // Because UDP is connectionless, we can't actually connect to the server to
        // ensure it's listening for incoming data. Instead, we try sending it datagrams
        // until it responds, at which point we know it's listening.
        const PROBE: &[u8] = b"probe";
        for _ in 0..5 {
            let bytes = socket.send_to(PROBE, SERVER_ADDR).await.expect("write to server");
            assert_eq!(bytes, PROBE.len());

            let mut buf = vec![0; PROBE.len()];
            let result = socket
                .recv_from(&mut buf)
                .map(Some)
                .on_timeout(std::time::Duration::from_secs(5), || None)
                .await
                .transpose()
                .expect("read response from server");
            let Some((bytes, from)) = result else {
                continue;
            };
            assert_eq!(from, SERVER_ADDR);
            if bytes == PROBE.len() && &buf[..bytes] == PROBE {
                return Self(socket);
            }
        }

        panic!("failed to connect after several attempts");
    }

    async fn write(&mut self) {
        let Self(socket) = self;

        let bytes = socket.send_to(PAYLOAD, SERVER_ADDR).await.expect("write to server");
        assert_eq!(bytes, PAYLOAD.len());
    }

    async fn read(&mut self) {
        let Self(socket) = self;

        let mut buf = vec![0; PAYLOAD.len()];
        let (bytes, from) = socket.recv_from(&mut buf).await.expect("read response from server");
        assert_eq!(from, SERVER_ADDR);
        assert_eq!(bytes, PAYLOAD.len());
        assert_eq!(buf, PAYLOAD);
    }
}

#[test_case(PhantomData::<Tcp>; "tcp")]
#[test_case(PhantomData::<Udp>; "udp")]
#[fuchsia::test]
async fn wake_from_suspend_for_socket<S: WakeupSocket>(_socket_type: PhantomData<S>) {
    let mut events = EventStream::open().await.unwrap();

    let component_name = format!("realm_{}", S::NAME);
    let component_url = format!("#meta/{}.cm", component_name);
    let builder =
        RealmBuilder::with_params(RealmBuilderParams::new().from_relative_url(component_url))
            .await
            .expect("create realm builder");
    let instance = builder.build().await.expect("build realm");

    // Register a wake watcher with the Starnix runner and observe the initial
    // "awake" signal.
    let manager: fstarnix_runner::ManagerProxy =
        instance.root.connect_to_protocol_at_exposed_dir().expect("connect to starnix runner");
    let (wake_watcher, wake_watcher_remote) = zx::EventPair::create();
    manager
        .register_wake_watcher(fstarnix_runner::ManagerRegisterWakeWatcherRequest {
            watcher: Some(wake_watcher_remote),
            ..Default::default()
        })
        .await
        .expect("register wake watcher");
    let _: zx::Signals = fasync::OnSignals::new(&wake_watcher, AWAKE_SIGNAL).await.expect("awake");

    // Connect to the Linux binary that should be listening.
    let provider: fposix_socket::ProviderProxy =
        instance.root.connect_to_protocol_at_exposed_dir().expect("connect to netstack");
    let mut socket = S::connect(&provider).await;

    // The Linux binary will actively suspend the system after accepting a single
    // incoming connection.
    let _: zx::Signals = fasync::OnSignals::new(&wake_watcher, ASLEEP_SIGNAL)
        .await
        .expect("container should suspend");

    // Write a message to the server, which should wake up the Starnix container.
    socket.write().await;
    let _: zx::Signals =
        fasync::OnSignals::new(&wake_watcher, AWAKE_SIGNAL).await.expect("container should wake");
    socket.read().await;

    let moniker = format!("{}/suspend_for_{}_wakeup", instance.root.moniker(), S::NAME);
    let stopped = EventMatcher::ok().moniker(&moniker).wait::<Stopped>(&mut events).await.unwrap();
    let StoppedPayload { status, exit_code: _ } = stopped.result().expect("extract event payload");
    assert_eq!(status, &ExitStatus::Clean);
}
