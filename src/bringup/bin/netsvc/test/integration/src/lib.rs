// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::prelude::*;
use fidl_fuchsia_hardware_network as fhardware_network;
use fidl_fuchsia_net_tun as fnet_tun;
use fidl_fuchsia_netemul_network as fnetemul_network;
use futures::{Future, FutureExt as _, StreamExt as _, TryStreamExt as _};
use net_declare::{fidl_mac, std_ip_v6};
use net_types::Witness as _;
use net_types::ip::Ipv6;
use netemul::RealmUdpSocket as _;
use netstack_testing_common::interfaces::TestInterfaceExt as _;
use netstack_testing_common::realms::{Netstack2, TestSandboxExt as _};
use netstack_testing_macros::netstack_test;
use netsvc_proto::{debuglog, netboot, tftp};
use packet::{
    FragmentedBuffer as _, InnerPacketBuilder as _, MaybeReuseBufferProvider,
    NestablePacketBuilder as _, NoOpSerializationContext, ParseBuffer as _, Serializer,
};
use std::borrow::Cow;
use std::convert::{TryFrom as _, TryInto as _};
use std::num::NonZeroU16;
use test_case::test_case;
use zerocopy::byteorder::native_endian::U32;
use zerocopy::{FromBytes, Immutable, KnownLayout, Ref, Unaligned};

const NETSVC_URL: &str = "#meta/netsvc.cm";
const NETSVC_NAME: &str = "netsvc";

const NAME_PROVIDER_URL: &str = "#meta/device-name-provider.cm";
const NAME_PROVIDER_NAME: &str = "device-name-provider";

const MOCK_SERVICES_NAME: &str = "mock";

const DEV_NETWORK_DIRECTORY: &str = "dev-class-network";

const PRIMARY_INTERFACE_CONFIGURATION: &str = "fuchsia.network.PrimaryInterface";
const NAMEGEN_CONFIGURATION: &str = "fuchsia.zircon.namegen";
const NODENAME_CONFIGURATION: &str = "fuchsia.zircon.nodename";
const DISABLE_CONFIGURATION: &str = "fuchsia.netsvc.disable";
const NETBOOT_CONFIGURATION: &str = "fuchsia.netsvc.netboot";
const ADVERTISE_CONFIGURATION: &str = "fuchsia.netsvc.advertise";
const ALL_FEATURES_CONFIGURATION: &str = "fuchsia.netsvc.all-features";
const BOOT_ITEM_INTERFACE_FOR_NODE_NAME_CONFIGURATION: &str =
    "fuchsia.device.BootItemInterfaceForNodeName";
const BOOT_ITEM_INTERFACE_FLIP_LOW_MAC_BIT_CONFIGURATION: &str =
    "fuchsia.device.BootItemInterfaceFlipLowMacBit";

const BUFFER_SIZE: usize = 2048;

const MOCK_BOARD_NAME: &str = "mock-board";
const MOCK_BOOTLOADER_VENDOR: &str = "mock-bootloader-vendor";
const MOCK_BOARD_REVISION: u32 = 0xDEADBEEF;

const LOG_MSG_PID: u64 = 1234;
const LOG_MSG_TID: u64 = 6789;
const LOG_MSG_TAG: &str = "tag";
const LOG_MSG_CONTENTS: &str = "hello world";

