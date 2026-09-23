// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

//! Netstack power framework integration tests.

use std::marker::PhantomData;
use std::pin::pin;
use std::time::Duration;

use assert_matches::assert_matches;
use fidl::endpoints::{DiscoverableProtocolMarker as _, ServiceMarker as _};
use fidl_fuchsia_hardware_network as fhardware_network;
use fidl_fuchsia_hardware_power_suspend as fhsuspend;
use fidl_fuchsia_net_interfaces as fnet_interfaces;
use fidl_fuchsia_net_interfaces_ext as finterfaces_ext;
use fidl_fuchsia_net_power as fnet_power;
use fidl_fuchsia_net_resources as fnet_resources;
use fidl_fuchsia_net_tun as fnet_tun;
use fidl_fuchsia_netemul as fnetemul;
use fidl_fuchsia_posix_socket as fposix_socket;
use fidl_fuchsia_power_broker as fpower_broker;
use fidl_fuchsia_power_system as fpower_system;
use fidl_test_sagcontrol as fsagcontrol;
use fidl_test_suspendcontrol as ftest_suspendcontrol;
use fuchsia_async::{self as fasync, TimeoutExt as _};
use futures::stream::FusedStream;
use futures::{AsyncReadExt as _, AsyncWriteExt as _, FutureExt as _, Stream, StreamExt as _};
use log::info;
use net_declare::{fidl_subnet, std_socket_addr_v6};
use netemul::{RealmTcpStream as _, RealmUdpSocket as _};
use netstack_testing_common::ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT;
use netstack_testing_common::realms::{
    KnownServiceProvider, Netstack3, NetstackVersion, TestSandboxExt as _,
};
use netstack_testing_macros::netstack_test;
use packet::ParsablePacket as _;
use packet_formats::ip::{IpPacket, IpProto, Ipv6Proto};
use packet_formats::ipv6::Ipv6Packet;
use packet_formats::udp::{UdpPacket, UdpParseArgs};
use test_case::test_case;
use zx::Peered as _;

// TODO(https://fxbug.dev/372010366): Revisit this test as we consider better integrating
// fake-suspend with fake SAG.
async fn set_up_default_suspender(device: &ftest_suspendcontrol::DeviceProxy) {
    device
        .set_suspend_states(&ftest_suspendcontrol::DeviceSetSuspendStatesRequest {
            suspend_states: Some(vec![fhsuspend::SuspendState {
                resume_latency: Some(0),
                ..Default::default()
            }]),
            ..Default::default()
        })
        .await
        .expect("fake-suspend set_suspend_states")
        .expect("fake-suspend set_suspend_states")
}

