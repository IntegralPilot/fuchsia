// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

use std::collections::HashSet;
use std::convert::{TryFrom as _, TryInto as _};
use std::num::NonZeroU16;
use std::pin::pin;
use std::str::FromStr as _;

use diagnostics_assertions::assert_data_tree;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_dhcp as net_dhcp;
use fidl_fuchsia_net_dhcpv6 as net_dhcpv6;
use fidl_fuchsia_net_ext as fnet_ext;
use fidl_fuchsia_net_name as net_name;
use fidl_fuchsia_net_policy_socketproxy as fnp_socketproxy;
use fidl_fuchsia_net_root as fnet_root;
use fidl_fuchsia_net_routes as fnet_routes;
use fidl_fuchsia_net_routes_admin as fnet_routes_admin;
use fidl_fuchsia_net_routes_ext as fnet_routes_ext;
use fidl_fuchsia_testing as ftesting;
use fuchsia_async::{DurationExt as _, TimeoutExt as _};
use futures::future::{self, FusedFuture, Future, FutureExt as _};
use futures::stream::{self, StreamExt as _};
use futures::{AsyncReadExt as _, AsyncWriteExt as _};
use itertools::Itertools as _;
use log::info;
use net_declare::{fidl_ip, fidl_ip_v4, fidl_ip_v6, fidl_subnet, std_ip_v6, std_socket_addr};
use net_types::ip::{self as net_types_ip, Ip, Ipv4};
use netemul::{RealmTcpListener as _, RealmUdpSocket as _};
use netstack_testing_common::constants::ipv6 as ipv6_consts;
use netstack_testing_common::ndp::send_ra_with_router_lifetime;
use netstack_testing_common::realms::{
    KnownServiceProvider, Manager, ManagerConfig, Netstack, NetstackExt, SocketProxyType,
    TestSandboxExt as _, constants,
};
use netstack_testing_common::{
    ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT, Result, pause_fake_clock, wait_for_component_stopped,
};
use netstack_testing_macros::netstack_test;
use packet::serialize::{InnerPacketBuilder as _, NoOpSerializationContext, Serializer as _};
use packet::{NestableSerializer as _, ParsablePacket as _};
use packet_formats::ethernet::{
    ETHERNET_MIN_BODY_LEN_NO_TAG, EtherType, EthernetFrame, EthernetFrameBuilder,
    EthernetFrameLengthCheck, EthernetIpExt as _,
};
use packet_formats::icmp::ndp::options::{NdpOptionBuilder, RecursiveDnsServer};
use packet_formats::ip::{IpPacket as _, IpProto, Ipv6Proto};
use packet_formats::ipv6::{Ipv6Packet, Ipv6PacketBuilder};
use packet_formats::udp::{UdpPacket, UdpPacketBuilder, UdpParseArgs};
use packet_formats_dhcp::v6;
use policy_testing_common::{
    NetcfgOwnedDeviceArgs, add_default_route, add_device_to_devfs, verify_interface_added,
    with_netcfg_owned_device,
};
use test_case::test_case;

const DEFAULT_DNS_PORT: u16 = 53;

#[netstack_test]
#[variant(N, Netstack)]
async fn no_ip_literal<N: Netstack>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox
        .create_netstack_realm_with::<N, _, _>(
            name,
            &[KnownServiceProvider::DnsResolver, KnownServiceProvider::FakeClock],
        )
        .expect("create realm");

    let name_lookup =
        realm.connect_to_protocol::<net_name::LookupMarker>().expect("connect to protocol");

    let range = || [true, false].into_iter();

    let name_lookup = &name_lookup;
    futures::stream::iter(range().cartesian_product(range()))
        .for_each_concurrent(None, move |(ipv4_lookup, ipv6_lookup)| async move {
            assert_eq!(
                name_lookup
                    .lookup_ip(
                        "240.0.0.2",
                        &net_name::LookupIpOptions {
                            ipv4_lookup: Some(ipv4_lookup),
                            ipv6_lookup: Some(ipv6_lookup),
                            ..Default::default()
                        },
                    )
                    .await
                    .expect("call lookup IP"),
                Err(net_name::LookupError::InvalidArgs),
                "ipv4_lookup={},ipv6_lookup={}",
                ipv4_lookup,
                ipv6_lookup,
            );

            assert_eq!(
                name_lookup
                    .lookup_ip(
                        "abcd::2",
                        &net_name::LookupIpOptions {
                            ipv4_lookup: Some(ipv4_lookup),
                            ipv6_lookup: Some(ipv6_lookup),
                            ..Default::default()
                        },
                    )
                    .await
                    .expect("call lookup IP"),
                Err(net_name::LookupError::InvalidArgs),
                "ipv4_lookup={},ipv6_lookup={}",
                ipv4_lookup,
                ipv6_lookup,
            );
        })
        .await
}

/// Keep polling the lookup admin's DNS servers until it returns `expect`.
async fn poll_lookup_admin<
    F: Unpin + FusedFuture + Future<Output = Result<component_events::events::Stopped>>,
>(
    lookup_admin: &net_name::LookupAdminProxy,
    expect: &HashSet<fnet::SocketAddress>,
    mut wait_for_netmgr_fut: &mut F,
    poll_wait: zx::MonotonicDuration,
    retry_count: u64,
) {
    for i in 0..retry_count {
        futures::select! {
            () = fuchsia_async::Timer::new(poll_wait.after_now()).fuse() => (),
            stopped_event = wait_for_netmgr_fut => {
                panic!(
                    "the network manager unexpectedly exited with event: {:?}",
                    stopped_event,
                )
            }
        };

        let servers = HashSet::from_iter(
            lookup_admin.get_dns_servers().await.expect("failed to get DNS servers"),
        );
        println!("attempt {} got DNS servers {:?}", i, servers);

        if servers == *expect {
            return;
        }
    }

    // Too many retries.
    panic!(
        "timed out waiting for DNS server configurations; retry_count={}, poll_wait={:?}",
        retry_count, poll_wait,
    )
}

/// Keep polling the DnsServerWatcher's DNS servers until it returns `expect`.
async fn poll_dns_server_watcher<
    F: Unpin + FusedFuture + Future<Output = Result<component_events::events::Stopped>>,
>(
    dns_server_watcher_proxy: &net_name::DnsServerWatcherProxy,
    expect: &HashSet<fnet::SocketAddress>,
    mut wait_for_netmgr_fut: &mut F,
    wait_duration: zx::MonotonicDuration,
) {
    loop {
        futures::select! {
            servers = dns_server_watcher_proxy.watch_servers() => {
                match servers {
                    Ok(servers) => {
                        let servers: HashSet<_> = servers
                            .into_iter()
                            .filter_map(|server| server.address)
                            .collect();
                        if servers == *expect {
                            return;
                        }
                    }
                    Err(e) => panic!("received an error from DnsServerWatcher {e:?}"),
                };
            },
            () = fuchsia_async::Timer::new(wait_duration.after_now()).fuse() => {
                panic!("timed out waiting for DNS server list");
            }
            stopped_event = wait_for_netmgr_fut => {
                panic!(
                    "the network manager unexpectedly exited with event: {:?}",
                    stopped_event,
                )
            },
        };
    }
}

#[derive(Clone)]
enum DnsCheckType {
    LookupAdmin {
        /// Duration to sleep between polls.
        poll_wait: zx::MonotonicDuration,
        /// Maximum number of times to poll `LookupAdmin` to check
        // whether the DNS configuration succeeded.
        retry_count: u64,
    },
    DnsServerWatcher {
        /// The maximum Duration to wait for a response from the
        /// `DnsServerWatcher` hanging get to determine whether the
        /// DNS configuration succeeded.
        wait_duration: zx::MonotonicDuration,
    },
}

impl DnsCheckType {
    fn lookup_admin() -> Self {
        Self::LookupAdmin { poll_wait: zx::MonotonicDuration::from_seconds(1), retry_count: 60 }
    }

    fn dns_server_watcher() -> Self {
        Self::DnsServerWatcher { wait_duration: ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT }
    }

    // Determine whether the DNS servers are present given the CheckType.
    async fn evaluate_check<
        'a,
        M: Manager,
        F: Unpin + FusedFuture + Future<Output = Result<component_events::events::Stopped>>,
    >(
        self,
        realm: &netemul::TestRealm<'a>,
        mut wait_for_netmgr: &mut F,
        expect: &HashSet<fnet::SocketAddress>,
    ) {
        match self {
            DnsCheckType::LookupAdmin { poll_wait, retry_count } => {
                // Poll LookupAdmin until we get the servers
                // we want or after too many tries.
                let lookup_admin = realm
                    .connect_to_protocol::<net_name::LookupAdminMarker>()
                    .expect("failed to connect to LookupAdmin");
                poll_lookup_admin(
                    &lookup_admin,
                    &expect,
                    &mut wait_for_netmgr,
                    poll_wait,
                    retry_count,
                )
                .await
            }
            DnsCheckType::DnsServerWatcher { wait_duration } => {
                // Poll DnsServerWatcher hanging get until we
                // get the servers we want or after too long.
                let dns_server_watcher = realm
                    .connect_to_protocol_from_child::<net_name::DnsServerWatcherMarker>(
                        constants::netcfg::COMPONENT_NAME,
                    )
                    .expect("failed to connect to DnsServerWatcher");
                poll_dns_server_watcher(
                    &dns_server_watcher,
                    &expect,
                    &mut wait_for_netmgr,
                    wait_duration,
                )
                .await
            }
        }
    }
}

