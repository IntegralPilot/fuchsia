// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Fuchsia netdevice client tun test
use assert_matches::assert_matches;
use fidl::endpoints;
use fidl_fuchsia_hardware_network as netdev;
use fidl_fuchsia_net_tun as tun;
use fuchsia_async as fasync;
use fuchsia_component::client::connect_to_protocol;
use futures::TryStreamExt as _;
use futures::future::{Future, FutureExt as _};
use netdevice_client::{Client, DerivableConfig, Port, RxReadyStorage, Session};
use std::convert::TryInto as _;
use std::io::{Read as _, Write as _};
use std::pin::pin;
use std::task::Poll;

const DEFAULT_PORT_ID: u8 = 2;
const DEFAULT_MTU: u32 = 1500;
const DATA_BYTE: u8 = 42;
const DATA_LEN: usize = 4;

#[fuchsia::test]
async fn test_rx() {
    let (tun, _tun_port, port) = create_tun_device_and_port().await;
    let client = create_netdev_client(&tun);
    let () = with_netdev_session(
        client,
        DerivableConfig::default(),
        port,
        "test_rx",
        |session, _client| async move {
            let frame = tun::Frame {
                frame_type: Some(netdev::FrameType::Ethernet),
                data: Some(vec![DATA_BYTE; DATA_LEN]),
                port: Some(DEFAULT_PORT_ID),
                ..Default::default()
            };
            let () = tun.write_frame(&frame).await.unwrap().expect("failed to write frame");
            let mut rx_ready = RxReadyStorage::new(1);
            let buff = session
                .recv(&mut rx_ready)
                .await
                .expect("failed to recv buffer")
                .next()
                .unwrap()
                .expect("failed to recv buffer");
            let mut bytes = [0u8; DATA_LEN];
            assert_eq!(buff.io().read_at(0, &mut bytes[..]), DATA_LEN);
            for i in bytes.iter() {
                assert_eq!(*i, DATA_BYTE);
            }
        },
    )
    .await;
}

#[fuchsia::test]
async fn test_tx() {
    let (tun, _tun_port, port) = create_tun_device_and_port().await;
    let client = create_netdev_client(&tun);
    let () = with_netdev_session(
        client,
        DerivableConfig::default(),
        port,
        "test_tx",
        |session, _client| async move {
            let mut buffer =
                session.alloc_tx_buffer(DATA_LEN).await.expect("failed to alloc tx buffer");
            assert_eq!(
                buffer
                    .io_mut()
                    .write(&[DATA_BYTE; DATA_LEN][..])
                    .expect("failed to write into the buffer"),
                DATA_LEN
            );
            {
                let mut meta = buffer.meta_mut();
                meta.set_port(port);
                meta.set_frame_type(netdev::FrameType::Ethernet);
            }
            session.send(buffer);
            let frame = tun
                .read_frame()
                .await
                .unwrap()
                .map_err(zx::Status::err_from_raw)
                .expect("failed to read frame from the tun device");
            assert_eq!(frame.data, Some(vec![DATA_BYTE; DATA_LEN]));
            assert_eq!(frame.frame_type, Some(netdev::FrameType::Ethernet));
            assert_eq!(frame.port, Some(DEFAULT_PORT_ID));
        },
    )
    .await;
}

// Receives buffer from session and echoes back.
async fn echo(session: Session, port: Port, frame_count: u32) {
    let mut rx_ready = RxReadyStorage::new(1);
    for i in 0..frame_count {
        let buffer = session
            .recv(&mut rx_ready)
            .await
            .expect("failed to recv from session")
            .next()
            .unwrap()
            .expect("failed to recv from session");
        assert_eq!(buffer.len(), DATA_LEN);
        let mut bytes = [0u8; DATA_LEN];
        assert_eq!(buffer.io().read(&mut bytes[..]).unwrap(), DATA_LEN);
        assert_eq!(u32::from_le_bytes(bytes), i);
        let mut buffer = session.alloc_tx_buffer(DATA_LEN).await.expect("no tx buffer available");
        {
            let mut meta = buffer.meta_mut();
            meta.set_frame_type(netdev::FrameType::Ethernet);
            meta.set_port(port);
        }
        assert_eq!(buffer.io_mut().write(&bytes).unwrap(), DATA_LEN);
        session.send(buffer);
    }
}