async fn create_power_realm<'a>(
    sandbox: &'a netemul::TestSandbox,
    name: &'a str,
    netstack_suspend_enabled: bool,
) -> netemul::TestRealm<'a> {
    const SUSPENDER_URL: &str = "#meta/fake-suspend.cm";
    const SUSPENDER_NAME: &str = "fake-suspend";

    const SAG_URL: &str = "#meta/fake-system-activity-governor.cm";
    const SAG_NAME: &str = "system-activity-governor";

    const PB_URL: &str = "#meta/power-broker.cm";
    const PB_NAME: &str = "power-broker";

    const CONFIG_USE_SUSPENDER_URL: &str = "config-use-suspender#meta/config-use-suspender.cm";
    const CONFIG_USE_SUSPENDER_NAME: &str = "config-use-suspender";
    const CONFIG_USE_SUSPENDER_CONFIG: &str = "fuchsia.power.UseSuspender";

    const SHUTDOWN_SHIM_URL: &str = "#meta/fake-shutdown-shim.cm";
    const SHUTDOWN_SHIM_NAME: &str = "fake-shutdown-shim";

    const CONFIG_NO_SUSPENDING_TOKEN_CONFIG: &str = "fuchsia.power.WaitForSuspendingToken";
    const CONFIG_SUSPEND_RESUME_STUCK_WARNING_TIMEOUT_CONFIG: &str =
        "fuchsia.power.SuspendResumeStuckWarningTimeout";
    const CONFIG_REBOOT_ON_SUSPEND_STUCK_CONFIG: &str =
        "fuchsia.power.RebootOnStalledSuspendBlocker";
    const CONFIG_LONG_WAKE_LEASE_TIMEOUT_CONFIG: &str = "fuchsia.power.LongWakeLeaseTimeout";

    fn suspender_dep() -> fnetemul::Capability {
        fnetemul::Capability::ChildDep(fnetemul::ChildDep {
            name: Some(SUSPENDER_NAME.to_string()),
            capability: Some(fnetemul::ExposedCapability::Service(
                fhsuspend::SuspendServiceMarker::SERVICE_NAME.to_string(),
            )),
            ..Default::default()
        })
    }

    fn power_broker_dep() -> fnetemul::Capability {
        fnetemul::Capability::ChildDep(fnetemul::ChildDep {
            name: Some(PB_NAME.to_string()),
            capability: Some(fnetemul::ExposedCapability::Protocol(
                fpower_broker::TopologyMarker::PROTOCOL_NAME.to_string(),
            )),
            ..Default::default()
        })
    }

    fn shutdown_shim_dep() -> fnetemul::Capability {
        fnetemul::Capability::ChildDep(fnetemul::ChildDep {
            name: Some(SHUTDOWN_SHIM_NAME.to_string()),
            capability: Some(fnetemul::ExposedCapability::Protocol(
                "fuchsia.hardware.power.statecontrol.ShutdownWatcherRegister".to_string(),
            )),
            ..Default::default()
        })
    }

    let mut netstack_def: fnetemul::ChildDef =
        KnownServiceProvider::Netstack(NetstackVersion::Netstack3).into();
    // Add the dependencies on SAG and PB on top of what netemul is aware of.
    let fnetemul::ChildUses::Capabilities(netstack_uses) =
        netstack_def.uses.get_or_insert_with(|| fnetemul::ChildUses::Capabilities(vec![]));
    netstack_uses.push(power_broker_dep());
    netstack_uses.push(fnetemul::Capability::ChildDep(fnetemul::ChildDep {
        name: Some(SAG_NAME.to_string()),
        capability: Some(fnetemul::ExposedCapability::Protocol(
            fpower_system::ActivityGovernorMarker::PROTOCOL_NAME.to_string(),
        )),
        ..Default::default()
    }));
    netstack_testing_common::realms::set_netstack3_suspend_enabled(
        &mut netstack_def,
        netstack_suspend_enabled,
    );

    let sag_def = fnetemul::ChildDef {
        source: Some(fnetemul::ChildSource::Component(SAG_URL.to_string())),
        name: Some(SAG_NAME.to_string()),
        uses: Some(fnetemul::ChildUses::Capabilities(vec![
            fnetemul::Capability::LogSink(fnetemul::Empty {}),
            power_broker_dep(),
            shutdown_shim_dep(),
            suspender_dep(),
            fnetemul::Capability::ChildDep(fnetemul::ChildDep {
                name: Some(CONFIG_USE_SUSPENDER_NAME.to_string()),
                capability: Some(fnetemul::ExposedCapability::Configuration(
                    CONFIG_USE_SUSPENDER_CONFIG.to_string(),
                )),
                ..Default::default()
            }),
            fnetemul::Capability::ChildDep(fnetemul::ChildDep {
                name: Some(CONFIG_USE_SUSPENDER_NAME.to_string()),
                capability: Some(fnetemul::ExposedCapability::Configuration(
                    CONFIG_SUSPEND_RESUME_STUCK_WARNING_TIMEOUT_CONFIG.to_string(),
                )),
                ..Default::default()
            }),
            fnetemul::Capability::ChildDep(fnetemul::ChildDep {
                name: Some(CONFIG_USE_SUSPENDER_NAME.to_string()),
                capability: Some(fnetemul::ExposedCapability::Configuration(
                    CONFIG_REBOOT_ON_SUSPEND_STUCK_CONFIG.to_string(),
                )),
                ..Default::default()
            }),
            fnetemul::Capability::ChildDep(fnetemul::ChildDep {
                name: Some(CONFIG_USE_SUSPENDER_NAME.to_string()),
                capability: Some(fnetemul::ExposedCapability::Configuration(
                    CONFIG_LONG_WAKE_LEASE_TIMEOUT_CONFIG.to_string(),
                )),
                ..Default::default()
            }),
            fnetemul::Capability::ChildDep(fnetemul::ChildDep {
                name: None,
                capability: Some(fnetemul::ExposedCapability::Configuration(
                    CONFIG_NO_SUSPENDING_TOKEN_CONFIG.to_string(),
                )),
                ..Default::default()
            }),
        ])),
        exposes: Some(vec![
            fpower_system::ActivityGovernorMarker::PROTOCOL_NAME.to_string(),
            fsagcontrol::StateMarker::PROTOCOL_NAME.to_string(),
        ]),
        ..Default::default()
    };

    let suspender_def = fnetemul::ChildDef {
        source: Some(fnetemul::ChildSource::Component(SUSPENDER_URL.to_string())),
        name: Some(SUSPENDER_NAME.to_string()),
        uses: Some(fnetemul::ChildUses::Capabilities(vec![fnetemul::Capability::LogSink(
            fnetemul::Empty {},
        )])),
        exposes: Some(vec![ftest_suspendcontrol::DeviceMarker::PROTOCOL_NAME.to_string()]),
        ..Default::default()
    };

    let pb_def = fnetemul::ChildDef {
        source: Some(fnetemul::ChildSource::Component(PB_URL.to_string())),
        name: Some(PB_NAME.to_string()),
        uses: Some(fnetemul::ChildUses::Capabilities(vec![fnetemul::Capability::LogSink(
            fnetemul::Empty {},
        )])),
        ..Default::default()
    };

    let sag_config_suspender_def = fnetemul::ChildDef {
        source: Some(fnetemul::ChildSource::Component(CONFIG_USE_SUSPENDER_URL.to_string())),
        name: Some(CONFIG_USE_SUSPENDER_NAME.to_string()),
        ..Default::default()
    };

    let shutdown_shim_def = fnetemul::ChildDef {
        source: Some(fnetemul::ChildSource::Component(SHUTDOWN_SHIM_URL.to_string())),
        name: Some(SHUTDOWN_SHIM_NAME.to_string()),
        uses: Some(fnetemul::ChildUses::Capabilities(vec![fnetemul::Capability::LogSink(
            fnetemul::Empty {},
        )])),
        exposes: Some(vec![
            "fuchsia.hardware.power.statecontrol.ShutdownWatcherRegister".to_string(),
        ]),
        ..Default::default()
    };

    let realm = sandbox
        .create_realm(
            name,
            [
                netstack_def,
                sag_def,
                suspender_def,
                pb_def,
                sag_config_suspender_def,
                shutdown_shim_def,
            ],
        )
        .expect("failed to create realm");

    // Start SAG and put it in a good state for all tests before we go starting
    // netstack.
    let sagctl =
        realm.connect_to_protocol::<fsagcontrol::StateMarker>().expect("connect to SAG ctl");
    let mut sag_state = pin!(execution_state_level_stream(&sagctl));
    sagctl
        .set(&fsagcontrol::SystemActivityGovernorState {
            application_activity_level: Some(fpower_system::ApplicationActivityLevel::Active),
            ..Default::default()
        })
        .await
        .expect("SAG set")
        .expect("SAG set");
    assert_eq!(sag_state.next().await, Some(fpower_system::ExecutionStateLevel::Active));

    // Kick SAG out of boot mode.
    sagctl.set_boot_complete().await.expect("SetBootComplete");

    realm
}