fn create_netsvc_realm<'a, N, T, V>(
    sandbox: &'a netemul::TestSandbox,
    name: N,
    args: V,
) -> (netemul::TestRealm<'a>, impl Future<Output = ()> + Unpin)
where
    N: Into<Cow<'a, str>>,
    T: Into<String>,
    V: IntoIterator<Item = T>,
{
    use fuchsia_component::server::{ServiceFs, ServiceFsDir};

    let (mock_dir, server_end) = fidl::endpoints::create_endpoints();

    enum Services {
        Log(fidl_fuchsia_logger::LogRequestStream),
        SysInfo(fidl_fuchsia_sysinfo::SysInfoRequestStream),
    }

    let mut fs = ServiceFs::new();
    let _: &mut ServiceFsDir<'_, _> =
        fs.dir("svc").add_fidl_service(Services::Log).add_fidl_service(Services::SysInfo);
    let _: &mut ServiceFs<_> = fs.serve_connection(server_end).expect("serve connection");

    let fs = fs.for_each_concurrent(None, |r| async move {
        match r {
            Services::Log(rs) => {
                let () = rs
                    .for_each_concurrent(None, |req| async move {
                        let log_listener = match req.expect("request stream error") {
                            fidl_fuchsia_logger::LogRequest::ListenSafe {
                                log_listener,
                                control_handle: _,
                                options,
                            } => {
                                assert_eq!(options, None);
                                log_listener.into_proxy()
                            }
                        };
                        // NB: Start iterator at 1 so it matches debuglog
                        // sequence numbers.
                        let messages_gen = (1..).map(|v| fidl_fuchsia_logger::LogMessage {
                            pid: LOG_MSG_PID,
                            tid: LOG_MSG_TID,
                            time: zx::BootInstant::from_nanos(
                                zx::BootDuration::from_seconds(v).into_nanos()),
                            severity: fidl_fuchsia_logger::LOG_LEVEL_DEFAULT.into_primitive().into(),
                            dropped_logs: 0,
                            tags: vec![LOG_MSG_TAG.to_string()],
                            msg: LOG_MSG_CONTENTS.to_string(),
                        });
                        let log_listener = &log_listener;
                        futures::stream::iter(messages_gen)
                            .for_each(|msg| async move {
                                let () = log_listener
                                    .log(&msg)
                                    .await
                                    .expect("failed to send log to listener");
                            })
                            .await;
                    })
                    .await;
            }
            Services::SysInfo(rs) => {
                let () = rs
                    .for_each(|req| {
                        futures::future::ready(
                            match req.expect("request stream error") {
                                fidl_fuchsia_sysinfo::SysInfoRequest::GetBoardName { responder } => {
                                    responder.send(zx::sys::ZX_OK, Some(MOCK_BOARD_NAME))
                                }
                                fidl_fuchsia_sysinfo::SysInfoRequest::GetBoardRevision { responder } => {
                                    responder.send(zx::sys::ZX_OK, MOCK_BOARD_REVISION)
                                }
                                fidl_fuchsia_sysinfo::SysInfoRequest::GetBootloaderVendor { responder } => {
                                    responder.send(zx::sys::ZX_OK, Some(MOCK_BOOTLOADER_VENDOR))
                                }
                                r @ fidl_fuchsia_sysinfo::SysInfoRequest::GetInterruptControllerInfo {
                                    ..
                                } => panic!("unsupported request {:?}", r),
                                r @ fidl_fuchsia_sysinfo::SysInfoRequest::GetSerialNumber {
                                    ..
                                } => panic!("unsupported request {:?}", r),
                            }
                                .expect("failed to send response"),
                        )
                    })
                    .await;
            }
        }
    });

    let realm = sandbox
        .create_realm(
            name,
            [
                fidl_fuchsia_netemul::ChildDef {
                    source: Some(fidl_fuchsia_netemul::ChildSource::Component(
                        NETSVC_URL.to_string(),
                    )),
                    name: Some(NETSVC_NAME.to_string()),
                    program_args: Some(args.into_iter().map(Into::into).collect()),
                    uses: Some(fidl_fuchsia_netemul::ChildUses::Capabilities(vec![
                        fidl_fuchsia_netemul::Capability::NetemulDevfs(
                            fidl_fuchsia_netemul::DevfsDep {
                                name: Some(DEV_NETWORK_DIRECTORY.to_string()),
                                subdir: Some(netemul::NETDEVICE_DEVFS_PATH.to_string()),
                                ..Default::default()
                            },
                        ),
                        fidl_fuchsia_netemul::Capability::ChildDep(
                            fidl_fuchsia_netemul::ChildDep {
                                capability: Some(
                                    fidl_fuchsia_netemul::ExposedCapability::Configuration(
                                        PRIMARY_INTERFACE_CONFIGURATION.to_string(),
                                    ),
                                ),
                                ..Default::default()
                            },
                        ),
                        fidl_fuchsia_netemul::Capability::ChildDep(
                            fidl_fuchsia_netemul::ChildDep {
                                capability: Some(
                                    fidl_fuchsia_netemul::ExposedCapability::Configuration(
                                        DISABLE_CONFIGURATION.to_string(),
                                    ),
                                ),
                                ..Default::default()
                            },
                        ),
                        fidl_fuchsia_netemul::Capability::ChildDep(
                            fidl_fuchsia_netemul::ChildDep {
                                capability: Some(
                                    fidl_fuchsia_netemul::ExposedCapability::Configuration(
                                        NETBOOT_CONFIGURATION.to_string(),
                                    ),
                                ),
                                ..Default::default()
                            },
                        ),
                        fidl_fuchsia_netemul::Capability::ChildDep(
                            fidl_fuchsia_netemul::ChildDep {
                                capability: Some(
                                    fidl_fuchsia_netemul::ExposedCapability::Configuration(
                                        ADVERTISE_CONFIGURATION.to_string(),
                                    ),
                                ),
                                ..Default::default()
                            },
                        ),
                        fidl_fuchsia_netemul::Capability::ChildDep(
                            fidl_fuchsia_netemul::ChildDep {
                                capability: Some(
                                    fidl_fuchsia_netemul::ExposedCapability::Configuration(
                                        ALL_FEATURES_CONFIGURATION.to_string(),
                                    ),
                                ),
                                ..Default::default()
                            },
                        ),
                        fidl_fuchsia_netemul::Capability::ChildDep(
                            fidl_fuchsia_netemul::ChildDep {
                                name: Some(NAME_PROVIDER_NAME.to_string()),
                                capability: Some(
                                    fidl_fuchsia_netemul::ExposedCapability::Protocol(
                                        fidl_fuchsia_device::NameProviderMarker::PROTOCOL_NAME
                                            .to_string(),
                                    ),
                                ),
                                ..Default::default()
                            },
                        ),
                        fidl_fuchsia_netemul::Capability::ChildDep(
                            fidl_fuchsia_netemul::ChildDep {
                                name: Some(MOCK_SERVICES_NAME.to_string()),
                                capability: Some(
                                    fidl_fuchsia_netemul::ExposedCapability::Protocol(
                                        fidl_fuchsia_logger::LogMarker::PROTOCOL_NAME.to_string(),
                                    ),
                                ),
                                ..Default::default()
                            },
                        ),
                        fidl_fuchsia_netemul::Capability::ChildDep(
                            fidl_fuchsia_netemul::ChildDep {
                                name: Some(MOCK_SERVICES_NAME.to_string()),
                                capability: Some(
                                    fidl_fuchsia_netemul::ExposedCapability::Protocol(
                                        fidl_fuchsia_sysinfo::SysInfoMarker::PROTOCOL_NAME
                                            .to_string(),
                                    ),
                                ),
                                ..Default::default()
                            },
                        ),
                        fidl_fuchsia_netemul::Capability::LogSink(fidl_fuchsia_netemul::Empty {}),
                    ])),
                    eager: Some(true),
                    ..Default::default()
                },
                fidl_fuchsia_netemul::ChildDef {
                    source: Some(fidl_fuchsia_netemul::ChildSource::Component(
                        NAME_PROVIDER_URL.to_string(),
                    )),
                    name: Some(NAME_PROVIDER_NAME.to_string()),
                    uses: Some(fidl_fuchsia_netemul::ChildUses::Capabilities(vec![
                        fidl_fuchsia_netemul::Capability::NetemulDevfs(
                            fidl_fuchsia_netemul::DevfsDep {
                                name: Some(DEV_NETWORK_DIRECTORY.to_string()),
                                subdir: Some(netemul::NETDEVICE_DEVFS_PATH.to_string()),
                                ..Default::default()
                            },
                        ),
                        fidl_fuchsia_netemul::Capability::ChildDep(
                            fidl_fuchsia_netemul::ChildDep {
                                capability: Some(
                                    fidl_fuchsia_netemul::ExposedCapability::Configuration(
                                        PRIMARY_INTERFACE_CONFIGURATION.to_string(),
                                    ),
                                ),
                                ..Default::default()
                            },
                        ),
                        fidl_fuchsia_netemul::Capability::ChildDep(
                            fidl_fuchsia_netemul::ChildDep {
                                capability: Some(
                                    fidl_fuchsia_netemul::ExposedCapability::Configuration(
                                        NODENAME_CONFIGURATION.to_string(),
                                    ),
                                ),
                                ..Default::default()
                            },
                        ),
                        fidl_fuchsia_netemul::Capability::ChildDep(
                            fidl_fuchsia_netemul::ChildDep {
                                capability: Some(
                                    fidl_fuchsia_netemul::ExposedCapability::Configuration(
                                        NAMEGEN_CONFIGURATION.to_string(),
                                    ),
                                ),
                                ..Default::default()
                            },
                        ),
                        fidl_fuchsia_netemul::Capability::ChildDep(
                            fidl_fuchsia_netemul::ChildDep {
                                capability: Some(
                                    fidl_fuchsia_netemul::ExposedCapability::Configuration(
                                        BOOT_ITEM_INTERFACE_FOR_NODE_NAME_CONFIGURATION.to_string(),
                                    ),
                                ),
                                ..Default::default()
                            },
                        ),
                        fidl_fuchsia_netemul::Capability::ChildDep(
                            fidl_fuchsia_netemul::ChildDep {
                                capability: Some(
                                    fidl_fuchsia_netemul::ExposedCapability::Configuration(
                                        BOOT_ITEM_INTERFACE_FLIP_LOW_MAC_BIT_CONFIGURATION
                                            .to_string(),
                                    ),
                                ),
                                ..Default::default()
                            },
                        ),
                        fidl_fuchsia_netemul::Capability::LogSink(fidl_fuchsia_netemul::Empty {}),
                    ])),
                    exposes: Some(vec![
                        fidl_fuchsia_device::NameProviderMarker::PROTOCOL_NAME.to_string(),
                    ]),
                    ..Default::default()
                },
                fidl_fuchsia_netemul::ChildDef {
                    source: Some(fidl_fuchsia_netemul::ChildSource::Mock(mock_dir)),
                    name: Some(MOCK_SERVICES_NAME.to_string()),
                    ..Default::default()
                },
            ],
        )
        .expect("create realm");

    (realm, fs)
}

async fn with_netsvc_and_netstack_full<F1, F2, Fut2, X, A, V>(
    port: Option<NonZeroU16>,
    name: &str,
    args: V,
    prepare_realm: F1,
    test: F2,
) where
    // NOTE: We use a boxed future here so we can tie the lifetime of the
    // returned future to the closure, which is necessary because we don't have
    // higher-kinded types.
    F1: for<'a> FnOnce(&'a netemul::TestRealm<'a>) -> futures::future::BoxFuture<'a, X>,
    F2: FnOnce(fuchsia_async::net::UdpSocket, u32, X) -> Fut2,
    Fut2: futures::Future<Output = ()>,
    A: Into<String>,
    V: IntoIterator<Item = A>,
{
    let netsvc_name = format!("{}-netsvc", name);
    let ns_name = format!("{}-netstack", name);

    // Create an event stream watcher before starting any realms so we're sure
    // to observe netsvc early stop events.
    let mut component_event_stream = netstack_testing_common::get_component_stopped_event_stream()
        .await
        .expect("get event stream");

    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let (netsvc_realm, services) = create_netsvc_realm(&sandbox, &netsvc_name, args);

    let netsvc_stopped_fut = netstack_testing_common::wait_for_component_stopped_with_stream(
        &mut component_event_stream,
        &netsvc_realm,
        NETSVC_NAME,
        None,
    );

    let network = sandbox.create_network("net").await.expect("create network");
    let ep = network.create_endpoint(&netsvc_name).await.expect("create endpoint");
    let () = ep.set_link_up(true).await.expect("set link up");

    let () = netsvc_realm
        .add_virtual_device(&ep, netemul::devfs_device_path("ep").as_path())
        .await
        .expect("add virtual device");

    let netstack_realm =
        sandbox.create_netstack_realm::<Netstack2, _>(&ns_name).expect("create netstack realm");

    let interface: netemul::TestInterface<'_> =
        netstack_realm.join_network(&network, &ns_name).await.expect("join network");
    interface.apply_nud_flake_workaround().await.expect("nud flake workaround");

    let _: net_types::ip::Ipv6Addr = netstack_testing_common::interfaces::wait_for_v6_ll(
        &netstack_realm
            .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
            .expect("connect to protocol"),
        interface.id(),
    )
    .await
    .expect("wait ll address");

    // Bind to the specified ports to avoid later binding to an unspecified port
    // that ends up matching these. Used by tests to avoid receiving unexpected
    // traffic.

    let (port, _captive_port_socks) = if let Some(port) = port {
        (port.get(), vec![])
    } else {
        const AVOID_PORTS: [u16; 2] = [debuglog::MULTICAST_PORT.get(), netboot::ADVERT_PORT.get()];
        let captive_ports_socks = futures::stream::iter(AVOID_PORTS.into_iter())
            .then(|port| {
                fuchsia_async::net::UdpSocket::bind_in_realm(
                    &netstack_realm,
                    std::net::SocketAddrV6::new(
                        std::net::Ipv6Addr::UNSPECIFIED,
                        port,
                        /* flowinfo */ 0,
                        /* scope id */ 0,
                    )
                    .into(),
                )
                .map(move |r| r.unwrap_or_else(|e| panic!("bind in realm with {port}: {:?}", e)))
            })
            .collect::<Vec<_>>()
            .await;
        (0, captive_ports_socks)
    };

    let sock = fuchsia_async::net::UdpSocket::bind_in_realm(
        &netstack_realm,
        std::net::SocketAddrV6::new(
            std::net::Ipv6Addr::UNSPECIFIED,
            port,
            /* flowinfo */ 0,
            /* scope id */ 0,
        )
        .into(),
    )
    .await
    .expect("bind in realm");

    // Disable looping multicast sockets back to us; That prevents us from
    // seeing our own generated multicast traffic in case the local port we get
    // matches some netsvc service port.
    let () = sock.as_ref().set_multicast_loop_v6(false).expect("failed to disable multicast loop");

    let custom_args = prepare_realm(&netstack_realm).await;

    let test_fut =
        test(sock, interface.id().try_into().expect("interface ID doesn't fit u32"), custom_args);
    futures::select! {
        r = netsvc_stopped_fut.fuse() => {
            let e: component_events::events::Stopped = r.expect("failed to observe stopped event");
            panic!("netsvc stopped unexpectedly with {:?}", e);
        },
        () = services.fuse() => panic!("ServiceFs ended unexpectedly"),
        () =  test_fut.fuse() => (),
    }
}

const DEFAULT_NETSVC_ARGS: [&str; 4] = ["--enable", "--netboot", "--all-features", "--log-packets"];

async fn with_netsvc_and_netstack_bind_port<F, Fut, A, V>(
    port: Option<NonZeroU16>,
    name: &str,
    args: V,
    test: F,
) where
    F: FnOnce(fuchsia_async::net::UdpSocket, u32) -> Fut,
    Fut: futures::Future<Output = ()>,
    A: Into<String>,
    V: IntoIterator<Item = A>,
{
    with_netsvc_and_netstack_full(
        port,
        name,
        args,
        |_: &netemul::TestRealm<'_>| futures::future::ready(()).boxed(),
        |sock, scope_id, ()| test(sock, scope_id),
    )
    .await
}

async fn with_netsvc_and_netstack<F, Fut>(name: &str, test: F)
where
    F: FnOnce(fuchsia_async::net::UdpSocket, u32) -> Fut,
    Fut: futures::Future<Output = ()>,
{
    with_netsvc_and_netstack_bind_port(None, name, DEFAULT_NETSVC_ARGS, test).await
}

async fn discover(sock: &fuchsia_async::net::UdpSocket, scope_id: u32) -> std::net::Ipv6Addr {
    const ARG: u32 = 0;
    let mut cookie = 1234;

    // NB: We can't guarantee there isn't a race between netsvc starting and all
    // the socket set up. The safe way is to send queries periodically in case
    // netsvc misses our first query to prevent flakes.
    let mut send_interval = futures::stream::once(futures::future::ready(()))
        .chain(fuchsia_async::Interval::new(zx::MonotonicDuration::from_seconds(1)));

    let mut buf = [0; BUFFER_SIZE];

    loop {
        enum Action<'a> {
            Poll,
            Data(&'a [u8], std::net::SocketAddr),
        }

        let action = futures::select! {
            n = send_interval.next() => {
                let () = n.expect("interval stream ended unexpectedly");
                Action::Poll
            }
            r = sock.recv_from(&mut buf[..]).fuse() => {
                let (n, addr) = r.expect("recv_from failed");
                Action::Data(&buf[..n], addr)
            }
        };

        match action {
            Action::Poll => {
                cookie += 1;
                // Build a query for all nodes ("*" + null termination).
                let query = netboot::NetbootPacketBuilder::new(
                    netboot::OpcodeOrErr::Op(netboot::Opcode::Query),
                    cookie,
                    ARG,
                )
                .wrap_body(("*\0".as_bytes()).into_serializer())
                .serialize_vec_outer(&mut NoOpSerializationContext)
                .expect("serialize query")
                .unwrap_b();

                let sent = sock
                    .send_to(
                        query.as_ref(),
                        std::net::SocketAddrV6::new(
                            net_types::ip::Ipv6::ALL_NODES_LINK_LOCAL_MULTICAST_ADDRESS
                                .into_addr()
                                .into(),
                            netboot::SERVER_PORT.get(),
                            /* flowinfo */ 0,
                            scope_id,
                        )
                        .into(),
                    )
                    .await
                    .expect("sendto");
                assert_eq!(sent, query.len());
            }
            Action::Data(mut buf, addr) => {
                let pkt = buf.parse::<netboot::NetbootPacket<_>>().expect("failed to parse");
                assert_eq!(pkt.command(), netboot::OpcodeOrErr::Op(netboot::Opcode::Ack));
                if pkt.cookie() != cookie {
                    println!("ignoring {:?} with old cookie", pkt);
                    continue;
                }
                assert_eq!(pkt.arg(), ARG);
                let nodename =
                    std::str::from_utf8(pkt.payload()).expect("failed to parse advertisement");
                assert!(nodename.starts_with("fuchsia-"), "invalid nodename {}", nodename);

                let ip = match addr {
                    std::net::SocketAddr::V4(v4) => panic!("unexpected v4 sender: {}", v4),
                    std::net::SocketAddr::V6(addr) => {
                        assert_eq!(addr.port(), netboot::SERVER_PORT.get());

                        let ip = addr.ip();
                        // Should be link local address with non zero scope ID.
                        assert!(
                            net_types::ip::Ipv6Addr::from_bytes(ip.octets())
                                .is_unicast_link_local(),
                            "bad address {}",
                            ip
                        );
                        assert_ne!(addr.scope_id(), 0);

                        ip.clone()
                    }
                };

                break ip;
            }
        }
    }
}

async fn send_message<S>(ser: S, sock: &fuchsia_async::net::UdpSocket, to: std::net::SocketAddr)
where
    S: Serializer<NoOpSerializationContext> + std::fmt::Debug,
    S::Buffer: packet::ReusableBuffer + std::fmt::Debug + AsRef<[u8]>,
{
    let b = ser
        .serialize_outer(
            &mut NoOpSerializationContext,
            MaybeReuseBufferProvider(|length| {
                assert!(length <= BUFFER_SIZE, "{} > {}", length, BUFFER_SIZE);
                Result::<_, std::convert::Infallible>::Ok(packet::Buf::new([0u8; BUFFER_SIZE], ..))
            }),
        )
        .expect("failed to serialize");
    let sent = sock.send_to(b.as_ref(), to).await.expect("send to failed");
    assert_eq!(sent, b.len());
}

async fn read_message<'a, P, B>(
    buffer: &'a mut B,
    sock: &fuchsia_async::net::UdpSocket,
    expect_src: std::net::SocketAddr,
) -> P
where
    P: packet::ParsablePacket<&'a [u8], ()>,
    P::Error: std::fmt::Debug,
    B: packet::ParseBufferMut,
{
    let (n, addr) = sock.recv_from(buffer.as_mut()).await.expect("recv from failed");
    assert_eq!(addr, expect_src);
    let () = buffer.shrink_back_to(n);
    buffer.parse::<P>().expect("parse failed")
}