#[fuchsia::test]
async fn test_echo_tun() {
    const FRAME_TOTAL_COUNT: u32 = 512;
    let (tun, _tun_port, port) = create_tun_device_and_port().await;
    let client = create_netdev_client(&tun);
    with_netdev_session(
        client,
        DerivableConfig::default(),
        port,
        "test_echo_tun",
        |session, _client| async {
            let echo_fut = echo(session, port, FRAME_TOTAL_COUNT);
            let main_fut = async move {
                for i in 0..FRAME_TOTAL_COUNT {
                    let frame = tun::Frame {
                        frame_type: Some(netdev::FrameType::Ethernet),
                        data: Some(Vec::from(i.to_le_bytes())),
                        port: Some(DEFAULT_PORT_ID),
                        ..Default::default()
                    };
                    let () = tun
                        .write_frame(&frame)
                        .await
                        .unwrap()
                        .map_err(zx::Status::err_from_raw)
                        .expect("cannot write frame");
                    let frame = tun
                        .read_frame()
                        .await
                        .unwrap()
                        .map_err(zx::Status::err_from_raw)
                        .expect("failed to read frame");
                    let data = frame.data.unwrap();
                    assert_eq!(data.len(), DATA_LEN);
                    let bytes: [u8; DATA_LEN] = data.try_into().unwrap();
                    assert_eq!(u32::from_le_bytes(bytes), i);
                }
            };
            let ((), ()) = futures::join!(echo_fut, main_fut);
        },
    )
    .await;
}

#[fuchsia::test]
async fn test_echo_pair() {
    const FRAME_TOTAL_COUNT: u32 = 512;
    let pair = create_tun_device_pair();
    let (client1, port1, client2, port2) = create_netdev_client_pair(&pair).await;
    let () = with_netdev_session(
        client1,
        DerivableConfig::default(),
        port1,
        "test_echo_pair_1",
        |session1, client1| async move {
            let () = with_netdev_session(
                client2,
                DerivableConfig::default(),
                port2,
                "test_echo_pair_2",
                |session2, client2| async move {
                    // Wait for the ports to be online before we send anything.
                    assert_matches!(
                        client1.wait_online(port1).await,
                        Ok(netdevice_client::PortStatus {
                            flags: netdev::StatusFlags::ONLINE,
                            mtu: DEFAULT_MTU
                        })
                    );
                    assert_matches!(
                        client2.wait_online(port2).await,
                        Ok(netdevice_client::PortStatus {
                            flags: netdev::StatusFlags::ONLINE,
                            mtu: DEFAULT_MTU
                        })
                    );
                    let echo_fut = echo(session1, port1, FRAME_TOTAL_COUNT);
                    let main_fut = async {
                        let mut rx_ready2 = RxReadyStorage::new(1);
                        for i in 0..FRAME_TOTAL_COUNT {
                            let mut buffer = session2
                                .alloc_tx_buffer(DATA_LEN)
                                .await
                                .expect("failed to alloc tx buffer");
                            {
                                let mut meta = buffer.meta_mut();
                                meta.set_frame_type(netdev::FrameType::Ethernet);
                                meta.set_port(port2);
                            }
                            let mut bytes = i.to_le_bytes();
                            assert_eq!(
                                buffer
                                    .io_mut()
                                    .write(&bytes[..])
                                    .expect("failed to write into the buffer"),
                                DATA_LEN
                            );
                            session2.send(buffer);

                            let buffer = session2
                                .recv(&mut rx_ready2)
                                .await
                                .expect("failed to recv from the session")
                                .next()
                                .unwrap()
                                .expect("failed to recv from the session");
                            assert_eq!(
                                buffer
                                    .io()
                                    .read(&mut bytes[..])
                                    .expect("failed to read from the buffer"),
                                DATA_LEN
                            );
                            assert_eq!(u32::from_le_bytes(bytes), i);
                        }
                    };
                    futures::join!(echo_fut, main_fut);
                },
            )
            .await;
        },
    )
    .await;
}

#[fuchsia::test]
async fn test_status_stream() {
    const TOGGLE_COUNT: usize = 3;
    let (tun, tun_port, port) = create_tun_device_and_port().await;
    let client = create_netdev_client(&tun);
    let mut watcher = client.port_status_stream(port).expect("failed to create a status watcher");

    for _ in 0..TOGGLE_COUNT {
        assert_matches!(
            watcher.try_next().await,
            Ok(Some(netdevice_client::PortStatus {
                flags: netdev::StatusFlags::ONLINE,
                mtu: DEFAULT_MTU
            }))
        );
        tun_port.set_online(false).await.expect("failed to flip online flag");
        assert_eq!(
            watcher.try_next().await.expect("failed to get next status update"),
            Some(netdevice_client::PortStatus {
                flags: netdev::StatusFlags::empty(),
                mtu: DEFAULT_MTU
            })
        );
        tun_port.set_online(true).await.expect("failed to flip online flag");
    }
}

