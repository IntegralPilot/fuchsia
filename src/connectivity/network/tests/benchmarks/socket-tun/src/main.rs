// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::io::{Read as _, Write as _};

use fidl_fuchsia_hardware_network as fhardware_network;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_ext as fnet_ext;
use fidl_fuchsia_net_interfaces_admin as fnet_interfaces_admin;
use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
use fidl_fuchsia_net_tun as fnet_tun;
use fidl_fuchsia_posix_socket as fposix_socket;
use fidl_fuchsia_tracing_controller as ftracing_controller;
use fuchsia_async as fasync;
use fuchsiaperf::{Direction, Unit};
use futures::{AsyncReadExt as _, FutureExt as _};

use libc as _;
use net_declare::fidl_subnet;
use netemul::{PacketCapture, TestEndpoint, TestNetwork};
use netstack_testing_common::realms::{KnownServiceProvider, NetstackVersion};
use netstack_testing_common::{devices, interfaces};

trait IpExt {
    const CLIENT_SUBNET: fnet::Subnet;
    const SERVER_SUBNET: fnet::Subnet;
    const NAME: &'static str;
    const DOMAIN: fposix_socket::Domain;
}

impl IpExt for net_types::ip::Ipv4 {
    const CLIENT_SUBNET: fnet::Subnet = fidl_subnet!("192.0.2.1/24");
    const SERVER_SUBNET: fnet::Subnet = fidl_subnet!("192.0.2.254/24");
    const NAME: &'static str = "IPv4";
    const DOMAIN: fposix_socket::Domain = fposix_socket::Domain::Ipv4;
}

impl IpExt for net_types::ip::Ipv6 {
    const CLIENT_SUBNET: fnet::Subnet = fidl_subnet!("2001:0db8:abcd:efff::1/64");
    const SERVER_SUBNET: fnet::Subnet = fidl_subnet!("2001:0db8:abcd:efff:ffff:ffff:ffff:ffff/64");
    const NAME: &'static str = "IPv6";
    const DOMAIN: fposix_socket::Domain = fposix_socket::Domain::Ipv6;
}

fn generate_send_recv_bufs(size: usize) -> (Vec<u8>, Vec<u8>) {
    (
        // It may be natural to fill the buffer with a single byte, but a
        // property of sending such a message is that any permutation of
        // the bytes look identical, which hides certain TCP bugs where the
        // bytes sent have been permuted due to wrong sequence numbers, e.g.
        // https://fxbug.dev/42079275.
        (0u8..=254).cycle().take(size).collect(),
        // The choice of filling the receive buffer with a non-zero byte
        // is deliberate: the [zero page scanner] may reclaim large memory
        // filled with zeroes (note that it is turned off in environments
        // where the benchmark results are collected, but there is no harm
        // in doing this to be extra safe).
        //
        // [zero page scanner]: https://fuchsia.dev/fuchsia-src/gen/boot-options?hl=en#kernelpage-scannerzero-page-scans-per-seconduint64_t
        vec![255; size],
    )
}

// This type holds unused FIDL proxies so that the underlying object stays
// alive.
struct InterfaceFidlProxies {
    _device_control: fnet_interfaces_admin::DeviceControlProxy,
    _control: fidl_fuchsia_net_interfaces_ext::admin::Control,
}

// This type holds unused FIDL proxies so that the underlying object stays
// alive.
struct FidlProxies<'a> {
    tun_or_network: TunOrDebugNetwork<'a>,
    _client_if: InterfaceFidlProxies,
    _server_if: InterfaceFidlProxies,
}

const DEBUG_NETWORK_NAME: &'static str = "sniff";

enum TunOrDebugNetwork<'a> {
    Tun(fnet_tun::DevicePairProxy),
    DebugNetwork { net: TestNetwork<'a>, _ep1: TestEndpoint<'a>, _ep2: TestEndpoint<'a> },
}

impl<'a> TunOrDebugNetwork<'a> {
    async fn start_capture(&mut self, name: &str) -> Option<PacketCapture> {
        match self {
            TunOrDebugNetwork::Tun(_tun) => None,
            TunOrDebugNetwork::DebugNetwork { net, _ep1, _ep2 } => {
                // We exclusively borrowed `net` so no one should be able to capture
                // packets at the moment.
                Some(net.start_capture(name).await.expect("failed to start packet capture"))
            }
        }
    }
}