async fn can_discover_inner(sock: fuchsia_async::net::UdpSocket, scope_id: u32) {
    let _: std::net::Ipv6Addr = discover(&sock, scope_id).await;
}

#[netstack_test]
async fn can_discover(name: &str) {
    with_netsvc_and_netstack(name, can_discover_inner).await;
}

async fn debuglog_inner(sock: fuchsia_async::net::UdpSocket, _scope_id: u32) {
    #[derive(Clone)]
    enum Ack {
        Yes,
        No,
    }
    // Test that we observe and acknowledge multiple log messages. Then assert
    // that an unacknowledged message gets resent.
    // The delay for retransmission is low on the first retransmission, which
    // should not make this test unnecessarily long, but we keep it to one
    // observation of that event.
    let _: (fuchsia_async::net::UdpSocket, Option<u32>) =
        futures::stream::iter(std::iter::repeat(Ack::Yes).take(10).chain(std::iter::once(Ack::No)))
            .fold((sock, None), |(sock, seqno), ack| async move {
                let mut buf = [0; BUFFER_SIZE];
                let (n, addr) = sock.recv_from(&mut buf[..]).await.expect("recv_from failed");
                let mut bv = &buf[..n];
                let pkt = bv.parse::<debuglog::DebugLogPacket<_>>().expect("parse failed");

                match ack {
                    Ack::Yes => {
                        let () = send_message(
                            debuglog::AckPacketBuilder::new(pkt.seqno()).into_serializer(),
                            &sock,
                            addr,
                        )
                        .await;
                    }
                    Ack::No => (),
                }

                let seqno = match seqno {
                    None => pkt.seqno(),
                    Some(s) => {
                        if pkt.seqno() <= s {
                            // Don't verify repeat or old packets.
                            return (sock, Some(s));
                        }
                        let nxt = s + 1;
                        assert_eq!(pkt.seqno(), nxt);
                        nxt
                    }
                };

                let nodename = pkt.nodename();
                assert!(nodename.starts_with("fuchsia-"), "bad nodename {}", nodename);
                let msg: &str = pkt.data();
                assert_eq!(
                    msg,
                    format!(
                        "[{:05}.000] {:05}.{:05} [{}] {}\n",
                        seqno, LOG_MSG_PID, LOG_MSG_TID, LOG_MSG_TAG, LOG_MSG_CONTENTS,
                    )
                );

                // Wait for a repeat of the packet if we didn't ack.
                match ack {
                    Ack::No => {
                        // NB: we need to read into a new buffer because we use
                        // variables stored in the old one for comparison.
                        let mut buf = [0; BUFFER_SIZE];
                        let (n, next_addr) =
                            sock.recv_from(&mut buf[..]).await.expect("recv_from failed");
                        let mut bv = &buf[..n];
                        let pkt = bv.parse::<debuglog::DebugLogPacket<_>>().expect("parse failed");
                        assert_eq!(next_addr, addr);
                        assert_eq!(pkt.seqno(), seqno);
                        assert_eq!(pkt.nodename(), nodename);
                        assert_eq!(pkt.data(), msg);
                    }
                    Ack::Yes => (),
                }

                (sock, Some(seqno))
            })
            .await;
}