fn extract_udp_frame_in_ipv6_packet(ipv6_frame: &[u8]) -> Option<&[u8]> {
    let mut buffer = ipv6_frame;
    let ipv6 = Ipv6Packet::parse(&mut buffer, ()).expect("failed to parse IPv6");
    if ipv6.proto() != Ipv6Proto::Proto(IpProto::Udp) {
        return None;
    }
    let udp = UdpPacket::parse(&mut buffer, UdpParseArgs::new(ipv6.src_ip(), ipv6.dst_ip()))
        .expect("failed to parse UDP");
    Some(udp.into_body())
}

fn execution_state_level_stream(
    sagctl: &fsagcontrol::StateProxy,
) -> impl Stream<Item = fpower_system::ExecutionStateLevel> + FusedStream + '_ {
    futures::stream::unfold(None, move |prev| async move {
        loop {
            let execution_level = sagctl
                .watch()
                .await
                .expect("sagctl watch")
                .execution_state_level
                .expect("missing execution state level");
            if prev.as_ref().map(|l| l != &execution_level).unwrap_or(true) {
                break Some((execution_level, Some(execution_level)));
            }
        }
    })
    .fuse()
}

#[netstack_test]
#[test_case(true; "suspend enabled")]
#[test_case(false; "suspend disabled")]
async fn tx_suspension(name: &str, netstack_suspend_enabled: bool) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm = create_power_realm(&sandbox, name, netstack_suspend_enabled).await;

    let suspend_device = realm
        .connect_to_protocol::<ftest_suspendcontrol::DeviceMarker>()
        .expect("Couldn't connect to suspend device");
    set_up_default_suspender(&suspend_device).await;

    let sagctl =
        realm.connect_to_protocol::<fsagcontrol::StateMarker>().expect("connect to SAG ctl");
    let mut sag_state = pin!(execution_state_level_stream(&sagctl));
    assert_eq!(sag_state.next().await, Some(fpower_system::ExecutionStateLevel::Active));

    let (tun_device, device) =
        netstack_testing_common::devices::create_tun_device_with(fnet_tun::DeviceConfig {
            blocking: Some(true),
            ..Default::default()
        });
    let (tun_port, port) = netstack_testing_common::devices::create_ip_tun_port(
        &tun_device,
        netstack_testing_common::devices::TUN_DEFAULT_PORT_ID,
    )
    .await;
    tun_port.set_online(true).await.expect("set online");

    let mut udp_frame_stream = pin!(
        futures::stream::unfold((), |()| async {
            loop {
                let fnet_tun::Frame { data, frame_type, .. } =
                    tun_device.read_frame().await.expect("FIDL error").expect("read frame error");
                let data = data.unwrap();
                let frame_type = frame_type.unwrap();
                if frame_type != fhardware_network::FrameType::Ipv6 {
                    continue;
                }
                if let Some(udp_frame) = extract_udp_frame_in_ipv6_packet(&data) {
                    break Some((udp_frame.to_vec(), ()));
                }
            }
        })
        .fuse()
    );

    let device_control = netstack_testing_common::devices::install_device(&realm, device);
    let interface_control = finterfaces_ext::admin::Control::new(
        netstack_testing_common::devices::add_pure_ip_interface(&port, &device_control, name).await,
    );
    let if_id = interface_control.get_id().await.expect("get id");
    assert_matches!(interface_control.enable().await, Ok(Ok(true)));

    let src = fidl_subnet!("fe80::1/64");
    let mut dst = std_socket_addr_v6!("[fe80::2]:1010");
    dst.set_scope_id(if_id.try_into().unwrap());

    // Add a local IP address so we can send some traffic.
    let _asp = netstack_testing_common::interfaces::add_address_wait_assigned(
        &interface_control,
        src,
        Default::default(),
    )
    .await
    .expect("add addr");
    let sock = realm
        .datagram_socket(fposix_socket::Domain::Ipv6, fposix_socket::DatagramSocketProtocol::Udp)
        .await
        .expect("create socket");

    let payload = &[1, 2, 3][..];
    assert_eq!(sock.send_to(payload, &dst.into()).expect("sendto"), payload.len());
    // Wait for the frame to show up in tun.
    assert_eq!(udp_frame_stream.next().await.unwrap(), payload);

    // Send another frame and make sure it's ready to be read by tun.
    let payload = &[4, 5, 6][..];
    assert_eq!(sock.send_to(payload, &dst.into()).expect("sendto"), payload.len());
    let tun_signals = tun_device.get_signals().await.expect("get tun signals");
    let _: zx::Signals = fasync::OnSignals::new(
        &tun_signals,
        zx::Signals::from_bits(fnet_tun::Signals::READABLE.bits()).unwrap(),
    )
    .await
    .expect("waiting readable");

    // Allow the system to go through suspension now.
    sagctl
        .set(&fsagcontrol::SystemActivityGovernorState {
            execution_state_level: Some(fpower_system::ExecutionStateLevel::Inactive),
            application_activity_level: Some(fpower_system::ApplicationActivityLevel::Inactive),
            ..Default::default()
        })
        .await
        .expect("SAG set")
        .expect("SAG set");

    if netstack_suspend_enabled {
        // When suspension is enabled, netstack is holding the system up.

        // TODO(https://fxbug.dev/367774549): We could do better than a timeout
        // here, we're just trying to guarantee that netstack is holding the system
        // from suspension. Ideally we'd observe the required and current power
        // levels of the tx power element instead.
        assert_eq!(
            sag_state
                .next()
                .on_timeout(
                    fasync::MonotonicInstant::after(ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT),
                    || None
                )
                .await,
            None
        );
    } else {
        // When netstack suspension is disabled the system goes immediately to
        // inactive.
        assert_eq!(sag_state.next().await, Some(fpower_system::ExecutionStateLevel::Inactive));
    }

    // Now just drain the tun interface until we see the system going inactive
    // (in the suspend enabled case).
    assert_eq!(udp_frame_stream.next().await.unwrap(), payload);

    if netstack_suspend_enabled {
        // Keep polling UDP state stream until we observe inactive in case there
        // were other netstack-originated frames here, but there should be no UDP
        // traffic.
        let system_state = futures::select! {
            s = sag_state.next() => s,
            v = udp_frame_stream.next() => panic!("unexpected extra UDP frame {v:?}"),
        };
        assert_eq!(system_state, Some(fpower_system::ExecutionStateLevel::Inactive));
        assert_eq!(suspend_device.await_suspend().await.unwrap().unwrap().state_index, Some(0));
    }

    // Send more data over the socket.
    let payload = &[7, 8, 9][..];
    assert_eq!(sock.send_to(payload, &dst.into()).expect("sendto"), payload.len());

    if netstack_suspend_enabled {
        // While the system is inactive we should not observe netstack
        // attempting to send anything over the device.
        assert_eq!(
            udp_frame_stream
                .next()
                .on_timeout(
                    fasync::MonotonicInstant::after(ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT),
                    || None
                )
                .await,
            None,
        );

        // But it does come out when the system wakes back up from suspension.
        suspend_device
            .resume(&ftest_suspendcontrol::DeviceResumeRequest::Result(
                ftest_suspendcontrol::SuspendResult { ..Default::default() },
            ))
            .await
            .expect("fake-suspend resume")
            .expect("fake-suspend resume");
        sagctl
            .set(&fsagcontrol::SystemActivityGovernorState {
                execution_state_level: Some(fpower_system::ExecutionStateLevel::Active),
                application_activity_level: Some(fpower_system::ApplicationActivityLevel::Active),
                ..Default::default()
            })
            .await
            .expect("SAG set")
            .expect("SAG set");

        assert_eq!(udp_frame_stream.next().await.unwrap(), payload);
    } else {
        // In the no suspension case, the payload should be available even if
        // SAG is still in the inactive state, because netstack is not observing
        // suspension.
        assert_eq!(udp_frame_stream.next().await.unwrap(), payload);
    }
}