async fn setup<'a>(
    sandbox: &'a netemul::TestSandbox,
    netstack_version: NetstackVersion,
    capture_packets: bool,
) -> (netemul::TestRealm<'a>, netemul::TestRealm<'a>, FidlProxies<'a>) {
    let client_realm = sandbox
        .create_realm(
            "client",
            [KnownServiceProvider::SecureStash, KnownServiceProvider::Netstack(netstack_version)],
        )
        .expect("create client netstack");
    let server_realm = sandbox
        .create_realm(
            "server",
            [KnownServiceProvider::SecureStash, KnownServiceProvider::Netstack(netstack_version)],
        )
        .expect("create server netstack");

    let (tun_or_network, left_port, right_port) = if capture_packets {
        let net = sandbox.create_network(DEBUG_NETWORK_NAME).await.expect("create network");
        let (left_port, left_port_server_end) =
            fidl::endpoints::create_proxy::<fhardware_network::PortMarker>();
        let (right_port, right_port_server_end) =
            fidl::endpoints::create_proxy::<fhardware_network::PortMarker>();
        let ep1 = net.create_endpoint("ep1").await.expect("failed to create endpoint");
        let ep2 = net.create_endpoint("ep2").await.expect("failed to create endpoint");
        ep1.get_port(left_port_server_end).expect("failed to get port for endpoint");
        ep2.get_port(right_port_server_end).expect("failed to get port for endpoint");
        ep1.set_link_up(true).await.expect("failed to enable endpoint");
        ep2.set_link_up(true).await.expect("failed to enable endpoint");
        (TunOrDebugNetwork::DebugNetwork { net, _ep1: ep1, _ep2: ep2 }, left_port, right_port)
    } else {
        let (tun_dev_pair, left_port, right_port) =
            netstack_testing_common::devices::create_eth_tun_pair().await;
        (TunOrDebugNetwork::Tun(tun_dev_pair), left_port, right_port)
    };

    async fn install_interface(
        realm: &netemul::TestRealm<'_>,
        port: fhardware_network::PortProxy,
        subnets: impl IntoIterator<Item = fnet::Subnet>,
    ) -> InterfaceFidlProxies {
        let device = {
            let (device, server_end) =
                fidl::endpoints::create_endpoints::<fhardware_network::DeviceMarker>();
            port.get_device(server_end).expect("get device");
            device
        };
        let device_control = devices::install_device(realm, device);

        let control = {
            let port_id =
                port.get_info().await.expect("get port info").id.expect("port ID must be present");
            let (control, server_end) =
                fnet_interfaces_ext::admin::Control::create_endpoints().expect("create endpoints");
            device_control
                .create_interface(&port_id, server_end, fnet_interfaces_admin::Options::default())
                .expect("create interface");
            control
        };
        assert!(
            control.enable().await.expect("enable interface FIDL call").expect("enable interface")
        );

        for subnet in subnets {
            let addr_state_provider = interfaces::add_address_wait_assigned(
                &control,
                subnet,
                fnet_interfaces_admin::AddressParameters {
                    add_subnet_route: Some(true),
                    ..Default::default()
                },
            )
            .await
            .expect("add address and wait for it to be assigned");
            addr_state_provider.detach().expect("address state provider detach FIDL call");
        }

        InterfaceFidlProxies { _device_control: device_control, _control: control }
    }
    let (_client_if, _server_if) = futures::future::join(
        install_interface(
            &client_realm,
            left_port,
            [net_types::ip::Ipv4::CLIENT_SUBNET, net_types::ip::Ipv6::CLIENT_SUBNET],
        ),
        install_interface(
            &server_realm,
            right_port,
            [net_types::ip::Ipv4::SERVER_SUBNET, net_types::ip::Ipv6::SERVER_SUBNET],
        ),
    )
    .await;

    (client_realm, server_realm, FidlProxies { tun_or_network, _client_if, _server_if })
}

fn format_byte_count(byte_count: usize) -> String {
    if byte_count >= 1024 {
        format!("{}KiB", byte_count / 1024)
    } else {
        format!("{}B", byte_count)
    }
}