#[netstack_test]
async fn debuglog(name: &str) {
    with_netsvc_and_netstack_bind_port(
        Some(debuglog::MULTICAST_PORT),
        name,
        DEFAULT_NETSVC_ARGS,
        debuglog_inner,
    )
    .await
}

async fn get_board_info_inner(sock: fuchsia_async::net::UdpSocket, scope_id: u32) {
    const BOARD_NAME_FILE: &str = "<<image>>board_info";
    let device = discover(&sock, scope_id).await;
    let socket_addr = std::net::SocketAddrV6::new(
        device,
        tftp::INCOMING_PORT.get(),
        /* flowinfo */ 0,
        scope_id,
    )
    .into();

    // Request a very large timeout to make sure we don't get flakes.
    const TIMEOUT_OPTION_SECS: u8 = u8::MAX;

    #[repr(C)]
    #[derive(KnownLayout, FromBytes, Immutable, Unaligned)]
    // Defined in zircon/system/public/zircon/boot/netboot.h.
    struct BoardInfo {
        board_name: [u8; 32],
        board_revision: U32,
        mac_address: [u8; 6],
        _padding: [u8; 2],
    }

    let () = send_message(
        tftp::TransferRequestBuilder::new_with_options(
            tftp::TransferDirection::Read,
            BOARD_NAME_FILE,
            tftp::TftpMode::OCTET,
            [
                tftp::TftpOption::TransferSize(u64::MAX).not_forced(),
                tftp::TftpOption::Timeout(TIMEOUT_OPTION_SECS).not_forced(),
            ],
        )
        .into_serializer(),
        &sock,
        socket_addr,
    )
    .await;

    // After the first message, everything must happen on a different port.
    let socket_addr = std::net::SocketAddrV6::new(
        device,
        tftp::OUTGOING_PORT.get(),
        /* flowinfo */ 0,
        scope_id,
    )
    .into();

    expect_oack(
        &sock,
        socket_addr,
        tftp::AllOptions {
            window_size: None,
            block_size: None,
            timeout: Some(tftp::Forceable { value: TIMEOUT_OPTION_SECS, forced: false }),
            transfer_size: Some(tftp::Forceable {
                value: u64::try_from(std::mem::size_of::<BoardInfo>()).expect("doesn't fit u64"),
                forced: false,
            }),
        },
    )
    .await;

    // Acknowledge options by sending an ack.
    let () = send_message(
        tftp::AckPacketBuilder::new(/* block */ 0).into_serializer(),
        &sock,
        socket_addr,
    )
    .await;

    {
        let mut buffer = [0u8; BUFFER_SIZE];
        let mut pb = &mut buffer[..];
        let data = read_message::<tftp::TftpPacket<_>, _>(&mut pb, &sock, socket_addr)
            .await
            .into_data()
            .expect("unexpected message");
        assert_eq!(data.block(), 1);
        assert_eq!(data.payload().len(), std::mem::size_of::<BoardInfo>());
        let board_info = Ref::<_, BoardInfo>::from_bytes(data.payload().as_ref())
            .expect("failed to get board info");
        let BoardInfo { board_name, board_revision, mac_address, _padding } = &*board_info;
        // mac_address is not filled by netsvc.
        assert_eq!(mac_address, [0u8; 6].as_ref());
        assert_eq!(board_revision.get(), MOCK_BOARD_REVISION);
        let board_name =
            board_name.split(|b| *b == 0).next().expect("failed to find null termination");
        let board_name = std::str::from_utf8(board_name).expect("failed to parse board name");

        let expected_board_name = if cfg!(target_arch = "x86_64") {
            // netsvc overrides the board name on x64 boards 🤷.
            "x64"
        } else {
            MOCK_BOARD_NAME
        };
        assert_eq!(board_name, expected_board_name);
    }
}