#[netstack_test]
#[test_case(true; "suspend enabled")]
#[test_case(false; "suspend disabled")]
async fn rx_lease_drops(name: &str, netstack_suspend_enabled: bool) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm = create_power_realm(&sandbox, name, netstack_suspend_enabled).await;
    let (tun_device, device) =
        netstack_testing_common::devices::create_tun_device_with(fnet_tun::DeviceConfig {
            blocking: Some(true),
            ..Default::default()
        });
    let (tun_port, port) = netstack_testing_common::devices::create_ip_tun_port(
        &tun_device,
        netstack_testing_common::devices::TUN_DEFAULT_PORT_ID,
    )
    .await;
    tun_port.set_online(true).await.expect("set online");
    let device_control = netstack_testing_common::devices::install_device(&realm, device);
    let interface_control = finterfaces_ext::admin::Control::new(
        netstack_testing_common::devices::add_pure_ip_interface(&port, &device_control, name).await,
    );
    let if_id = interface_control.get_id().await.expect("get id");
    assert_matches!(interface_control.enable().await, Ok(Ok(true)));

    let interfaces_state =
        realm.connect_to_protocol::<fnet_interfaces::StateMarker>().expect("connect to protocol");
    netstack_testing_common::interfaces::wait_for_online(&interfaces_state, if_id, true)
        .await
        .expect("wait online");

    for packet_number in 1..=4 {
        let (lease, send_lease) = zx::Channel::create();
        let frame = fnet_tun::Frame {
            frame_type: Some(fhardware_network::FrameType::Ipv4),
            // NB: We don't need to send a proper packet, we just need it to
            // make it to netstack.
            data: Some(vec![0x01, 0x02, 0x03, 0x04]),
            port: Some(netstack_testing_common::devices::TUN_DEFAULT_PORT_ID),
            ..Default::default()
        };
        let delegated_lease = fhardware_network::DelegatedRxLease {
            handle: Some(fhardware_network::DelegatedRxLeaseHandle::Channel(send_lease)),
            hold_until_frame: Some(packet_number),
            ..Default::default()
        };

        // Make the test more interesting by changing the order of things
        // reaching tun.
        if packet_number % 2 == 0 {
            tun_device.delegate_rx_lease(delegated_lease).expect("delegate lease");
            tun_device.write_frame(&frame).await.expect("write frame").expect("write frame error");
        } else {
            tun_device.write_frame(&frame).await.expect("write frame").expect("write frame error");
            tun_device.delegate_rx_lease(delegated_lease).expect("delegate lease");
        };
        // Lease should always be dropped because netdevice drops leases even
        // when netstack is not subscribed to it.
        assert_eq!(
            lease
                .wait_one(zx::Signals::CHANNEL_PEER_CLOSED, zx::MonotonicInstant::INFINITE)
                .expect("wait closed"),
            zx::Signals::CHANNEL_PEER_CLOSED
        );

        // Check that netstack was the one dropping the lease via inspect. It's
        // sad to depend on the inspect interface here, but we're guaranteeing
        // this is not racy because we're waiting for the lease to be closed
        // above before checking.
        let expect_inspect_value = if netstack_suspend_enabled { packet_number } else { 0 };
        let property = netstack_testing_common::get_inspect_property(
            &realm,
            "netstack",
            "root/Counters/Bindings/Power:DroppedRxLeases",
        )
        .await
        .expect("getting inspect property");
        assert_eq!(property.uint(), Some(expect_inspect_value));
    }
}