async fn bench_tcp<'a, I: IpExt>(
    test_suite: &'static str,
    iter_count: usize,
    client_realm: &netemul::TestRealm<'a>,
    server_realm: &netemul::TestRealm<'a>,
    transfer: usize,
    tun_or_network: &mut TunOrDebugNetwork<'a>,
) -> fuchsiaperf::FuchsiaPerfBenchmarkResult {
    let label = format!("WriteRead/TCP/{}/{}", I::NAME, format_byte_count(transfer));
    let _packet_capture = tun_or_network.start_capture(&label.replace("/", "-")).await;
    let (mut client_sock, mut server_sock) = {
        let (listen_sock, client_sock) = futures::future::join(
            server_realm
                .stream_socket(I::DOMAIN, fposix_socket::StreamSocketProtocol::Tcp)
                .map(|r| r.expect("create listening socket")),
            client_realm
                .stream_socket(I::DOMAIN, fposix_socket::StreamSocketProtocol::Tcp)
                .map(|r| r.expect("create client socket")),
        )
        .await;

        // Since we want to avoid including the overhead of the async
        // executor in the benchmarked read/write steps, intentionally keep
        // the sockets non-async-aware and use `socket2` calls directly to
        // connect the sockets.
        let bind_sockaddr = {
            let fnet_ext::IpAddress(listen_addr) = I::SERVER_SUBNET.addr.into();
            socket2::SockAddr::from(std::net::SocketAddr::from((listen_addr, 0)))
        };
        listen_sock.bind(&bind_sockaddr).expect("bind");
        listen_sock.listen(0).expect("listen");
        let listen_sockaddr = listen_sock.local_addr().expect("local addr");

        const BUFFER_SIZE_MULTIPLER: usize = 4;

        // Set send buffer to a multiple of the transfer size to ensure we can
        // write `transfer` bytes before reading it on the other end. The
        // multiplier allows the receiver to delay acknowledgements but the
        // transfer still proceeds. This is especially impactful for small
        // transfer sizes.
        client_sock
            .set_send_buffer_size(transfer * BUFFER_SIZE_MULTIPLER)
            .expect("set send buffer size");

        // Also set the receive buffer size.
        //
        // We use the same multiplier on the transfer size so silly window
        // avoidance doesn't kick in in-between test iterations which causes
        // pollution in the results.
        //
        // This ensures fairness in the benchmark since TCP will base the window
        // value on the available receive buffer size and different numbers will
        // skew the test results.
        listen_sock
            .set_recv_buffer_size(transfer * BUFFER_SIZE_MULTIPLER)
            .expect("set receive buffer size");

        // Disable the Nagle algorithm, it introduces artificial
        // latency that defeats this benchmark.
        client_sock.set_tcp_nodelay(true).expect("set TCP NODELAY to true");
        client_sock.connect(&listen_sockaddr).expect("connect");

        let (server_sock, _): (_, socket2::SockAddr) = listen_sock.accept().expect("accept");

        (client_sock, server_sock)
    };

    fuchsia_trace::duration!("tun_socket_benchmarks", "test_group", "label" => &*label);
    let values = (0..iter_count)
        .map(|_| {
            let (send_buf, mut recv_buf) = generate_send_recv_bufs(transfer);

            fuchsia_trace::duration!("tun_socket_benchmarks", "test_case");
            let now = std::time::Instant::now();
            let mut transferred = 0;
            while transferred < transfer {
                fuchsia_trace::duration_begin!("tun_socket_benchmarks", "tcp_write");
                let wrote = client_sock.write(&send_buf[transferred..]).expect("write failed");
                fuchsia_trace::duration_end!(
                    "tun_socket_benchmarks", "tcp_write",
                    "bytes_written" => wrote as u64
                );
                transferred += wrote;
            }
            let mut transferred = 0;
            while transferred < transfer {
                fuchsia_trace::duration_begin!("tun_socket_benchmarks", "tcp_read");
                let read = server_sock.read(&mut recv_buf[transferred..]).expect("read failed");
                fuchsia_trace::duration_end!(
                    "tun_socket_benchmarks", "tcp_read",
                    "bytes_read" => read as u64
                );
                transferred += read;
            }
            let duration = now.elapsed().as_nanos() as f64;
            assert_eq!(recv_buf, send_buf);
            duration
        })
        .collect();

    fuchsiaperf::FuchsiaPerfBenchmarkResult {
        test_suite: test_suite.into(),
        label: label.clone(),
        unit: Unit::Nanoseconds,
        direction: Direction::SmallerBetter,
        values,
    }
}