/// DNS server served by NDP.
const NDP_DNS_SERVER: fnet::Ipv6Address = fidl_ip_v6!("20a::1234:5678");

/// Tests that Netstack exposes DNS servers discovered through NDP and NetworkManager
/// appropriately publishes the DNS servers.
#[netstack_test]
#[variant(M, Manager)]
#[variant(N, Netstack)]
#[test_case(DnsCheckType::lookup_admin(); "lookup_admin")]
#[test_case(DnsCheckType::dns_server_watcher(); "dns_server_watcher")]
async fn discovered_ndp_dns<M: Manager, N: Netstack>(name: &str, check_type: DnsCheckType) {
    let name = name.to_string();
    // The device must be installed by netcfg in order to start the NDP watcher
    // on the interface.
    let _if_name = with_netcfg_owned_device::<M, N, _>(
        &name.clone(),
        ManagerConfig::Empty,
        NetcfgOwnedDeviceArgs {
            use_out_of_stack_dhcp_client: N::USE_OUT_OF_STACK_DHCP_CLIENT,
            socket_proxy_type: SocketProxyType::None,
            extra_known_service_providers: vec![],
        },
        |_, network, _, client_realm, _sandbox| {
            async move {
                info!("Observed interface up, beginning test");
                // Send a Router Advertisement to an EP on the same network with DNS server
                // configurations.
                let fake_ep =
                    network.create_fake_endpoint().expect("failed to create fake endpoint");

                let addresses = [NDP_DNS_SERVER.addr.into()];
                let rdnss = RecursiveDnsServer::new(9999, &addresses);
                let options = [NdpOptionBuilder::RecursiveDnsServer(rdnss)];
                send_ra_with_router_lifetime(&fake_ep, 0, &options, ipv6_consts::LINK_LOCAL_ADDR)
                    .await
                    .expect("failed to send router advertisement");
                info!("Sent Router Advertisement");

                // The list of servers we expect to observe via NDP.
                let expect = HashSet::from([fnet::SocketAddress::Ipv6(fnet::Ipv6SocketAddress {
                    address: NDP_DNS_SERVER,
                    port: DEFAULT_DNS_PORT,
                    zone_index: 0,
                })]);

                let wait_for_netmgr = wait_for_component_stopped(
                    &client_realm,
                    constants::netcfg::COMPONENT_NAME,
                    None,
                )
                .fuse();
                let mut wait_for_netmgr = pin!(wait_for_netmgr);
                check_type
                    .evaluate_check::<M, _>(&client_realm, &mut wait_for_netmgr, &expect)
                    .await
            }
            .boxed_local()
        },
    )
    .await;
}

/// Tests that DHCPv4 exposes DNS servers discovered through DHCPv4 and NetworkManager
/// appropriately publishes the DNS servers.
#[netstack_test]
#[variant(M, Manager)]
#[variant(N, Netstack)]
#[test_case(DnsCheckType::lookup_admin(); "lookup_admin")]
#[test_case(DnsCheckType::dns_server_watcher(); "dns_server_watcher")]
async fn discovered_dhcpv4_dns<M: Manager, N: Netstack>(name: &str, check_type: DnsCheckType) {
    const SERVER_ADDR: fnet::Subnet = fidl_subnet!("192.168.0.1/24");
    /// DNS server served by DHCP.
    const DHCP_DNS_SERVER: fnet::Ipv4Address = fidl_ip_v4!("123.12.34.56");

    const DEFAULT_DNS_PORT: u16 = 53;

    let name = name.to_string();
    // The device must be installed by netcfg in order to start the DHCPv4 client
    // on the interface, as the DHCPv4 DNS servers are found through the DHCPv4
    // client configuration and not the DnsServerWatcher.
    let _if_name = with_netcfg_owned_device::<M, N, _>(
        &name.clone(),
        ManagerConfig::Empty,
        NetcfgOwnedDeviceArgs {
            use_out_of_stack_dhcp_client: N::USE_OUT_OF_STACK_DHCP_CLIENT,
            socket_proxy_type: SocketProxyType::None,
            extra_known_service_providers: vec![],
        },
        |_, network, _, client_realm, sandbox| {
            async move {
                let server_realm = sandbox
                    .create_netstack_realm_with::<N, _, _>(
                        format!("{}_server", name),
                        &[
                            KnownServiceProvider::DnsResolver,
                            KnownServiceProvider::DhcpServer { persistent: false },
                            KnownServiceProvider::FakeClock,
                            KnownServiceProvider::SecureStash,
                        ],
                    )
                    .expect("failed to create server realm");

                // Start networking on server realm.
                let server_iface = server_realm
                    .join_network(&network, "server-ep")
                    .await
                    .expect("failed to configure server networking");
                server_iface
                    .add_address_and_subnet_route(SERVER_ADDR)
                    .await
                    .expect("configure address");

                let dhcp_server = server_realm
                    .connect_to_protocol::<net_dhcp::Server_Marker>()
                    .expect("failed to connect to DHCP server");

                let dhcp_server_ref = &dhcp_server;

                let server_addr_v4 = assert_matches::assert_matches!(
                    SERVER_ADDR.addr,
                    fnet::IpAddress::Ipv4(v4) => v4
                );
                let (range_start, range_stop) = {
                    let [a, b, c, d] = server_addr_v4.addr;
                    // A small pool of addresses derived from the server's.
                    (
                        fnet::Ipv4Address { addr: [a, b, c, d + 1] },
                        fnet::Ipv4Address { addr: [a, b, c, d + 4] },
                    )
                };
                stream::iter(
                    [
                        fidl_fuchsia_net_dhcp::Parameter::IpAddrs(vec![server_addr_v4]),
                        fidl_fuchsia_net_dhcp::Parameter::AddressPool(
                            fidl_fuchsia_net_dhcp::AddressPool {
                                prefix_length: Some(25),
                                range_start: Some(range_start),
                                range_stop: Some(range_stop),
                                ..Default::default()
                            },
                        ),
                        fidl_fuchsia_net_dhcp::Parameter::BoundDeviceNames(vec![
                            "eth2".to_string(),
                        ]),
                    ]
                    .iter_mut(),
                )
                .for_each_concurrent(None, |parameter| async move {
                    dhcp_server_ref
                        .set_parameter(parameter)
                        .await
                        .expect("failed to call dhcp/Server.SetParameter")
                        .map_err(zx::Status::err_from_raw)
                        .unwrap_or_else(|e| {
                            panic!(
                                "dhcp/Server.SetParameter({:?}) returned error: {:?}",
                                parameter, e
                            )
                        })
                })
                .await;

                dhcp_server
                    .set_option(&net_dhcp::Option_::DomainNameServer(vec![DHCP_DNS_SERVER]))
                    .await
                    .expect("Failed to set DNS option")
                    .map_err(zx::Status::err_from_raw)
                    .expect("dhcp/Server.SetOption returned error");

                dhcp_server
                    .start_serving()
                    .await
                    .expect("failed to call dhcp/Server.StartServing")
                    .map_err(zx::Status::err_from_raw)
                    .expect("dhcp/Server.StartServing returned error");

                // The list of servers we expect to retrieve from `fuchsia.net.name/LookupAdmin`.
                let expect = HashSet::from([fnet::SocketAddress::Ipv4(fnet::Ipv4SocketAddress {
                    address: DHCP_DNS_SERVER,
                    port: DEFAULT_DNS_PORT,
                })]);

                let wait_for_netmgr = wait_for_component_stopped(
                    &client_realm,
                    constants::netcfg::COMPONENT_NAME,
                    None,
                )
                .fuse();
                let mut wait_for_netmgr = pin!(wait_for_netmgr);
                check_type.evaluate_check::<M, _>(client_realm, &mut wait_for_netmgr, &expect).await
            }
            .boxed_local()
        },
    )
    .await;
}

