// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

use fdio::{SpawnAction, SpawnOptions};
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_interfaces_admin as fnet_interfaces_admin;
use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
use fidl_fuchsia_net_root as fnet_root;
use fidl_fuchsia_net_stack as fnet_stack;
use fidl_fuchsia_posix_socket as fposix_socket;
use fidl_fuchsia_posix_socket_packet as fposix_socket_packet;
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use fuchsia_runtime::{HandleInfo, HandleType, duplicate_utc_clock_handle, job_default};
use futures::future;
use futures::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use futures::stream::StreamExt as _;
use libc::{STDERR_FILENO, STDOUT_FILENO};
use net_declare::{fidl_ip_v4, fidl_mac, std_socket_addr};
use netemul::RealmUdpSocket as _;
use netstack_testing_common::interfaces;
use netstack_testing_common::realms::{Netstack, TestSandboxExt as _};
use netstack_testing_macros::netstack_test;
use packet::{Buf, NoOpSerializationContext, Serializer, NestableSerializer as _};
use packet_formats::ethernet::{ETHERNET_MIN_BODY_LEN_NO_TAG, EtherType, EthernetFrameBuilder};
use packet_formats::ip::{IpProto, Ipv4Proto};
use packet_formats::ipv4::Ipv4PacketBuilder;
use packet_formats::udp::UdpPacketBuilder;
use regex::Regex;
use std::convert::TryInto as _;
use std::ffi::{CStr, CString};
use std::num::NonZeroU16;
use zx::ProcessInfo;

const BINARY_PATH: &str = "/pkg/bin/tcpdump";

/// Returns true iff the patterns were found, false if the stream ended.
async fn wait_for_pattern<SO: AsyncBufReadExt + Unpin, SE: AsyncReadExt + Unpin>(
    reader: &mut SO,
    other_reader: &mut SE,
    mut patterns: Vec<Regex>,
) {
    while !patterns.is_empty() {
        let mut line = String::new();
        let read_bytes = reader.read_line(&mut line).await.expect("read_line");

        if read_bytes == 0 {
            let mut buf = String::new();
            let _read_bytes: usize =
                other_reader.read_to_string(&mut buf).await.expect("read other reader");
            panic!(
                "failed to match all patterns from reader; patterns = {:?}\nOTHER READER:\n{}",
                patterns, buf
            )
        }

        println!("GOT LINE FROM READER: {}", line);

        // Trim the trailing new line.
        let line = &line[..line.len() - 1];
        let () = patterns.retain(|pattern| !pattern.is_match(&line));
    }
}

fn start_tcpdump(
    args: impl IntoIterator<Item = &'static str>,
    mut spawn_actions: Vec<SpawnAction<'_>>,
) -> (zx::Process, zx::Socket, zx::Socket) {
    let (stdout_reader, stdout_writer) = zx::Socket::create_stream();
    let (stderr_reader, stderr_writer) = zx::Socket::create_stream();

    // The reader-ends should not write.
    let () = stdout_writer.half_close().expect("stdout_reader.half_close");
    let () = stdout_writer.half_close().expect("stderr_reader.half_close");

    let path = CString::new(BINARY_PATH).expect("cstring path");
    let path = path.as_c_str();

    let args: Vec<CString> = std::iter::once(BINARY_PATH)
        .chain(args.into_iter())
        .map(|a| {
            CString::new(a).unwrap_or_else(|e| panic!("failed to parse {} to CString: {}", a, e))
        })
        .collect();
    let args: Vec<&CStr> = args.iter().map(|s| s.as_c_str()).collect();

    // Provide the socket that TCPDump should use for stdout.
    spawn_actions.push(SpawnAction::add_handle(
        HandleInfo::new(
            HandleType::FileDescriptor,
            STDOUT_FILENO.try_into().expect("STDOUT_FILENO.try_into"),
        ),
        stdout_writer.into(),
    ));
    // Provide the socket that TCPDump should use for stderr.
    spawn_actions.push(SpawnAction::add_handle(
        HandleInfo::new(
            HandleType::FileDescriptor,
            STDERR_FILENO.try_into().expect("STDERR_FILENO.try_into"),
        ),
        stderr_writer.into(),
    ));

    let process = fdio::spawn_etc(
        &job_default(),
        SpawnOptions::DEFAULT_LOADER,
        path,
        &args[..],
        None,
        &mut spawn_actions,
    )
    .expect("spawn tcpdump");

    (process, stdout_reader, stderr_reader)
}