async fn bench_udp<'a, I: IpExt>(
    test_suite: &'static str,
    iter_count: usize,
    client_realm: &netemul::TestRealm<'a>,
    server_realm: &netemul::TestRealm<'a>,
    message_size: usize,
    message_count: usize,
) -> fuchsiaperf::FuchsiaPerfBenchmarkResult {
    let label = if message_count > 1 {
        format!(
            "MultiWriteRead/UDP/{}/{}/{}Messages",
            I::NAME,
            format_byte_count(message_size),
            message_count
        )
    } else {
        format!("WriteRead/UDP/{}/{}", I::NAME, format_byte_count(message_size))
    };

    let (mut client_sock, mut server_sock) = {
        let (server_sock, client_sock) = futures::future::join(
            server_realm
                .datagram_socket(I::DOMAIN, fposix_socket::DatagramSocketProtocol::Udp)
                .map(|r| r.expect("create server socket")),
            client_realm
                .datagram_socket(I::DOMAIN, fposix_socket::DatagramSocketProtocol::Udp)
                .map(|r| r.expect("create client socket")),
        )
        .await;

        // Since we want to avoid including the async executor from the benchmark,
        // here we "manually" use non-blocking calls to connect sockets instead of
        // using the normal async facilities.
        let bind_sockaddr = {
            let fnet_ext::IpAddress(listen_addr) = I::SERVER_SUBNET.addr.into();
            socket2::SockAddr::from(std::net::SocketAddr::from((listen_addr, 0)))
        };
        server_sock.bind(&bind_sockaddr).expect("bind");
        let server_sockaddr = server_sock.local_addr().expect("local addr");

        let transfer_size = message_size * message_count;

        // Set receive buffer to the total size of the write to ensure the
        // entire write can complete before reading.
        server_sock.set_recv_buffer_size(transfer_size).expect("set receive buffer size");

        // Also set the send buffer size to the transfer value.
        //
        // This ensures fairness in the benchmark when comparing platforms so
        // we're not relying on default value differences.
        client_sock
            .set_send_buffer_size(transfer_size)
            .expect("set send buffer size to transfer size");

        client_sock.connect(&server_sockaddr).expect("connect");

        (client_sock, server_sock)
    };

    fuchsia_trace::duration!("tun_socket_benchmarks", "test_group", "label" => &*label);
    let values = (0..iter_count)
        .map(|_| {
            // Allocate buffers that can hold the total transfer size so that the
            // assertion that the bytes transferred are as expected can be done after
            // the entire transfer has completed and excluded from the benchmark
            // duration.
            let (send_buf, mut recv_buf) = generate_send_recv_bufs(message_count * message_size);

            fuchsia_trace::duration!("tun_socket_benchmarks", "test_case");
            let now = std::time::Instant::now();
            for message_bytes in send_buf.chunks(message_size) {
                let wrote = {
                    fuchsia_trace::duration!("tun_socket_benchmarks", "udp_write");
                    client_sock.write(message_bytes.as_ref()).expect("write failed")
                };
                assert_eq!(wrote, message_size);
            }
            for recv_bytes in recv_buf.chunks_mut(message_size) {
                let read = {
                    fuchsia_trace::duration!("tun_socket_benchmarks", "udp_read");
                    server_sock.read(recv_bytes.as_mut()).expect("read failed")
                };
                assert_eq!(read, message_size);
            }
            let duration = now.elapsed().as_nanos() as f64;
            assert_eq!(send_buf, recv_buf);
            duration
        })
        .collect();

    fuchsiaperf::FuchsiaPerfBenchmarkResult {
        test_suite: test_suite.into(),
        label: label.clone(),
        unit: Unit::Nanoseconds,
        direction: Direction::SmallerBetter,
        values,
    }
}

#[derive(argh::FromArgs)]
/// E2E socket benchmark arguments.
struct Args {
    /// run in perf mode and write metrics in fuchsiaperf JSON format to
    /// the provided path
    #[argh(option)]
    output_fuchsiaperf: Option<std::path::PathBuf>,

    /// run with trace events enabled
    #[argh(switch)]
    tracing: bool,

    /// produce packet capture
    #[argh(switch)]
    pcap: bool,
}