trait WakeupSocket {
    const PAYLOAD: &'static str = "hello, world!";

    /// Sets up a pair of connected sockets in the provided realm, with the server
    /// belonging to the provided wake group, such that receiving a message from the
    /// client should notify the wake group.
    async fn setup(
        realm: &netemul::TestRealm<'_>,
        wake_group: fnet_resources::WakeGroupToken,
    ) -> Self;

    /// Sends a message from the client to the server.
    async fn client_write(&mut self);

    /// Reads a message from the client.
    async fn server_read(&mut self);
}

struct TcpSocketPair {
    client: fasync::net::TcpStream,
    server: fasync::net::TcpStream,
}

impl WakeupSocket for TcpSocketPair {
    async fn setup(
        realm: &netemul::TestRealm<'_>,
        wake_group: fnet_resources::WakeGroupToken,
    ) -> Self {
        let socket = realm
            .stream_socket_with_options(
                fposix_socket::Domain::Ipv4,
                fposix_socket::StreamSocketProtocol::Tcp,
                fposix_socket::SocketCreationOptions {
                    group: Some(wake_group),
                    ..Default::default()
                },
            )
            .await
            .expect("create stream socket");
        let server_addr =
            std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 8080);
        socket.bind(&server_addr.into()).expect("bind server socket");
        socket.listen(1).expect("listen on server socket");
        let listener =
            fasync::net::TcpListener::from_std(socket.into()).expect("socket2 into async listener");