#[fuchsia::test]
async fn test_port_stream() {
    let (_tun, mut stream, port) = {
        let (tun, _tun_port, port) = create_tun_device_and_port().await;
        let client = create_netdev_client(&tun);
        let mut stream = client.device_port_event_stream().expect("failed to create port stream");
        assert_matches!(
            stream.try_next().await.expect("failed to get next event"),
            Some(netdev::DevicePortEvent::Existing(p)) if p == port.into()
        );
        assert_matches!(
            stream.try_next().await.expect("failed to get next event"),
            Some(netdev::DevicePortEvent::Idle(netdev::Empty { .. }))
        );
        (tun, stream, port)
    };
    assert_matches!(
        stream.try_next().await.expect("failed to get next event"),
        Some(netdev::DevicePortEvent::Removed(p)) if p == port.into()
    );
}

#[test]
fn tx_wait_idle() {
    let mut executor = fasync::TestExecutor::new();

    let (tun, _tun_port, port) = executor.run_singlethreaded(create_tun_device_and_port());
    let client = create_netdev_client(&tun);

    let session = executor.run_singlethreaded(async {
        let (session, task) = client
            .new_session_with_derivable_config("tx_wait_idle", DerivableConfig::default())
            .await
            .expect("failed to create session");
        session
            .attach(port, &[netdev::FrameType::Ethernet])
            .await
            .expect("failed to attach session");
        // Given we're manually running the executor, detaching the task and
        // panicking on exit is easier than driving it manually and we don't
        // lose any signal.
        fasync::Task::spawn(
            task.map(|res| panic!("the background task for session terminated with {:?}", res)),
        )
        .detach();
        session
    });

    let mut fut = pin!(session.wait_tx_idle());
    assert_eq!(executor.run_until_stalled(&mut fut), Poll::Ready(()));

    // Send 2 buffers.
    executor.run_singlethreaded(async {
        let buffers = session
            .alloc_tx_buffers(DATA_LEN)
            .await
            .expect("failed to alloc tx buffers")
            .take(2)
            .collect::<Result<Vec<_>, _>>()
            .expect("failed to collect tx buffers");
        for mut buffer in buffers {
            assert_eq!(
                buffer
                    .io_mut()
                    .write(&[DATA_BYTE; DATA_LEN][..])
                    .expect("failed to write into the buffer"),
                DATA_LEN
            );
            {
                let mut meta = buffer.meta_mut();
                meta.set_port(port);
                meta.set_frame_type(netdev::FrameType::Ethernet);
            }
            session.send(buffer);
        }
    });

    // We have now 2 outstanding buffers so we try to wait idle it should block.
    let mut fut = pin!(session.wait_tx_idle());
    assert_eq!(executor.run_until_stalled(&mut fut), Poll::Pending);

    // Complete a single frame.
    executor.run_singlethreaded(async {
        let tun::Frame { .. } = tun
            .read_frame()
            .await
            .unwrap()
            .map_err(zx::Status::err_from_raw)
            .expect("failed to read frame from the tun device");
    });

    // Must still be pending.
    assert_eq!(executor.run_until_stalled(&mut fut), Poll::Pending);

    // Complete the second frame then the future will eventually resolve.
    let ((), ()) = executor.run_singlethreaded(futures::future::join(fut, async {
        let tun::Frame { .. } = tun
            .read_frame()
            .await
            .unwrap()
            .map_err(zx::Status::err_from_raw)
            .expect("failed to read frame from the tun device");
    }));
}

#[fuchsia::test]
async fn watch_rx_leases() {
    let (tun, _tun_port, port) = create_tun_device_and_port().await;
    let client = create_netdev_client(&tun);

    with_netdev_session(
        client,
        DerivableConfig { watch_rx_leases: true, ..Default::default() },
        port,
        "watch_rx_leases",
        |session, _client| async move {
            let mut leases_stream = pin!(session.watch_rx_leases());
            let mut rx_ready = RxReadyStorage::new(1);
            for frame_idx in 1..=2 {
                let (lease, lease_send) = zx::Channel::create();
                tun.delegate_rx_lease(netdev::DelegatedRxLease {
                    hold_until_frame: Some(frame_idx),
                    handle: Some(netdev::DelegatedRxLeaseHandle::Channel(lease_send)),
                    __source_breaking: fidl::marker::SourceBreaking,
                })
                .expect("delegate lease");
                let frame = tun::Frame {
                    frame_type: Some(netdev::FrameType::Ethernet),
                    data: Some(vec![1, 2, 3]),
                    port: Some(DEFAULT_PORT_ID),
                    ..Default::default()
                };
                tun.write_frame(&frame).await.expect("writing frame").expect("write frame error");
                let frame = session
                    .recv(&mut rx_ready)
                    .await
                    .expect("receive frame")
                    .next()
                    .unwrap()
                    .expect("receive frame");
                assert_matches!(leases_stream.try_next().now_or_never(), None);
                drop(frame);
                let yielded_lease = leases_stream
                    .try_next()
                    .await
                    .expect("lease error")
                    .expect("lease stream ended unexpectedly");
                let yielded_lease = assert_matches!(yielded_lease.inner(),
                 netdev::DelegatedRxLeaseHandle::Channel(c) => c);
                // Prove we have the same lease.
                assert_eq!(yielded_lease.koid().unwrap(), lease.basic_info().unwrap().related_koid);
            }
        },
    )
    .await;
}