#[fuchsia::main]
async fn main() {
    let Args { output_fuchsiaperf, tracing, pcap } = argh::from_env();
    let iter_count = if output_fuchsiaperf.is_some() {
        1000
    } else {
        // This iteration count is used both for the test variants to keep
        // run time low, and for the trace variants to ensure that the trace
        // file generated aren't too large and result in truncated trace
        // events.
        3
    };
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");

    let test_suite = "fuchsia.network.socket.tun.netstack3";
    let (client_realm, server_realm, mut fidl_proxies) =
        setup(&sandbox, NetstackVersion::ProdNetstack3, pcap).await;

    let tracer = if tracing {
        // TODO(https://fxbug.dev/42096938): Use race-free trace provider
        // initialization when available.
        fuchsia_trace_provider::trace_provider_create_with_fdio();

        let file = std::fs::File::create("/custom_artifacts/trace.fxt").expect("create trace file");
        let trace_provisioner = fuchsia_component::client::connect_to_protocol::<
            ftracing_controller::ProvisionerMarker,
        >()
        .expect("connect to tracing controller");
        let (tracing_socket, tracing_socket_write) = fidl::Socket::create_stream();
        let (trace_session, server) =
            fidl::endpoints::create_proxy::<ftracing_controller::SessionMarker>();
        trace_provisioner
            .initialize_tracing(
                server,
                &ftracing_controller::TraceConfig {
                    categories: Some(
                        [
                            "kernel:meta",
                            "kernel:sched",
                            "kernel:syscall",
                            "net",
                            "tun_socket_benchmarks",
                        ]
                        .into_iter()
                        .map(ToString::to_string)
                        .collect(),
                    ),
                    // Since oneshot mode is used, set the buffer size as large
                    // as possible so that trace events don't get dropped.
                    buffer_size_megabytes_hint: Some(64),
                    ..Default::default()
                },
                tracing_socket_write,
            )
            .expect("initialize tracing FIDL");
        trace_session
            .start_tracing(&ftracing_controller::StartOptions::default())
            .await
            .expect("starting tracing FIDL")
            .expect("start tracing");
        Some((trace_session, fasync::Socket::from_socket(tracing_socket), file))
    } else {
        None
    };

    let metrics = {
        let mut metrics = Vec::new();
        for transfer in [1 << 10, 10 << 10, 100 << 10, 500 << 10, 1000 << 10] {
            metrics.push(
                bench_tcp::<net_types::ip::Ipv4>(
                    test_suite,
                    iter_count,
                    &client_realm,
                    &server_realm,
                    transfer,
                    &mut fidl_proxies.tun_or_network,
                )
                .await,
            );
            metrics.push(
                bench_tcp::<net_types::ip::Ipv6>(
                    test_suite,
                    iter_count,
                    &client_realm,
                    &server_realm,
                    transfer,
                    &mut fidl_proxies.tun_or_network,
                )
                .await,
            );
        }

        // NB: All of these message sizes are kept below the MTU of 1500 bytes
        // so that fragmentation is not needed (NS3 doesn't currently support
        // fragmentation c.f. https://fxbug.dev/42079055).
        for message_size in [1, 100, 1 << 10] {
            // NB: The 1, 10, and 50 message counts match those in the loopback socket
            // benchmark to facilitate comparison of results, and 250 is as large as
            // reasonably possible to stay within Infra runtime limits.
            for message_count in [1, 10, 50, 250] {
                metrics.push(
                    bench_udp::<net_types::ip::Ipv4>(
                        test_suite,
                        iter_count,
                        &client_realm,
                        &server_realm,
                        message_size,
                        message_count,
                    )
                    .await,
                );
                metrics.push(
                    bench_udp::<net_types::ip::Ipv6>(
                        test_suite,
                        iter_count,
                        &client_realm,
                        &server_realm,
                        message_size,
                        message_count,
                    )
                    .await,
                );
            }
        }
        metrics
    };

    if let Some((trace_session, mut tracing_socket, mut file)) = tracer {
        let mut trace = Vec::new();
        let stop_and_terminate_fut = async move {
            let res = trace_session
                .stop_tracing(&ftracing_controller::StopOptions {
                    write_results: Some(true),
                    ..Default::default()
                })
                .await
                .map(|r| r.expect("stop tracing"));
            res.expect("stop tracing")
        };
        // NB: Terminating tracing is essentially infallible right now, since
        // StopResult is not a result, but a FIDL table with no fields in it,
        // thus it's safe to ignore.
        let (_, _): (ftracing_controller::StopResult, usize) = futures::future::join(
            stop_and_terminate_fut,
            tracing_socket.read_to_end(&mut trace).map(|r| r.expect("read trace")),
        )
        .await;

        file.write_all(trace.as_ref()).expect("write trace to file");
    }

    if let Some(output_fuchsiaperf) = output_fuchsiaperf {
        let metrics_json =
            serde_json::to_string_pretty(&metrics).expect("serialize metrics as JSON");
        std::fs::write(output_fuchsiaperf, metrics_json).expect("write metrics as custom artifact");
    }
}