        let (client, server) = futures::future::join(
            async {
                fasync::net::TcpStream::connect_in_realm(&realm, server_addr)
                    .await
                    .expect("connect to server")
            },
            async {
                let (_, stream, from) =
                    listener.accept().await.expect("accept incoming connection");
                assert_eq!(from.ip(), std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
                stream
            },
        )
        .await;

        Self { client, server }
    }

    async fn client_write(&mut self) {
        let write_count =
            self.client.write(Self::PAYLOAD.as_bytes()).await.expect("send payload to server");
        assert_eq!(write_count, Self::PAYLOAD.as_bytes().len());
    }

    async fn server_read(&mut self) {
        let mut buf = [0u8; Self::PAYLOAD.as_bytes().len()];
        let read_count = self.server.read(&mut buf).await.expect("read payload from client");
        assert_eq!(read_count, Self::PAYLOAD.as_bytes().len());
        assert_eq!(&buf[..read_count], Self::PAYLOAD.as_bytes());
    }
}

struct UdpSocketPair {
    client: fasync::net::UdpSocket,
    client_addr: std::net::SocketAddr,
    server: fasync::net::UdpSocket,
    server_addr: std::net::SocketAddr,
}

impl WakeupSocket for UdpSocketPair {
    async fn setup(
        realm: &netemul::TestRealm<'_>,
        wake_group: fnet_resources::WakeGroupToken,
    ) -> Self {
        let server = realm
            .datagram_socket_with_options(
                fposix_socket::Domain::Ipv4,
                fposix_socket::DatagramSocketProtocol::Udp,
                fposix_socket::SocketCreationOptions {
                    group: Some(wake_group),
                    ..Default::default()
                },
            )
            .await
            .expect("create datagram socket");
        let server_addr =
            std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 8080);
        server.bind(&server_addr.into()).expect("bind server socket");
        let server =
            fasync::net::UdpSocket::from_socket(server.into()).expect("socket2 into async socket");