#[netstack_test]
async fn get_board_info(name: &str) {
    with_netsvc_and_netstack(name, get_board_info_inner).await;
}

async fn expect_oack(
    sock: &fuchsia_async::net::UdpSocket,
    socket_addr: std::net::SocketAddr,
    options: tftp::AllOptions,
) {
    let mut buffer = [0u8; BUFFER_SIZE];
    let mut pb = &mut buffer[..];
    let oack = read_message::<tftp::TftpPacket<_>, _>(&mut pb, &sock, socket_addr)
        .await
        .into_oack()
        .expect("unexpected response");

    let all_options = oack.options().collect();
    assert_eq!(all_options, options);
}

#[netstack_test]
async fn advertises(name: &str) {
    let () = with_netsvc_and_netstack_bind_port(
        Some(netsvc_proto::netboot::ADVERT_PORT),
        name,
        IntoIterator::into_iter(DEFAULT_NETSVC_ARGS).chain(["--advertise"]),
        |sock, scope| async move {
            let mut buffer = [0u8; BUFFER_SIZE];
            let (n, addr) = sock.recv_from(&mut buffer[..]).await.expect("recv from failed");
            match addr {
                std::net::SocketAddr::V6(addr) => {
                    assert_eq!(addr.scope_id(), scope);
                    assert!(
                        net_types::ip::Ipv6Addr::from_bytes(addr.ip().octets())
                            .is_unicast_link_local(),
                        "{} is not a unicast link local address",
                        addr
                    );
                }
                std::net::SocketAddr::V4(addr) => panic!("unexpected IPv4 address {}", addr),
            }

            let mut buffer = &mut buffer[..n];
            let pkt = buffer.parse::<netboot::NetbootPacket<_>>().expect("parse failed");
            assert_eq!(pkt.command(), netboot::OpcodeOrErr::Op(netboot::Opcode::Advertise));
            let payload =
                std::str::from_utf8(pkt.payload()).expect("failed to parse advertisement");
            let (nodename, version) =
                payload.split(';').fold((None, None), |(nodename, version), kv| {
                    let mut it = kv.split('=');
                    let k = it.next().unwrap_or_else(|| panic!("missing key on {}", kv));
                    let v = it.next().unwrap_or_else(|| panic!("missing value on {}", kv));
                    assert_eq!(it.next(), None);
                    match k {
                        "nodename" => {
                            assert_eq!(nodename, None);
                            (Some(v), version)
                        }
                        "version" => {
                            assert_eq!(version, None);
                            (nodename, Some(v))
                        }
                        k => panic!("unexpected key {} = {}", k, v),
                    }
                });
            // No checks on version other than presence and not empty.
            assert_matches::assert_matches!(version, Some(v) if !v.is_empty());
            assert_matches::assert_matches!(nodename, Some(n) if n.starts_with("fuchsia-"));
        },
    )
    .await;
}