/// Tests that DHCPv6 exposes DNS servers discovered dynamically and NetworkManager
/// appropriately publishes the DNS servers.
#[netstack_test]
#[variant(M, Manager)]
#[variant(N, Netstack)]
#[test_case(DnsCheckType::lookup_admin(); "lookup_admin")]
#[test_case(DnsCheckType::dns_server_watcher(); "dns_server_watcher")]
async fn discovered_dhcpv6_dns<M: Manager, N: Netstack>(name: &str, check_type: DnsCheckType) {
    /// DHCPv6 server IP.
    const DHCPV6_SERVER: net_types_ip::Ipv6Addr =
        net_types_ip::Ipv6Addr::from_bytes(std_ip_v6!("fe80::1").octets());
    /// DNS server served by DHCPv6.
    const DHCPV6_DNS_SERVER: fnet::Ipv6Address = fidl_ip_v6!("20a::1234:5678");

    const DEFAULT_DNS_PORT: u16 = 53;

    let name = name.to_string();
    // Install the device into the Netstack via netcfg so that the DHCPv6
    // client is started on the interface.
    let _if_name = with_netcfg_owned_device::<M, N, _>(
        &name.clone(),
        ManagerConfig::Dhcpv6,
        NetcfgOwnedDeviceArgs {
            use_out_of_stack_dhcp_client: N::USE_OUT_OF_STACK_DHCP_CLIENT,
            socket_proxy_type: SocketProxyType::None,
            extra_known_service_providers: vec![KnownServiceProvider::Dhcpv6Client],
        },
        |_, network, _, client_realm, _sandbox| {
            async move {
                // Wait for the DHCPv6 information request.
                let fake_ep =
                    network.create_fake_endpoint().expect("failed to create fake endpoint");
                let (src_mac, dst_mac, src_ip, dst_ip, src_port, tx_id) = fake_ep
                    .frame_stream()
                    .map(|r| r.expect("error getting OnData event"))
                    .filter_map(|(data, dropped)| {
                        assert_eq!(dropped, 0);

                        let mut data = data.as_slice();

                        future::ready((|| {
                            let ethernet =
                                EthernetFrame::parse(&mut data, EthernetFrameLengthCheck::NoCheck)
                                    .expect("parse ethernet");

                            // DHCPv6 messages are held in IPv6 packets.
                            if ethernet.ethertype() != Some(net_types_ip::Ipv6::ETHER_TYPE) {
                                return None;
                            }
                            let ipv6 = Ipv6Packet::parse(&mut data, ()).expect("parse ipv6");

                            let src_ip = ipv6.src_ip();
                            let dst_ip = ipv6.dst_ip();

                            // DHCPv6 messages are held in UDP packets.
                            if ipv6.proto() != Ipv6Proto::Proto(IpProto::Udp) {
                                return None;
                            }
                            let udp =
                                UdpPacket::parse(&mut data, UdpParseArgs::new(src_ip, dst_ip))
                                    .expect("parse udp");

                            // We only care about UDP packets directed at a DHCPv6 server.
                            if udp.dst_port().get() != net_dhcpv6::RELAY_AGENT_AND_SERVER_PORT {
                                return None;
                            }

                            // We only care about DHCPv6 messages.
                            let msg = v6::Message::parse(&mut data, ()).expect("parse dhcpv6");

                            // We only care about DHCPv6 information requests.
                            if msg.msg_type() != v6::MessageType::InformationRequest {
                                return None;
                            }

                            // We only care about DHCPv6 information requests for DNS servers.
                            for opt in msg.options() {
                                if let v6::ParsedDhcpOption::Oro(codes) = opt {
                                    if !codes.contains(&v6::OptionCode::DnsServers) {
                                        return None;
                                    }
                                }
                            }

                            Some((
                                ethernet.src_mac(),
                                ethernet.dst_mac(),
                                src_ip,
                                dst_ip,
                                udp.src_port(),
                                msg.transaction_id().clone(),
                            ))
                        })())
                    })
                    .next()
                    .on_timeout(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT.after_now(), || {
                        panic!("timed out waiting for the DHCPv6 Information request");
                    })
                    .await
                    .expect("ran out of incoming frames");
                assert!(
                    src_ip.is_unicast_link_local(),
                    "src ip {} should be a unicast link-local",
                    src_ip
                );
                assert_eq!(
                    Ok(std::net::Ipv6Addr::from(dst_ip.ipv6_bytes())),
                    std::net::Ipv6Addr::from_str(
                        net_dhcpv6::RELAY_AGENT_AND_SERVER_LINK_LOCAL_MULTICAST_ADDRESS
                    ),
                    "dst ip should be the DHCPv6 servers multicast IP"
                );
                assert_eq!(
                    src_port.map(|x| x.get()),
                    Some(net_dhcpv6::DEFAULT_CLIENT_PORT),
                    "should use RFC defined src port"
                );

                // Send the DHCPv6 reply.
                let dns_servers = [net_types_ip::Ipv6Addr::from(DHCPV6_DNS_SERVER.addr)];
                let options =
                    [v6::DhcpOption::ServerId(&[]), v6::DhcpOption::DnsServers(&dns_servers)];
                let builder = v6::MessageBuilder::new(v6::MessageType::Reply, tx_id, &options);
                let ser = builder
                    .into_serializer()
                    .wrap_in(UdpPacketBuilder::new(
                        DHCPV6_SERVER,
                        src_ip,
                        NonZeroU16::new(net_dhcpv6::RELAY_AGENT_AND_SERVER_PORT),
                        NonZeroU16::new(net_dhcpv6::DEFAULT_CLIENT_PORT)
                            .expect("default DHCPv6 client port is non-zero"),
                    ))
                    .wrap_in(Ipv6PacketBuilder::new(
                        DHCPV6_SERVER,
                        src_ip,
                        ipv6_consts::DEFAULT_HOP_LIMIT,
                        IpProto::Udp.into(),
                    ))
                    .wrap_in(EthernetFrameBuilder::new(
                        dst_mac,
                        src_mac,
                        EtherType::Ipv6,
                        ETHERNET_MIN_BODY_LEN_NO_TAG,
                    ))
                    .serialize_vec_outer(&mut NoOpSerializationContext)
                    .unwrap_or_else(|(err, _serializer)| {
                        panic!("failed to serialize DHCPv6 packet: {:?}", err)
                    })
                    .unwrap_b();
                let () =
                    fake_ep.write(ser.as_ref()).await.expect("failed to write to fake endpoint");

                // The list of servers we expect to retrieve from `fuchsia.net.name/LookupAdmin`.
                let expect = HashSet::from([fnet::SocketAddress::Ipv6(fnet::Ipv6SocketAddress {
                    address: DHCPV6_DNS_SERVER,
                    port: DEFAULT_DNS_PORT,
                    zone_index: 0,
                })]);

                let wait_for_netmgr = wait_for_component_stopped(
                    &client_realm,
                    constants::netcfg::COMPONENT_NAME,
                    None,
                )
                .fuse();
                let mut wait_for_netmgr = pin!(wait_for_netmgr);
                check_type.evaluate_check::<M, _>(client_realm, &mut wait_for_netmgr, &expect).await
            }
            .boxed_local()
        },
    )
    .await;
}