fn default_base_port_config() -> tun::BasePortConfig {
    tun::BasePortConfig {
        id: Some(DEFAULT_PORT_ID),
        mtu: Some(DEFAULT_MTU),
        rx_types: Some(vec![netdev::FrameType::Ethernet]),
        tx_types: Some(vec![netdev::FrameTypeSupport {
            type_: netdev::FrameType::Ethernet,
            features: netdev::FRAME_FEATURES_RAW,
            supported_flags: netdev::TxFlags::empty(),
        }]),
        ..Default::default()
    }
}

async fn create_tun_device_and_port() -> (tun::DeviceProxy, tun::PortProxy, Port) {
    let ctrl =
        connect_to_protocol::<tun::ControlMarker>().expect("failed to connect to tun.Control");
    let (device, server) = endpoints::create_proxy::<tun::DeviceMarker>();
    ctrl.create_device(&tun::DeviceConfig { blocking: Some(true), ..Default::default() }, server)
        .expect("failed to create device");
    let (port, server) = endpoints::create_proxy::<tun::PortMarker>();
    device
        .add_port(
            &tun::DevicePortConfig {
                base: Some(default_base_port_config()),
                online: Some(true),
                ..Default::default()
            },
            server,
        )
        .expect("failed to add port to device");

    let (device_port, server) = endpoints::create_proxy::<netdev::PortMarker>();
    port.get_port(server).expect("get device port");

    let id = device_port.get_info().await.expect("getting port info").id.expect("missing port id");

    (device, port, id.try_into().expect("bad port id"))
}

fn create_tun_device_pair() -> tun::DevicePairProxy {
    let ctrl =
        connect_to_protocol::<tun::ControlMarker>().expect("failed to connect to tun.Control");
    let (pair, server) = endpoints::create_proxy::<tun::DevicePairMarker>();
    ctrl.create_pair(&tun::DevicePairConfig::default(), server).expect("create device pair");
    pair
}

fn create_netdev_client_and_server() -> (Client, endpoints::ServerEnd<netdev::DeviceMarker>) {
    let (device, server) = endpoints::create_proxy::<netdev::DeviceMarker>();
    (Client::new(device), server)
}

fn create_netdev_client(tun: &tun::DeviceProxy) -> Client {
    let (client, server) = create_netdev_client_and_server();
    tun.get_device(server).expect("failed to connect device to tun");
    client
}

async fn create_netdev_client_pair(pair: &tun::DevicePairProxy) -> (Client, Port, Client, Port) {
    let (device_left, left) = endpoints::create_proxy::<netdev::DeviceMarker>();
    let (device_right, right) = endpoints::create_proxy::<netdev::DeviceMarker>();
    pair.get_left(left).expect("failed to connect left");
    pair.get_right(right).expect("failed to connect right");
    pair.add_port(&tun::DevicePairPortConfig {
        base: Some(default_base_port_config()),
        ..Default::default()
    })
    .await
    .unwrap()
    .expect("failed to create the default logical port");

    async fn get_port_id<F: FnOnce(fidl::endpoints::ServerEnd<netdev::PortMarker>)>(f: F) -> Port {
        let (port, server) = fidl::endpoints::create_proxy::<netdev::PortMarker>();
        f(server);
        port.get_info()
            .await
            .expect("getting info")
            .id
            .expect("missing id")
            .try_into()
            .expect("bad port id")
    }

    let port_left =
        get_port_id(|server| pair.get_left_port(DEFAULT_PORT_ID, server).expect("get left port"))
            .await;
    let port_right =
        get_port_id(|server| pair.get_right_port(DEFAULT_PORT_ID, server).expect("get right port"))
            .await;

    (Client::new(device_left), port_left, Client::new(device_right), port_right)
}

async fn with_netdev_session<F, Fut>(
    client: Client,
    config: DerivableConfig,
    port: Port,
    name: &str,
    f: F,
) where
    F: FnOnce(Session, Client) -> Fut,
    Fut: Future<Output = ()>,
{
    let (session, task) = client
        .new_session_with_derivable_config(name, config)
        .await
        .expect("failed to create session");
    let () = session
        .attach(port, &[netdev::FrameType::Ethernet])
        .await
        .expect("failed to attach session");
    futures::select! {
        () = f(session, client).fuse() => {},
        res = task.fuse() => panic!("the background task for session terminated with {:?}", res),
    }
}