enum SendToAddress {
    BoundAddress,
    Specified(std::net::SocketAddr),
}

async fn start_tcpdump_and_wait_for_patterns<
    Fut: future::Future<Output = ()>,
    F: FnOnce() -> Fut,
>(
    realm: &netemul::TestRealm<'_>,
    args: impl IntoIterator<Item = &'static str>,
    bind_addr: std::net::SocketAddr,
    send_to_addr: SendToAddress,
    inject_packet: F,
    patterns: Vec<Regex>,
) {
    let (svc_client_end, svc_server_end) = fidl::endpoints::create_endpoints();
    let (process, stdout_reader, stderr_reader) = start_tcpdump(
        args,
        vec![
            SpawnAction::add_namespace_entry(
                CString::new("/svc").expect("CString /svc").as_c_str(),
                svc_client_end.into_channel().into(),
            ),
            SpawnAction::add_handle(
                HandleInfo::new(HandleType::ClockUtc, 0),
                duplicate_utc_clock_handle(
                    zx::Rights::READ | zx::Rights::WAIT | zx::Rights::TRANSFER,
                )
                .expect("duplicate utc clock handle")
                .into_handle(),
            ),
        ],
    );
    let mut stdout_reader = BufReader::new(fasync::Socket::from_socket(stdout_reader));
    let mut stderr_reader = BufReader::new(fasync::Socket::from_socket(stderr_reader));

    let mut svcfs = ServiceFs::new_local();
    let svcfs = svcfs
        .add_service_connector::<_, fposix_socket::ProviderMarker>(|server_end| {
            realm
                .connect_to_protocol_with_server_end(server_end)
                .expect("connect to regular socket provider")
        })
        .add_service_connector::<_, fposix_socket_packet::ProviderMarker>(|server_end| {
            realm
                .connect_to_protocol_with_server_end(server_end)
                .expect("connect to packet socket provider")
        })
        .serve_connection(svc_server_end)
        .expect("servicefs serve connection")
        .collect::<()>();

    // Wait for TCPDump to start.
    let svcfs = {
        let wait_for_pattern_fut = wait_for_pattern(
            &mut stderr_reader,
            &mut stdout_reader,
            vec![Regex::new(r"listening on any, link-type LINUX_SLL2 \(Linux cooked v2\), snapshot length \d+ bytes").expect("parse tcpdump listening regex")],
        );
        futures::pin_mut!(wait_for_pattern_fut);
        match future::select(wait_for_pattern_fut, svcfs).await {
            future::Either::Left(((), svcfs)) => svcfs,
            future::Either::Right(((), _wait_for_pattern_fut)) => {
                panic!("service directory unexpectedly ended")
            }
        }
    };

    // Send a UDP packet and make sure TCPDump logs it.
    let sock = fuchsia_async::net::UdpSocket::bind_in_realm(&realm, bind_addr)
        .await
        .expect("create socket");
    let addr = match send_to_addr {
        SendToAddress::Specified(addr) => addr,
        SendToAddress::BoundAddress => sock.local_addr().expect("get bound socket address"),
    };
    const PAYLOAD: [u8; 4] = [1, 2, 3, 4];
    let sent = sock.send_to(&PAYLOAD[..], addr).await.expect("send_to failed");
    assert_eq!(sent, PAYLOAD.len());

    inject_packet().await;

    {
        let wait_for_pattern_fut =
            wait_for_pattern(&mut stdout_reader, &mut stderr_reader, patterns);
        futures::pin_mut!(wait_for_pattern_fut);
        match future::select(wait_for_pattern_fut, svcfs).await {
            future::Either::Left(((), _svcfs)) => {}
            future::Either::Right(((), _wait_for_pattern_fut)) => {
                panic!("service directory unexpectedly ended")
            }
        }
    }

    assert_eq!(
        fasync::OnSignals::new(&process, zx::Signals::PROCESS_TERMINATED)
            .await
            .expect("wait for process termination"),
        zx::Signals::PROCESS_TERMINATED
    );
    let ProcessInfo { return_code, .. } = process.info().expect("process info");
    assert_eq!(return_code, 0);
}