        let client = fasync::net::UdpSocket::bind_in_realm(
            &realm,
            std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 0),
        )
        .await
        .expect("bind client socket");
        let client_addr = client.local_addr().expect("get client addr");

        Self { client, client_addr, server, server_addr }
    }

    async fn client_write(&mut self) {
        let write_count = self
            .client
            .send_to(Self::PAYLOAD.as_bytes(), self.server_addr)
            .await
            .expect("send payload to server");
        assert_eq!(write_count, Self::PAYLOAD.as_bytes().len());
    }

    async fn server_read(&mut self) {
        let mut buf = [0u8; Self::PAYLOAD.as_bytes().len()];
        let (read_count, from) =
            self.server.recv_from(&mut buf).await.expect("read payload from client");
        assert_eq!(from, self.client_addr);
        assert_eq!(read_count, Self::PAYLOAD.as_bytes().len());
        assert_eq!(&buf[..read_count], Self::PAYLOAD.as_bytes());
    }
}

const GROUP_WAKEUP_SIGNAL: zx::Signals =
    zx::Signals::from_bits(fnet_power::GROUP_WAKEUP_SIGNAL).unwrap();
const WAITER_AWAKE_SIGNAL: zx::Signals =
    zx::Signals::from_bits(fnet_power::WAITER_AWAKE_SIGNAL).unwrap();