#[netstack_test]
async fn survives_device_removal(name: &str) {
    use packet_formats::ethernet::{EthernetFrame, EthernetFrameLengthCheck};

    let sandbox = netemul::TestSandbox::new().expect("create sandbox");

    // Create an event stream watcher before starting any realms so we're sure
    // to observe netsvc early stop events.
    let mut component_event_stream = netstack_testing_common::get_component_stopped_event_stream()
        .await
        .expect("get event stream");

    // NB: We intentionally don't poll `services` since we don't need to
    // observe proper interaction with them.
    let (netsvc_realm, _services) = create_netsvc_realm(
        &sandbox,
        name,
        IntoIterator::into_iter(DEFAULT_NETSVC_ARGS).chain(["--advertise"]),
    );
    let network = sandbox.create_network("net").await.expect("create network");
    let fake_ep = network.create_fake_endpoint().expect("create fake endpoint");
    let mut frames = fake_ep.frame_stream().map(|r| {
        let (frame, dropped) = r.expect("failed to read frame");
        assert_eq!(dropped, 0);
        let mut buffer_view = &frame[..];
        let eth = buffer_view
            .parse_with::<_, EthernetFrame<_>>(EthernetFrameLengthCheck::NoCheck)
            .expect("failed to parse ethernet");
        eth.src_mac()
    });

    let netsvc_stopped_fut = netstack_testing_common::wait_for_component_stopped_with_stream(
        &mut component_event_stream,
        &netsvc_realm,
        NETSVC_NAME,
        None,
    );

    let test_fut = async {
        for i in 0..3 {
            let ep_name = format!("ep-{}", i);
            let mac = net_types::ethernet::Mac::new([2, 3, 4, 5, 6, i]);
            let ep = network
                .create_endpoint_with(
                    &ep_name,
                    netemul::new_endpoint_config(
                        netemul::DEFAULT_MTU,
                        Some(fidl_fuchsia_net::MacAddress { octets: mac.bytes() }),
                    ),
                )
                .await
                .expect("create endpoint");
            let () = ep.set_link_up(true).await.expect("set link up");
            let () = netsvc_realm
                .add_virtual_device(&ep, netemul::devfs_device_path(&ep_name).as_path())
                .await
                .expect("add virtual device");

            // Wait until we observe any netsvc packet with the source mac set to
            // our endpoint, as proof that it is alive.
            frames
                .by_ref()
                .filter_map(|src_mac| {
                    futures::future::ready(if src_mac == mac {
                        Some(())
                    } else {
                        println!("ignoring frame with mac {}", src_mac);
                        None
                    })
                })
                .next()
                .await
                .expect("frames stream ended unexpectedly");

            // Destroy the device backed by netemul. Netsvc must survive this
            // and observe new devices in future iterations.
            drop(ep);
        }
    };
    futures::select! {
        r = netsvc_stopped_fut.fuse() => {
            let e: component_events::events::Stopped = r.expect("failed to observe stopped event");
            panic!("netsvc stopped unexpectedly with {:?}", e);
        },
        () =  test_fut.fuse() => (),
    }
}