#[netstack_test]
#[variant(M, Manager)]
#[variant(N, Netstack)]
#[test_case(DnsCheckType::lookup_admin(); "lookup_admin")]
#[test_case(DnsCheckType::dns_server_watcher(); "dns_server_watcher")]
async fn discovered_starnix_networks_dns<M: Manager, N: Netstack>(
    name: &str,
    check_type: DnsCheckType,
) {
    const NETWORK_ID1: u32 = 1;
    const NETWORK_ID2: u32 = 2;
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox
        .create_netstack_realm_with::<N, _, _>(
            format!("{}-realm", name),
            [
                KnownServiceProvider::Manager {
                    agent: M::MANAGEMENT_AGENT,
                    config: ManagerConfig::EnableSocketProxy,
                    use_dhcp_server: false,
                    use_out_of_stack_dhcp_client: N::USE_OUT_OF_STACK_DHCP_CLIENT,
                    socket_proxy_type: SocketProxyType::Real,
                },
                KnownServiceProvider::DnsResolver,
                KnownServiceProvider::FakeClock,
                KnownServiceProvider::SocketProxy,
            ]
            .into_iter()
            .chain(
                N::USE_OUT_OF_STACK_DHCP_CLIENT
                    .then_some(KnownServiceProvider::DhcpClient)
                    .into_iter(),
            ),
        )
        .expect("create netstack realm");

    let network_registry = realm
        .connect_to_protocol::<fnp_socketproxy::StarnixNetworksMarker>()
        .expect("while connecting to network registry");

    // Add a network and observe the DNS servers in the DNS update.
    let mut network1 = fnp_socketproxy::Network {
        network_id: Some(NETWORK_ID1),
        info: Some(starnix_network_info(123, 456)),
        dns_servers: Some(fnp_socketproxy::NetworkDnsServers {
            v4: Some(vec![fidl_ip_v4!("192.0.2.1")]),
            v6: Some(vec![]),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert_matches::assert_matches!(network_registry.add(&network1).await, Ok(Ok(())));

    // The list of servers we expect to retrieve from the DNS update. These DNS servers
    // can come through LookupAdmin or netcfg's instance of DnsServerWatcher since netcfg is
    // the source of truth for both DNS protocols.
    let expect1 = HashSet::from([fnet::SocketAddress::Ipv4(fnet::Ipv4SocketAddress {
        address: fidl_ip_v4!("192.0.2.1"),
        port: DEFAULT_DNS_PORT,
    })]);
    let wait_for_netmgr =
        wait_for_component_stopped(&realm, constants::netcfg::COMPONENT_NAME, None).fuse();
    let mut wait_for_netmgr = pin!(wait_for_netmgr);
    check_type.clone().evaluate_check::<M, _>(&realm, &mut wait_for_netmgr, &expect1).await;

    // Perform an update and confirm the updated DNS server appears in the DNS update.
    network1.dns_servers = Some(fnp_socketproxy::NetworkDnsServers {
        v4: Some(vec![fidl_ip_v4!("192.0.2.2")]),
        v6: Some(vec![]),
        ..Default::default()
    });
    assert_matches::assert_matches!(network_registry.update(&network1).await, Ok(Ok(())));
    let expect2 = HashSet::from([fnet::SocketAddress::Ipv4(fnet::Ipv4SocketAddress {
        address: fidl_ip_v4!("192.0.2.2"),
        port: DEFAULT_DNS_PORT,
    })]);
    check_type.clone().evaluate_check::<M, _>(&realm, &mut wait_for_netmgr, &expect2).await;

    // Add a second network and observe the DNS servers in the DNS update.
    let network2 = fnp_socketproxy::Network {
        network_id: Some(NETWORK_ID2),
        info: Some(starnix_network_info(456, 789)),
        dns_servers: Some(fnp_socketproxy::NetworkDnsServers {
            v4: Some(vec![]),
            v6: Some(vec![fidl_ip_v6!("2001:db8::2222")]),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert_matches::assert_matches!(network_registry.add(&network2).await, Ok(Ok(())));
    let expect3 = HashSet::from([
        fnet::SocketAddress::Ipv4(fnet::Ipv4SocketAddress {
            address: fidl_ip_v4!("192.0.2.2"),
            port: DEFAULT_DNS_PORT,
        }),
        fnet::SocketAddress::Ipv6(fnet::Ipv6SocketAddress {
            address: fidl_ip_v6!("2001:db8::2222"),
            port: DEFAULT_DNS_PORT,
            zone_index: 0,
        }),
    ]);
    check_type.clone().evaluate_check::<M, _>(&realm, &mut wait_for_netmgr, &expect3).await;

    // To clear the available DNS servers, the networks in the Starnix
    // registry must be fully removed.
    assert_matches::assert_matches!(network_registry.remove(NETWORK_ID1).await, Ok(Ok(())));
    assert_matches::assert_matches!(network_registry.remove(NETWORK_ID2).await, Ok(Ok(())));

    check_type.evaluate_check::<M, _>(&realm, &mut wait_for_netmgr, &HashSet::new()).await;

    realm.shutdown().await.expect("failed to shutdown realm");
}

fn starnix_network_info(mark: u32, handle: u64) -> fnp_socketproxy::NetworkInfo {
    fnp_socketproxy::NetworkInfo::Starnix(fnp_socketproxy::StarnixNetworkInfo {
        mark: Some(mark),
        handle: Some(handle),
        ..Default::default()
    })
}

#[netstack_test]
#[variant(M, Manager)]
#[variant(N, Netstack)]
#[test_case(DnsCheckType::lookup_admin(); "lookup_admin")]
#[test_case(DnsCheckType::dns_server_watcher(); "dns_server_watcher")]
async fn discovered_starnix_fuchsia_networks_dns<M: Manager, N: Netstack>(
    name: &str,
    check_type: DnsCheckType,
) {
    const STARNIX_NETWORK_ID: u32 = 2;
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox
        .create_netstack_realm_with::<N, _, _>(
            format!("{name}-realm"),
            [
                KnownServiceProvider::Manager {
                    agent: M::MANAGEMENT_AGENT,
                    config: ManagerConfig::EnableSocketProxy,
                    use_dhcp_server: false,
                    use_out_of_stack_dhcp_client: N::USE_OUT_OF_STACK_DHCP_CLIENT,
                    socket_proxy_type: SocketProxyType::Real,
                },
                KnownServiceProvider::DnsResolver,
                KnownServiceProvider::FakeClock,
                KnownServiceProvider::SocketProxy,
            ]
            .into_iter()
            .chain(
                N::USE_OUT_OF_STACK_DHCP_CLIENT
                    .then_some(KnownServiceProvider::DhcpClient)
                    .into_iter(),
            ),
        )
        .expect("create netstack realm");
    let starnix_networks = realm
        .connect_to_protocol::<fnp_socketproxy::StarnixNetworksMarker>()
        .expect("while connecting to StarnixNetworks");

    // Add a Starnix network to the Starnix NetworkRegistry. Since there is no
    // default network in the Fuchsia NetworkRegistry, the network will be
    // observed via the watcher without setting it as the default.
    let mut starnix_network = fnp_socketproxy::Network {
        network_id: Some(STARNIX_NETWORK_ID),
        info: Some(starnix_network_info(123, 456)),
        dns_servers: Some(fnp_socketproxy::NetworkDnsServers {
            v4: Some(vec![fidl_ip_v4!("192.0.2.1")]),
            v6: Some(vec![]),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert_matches::assert_matches!(starnix_networks.add(&starnix_network).await, Ok(Ok(())));
    let expect1 = HashSet::from([fnet::SocketAddress::Ipv4(fnet::Ipv4SocketAddress {
        address: fidl_ip_v4!("192.0.2.1"),
        port: DEFAULT_DNS_PORT,
    })]);
    let wait_for_netmgr =
        wait_for_component_stopped(&realm, constants::netcfg::COMPONENT_NAME, None).fuse();
    let mut wait_for_netmgr = pin!(wait_for_netmgr);
    check_type.clone().evaluate_check::<M, _>(&realm, &mut wait_for_netmgr, &expect1).await;

    // Add a device to devfs for netcfg to manage and install into the Netstack.
    // The network will not yet be added to FuchsiaNetworks or set it as default.
    // It will not be a default network candidate until it has a default route.
    let network = sandbox.create_network(name).await.expect("create network");
    let _endpoint =
        add_device_to_devfs::<M>(&network, &realm, format!("{}-fuchsia-ep", name), None).await;

    // Make sure the Netstack got the new device added.
    let (_interface_state, if_id, _if_name) = verify_interface_added::<M>(&realm).await;

    // Update the Starnix network's DNS servers and wait for the update. When we
    // observe this update, we observed there was not a Fuchsia NetworkRegistry
    // related DNS update.
    starnix_network.dns_servers = Some(fnp_socketproxy::NetworkDnsServers {
        v4: Some(vec![fidl_ip_v4!("192.0.2.2")]),
        v6: Some(vec![]),
        ..Default::default()
    });
    assert_matches::assert_matches!(starnix_networks.update(&starnix_network).await, Ok(Ok(())));
    let starnix_updated_dns = fnet::SocketAddress::Ipv4(fnet::Ipv4SocketAddress {
        address: fidl_ip_v4!("192.0.2.2"),
        port: DEFAULT_DNS_PORT,
    });
    check_type
        .clone()
        .evaluate_check::<M, _>(&realm, &mut wait_for_netmgr, &HashSet::from([starnix_updated_dns]))
        .await;

    let root_routes = realm
        .connect_to_protocol::<fnet_root::RoutesV4Marker>()
        .expect("connect to fuchsia.net.root.RoutesV4");
    let (global_route_set, server_end) =
        fidl::endpoints::create_proxy::<fnet_routes_admin::RouteSetV4Marker>();
    root_routes.global_route_set(server_end).expect("create global RouteSetV4");

    // Acquire the control for the interface so we can directly add a default route to the
    // interface without going through DHCP. This meets the condition for the interface to
    // be added to Fuchsia's NetworkRegistry and set it as the default.
    assert!(add_default_route::<Ipv4>(&realm, if_id, &global_route_set).await);

    // The current state should be an empty DNS server list because the Fuchsia
    // network is taking precedence over the Starnix NetworkRegistry.
    check_type.clone().evaluate_check::<M, _>(&realm, &mut wait_for_netmgr, &HashSet::new()).await;

    // Send a Router Advertisement to an EP on the same network with DNS server
    // configurations.
    let fake_ep = network.create_fake_endpoint().expect("failed to create fake endpoint");

    let addresses = [NDP_DNS_SERVER.addr.into()];
    let rdnss = RecursiveDnsServer::new(9999, &addresses);
    let options = [NdpOptionBuilder::RecursiveDnsServer(rdnss)];
    send_ra_with_router_lifetime(&fake_ep, 0, &options, ipv6_consts::LINK_LOCAL_ADDR)
        .await
        .expect("failed to send router advertisement");

    // The list of servers we expect to observe via NDP.
    let ndp_dns = fnet::SocketAddress::Ipv6(fnet::Ipv6SocketAddress {
        address: NDP_DNS_SERVER,
        port: DEFAULT_DNS_PORT,
        zone_index: 0,
    });
    check_type
        .clone()
        .evaluate_check::<M, _>(&realm, &mut wait_for_netmgr, &HashSet::from([ndp_dns]))
        .await;

    // TODO(https://fxbug.dev/42175016): Respect DNS servers lookups based on
    // the active default network.
    //
    // Remove the default route, making the Fuchsia network no longer default.
    // The DNS servers should reflect the Starnix network's DNS too.
    let _: bool = fnet_routes_ext::admin::remove_route::<Ipv4>(
        &global_route_set,
        &fnet_routes_ext::Route {
            destination: Ipv4::ALL_ADDRS_SUBNET,
            action: fnet_routes_ext::RouteAction::Forward(fnet_routes_ext::RouteTarget::<Ipv4> {
                outbound_interface: if_id,
                next_hop: None,
            }),
            properties: fnet_routes_ext::RouteProperties {
                specified_properties: fnet_routes_ext::SpecifiedRouteProperties {
                    metric: fnet_routes::SpecifiedMetric::InheritedFromInterface(
                        fnet_routes::Empty,
                    ),
                },
            },
        }
        .try_into()
        .expect("convert to FIDL route"),
    )
    .await
    .expect("should not see RouteSet FIDL error")
    .expect("should not see RouteSet error");

    // We should observe both the NDP and Starnix network-provided DNS servers.
    // The Fuchsia network is no longer taking precedence.
    check_type
        .evaluate_check::<M, _>(
            &realm,
            &mut wait_for_netmgr,
            &HashSet::from([starnix_updated_dns, ndp_dns]),
        )
        .await;

    realm.shutdown().await.expect("failed to shutdown realm");
}

async fn mock_udp_name_server(
    socket: &fuchsia_async::net::UdpSocket,
    handle_query: impl Fn(&trust_dns_proto::op::Message) -> trust_dns_proto::op::Message,
) {
    use trust_dns_proto::op::{Message, MessageType, OpCode};

    let mut buf = [0; MAX_DNS_UDP_MESSAGE_LEN];
    loop {
        let (read, src_addr) = socket.recv_from(&mut buf).await.expect("receive DNS query");
        let query = Message::from_vec(&buf[..read]).expect("deserialize DNS query");
        let mut response = handle_query(&query);
        let _: &mut Message = response
            .set_message_type(MessageType::Response)
            .set_op_code(OpCode::Update)
            .set_id(query.id())
            .add_queries(query.queries().to_vec());
        let response = response.to_vec().expect("serialize DNS response");
        let written = socket.send_to(&response, src_addr).await.expect("send DNS response");
        assert_eq!(written, response.len());
    }
}

async fn mock_tcp_name_server(
    listener: fuchsia_async::net::TcpListener,
    handle_query: impl Fn(&trust_dns_proto::op::Message) -> trust_dns_proto::op::Message,
) {
    use trust_dns_proto::op::{Message, MessageType, OpCode};

    let mut incoming = listener.accept_stream();
    while let Some(conn) = incoming.next().await {
        let (mut stream, _src_addr) = conn.expect("accept incoming TCP connection");
        let handle_query = &handle_query;
        // Read the two-octet length field, which tells us the length of the following DNS
        // message, in network (big-endian) order.
        let mut len_buf = [0_u8; 2];
        while let Ok(()) = stream.read_exact(&mut len_buf).await {
            let len = usize::from(u16::from_be_bytes(len_buf));
            let mut buf = vec![0_u8; len];
            stream.read_exact(&mut buf).await.expect("receive DNS query");
            let query = Message::from_vec(&buf).expect("deserialize DNS query");

            let mut response = handle_query(&query);
            let _: &mut Message = response
                .set_message_type(MessageType::Response)
                .set_op_code(OpCode::Update)
                .set_id(query.id())
                .add_queries(query.queries().to_vec());
            let response = response.to_vec().expect("serialize DNS response");

            // Write the two-octet length field.
            let len = u16::try_from(response.len()).unwrap().to_be_bytes();
            stream.write_all(&len).await.expect("send length field");
            stream.write_all(&response).await.expect("send DNS response");
        }
    }
}

fn answer_for_hostname(
    hostname: &str,
    resolved_addr: fnet::IpAddress,
) -> trust_dns_proto::rr::Record {
    use trust_dns_proto::rr::{DNSClass, Name, RData, Record, RecordType};

    let mut answer = Record::new();
    let fnet_ext::IpAddress(addr) = resolved_addr.into();
    let _: &mut Record = match addr {
        std::net::IpAddr::V4(addr) => {
            answer.set_rr_type(RecordType::A).set_data(Some(RData::A(addr)))
        }
        std::net::IpAddr::V6(addr) => {
            answer.set_rr_type(RecordType::AAAA).set_data(Some(RData::AAAA(addr)))
        }
    }
    .set_dns_class(DNSClass::IN)
    .set_name(Name::from_str(hostname).expect("parse hostname"));
    answer
}

const MAX_DNS_UDP_MESSAGE_LEN: usize = 512;
const EXAMPLE_HOSTNAME: &str = "www.example.com.";
const EXAMPLE_IPV4_ADDR: fnet::IpAddress = fidl_ip!("93.184.216.34");
const EXAMPLE_IPV6_ADDR: fnet::IpAddress = fidl_ip!("2606:2800:220:1:248:1893:25c8:1946");

#[netstack_test]
#[variant(N, Netstack)]
async fn successfully_retrieves_ipv6_record_despite_ipv4_timeout<N: Netstack>(name: &str) {
    use trust_dns_proto::op::{Message, ResponseCode};
    use trust_dns_proto::rr::RecordType;

    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm = sandbox
        .create_netstack_realm_with::<N, _, _>(
            name,
            &[KnownServiceProvider::DnsResolver, KnownServiceProvider::FakeClock],
        )
        .expect("failed to create realm");

    let fake_clock: ftesting::FakeClockControlProxy = realm
        .connect_to_protocol::<ftesting::FakeClockControlMarker>()
        .expect("failed to connect to FakeClockControl");
    fake_clock.pause().await.expect("failed to pause fake clock");

    let dns_server: std::net::SocketAddr = std_socket_addr!("127.0.0.1:1234");
    let (socket, fuchsia_async::net::TcpListener { .. }) =
        setup_dns_server(&realm, dns_server).await;

    let name_lookup =
        realm.connect_to_protocol::<net_name::LookupMarker>().expect("failed to connect to Lookup");

    let mut lookup_fut = pin!(
        async {
            let ips = name_lookup
                .lookup_ip(
                    EXAMPLE_HOSTNAME,
                    &net_name::LookupIpOptions {
                        ipv4_lookup: Some(true),
                        ipv6_lookup: Some(true),
                        sort_addresses: Some(true),
                        ..Default::default()
                    },
                )
                .await
                .expect("FIDL error")
                .expect("lookup_ip error");
            let want = net_name::LookupResult {
                addresses: Some(vec![EXAMPLE_IPV6_ADDR]),
                ..Default::default()
            };
            assert_eq!(ips, want);
        }
        .fuse()
    );

    let mut response_fut = pin!(
        async {
            use trust_dns_proto::op::{MessageType, OpCode};

            match async_utils::fold::fold_while(
                {
                    let socket = &socket;
                    let buf = [0; MAX_DNS_UDP_MESSAGE_LEN];
                    futures::stream::unfold((socket, buf), |(socket, mut buf)| async move {
                        let (read, src_addr) =
                            socket.recv_from(&mut buf).await.expect("receive DNS query");
                        let query = Message::from_vec(&buf[..read]).expect("deserialize DNS query");
                        Some(((query, src_addr), (socket, buf)))
                    })
                },
                (false, false),
                |(seen_v4, seen_v6), (query, src_addr)| {
                    let socket = &socket;
                    async move {
                        let id = query.id();
                        let queries = query.queries().to_vec();
                        let query = match query.queries() {
                            [single_query] => single_query,
                            slice => panic!(
                                "Unexpectedly received message with {}: {:?}",
                                if slice.len() == 0 { "no queries" } else { "more than one query" },
                                query
                            ),
                        };
                        let requested_record_type = query.query_type();
                        let state @ (seen_v4, seen_v6) = match &requested_record_type {
                            RecordType::A => (true, seen_v6),
                            RecordType::AAAA => (seen_v4, true),
                            _ => (seen_v4, seen_v6),
                        };
                        let fold_step = if seen_v4 && seen_v6 {
                            async_utils::fold::FoldWhile::Done(())
                        } else {
                            async_utils::fold::FoldWhile::Continue(state)
                        };
                        let answer_addr = match requested_record_type {
                            RecordType::A => return fold_step,
                            RecordType::AAAA => EXAMPLE_IPV6_ADDR,
                            _ => {
                                panic!(
                                    "DNS resolver should only request A or AAAA records, got {}",
                                    query
                                )
                            }
                        };
                        let answer = answer_for_hostname(EXAMPLE_HOSTNAME, answer_addr);
                        let mut response = Message::new();
                        let _: &mut Message = response
                            .set_response_code(ResponseCode::NoError)
                            .add_answer(answer)
                            .set_message_type(MessageType::Response)
                            .set_op_code(OpCode::Update)
                            .set_id(id)
                            .add_queries(queries);
                        let response = response.to_vec().expect("serialize DNS response");
                        let written =
                            socket.send_to(&response, src_addr).await.expect("send DNS response");
                        assert_eq!(written, response.len());
                        fold_step
                    }
                },
            )
            .await
            {
                async_utils::fold::FoldResult::ShortCircuited(()) => {}
                async_utils::fold::FoldResult::StreamEnded((seen_v4, seen_v6)) => panic!(
                    "Stream unexpectedly ended before seeing requests for both v4 and v6 records. \
                    seen_v4: {}, seen_v6: {}",
                    seen_v4, seen_v6
                ),
            }
        }
        .fuse()
    );

    futures::select! {
        () = lookup_fut => panic!("lookup_fut not expected to have completed"),
        () = response_fut => (),
    };

    futures::stream::once(futures::future::ready(()))
        .chain(fuchsia_async::Interval::new(
            // Wait an arbitrary short duration to avoid busy-looping.
            zx::MonotonicDuration::from_millis(100),
        ))
        .take_until(lookup_fut)
        .for_each(|()| async {
            fake_clock
                .advance(&ftesting::Increment::Determined(
                    // Advance the fake clock by a larger amount than the real time we are waiting
                    // in order to speed up the test run-time.
                    zx::MonotonicDuration::from_seconds(1).into_nanos(),
                ))
                .await
                .expect("fake clock FIDL error")
                .expect("failed to increment fake clock");
        })
        .await;
}

#[netstack_test]
#[variant(N, Netstack)]
async fn fallback_on_error_response_code<N: Netstack>(name: &str) {
    use itertools::Itertools as _;
    use trust_dns_proto::op::{Message, ResponseCode};
    use trust_dns_proto::rr::RecordType;

    #[derive(Debug)]
    struct FallbackTestCase {
        ipv4_lookup: bool,
        ipv6_lookup: bool,
        error_response_code: trust_dns_proto::op::ResponseCode,
    }

    let range = || IntoIterator::into_iter([true, false]);

    for test_case in range()
        .cartesian_product(range())
        .cartesian_product([
            ResponseCode::Refused,
            ResponseCode::NXDomain,
            ResponseCode::FormErr,
            ResponseCode::ServFail,
            ResponseCode::NotAuth,
            ResponseCode::BADMODE,
        ])
        .filter_map(|((ipv4_lookup, ipv6_lookup), error_response_code)| {
            (ipv4_lookup || ipv6_lookup).then(|| FallbackTestCase {
                ipv4_lookup,
                ipv6_lookup,
                error_response_code,
            })
        })
    {
        eprintln!("Entering test case: {:?}", test_case);
        let FallbackTestCase { ipv4_lookup, ipv6_lookup, error_response_code } = test_case;

        let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
        let realm = sandbox
            .create_netstack_realm_with::<N, _, _>(
                name,
                &[KnownServiceProvider::DnsResolver, KnownServiceProvider::FakeClock],
            )
            .expect("failed to create realm");
        // The DNS resolver contains a timeout after which any in-flight queries are dropped, which
        // can cause test flakes.
        pause_fake_clock(&realm).await.expect("failed to pause fake clock");

        // Mock name servers in priority order.
        let erroring_dns_server: std::net::SocketAddr = std_socket_addr!("127.0.0.1:1234");
        let fallback_dns_server: std::net::SocketAddr = std_socket_addr!("127.0.0.2:5678");
        let dns_servers = &[
            fnet_ext::SocketAddress(erroring_dns_server).into(),
            fnet_ext::SocketAddress(fallback_dns_server).into(),
        ];
        let lookup_admin = realm
            .connect_to_protocol::<net_name::LookupAdminMarker>()
            .expect("failed to connect to LookupAdmin");
        lookup_admin
            .set_dns_servers(dns_servers)
            .await
            .expect("FIDL error")
            .expect("failed to set DNS servers");
        let servers = lookup_admin.get_dns_servers().await.expect("failed to get DNS servers");
        assert_eq!(servers, dns_servers);

        let erroring_sock =
            fuchsia_async::net::UdpSocket::bind_in_realm(&realm, erroring_dns_server)
                .await
                .expect("failed to create socket");
        let fallback_sock =
            fuchsia_async::net::UdpSocket::bind_in_realm(&realm, fallback_dns_server)
                .await
                .expect("failed to create socket");

        let name_lookup = realm
            .connect_to_protocol::<net_name::LookupMarker>()
            .expect("failed to connect to Lookup");
        let mut lookup_fut = pin!(
            async {
                let ips = {
                    let mut ips = name_lookup
                        .lookup_ip(
                            EXAMPLE_HOSTNAME,
                            &net_name::LookupIpOptions {
                                ipv4_lookup: Some(ipv4_lookup),
                                ipv6_lookup: Some(ipv6_lookup),
                                sort_addresses: Some(true),
                                ..Default::default()
                            },
                        )
                        .await
                        .expect("FIDL error")
                        .expect("lookup_ip error");
                    if let Some(ref mut addresses) = ips.addresses {
                        // Sort the returned addresses because we don't care about their order when
                        // asserting on the contents of the response.
                        addresses.sort();
                    }
                    ips
                };
                let want_addresses = std::iter::empty()
                    .chain(ipv4_lookup.then(|| EXAMPLE_IPV4_ADDR))
                    .chain(ipv6_lookup.then(|| EXAMPLE_IPV6_ADDR))
                    // Sort the expected addresses because we don't care about their order when
                    // asserting on the contents of the response.
                    .sorted()
                    .collect::<Vec<_>>();
                let want = net_name::LookupResult {
                    addresses: Some(want_addresses),
                    ..Default::default()
                };
                assert_eq!(ips, want, "test case: {:?}", test_case);
            }
            .fuse()
        );

        // The erroring name server expects initial queries from dns-resolver, and always replies
        // with an error response code.
        let mut error_server_fut = pin!(
            mock_udp_name_server(&erroring_sock, |_: &Message| {
                let mut response = Message::new();
                let _: &mut Message = response.set_response_code(error_response_code);
                response
            })
            .fuse()
        );
        let expected_record_types = std::iter::empty()
            .chain(ipv4_lookup.then(|| RecordType::A))
            .chain(ipv6_lookup.then(|| RecordType::AAAA))
            .collect::<std::collections::HashSet<_>>();
        // The fallback name server expects fallback queries from dns-resolver, and replies with a
        // non-error response, unless it gets a query for a different record type than it expects.
        let mut fallback_fut = pin!(
            {
                mock_udp_name_server(&fallback_sock, move |message| {
                    let query = {
                        let mut queries = message.queries().iter();
                        let query = queries.next().unwrap_or_else(|| {
                            panic!(
                                "message {:?} should have exactly one query, got zero instead",
                                message
                            )
                        });
                        assert_eq!(
                            queries.count(),
                            0,
                            "message {:?} should have exactly one query, got multiple instead",
                            message
                        );
                        query
                    };
                    let requested_record_type = query.query_type();

                    if !expected_record_types.contains(&requested_record_type) {
                        // Reply with a `SERVFAIL` response since we want to ignore this query.
                        let mut response = Message::new();
                        let _: &mut Message = response.set_response_code(ResponseCode::ServFail);
                        response
                    } else {
                        let answer = answer_for_hostname(
                            EXAMPLE_HOSTNAME,
                            match requested_record_type {
                                RecordType::A => EXAMPLE_IPV4_ADDR,
                                RecordType::AAAA => EXAMPLE_IPV6_ADDR,
                                _ => panic!(
                                    "DNS resolver should only request A or AAAA records, \
                                 got {:?} with type {:?} instead",
                                    query, requested_record_type,
                                ),
                            },
                        );
                        let mut response = Message::new();
                        let _: &mut Message =
                            response.set_response_code(ResponseCode::NoError).add_answer(answer);
                        response
                    }
                })
            }
            .fuse()
        );

        futures::select! {
            () = lookup_fut => {},
            () = error_server_fut => panic!("error_server_fut should never complete"),
            () = fallback_fut => panic!("fallback_fut should never complete"),
        };
    }
}

async fn setup_dns_server(
    realm: &netemul::TestRealm<'_>,
    addr: std::net::SocketAddr,
) -> (fuchsia_async::net::UdpSocket, fuchsia_async::net::TcpListener) {
    let dns_servers = &[fnet_ext::SocketAddress(addr).into()];
    let lookup_admin =
        realm.connect_to_protocol::<net_name::LookupAdminMarker>().expect("connect to protocol");
    lookup_admin
        .set_dns_servers(dns_servers)
        .await
        .expect("call set DNS servers")
        .expect("set DNS servers");
    let servers = lookup_admin.get_dns_servers().await.expect("get DNS servers");
    assert_eq!(servers, dns_servers);

    let udp_socket = fuchsia_async::net::UdpSocket::bind_in_realm(&realm, addr)
        .await
        .expect("create UDP socket and bind in realm");
    let tcp_listener = fuchsia_async::net::TcpListener::listen_in_realm(&realm, addr)
        .await
        .expect("create TCP socket and bind in realm");
    (udp_socket, tcp_listener)
}

#[netstack_test]
#[variant(N, Netstack)]
async fn no_fallback_to_tcp_on_failed_udp<N: Netstack>(name: &str) {
    use trust_dns_proto::op::{Message, ResponseCode};

    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox
        .create_netstack_realm_with::<N, _, _>(
            name,
            &[KnownServiceProvider::DnsResolver, KnownServiceProvider::FakeClock],
        )
        .expect("create realm");
    // The DNS resolver contains a timeout after which any in-flight queries are dropped, which can
    // cause test flakes.
    pause_fake_clock(&realm).await.expect("failed to pause fake clock");

    let name_lookup =
        realm.connect_to_protocol::<net_name::LookupMarker>().expect("connect to protocol");
    let mut lookup_fut = pin!(
        async {
            let lookup_result = name_lookup
                .lookup_ip(
                    EXAMPLE_HOSTNAME,
                    &net_name::LookupIpOptions { ipv4_lookup: Some(true), ..Default::default() },
                )
                .await
                .expect("call lookup IP");
            // The DNS resolver should not retry UDP errors over TCP, so when the request over UDP
            // fails, the overall lookup should result in an error.
            assert_eq!(lookup_result, Err(net_name::LookupError::NotFound));
        }
        .fuse()
    );

    let (udp_socket, tcp_listener) =
        setup_dns_server(&realm, std_socket_addr!("127.0.0.1:1234")).await;
    // The name server responds to queries over UDP with a `SERVFAIL` response.
    let mut udp_fut = pin!(
        mock_udp_name_server(&udp_socket, |_: &Message| {
            let mut response = Message::new();
            let _: &mut Message = response.set_response_code(ResponseCode::ServFail);
            response
        })
        .fuse()
    );
    // The name server panics if it gets any connection requests over TCP.
    let mut tcp_fut = pin!(
        async {
            let mut incoming = tcp_listener.accept_stream();
            if let Some(result) = incoming.next().await {
                let (_stream, addr) = result.expect("accept incoming TCP connection");
                panic!("we expect no queries over TCP; got a connection request from {:?}", addr);
            }
        }
        .fuse()
    );

    futures::select! {
        () = lookup_fut => {},
        () = udp_fut => panic!("mock UDP name server future should never complete"),
        () = tcp_fut => panic!("mock TCP name server future should never complete"),
    };
}

#[netstack_test]
#[variant(N, Netstack)]
async fn fallback_to_tcp_on_truncated_response<N: Netstack>(name: &str) {
    use trust_dns_proto::op::{Message, ResponseCode};

    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox
        .create_netstack_realm_with::<N, _, _>(
            name,
            &[KnownServiceProvider::DnsResolver, KnownServiceProvider::FakeClock],
        )
        .expect("create realm");
    // The DNS resolver contains a timeout after which any in-flight queries are dropped, which can
    // cause test flakes.
    pause_fake_clock(&realm).await.expect("failed to pause fake clock");

    let name_lookup =
        realm.connect_to_protocol::<net_name::LookupMarker>().expect("connect to protocol");
    let mut lookup_fut = pin!(
        async {
            let ips = name_lookup
                .lookup_ip(
                    EXAMPLE_HOSTNAME,
                    &net_name::LookupIpOptions { ipv4_lookup: Some(true), ..Default::default() },
                )
                .await
                .expect("call lookup IP")
                .expect("lookup IP");
            assert_eq!(
                ips,
                net_name::LookupResult {
                    addresses: Some(vec![EXAMPLE_IPV4_ADDR]),
                    ..Default::default()
                }
            );
        }
        .fuse()
    );

    let (udp_socket, tcp_listener) =
        setup_dns_server(&realm, std_socket_addr!("127.0.0.1:1234")).await;
    // The name server responds to queries over UDP with a response with the `truncated` bit set,
    // indicating that the query should be retried over TCP.
    //
    // Also, reply with an incorrect resolved IP address here to ensure that the eventual lookup
    // result comes from the TCP name server.
    let mut udp_fut = pin!(
        mock_udp_name_server(&udp_socket, |_: &Message| {
            let answer = answer_for_hostname(EXAMPLE_HOSTNAME, fidl_ip!("2.2.2.2"));
            let mut response = Message::new();
            let _: &mut Message = response
                .set_response_code(ResponseCode::NoError)
                .add_answer(answer)
                .set_truncated(true);
            response
        })
        .fuse()
    );
    // The name server responds to queries over TCP with the full response.
    let mut tcp_fut = pin!(
        mock_tcp_name_server(tcp_listener, |_: &Message| {
            let answer = answer_for_hostname(EXAMPLE_HOSTNAME, EXAMPLE_IPV4_ADDR);
            let mut response = Message::new();
            let _: &mut Message =
                response.set_response_code(ResponseCode::NoError).add_answer(answer);
            response
        })
        .fuse()
    );

    futures::select! {
        () = lookup_fut => {},
        () = udp_fut => panic!("mock UDP name server future should never complete"),
        () = tcp_fut => panic!("mock TCP name server future should never complete"),
    };
}

#[netstack_test]
#[variant(N, Netstack)]
async fn query_preferred_name_servers_first<N: Netstack>(name: &str) {
    use trust_dns_proto::op::{Message, MessageType, OpCode, ResponseCode};

    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox
        .create_netstack_realm_with::<N, _, _>(
            name,
            &[KnownServiceProvider::DnsResolver, KnownServiceProvider::FakeClock],
        )
        .expect("create realm");
    // Pause the clock so that we can always trigger timeouts manually.
    let fake_clock = realm
        .connect_to_protocol::<ftesting::FakeClockControlMarker>()
        .expect("connect to protocol");
    fake_clock.pause().await.expect("pause fake clock");

    struct NameServer {
        addr: std::net::SocketAddr,
        socket: fuchsia_async::net::UdpSocket,
    }

    impl NameServer {
        async fn bind_in_realm(addr: std::net::SocketAddr, realm: &netemul::TestRealm<'_>) -> Self {
            let socket = fuchsia_async::net::UdpSocket::bind_in_realm(realm, addr)
                .await
                .expect("create UDP socket and bind in realm");
            Self { addr, socket }
        }

        async fn ignore_next_query(&self) {
            let mut buf = [0; MAX_DNS_UDP_MESSAGE_LEN];
            let (read, src_addr) =
                self.socket.recv_from(&mut buf).await.expect("receive DNS query");
            assert_eq!(src_addr.ip(), std::net::Ipv4Addr::LOCALHOST);
            let query = Message::from_vec(&buf[..read]).expect("deserialize DNS query");
            assert_eq!(query.message_type(), MessageType::Query);
        }

        async fn handle_next_query(
            &self,
            configure_response: impl Fn(&mut trust_dns_proto::op::Message),
        ) {
            let mut buf = [0; MAX_DNS_UDP_MESSAGE_LEN];
            let (read, src_addr) =
                self.socket.recv_from(&mut buf).await.expect("receive DNS query");
            let query = Message::from_vec(&buf[..read]).expect("deserialize DNS query");
            let mut response = Message::new();
            configure_response(&mut response);
            let _: &mut Message = response
                .set_message_type(MessageType::Response)
                .set_op_code(OpCode::Update)
                .set_id(query.id())
                .add_queries(query.queries().to_vec());
            let response = response.to_vec().expect("serialize DNS response");
            let written =
                self.socket.send_to(&response, src_addr).await.expect("send DNS response");
            assert_eq!(written, response.len());
        }
    }

    // NB: this should match the setting for `ResolverOpts::num_concurrent_reqs` set
    // in the DNS resolver, to ensure that the test is deterministic and doesn't
    // rely on external factors to avoid flakiness.
    const CONCURRENT_NAME_SERVER_QUERIES: usize = 10;
    const QUERY_TIMEOUT: zx::MonotonicDuration = zx::MonotonicDuration::from_minutes(1);

    let preferred_servers: Vec<_> =
        futures::future::join_all((0..CONCURRENT_NAME_SERVER_QUERIES).map(|i| {
            NameServer::bind_in_realm(
                std::net::SocketAddr::new(
                    std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                    (8000 + i).try_into().unwrap(),
                ),
                &realm,
            )
        }))
        .await;
    let secondary_server =
        NameServer::bind_in_realm(std_socket_addr!("127.0.0.1:1234"), &realm).await;

    // Configure name servers in priority order.
    {
        let dns_servers = preferred_servers
            .iter()
            .chain(std::iter::once(&secondary_server))
            .map(|name_server| fnet_ext::SocketAddress(name_server.addr).into())
            .collect::<Vec<_>>();
        let lookup_admin = realm
            .connect_to_protocol::<net_name::LookupAdminMarker>()
            .expect("connect to protocol");
        lookup_admin
            .set_dns_servers(&dns_servers)
            .await
            .expect("call set DNS servers")
            .expect("set DNS servers");
    }

    let name_servers_fut = async {
        // First, expect a query to each of the preferred servers. Ignore the queries to
        // cause Trust-DNS to note that queries to the preferred servers have failed.
        for server in &preferred_servers {
            server.ignore_next_query().await;
        }

        // Advance the clock in order to trigger the timeout on the first batch of
        // queries.
        fake_clock
            .advance(&ftesting::Increment::Determined(QUERY_TIMEOUT.into_nanos()))
            .await
            .expect("call advance")
            .expect("advance fake clock");

        // Then, respond to a query to the other name server with a successful response,
        // so Trust-DNS will note that a query to it has succeeded.
        secondary_server
            .handle_next_query(|response| {
                let _: &mut Message = response
                    .set_response_code(ResponseCode::NoError)
                    .add_answer(answer_for_hostname(EXAMPLE_HOSTNAME, EXAMPLE_IPV4_ADDR));
            })
            .await;

        // At this point, either Trust-DNS is (correctly) respecting its static
        // configuration of preferred servers, and on the next query will again query
        // the preferred servers first, or it is (incorrectly) performing sorting
        // internally based on query stats, which would cause it to query the secondary
        // server first, given it's responded successfully and the preferred servers
        // haven't.
        //
        // Expect that the preferred name servers are queried first.
        let mut preferred_queried = pin!(
            futures::future::select_all(preferred_servers.iter().map(|server| {
                server
                    .handle_next_query(|message| {
                        let _: &mut Message = message
                            .set_response_code(ResponseCode::NoError)
                            .add_answer(answer_for_hostname(EXAMPLE_HOSTNAME, EXAMPLE_IPV6_ADDR));
                    })
                    .boxed()
            }))
            .fuse()
        );
        let mut secondary_queried = pin!(secondary_server.ignore_next_query().fuse());
        futures::select! {
            ((), i, _remaining) = preferred_queried =>
                println!("observed a query to preferred name server {}, exiting", i),
            () = secondary_queried => panic!("a preferred name server was not queried first"),
        }
    };

    let name_lookup =
        realm.connect_to_protocol::<net_name::LookupMarker>().expect("connect to protocol");
    let lookup_fut = async {
        // First, send a query that we expect to be answered by the non-preferred
        // server. If Trust-DNS is (incorrectly) doing its own internal
        // reprioritization, then the follow-up query will fail, because that server
        // only responds once and then drops future queries.
        let lookup_result = name_lookup
            .lookup_ip(
                EXAMPLE_HOSTNAME,
                &net_name::LookupIpOptions { ipv4_lookup: Some(true), ..Default::default() },
            )
            .await
            .expect("call lookup IP");
        assert_eq!(
            lookup_result,
            Ok(net_name::LookupResult {
                addresses: Some(vec![EXAMPLE_IPV4_ADDR]),
                ..Default::default()
            })
        );

        // Then make another query (for AAAA records this time to distinguish it from
        // the first), to ensure that the resolver still queries the preferred servers
        // first, even though they failed to respond the first time.
        let lookup_result = name_lookup
            .lookup_ip(
                EXAMPLE_HOSTNAME,
                &net_name::LookupIpOptions { ipv6_lookup: Some(true), ..Default::default() },
            )
            .await
            .expect("call lookup IP");
        assert_eq!(
            lookup_result,
            Ok(net_name::LookupResult {
                addresses: Some(vec![EXAMPLE_IPV6_ADDR]),
                ..Default::default()
            })
        );
    };

    futures::join!(name_servers_fut, lookup_fut);
}

#[netstack_test]
#[variant(N, Netstack)]
async fn dns_name_server_stats_inspect<N: Netstack>(name: &str) {
    use trust_dns_proto::op::{Message, ResponseCode};

    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox
        .create_netstack_realm_with::<N, _, _>(
            name,
            &[KnownServiceProvider::DnsResolver, KnownServiceProvider::FakeClock],
        )
        .expect("create realm");

    let dns_server_addr: std::net::SocketAddr = std_socket_addr!("127.0.0.1:1234");
    let (udp_socket, _tcp_listener) = setup_dns_server(&realm, dns_server_addr).await;
    pause_fake_clock(&realm).await.expect("failed to pause fake clock");

    let name_lookup =
        realm.connect_to_protocol::<net_name::LookupMarker>().expect("connect to protocol");

    let mut lookup_udp_fut = pin!(
        async {
            let _ips = name_lookup
                .lookup_ip(
                    EXAMPLE_HOSTNAME,
                    &net_name::LookupIpOptions { ipv4_lookup: Some(true), ..Default::default() },
                )
                .await
                .expect("call lookup IP")
                .expect("lookup IP");
        }
        .fuse()
    );

    let mut udp_server_fut = pin!(
        mock_udp_name_server(&udp_socket, |_: &Message| {
            let answer = answer_for_hostname(EXAMPLE_HOSTNAME, EXAMPLE_IPV4_ADDR);
            let mut response = Message::new();
            let _: &mut Message =
                response.set_response_code(ResponseCode::NoError).add_answer(answer);
            response
        })
        .fuse()
    );

    futures::select! {
        () = lookup_udp_fut => {},
        () = udp_server_fut => panic!("mock UDP name server future should never complete"),
    };

    let data = netstack_testing_common::get_inspect_data(
        &realm,
        constants::dns_resolver::COMPONENT_NAME,
        "root/name_servers",
    )
    .await
    .expect("get inspect data");

    assert_data_tree!(data, root: {
        name_servers: {
            "0": {
                address: "127.0.0.1:1234",
                protocol: "Udp",
                successful_queries: 1u64,
                failed_queries: 0u64,
                success_streak: 1u64,
                errors: {
                    Timeout: 0u64,
                    Message: 0u64,
                    NoConnections: 0u64,
                    IoErrorCounts: {},
                    ProtoErrorCounts: {},
                    NoRecordsFoundResponseCodeCounts: {},
                    UnhandledResolveErrorKindCounts: {},
                }
            },
            "1": {
                address: "127.0.0.1:1234",
                protocol: "Tcp",
                successful_queries: 0u64,
                failed_queries: 0u64,
                success_streak: 0u64,
                errors: {
                    Timeout: 0u64,
                    Message: 0u64,
                    NoConnections: 0u64,
                    IoErrorCounts: {},
                    ProtoErrorCounts: {},
                    NoRecordsFoundResponseCodeCounts: {},
                    UnhandledResolveErrorKindCounts: {},
                }
            }
        }
    });
}

#[netstack_test]
#[variant(N, Netstack)]
async fn dns_name_server_stats_tcp_inspect<N: Netstack>(name: &str) {
    use trust_dns_proto::op::{Message, ResponseCode};

    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox
        .create_netstack_realm_with::<N, _, _>(
            name,
            &[KnownServiceProvider::DnsResolver, KnownServiceProvider::FakeClock],
        )
        .expect("create realm");
    pause_fake_clock(&realm).await.expect("failed to pause fake clock");

    let dns_server_addr: std::net::SocketAddr = std_socket_addr!("127.0.0.1:1234");
    let (udp_socket, tcp_listener) = setup_dns_server(&realm, dns_server_addr).await;

    let name_lookup =
        realm.connect_to_protocol::<net_name::LookupMarker>().expect("connect to protocol");

    let mut lookup_tcp_fut = pin!(
        async {
            let _ips = name_lookup
                .lookup_ip(
                    "tcp.example.com.",
                    &net_name::LookupIpOptions { ipv4_lookup: Some(true), ..Default::default() },
                )
                .await
                .expect("call lookup IP")
                .expect("lookup IP");
        }
        .fuse()
    );

    let mut udp_truncated_server_fut = pin!(
        mock_udp_name_server(&udp_socket, |_: &Message| {
            let mut response = Message::new();
            let _: &mut Message = response.set_truncated(true);
            response
        })
        .fuse()
    );

    let mut tcp_server_fut = pin!(
        mock_tcp_name_server(tcp_listener, |_: &Message| {
            let answer = answer_for_hostname("tcp.example.com.", EXAMPLE_IPV4_ADDR);
            let mut response = Message::new();
            let _: &mut Message =
                response.set_response_code(ResponseCode::NoError).add_answer(answer);
            response
        })
        .fuse()
    );

    futures::select! {
        () = lookup_tcp_fut => {},
        () = udp_truncated_server_fut => panic!("mock UDP server future should never complete"),
        () = tcp_server_fut => panic!("mock TCP server future should never complete"),
    };

    // Verify Inspect
    let data = netstack_testing_common::get_inspect_data(
        &realm,
        constants::dns_resolver::COMPONENT_NAME,
        "root/name_servers",
    )
    .await
    .expect("get inspect data");

    assert_data_tree!(data, root: {
        name_servers: {
            "0": {
                address: "127.0.0.1:1234",
                protocol: "Udp",
                successful_queries: 1u64,
                failed_queries: 0u64,
                success_streak: 1u64,
                errors: {
                    Timeout: 0u64,
                    Message: 0u64,
                    NoConnections: 0u64,
                    IoErrorCounts: {},
                    ProtoErrorCounts: {},
                    NoRecordsFoundResponseCodeCounts: {},
                    UnhandledResolveErrorKindCounts: {},
                }
            },
            "1": {
                address: "127.0.0.1:1234",
                protocol: "Tcp",
                successful_queries: 1u64,
                failed_queries: 0u64,
                success_streak: 1u64,
                errors: {
                    Timeout: 0u64,
                    Message: 0u64,
                    NoConnections: 0u64,
                    IoErrorCounts: {},
                    ProtoErrorCounts: {},
                    NoRecordsFoundResponseCodeCounts: {},
                    UnhandledResolveErrorKindCounts: {},
                }
            }
        }
    });
}

#[netstack_test]
#[variant(N, Netstack)]
async fn dns_name_server_stats_failure_inspect<N: Netstack>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox
        .create_netstack_realm_with::<N, _, _>(
            name,
            &[KnownServiceProvider::DnsResolver, KnownServiceProvider::FakeClock],
        )
        .expect("create realm");

    let dns_server_addr: std::net::SocketAddr = std_socket_addr!("127.0.0.1:1234");
    let (udp_socket, _tcp_listener) = setup_dns_server(&realm, dns_server_addr).await;

    let name_lookup =
        realm.connect_to_protocol::<net_name::LookupMarker>().expect("connect to protocol");

    // A mock server that returns SERVFAIL.
    let mut lookup_fut = pin!(
        async {
            let result = name_lookup
                .lookup_ip(
                    EXAMPLE_HOSTNAME,
                    &net_name::LookupIpOptions { ipv4_lookup: Some(true), ..Default::default() },
                )
                .await
                .expect("call lookup IP");
            assert_eq!(result, Err(net_name::LookupError::NotFound));
        }
        .fuse()
    );

    let mut udp_server_fut = pin!(
        mock_udp_name_server(&udp_socket, |_: &trust_dns_proto::op::Message| {
            let mut response = trust_dns_proto::op::Message::new();
            let _: &mut trust_dns_proto::op::Message =
                response.set_response_code(trust_dns_proto::op::ResponseCode::ServFail);
            response
        })
        .fuse()
    );

    futures::select! {
        () = lookup_fut => {},
        () = udp_server_fut => panic!("mock UDP server future should never complete"),
    };

    let data = netstack_testing_common::get_inspect_data(
        &realm,
        constants::dns_resolver::COMPONENT_NAME,
        "root/name_servers",
    )
    .await
    .expect("get inspect data");

    assert_data_tree!(data, root: {
        name_servers: {
            "0": {
                address: "127.0.0.1:1234",
                protocol: "Udp",
                successful_queries: 0u64,
                failed_queries: 1u64,
                success_streak: 0u64,
                errors: {
                    Timeout: 0u64,
                    Message: 0u64,
                    NoConnections: 0u64,
                    IoErrorCounts: {},
                    ProtoErrorCounts: {},
                    NoRecordsFoundResponseCodeCounts: {
                        ServFail: {
                            count: 1u64,
                        }
                    },
                    UnhandledResolveErrorKindCounts: {},
                }
            },
            "1": {
                address: "127.0.0.1:1234",
                protocol: "Tcp",
                successful_queries: 0u64,
                failed_queries: 0u64,
                success_streak: 0u64,
                errors: {
                    Timeout: 0u64,
                    Message: 0u64,
                    NoConnections: 0u64,
                    IoErrorCounts: {},
                    ProtoErrorCounts: {},
                    NoRecordsFoundResponseCodeCounts: {},
                    UnhandledResolveErrorKindCounts: {},
                }
            }
        }
    });
}