#[fuchsia::test]
async fn version_test() {
    let (process, stdout_reader, stderr_reader) = start_tcpdump(["--version"], Vec::new());

    assert_eq!(
        fasync::OnSignals::new(&process, zx::Signals::PROCESS_TERMINATED)
            .await
            .expect("wait for process termination"),
        zx::Signals::PROCESS_TERMINATED
    );
    let ProcessInfo { return_code, .. } = process.info().expect("process info");
    assert_eq!(return_code, 0);

    let mut stdout_reader = BufReader::new(fasync::Socket::from_socket(stdout_reader));
    let mut stderr_reader = fasync::Socket::from_socket(stderr_reader);

    wait_for_pattern(
        &mut stdout_reader,
        &mut stderr_reader,
        vec![
            Regex::new(r"tcpdump version ").expect("parse tcpdump version regex"),
            Regex::new(r"libpcap version ").expect("parse libpcap version regex"),
        ],
    )
    .await
}

#[netstack_test]
#[variant(N, Netstack)]
// TODO(https://fxbug.dev/42169332): Fix memory leak and run this with Lsan.
#[cfg_attr(feature = "variant_asan", ignore)]
// TODO(https://fxbug.dev/436867782): Fix memory leak and run this with HWASan.
#[cfg_attr(feature = "variant_hwasan", ignore)]
async fn packet_test<N: Netstack>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<N, _>(name).expect("create realm");

    start_tcpdump_and_wait_for_patterns(
        &realm,
        ["-c", "1", "--no-promiscuous-mode"],
        std_socket_addr!("127.0.0.1:9875"),
        SendToAddress::BoundAddress,
        || futures::future::ready(()),
        vec![
            Regex::new(r"lo\s+In\s+IP 127\.0\.0\.1\.9875 > 127\.0\.0\.1\.9875: UDP, length 4")
                .expect("parse tcpdump packet regex"),
        ],
    )
    .await
}