const WAITER_ASLEEP_SIGNAL: zx::Signals =
    zx::Signals::from_bits(fnet_power::WAITER_ASLEEP_SIGNAL).unwrap();

#[netstack_test]
#[test_case(PhantomData::<TcpSocketPair>; "tcp")]
#[test_case(PhantomData::<UdpSocketPair>; "udp")]
async fn wake_group_sockets<S: WakeupSocket>(name: &str, _socket_type: PhantomData<S>) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    let provider = realm
        .connect_to_protocol::<fnet_power::WakeGroupProviderMarker>()
        .expect("connect to protocol");
    let (wake_watcher, wake_watcher_other) = zx::EventPair::create();
    wake_watcher_other
        .signal_peer(zx::Signals::NONE, WAITER_AWAKE_SIGNAL)
        .expect("signal initial awake state");
    let fnet_power::CreateWakeGroupResponse { token, .. } = provider
        .create_wake_group(&fnet_power::WakeGroupOptions::default(), wake_watcher)
        .await
        .expect("create wake group");
    let fnet_resources::WakeGroupToken { token } =
        token.expect("netstack must provide wake group token");

    // Create a client and server socket, and add the server socket to a wake group.
    let mut setup = S::setup(
        &realm,
        fnet_resources::WakeGroupToken {
            token: token
                .duplicate_handle(zx::Rights::TRANSFER | zx::Rights::DUPLICATE | zx::Rights::WAIT)
                .expect("duplicate wake group handle"),
        },
    )
    .await;

    // Subscribe to be woken up when data arrives.
    let mut fut = pin!(fasync::OnSignals::new(&token, GROUP_WAKEUP_SIGNAL).fuse());
    assert_matches!((&mut fut).now_or_never(), None);

    // If we send some data while the peer is awake, we will not be
    // notified.
    setup.client_write().await;
    assert_matches!((&mut fut).now_or_never(), None);
    setup.server_read().await;

    // If the peer signals that it is asleep, incoming data should notify the wake group.
    wake_watcher_other
        .signal_peer(WAITER_AWAKE_SIGNAL, WAITER_ASLEEP_SIGNAL)
        .expect("signal suspend");

    // There's no explicit synchronization with the netstack. The netstack has
    // to have observed our asleep signal for it to signal us, so we retry a few
    // times to give it a chance to catch up.
    let mut saw_wakeup = false;
    for _ in 0..60 {
        setup.client_write().await;

        futures::select! {
            signals = fut => {
                assert_eq!(signals.expect("wait for data"), GROUP_WAKEUP_SIGNAL);
                saw_wakeup = true;
                break;
            }
            _ = fasync::Timer::new(Duration::from_secs(1)).fuse() => {
                info!("Timed out waiting for wake signal after 1 second");
            }
        }
    }

    if !saw_wakeup {
        panic!("Never saw wakeup signal from netstack");
    }

    wake_watcher_other
        .signal_peer(WAITER_ASLEEP_SIGNAL, WAITER_AWAKE_SIGNAL)
        .expect("signal resume");
    setup.server_read().await;

    // Closing the wake watcher will unregister the wake group, but the socket
    // can still be used after the wake group it was attached to has become
    // defunct.
    drop(wake_watcher_other);
    setup.client_write().await;
    setup.server_read().await;
}