#[netstack_test]
async fn starts_device_in_multicast_promiscuous(name: &str) {
    // Create an event stream watcher before starting any realms so we're sure
    // to observe netsvc early stop events.
    let mut component_event_stream = netstack_testing_common::get_component_stopped_event_stream()
        .await
        .expect("get event stream");

    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let (netsvc_realm, services) = create_netsvc_realm(&sandbox, name, DEFAULT_NETSVC_ARGS);

    let netsvc_stopped_fut = netstack_testing_common::wait_for_component_stopped_with_stream(
        &mut component_event_stream,
        &netsvc_realm,
        NETSVC_NAME,
        None,
    );

    let (tun, netdevice) = netstack_testing_common::devices::create_tun_device();
    let (tun_port, _): (_, fhardware_network::PortProxy) =
        netstack_testing_common::devices::create_eth_tun_port(
            &tun,
            /* port_id */ 1,
            fidl_mac!("02:00:00:00:00:01"),
        )
        .await;

    let mac_state_stream = futures::stream::unfold(
        (tun_port, Option::<fnet_tun::MacState>::None),
        |(tun_port, last_observed)| async move {
            loop {
                let fnet_tun::InternalState { mac, .. } =
                    tun_port.watch_state().await.expect("watch_state");
                let mac = mac.expect("missing mac state");
                if last_observed.as_ref().map(|l| l != &mac).unwrap_or(true) {
                    let last_observed = Some(mac.clone());
                    break Some((mac, (tun_port, last_observed)));
                }
            }
        },
    );
    futures::pin_mut!(mac_state_stream);

    assert_matches::assert_matches!(
        mac_state_stream.next().await,
        Some(fnet_tun::MacState {
            mode: Some(fhardware_network::MacFilterMode::MulticastFilter),
            multicast_filters: Some(mcast_filters),
            ..
        }) => {
            assert_eq!(mcast_filters, vec![]);
        }
    );

    // Add the device to devfs and wait for netsvc to set it to multicast
    // promiscuous.

    let (client_end, connector_stream) = fidl::endpoints::create_request_stream();
    let () = netsvc_realm
        .add_raw_device(netemul::devfs_device_path("ep").as_path(), client_end)
        .await
        .expect("add virtual device");

    let netdevice = netdevice.into_proxy();
    let netdevice = &netdevice;
    let connector_fut = connector_stream.for_each_concurrent(None, |r| async move {
        match r.expect("connector error") {
            fnetemul_network::DeviceProxy_Request::ServeDevice { req, control_handle: _ } => {
                netdevice.clone(req).expect("clone failed")
            }
            fnetemul_network::DeviceProxy_Request::ServeController { req, control_handle: _ } => {
                let rs = req.into_stream();
                rs.for_each(|req| async move {
                    match req.expect("request error") {
                        fidl_fuchsia_device::ControllerRequest::GetTopologicalPath {
                            responder,
                        } => responder.send(Ok("some_topopath")).expect("send topopath response"),
                        req => panic!("unexpected request {:?}", req),
                    }
                })
                .await
            }
        };
    });

    let new_state = futures::select! {
        state = mac_state_stream.next() => state,
        e = netsvc_stopped_fut.fuse() => {
            let e: component_events::events::Stopped = e.expect("failed to observe stopped event");
            panic!("netsvc stopped unexpectedly with {:?}", e);
        },
        () = services.fuse() => panic!("ServiceFs ended unexpectedly"),
        () = connector_fut.fuse() => panic!("connector future ended unexpectedly"),
    };

    assert_matches::assert_matches!(
        new_state,
        Some(fnet_tun::MacState {
            mode: Some(fhardware_network::MacFilterMode::MulticastPromiscuous),
            multicast_filters: Some(mcast_filters),
            ..
        }) => {
            assert_eq!(mcast_filters, vec![]);
        }
    );
    // Disable checking for clean shutdown on drop. This test exits before
    // netsvc completes opening the session, which may cause it to exit with a
    // sad exit code.
    netsvc_realm.set_checked_shutdown_on_drop(false);
}