#[netstack_test]
// TODO(https://fxbug.dev/42169332): Fix memory leak and run this with Lsan.
#[cfg_attr(feature = "variant_asan", ignore)]
// TODO(https://fxbug.dev/436867782): Fix memory leak and run this with HWASan.
#[cfg_attr(feature = "variant_hwasan", ignore)]
async fn bridged_packet_test(name: &str) {
    type N = netstack_testing_common::realms::Netstack2;
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<N, _>(name).expect("create realm");

    let net = sandbox.create_network(name).await.expect("error creating network");
    let iface =
        realm.join_network(&net, "ep").await.expect("failed to join network in gateway realm");

    const REMOTE_MAC: fnet::MacAddress = fidl_mac!("02:00:00:00:00:01");
    const NETWORK_ADDR: fnet::Ipv4Address = fidl_ip_v4!("192.168.1.0");
    const PREFIX_LEN: u8 = 24;
    let increment_and_get_addr = |mut new_addr| {
        let fnet::Ipv4Address { addr } = &mut new_addr;
        *addr.last_mut().expect("should have at least 1 byte") += 1;
        new_addr
    };
    let local_addr = increment_and_get_addr(NETWORK_ADDR);
    let remote_addr = increment_and_get_addr(local_addr);
    let (local_mac, _bridge_ctl) = {
        let stack = realm
            .connect_to_protocol::<fnet_stack::StackMarker>()
            .expect("failed to connect to stack in realm");
        let (control, server_end) =
            fnet_interfaces_ext::admin::Control::create_endpoints().expect("create endpoints");
        stack.bridge_interfaces(&[iface.id()][..], server_end).expect("bridge interfaces");
        let bridge_id = control.get_id().await.expect("get bridge id");
        let did_enable = control.enable().await.expect("send enable").expect("enable");
        assert!(did_enable);
        let address_state_provider = interfaces::add_address_wait_assigned(
            &control,
            fnet::Subnet { addr: fnet::IpAddress::Ipv4(local_addr), prefix_len: PREFIX_LEN },
            fnet_interfaces_admin::AddressParameters {
                add_subnet_route: Some(true),
                ..Default::default()
            },
        )
        .await
        .expect("add IPv4 address to bridge failed");
        let () = address_state_provider
            .detach()
            .expect("failed to detach from bridge interface address state provider");

        // Create a static neighbor entry so we skip neighbor resolution.
        realm
            .add_neighbor_entry(bridge_id.into(), fnet::IpAddress::Ipv4(remote_addr), REMOTE_MAC)
            .await
            .expect("error adding neighbor entry");

        let root = realm
            .connect_to_protocol::<fnet_root::InterfacesMarker>()
            .expect("failed to connect to root interfaces protocol");
        let mac = root
            .get_mac(bridge_id)
            .await
            .expect("error calling get_mac")
            .expect("error getting bridge's MAC address")
            .expect("expected bridge to have a MAC address");
        (mac, control)
    };

    start_tcpdump_and_wait_for_patterns(
        &realm,
        ["-i", "any", "-c", "4", "--no-promiscuous-mode", "udp"],
        std_socket_addr!("0.0.0.0:9876"),
        SendToAddress::Specified(std::net::SocketAddr::new(
            std::net::IpAddr::V4(std::net::Ipv4Addr::from(remote_addr.addr)),
            1234,
        )),
        || async {
            let net_types_addr = |fnet::Ipv4Address { addr }| net_types::ip::Ipv4Addr::new(addr);
            let local_addr = net_types_addr(local_addr);
            let remote_addr = net_types_addr(remote_addr);

            let net_types_mac = |fnet::MacAddress { octets }| net_types::ethernet::Mac::new(octets);

            let bytes = Buf::new(&mut [0; 8][..], ..)
                .wrap_in(UdpPacketBuilder::new(
                    remote_addr,
                    local_addr,
                    Some(NonZeroU16::new(2342).expect("non zero value should be valid")),
                    NonZeroU16::new(9876).expect("non zero value should be valid"),
                ))
                .wrap_in(Ipv4PacketBuilder::new(
                    remote_addr,
                    local_addr,
                    64, /* ttl */
                    Ipv4Proto::Proto(IpProto::Udp),
                ))
                .wrap_in(EthernetFrameBuilder::new(
                    net_types_mac(REMOTE_MAC),
                    net_types_mac(*local_mac),
                    EtherType::Ipv4,
                    ETHERNET_MIN_BODY_LEN_NO_TAG,
                ))
                .serialize_vec_outer(&mut NoOpSerializationContext)
                .expect("error serializing UDP packet")
                .unwrap_b();

            let fake_ep = net.create_fake_endpoint().expect("error creating fake endppint");
            fake_ep.write(bytes.as_ref()).await.expect("error writing packet to fake endpoint")
        },
        vec![
            Regex::new(
                r"br\d+\s+Out\s+IP 192\.168\.1\.1\.9876 > 192\.168\.1\.2\.1234: UDP, length 4",
            )
            .expect("parse tcpdump packet regex for packet sent through bridge"),
            Regex::new(
                r"eth\d+\s+Out\s+IP 192\.168\.1\.1\.9876 > 192\.168\.1\.2\.1234: UDP, length 4",
            )
            .expect("parse tcpdump packet regex for packet sent through ethernet interface"),
            // TODO(https://fxbug.dev/42071238): Change the direction from `P` to
            // `In` once packets destined to a bridge that arrive at a bridged
            // member is properly marked as being a packet directed to the host.
            Regex::new(
                r"eth\d+\s+P\s+IP 192\.168\.1\.2\.2342 > 192\.168\.1\.1\.9876: UDP, length 8",
            )
            .expect("parse tcpdump packet regex for packet received at ethernet interface"),
            Regex::new(
                r"br\d+\s+In\s+IP 192\.168\.1\.2\.2342 > 192\.168\.1\.1\.9876: UDP, length 8",
            )
            .expect("parse tcpdump packet regex for packet received at bridge"),
        ],
    )
    .await
}