#[netstack_test]
#[test_case(true; "unicast")]
#[test_case(false; "all_nodes")]
async fn replies_to_ping(name: &str, unicast: bool) {
    with_netsvc_and_netstack_full(
        None,
        name,
        DEFAULT_NETSVC_ARGS,
        |realm| {
            async move {
                let sock = realm.icmp_socket::<Ipv6>().await.expect("icmp socket");
                // Disable looping multicast packets back to us; That prevents
                // us from seeing, and resonding to, our own generated ping
                // requests.
                let () = sock
                    .as_ref()
                    .set_multicast_loop_v6(false)
                    .expect("failed to disable multicast loop");
                sock
            }
            .boxed()
        },
        |sock, scope_id, icmp_socket| async move {
            let device = discover(&sock, scope_id).await;

            const SEQUENCE: u16 = 1234;
            const BUFFER_SIZE: usize = 128;
            const PORT: u16 = 4321;

            let body: [u8; 4] = [1, 2, 3, 4];

            let dst = if unicast { device } else { std_ip_v6!("ff02::1") };
            let addr = std::net::SocketAddrV6::new(dst, PORT, /* flowinfo */ 0, scope_id);

            let mut ping_stream =
                ping::PingStream::<Ipv6, _, BUFFER_SIZE>::new(&icmp_socket).fuse();
            let ping_sink = ping::PingSink::<Ipv6, _>::new(&icmp_socket);

            // Create a future that tries to ping regularly so we don't suffer
            // flakes in case of bad neighbor resolution. We'll always send
            // pings with the same sequence number.
            let mut sink =
                fuchsia_async::Interval::new(fuchsia_async::MonotonicDuration::from_seconds(1))
                    .map(|()| Ok(ping::PingData { sequence: SEQUENCE, addr, body: body.to_vec() }))
                    .forward(ping_sink)
                    .map(|r| r.expect("sink error"))
                    .fuse();

            let reply = futures::select! {
                () = sink => panic!("sink ended unexpectedly"),
                reply = ping_stream.try_next() => reply,
            }
            .expect("ping stream error")
            .expect("ping stream ended unexpectedly");

            let ping::PingData { addr: reply_addr, sequence, body: reply_body } = reply;

            assert_eq!(reply_addr.ip(), &device);
            assert_eq!(reply_addr.scope_id(), scope_id);
            assert_eq!(sequence, SEQUENCE);
            assert_eq!(&reply_body[..], &body[..]);
        },
    )
    .await
}
