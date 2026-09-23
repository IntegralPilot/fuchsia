// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]
#![deny(clippy::await_holding_refcell_ref)]

use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_interfaces_admin as fnet_interfaces_admin;
use fidl_fuchsia_net_neighbor as fnet_neighbor;
use fidl_fuchsia_net_reachability as fnet_reachability;
use fidl_fuchsia_testing as ftesting;
use fuchsia_async as fasync;

use assert_matches::assert_matches;
use fidl::endpoints::Proxy;
use futures::channel::mpsc;
use futures::future::FusedFuture;
use futures::{FutureExt as _, StreamExt as _, TryFutureExt as _};
use log::info;
use net_declare::{net_ip_v4, net_ip_v6, net_mac};
use netstack_testing_common::constants::{ipv4 as ipv4_consts, ipv6 as ipv6_consts};
use netstack_testing_common::realms::{
    KnownServiceProvider, Netstack, TestSandboxExt as _, constants,
};
use netstack_testing_common::{get_inspect_data, interfaces, wait_for_component_stopped};
use netstack_testing_macros::netstack_test;
use packet::{
    Buf, InnerPacketBuilder as _, NestableSerializer as _, NoOpSerializationContext,
    Serializer as _,
};
use packet_formats::ethernet::{
    ETHERNET_MIN_BODY_LEN_NO_TAG, EtherType, EthernetFrameBuilder, EthernetFrameLengthCheck,
};
use packet_formats::icmp::{IcmpEchoRequest, IcmpPacketBuilder, IcmpZeroCode, MessageBody as _};
use packet_formats::ip::{Ipv4Proto, Ipv6Proto};
use packet_formats::ipv4::Ipv4PacketBuilder;
use packet_formats::ipv6::Ipv6PacketBuilder;
use packet_formats::testutil::parse_icmp_packet_in_ip_packet_in_ethernet_frame;
use reachability_core::{FIDL_TIMEOUT_ID, LinkState, State};
use std::cell::RefCell;
use std::collections::HashMap;
use std::pin::pin;
use std::rc::Rc;
use test_case::test_case;

const INTERNET_V4: net_types::ip::Ipv4Addr = net_ip_v4!("8.8.8.8");
const INTERNET_V6: net_types::ip::Ipv6Addr = net_ip_v6!("2001:4860:4860::8888");

const PREFIX_V4: u8 = 24;
const PREFIX_V6: u8 = 64;

const LOWER_METRIC: u32 = 0;
const HIGHER_METRIC: u32 = 100;

#[derive(Debug, Clone)]
struct InterfaceConfig {
    name_suffix: &'static str,
    gateway_v4: net_types::ip::Ipv4Addr,
    addr_v4: net_types::ip::Ipv4Addr,
    gateway_v6: net_types::ip::Ipv6Addr,
    addr_v6: net_types::ip::Ipv6Addr,
    gateway_mac: net_types::ethernet::Mac,
    metric: u32,
}

impl InterfaceConfig {
    const fn new_primary(metric: u32) -> Self {
        Self {
            name_suffix: "primary",
            gateway_v4: net_ip_v4!("192.168.0.1"),
            addr_v4: net_ip_v4!("192.168.0.2"),
            gateway_v6: net_ip_v6!("fe80::1"),
            addr_v6: net_ip_v6!("fe80::2"),
            gateway_mac: net_mac!("02:00:00:00:00:01"),
            metric,
        }
    }

    const fn new_secondary(metric: u32) -> Self {
        Self {
            name_suffix: "secondary",
            gateway_v4: net_ip_v4!("192.168.1.1"),
            addr_v4: net_ip_v4!("192.168.1.2"),
            gateway_v6: net_ip_v6!("fe81::1"),
            addr_v6: net_ip_v6!("fe81::2"),
            gateway_mac: net_mac!("04:00:00:00:00:01"),
            metric,
        }
    }
}

/// Try to parse `frame` as an ICMP or ICMPv6 Echo Request message, and if successful returns the
/// Echo Reply message that the netstack would expect as a reply.
#[track_caller]
fn reply_if_echo_request(
    frame: Vec<u8>,
    want: State,
    gateway_v4: &net_types::ip::Ipv4Addr,
    gateway_v6: &net_types::ip::Ipv6Addr,
) -> Option<Buf<Vec<u8>>> {
    let mut icmp_body = Vec::new();
    let r = parse_icmp_packet_in_ip_packet_in_ethernet_frame::<
        net_types::ip::Ipv4,
        _,
        IcmpEchoRequest,
        _,
    >(&frame, EthernetFrameLengthCheck::NoCheck, |p| {
        let (inner_header, inner_body) = p.body().bytes();
        assert!(inner_body.is_none());
        icmp_body.extend(inner_header);
    });
    match r {
        Ok((src_mac, dst_mac, src_ip, dst_ip, _ttl, message, _code)) => {
            return match want.link {
                LinkState::Gateway => dst_ip == *gateway_v4,
                LinkState::Internet => dst_ip == *gateway_v4 || dst_ip == INTERNET_V4,
                _ => false,
            }
            .then(|| {
                icmp_body
                    .into_serializer()
                    .wrap_in(IcmpPacketBuilder::<net_types::ip::Ipv4, _>::new(
                        dst_ip,
                        src_ip,
                        IcmpZeroCode,
                        message.reply(),
                    ))
                    .wrap_in(Ipv4PacketBuilder::new(
                        dst_ip,
                        src_ip,
                        ipv4_consts::DEFAULT_TTL,
                        Ipv4Proto::Icmp,
                    ))
                    .wrap_in(EthernetFrameBuilder::new(
                        dst_mac,
                        src_mac,
                        EtherType::Ipv4,
                        ETHERNET_MIN_BODY_LEN_NO_TAG,
                    ))
                    .serialize_vec_outer(&mut NoOpSerializationContext)
                    .expect("failed to serialize ICMPv4 packet")
                    .unwrap_b()
            });
        }
        Err(packet_formats::error::ParseError::NotExpected) => {}
        Err(e) => {
            panic!("parse packet as ICMPv4 error: {}\n{:02x?}", e, frame);
        }
    }

    let mut icmp_body = Vec::new();
    let r = parse_icmp_packet_in_ip_packet_in_ethernet_frame::<
        net_types::ip::Ipv6,
        _,
        IcmpEchoRequest,
        _,
    >(&frame, EthernetFrameLengthCheck::NoCheck, |p| {
        let (inner_header, inner_body) = p.body().bytes();
        assert!(inner_body.is_none());
        icmp_body.extend(inner_header);
    });
    match r {
        Ok((src_mac, dst_mac, src_ip, dst_ip, _ttl, message, _code)) => {
            return match want.link {
                LinkState::Gateway => dst_ip == *gateway_v6,
                LinkState::Internet => dst_ip == *gateway_v6 || dst_ip == INTERNET_V6,
                _ => false,
            }
            .then(|| {
                icmp_body
                    .into_serializer()
                    .wrap_in(IcmpPacketBuilder::<net_types::ip::Ipv6, _>::new(
                        dst_ip,
                        src_ip,
                        IcmpZeroCode,
                        message.reply(),
                    ))
                    .wrap_in(Ipv6PacketBuilder::new(
                        dst_ip,
                        src_ip,
                        ipv6_consts::DEFAULT_HOP_LIMIT,
                        Ipv6Proto::Icmpv6,
                    ))
                    .wrap_in(EthernetFrameBuilder::new(
                        dst_mac,
                        src_mac,
                        EtherType::Ipv6,
                        ETHERNET_MIN_BODY_LEN_NO_TAG,
                    ))
                    .serialize_vec_outer(&mut NoOpSerializationContext)
                    .expect("failed to serialize ICMPv6 packet")
                    .unwrap_b()
            });
        }
        Err(packet_formats::error::Ipv6ParseError::Parse {
            error: packet_formats::error::ParseError::NotExpected,
        }) => {}
        Err(e) => {
            panic!("parse packet as ICMPv6 error: {}\n{:02x?}", e, frame);
        }
    }
    None
}

#[derive(Copy, Clone, Default, Debug, PartialEq, Eq)]
struct IpStates {
    ipv4: State,
    ipv6: State,
}

impl IpStates {
    fn new(state: State) -> Self {
        Self { ipv4: state, ipv6: state }
    }
}

/// Extract the most recent reachability states for IPv4 and IPv6 from the inspect data.
fn extract_reachability_states(
    data: &diagnostics_hierarchy::DiagnosticsHierarchy,
) -> (HashMap<u64, IpStates>, IpStates) {
    let get_per_ip_state = |states: &diagnostics_hierarchy::DiagnosticsHierarchy| {
        let (_, state): (i64, _) = states.children.iter().fold(
            (-1, State::from(LinkState::None)),
            |(latest_seqnum, latest_state), state| {
                let seqnum = state
                    .name
                    .parse::<i64>()
                    .expect("failed to parse reachability state sequence number as integer");
                // NB: Since node creation is not atomic, it's possible to read a node with a
                // sequence number but no state value, so must ensure that state is `Some` before
                // using it as the latest state.
                match state.properties.iter().find_map(|p| {
                    if p.key() == "state" {
                        p.string().map(|s| {
                            s.parse::<LinkState>()
                                .unwrap_or_else(|e| {
                                    panic!("failed to parse reachability state: {}: {:?}", s, e)
                                })
                                .into()
                        })
                    } else {
                        None
                    }
                }) {
                    Some(state) if seqnum > latest_seqnum => (seqnum, state),
                    Some(_) | None => (latest_seqnum, latest_state),
                }
            },
        );
        state
    };
    let get_state = |data: &diagnostics_hierarchy::DiagnosticsHierarchy| {
        data.children.iter().fold(IpStates::default(), |IpStates { ipv4, ipv6 }, info| {
            if info.name == "IPv4" {
                IpStates { ipv4: get_per_ip_state(info), ipv6 }
            } else if info.name == "IPv6" {
                IpStates { ipv4, ipv6: get_per_ip_state(info) }
            } else {
                IpStates { ipv4, ipv6 }
            }
        })
    };
    data.children.iter().fold(
        (HashMap::new(), IpStates::default()),
        |(mut interfaces, mut system), data| {
            if data.name == "system" {
                system = get_state(data);
            } else {
                match data.name.parse::<u64>() {
                    Ok(id) => match interfaces.insert(id, get_state(data)) {
                        Some(state) => {
                            panic!(
                                "duplicate interface found in inspect data; id: {} state: {:?}",
                                id, state
                            );
                        }
                        None => {}
                    },
                    Err(_) => {}
                }
            }
            (interfaces, system)
        },
    )
}

struct NetemulInterface<'a> {
    _network: netemul::TestNetwork<'a>,
    interface: netemul::TestInterface<'a>,
    fake_ep: netemul::TestFakeEndpoint<'a>,
    state: Rc<RefCell<State>>,
}

impl<'a> NetemulInterface<'a> {
    fn id(&self) -> u64 {
        self.interface.id()
    }
}

async fn configure_interface<'a>(
    iface: &'a netemul::TestInterface<'a>,
    controller: &fnet_neighbor::ControllerProxy,
    InterfaceConfig {
        name_suffix: _,
        gateway_v4,
        addr_v4,
        gateway_v6,
        addr_v6,
        gateway_mac,
        metric,
    }: InterfaceConfig,
) {
    // Add addresses.
    futures::stream::iter([
        fnet::Subnet {
            addr: fnet::IpAddress::Ipv4(fnet::Ipv4Address { addr: addr_v4.ipv4_bytes() }),
            prefix_len: PREFIX_V4,
        },
        fnet::Subnet {
            addr: fnet::IpAddress::Ipv6(fnet::Ipv6Address { addr: addr_v6.ipv6_bytes() }),
            prefix_len: PREFIX_V6,
        },
    ])
    .for_each_concurrent(None, |subnet| async move {
        let address_state_provider = interfaces::add_address_wait_assigned(
            iface.control(),
            subnet,
            fnet_interfaces_admin::AddressParameters {
                add_subnet_route: Some(true),
                ..Default::default()
            },
        )
        .await
        .expect("add subnet address and route");
        let () = address_state_provider.detach().expect("address detach");
    })
    .await;

    let device_id = iface.id();

    // Add neighbor table entries for the gateway addresses.
    let gateway_v4 = fnet::IpAddress::Ipv4(fnet::Ipv4Address { addr: gateway_v4.ipv4_bytes() });
    let gateway_v6 = fnet::IpAddress::Ipv6(fnet::Ipv6Address { addr: gateway_v6.ipv6_bytes() });
    let gateway_mac = fnet::MacAddress { octets: gateway_mac.bytes() };
    let () = controller
        .add_entry(device_id, &gateway_v6, &gateway_mac)
        .await
        .expect("neighbor add_entry FIDL error")
        .expect("add IPv6 gateway neighbor table entry failed");
    let () = controller
        .add_entry(device_id, &gateway_v4, &gateway_mac)
        .await
        .expect("neighbor add_entry FIDL error")
        .expect("add IPv4 gateway neighbor table entry failed");

    // Add default routes.
    iface
        .add_default_route_with_explicit_metric(gateway_v4, metric)
        .await
        .expect("add IPv4 default route");
    iface
        .add_default_route_with_explicit_metric(gateway_v6, metric)
        .await
        .expect("add IPv6 default route");
}

async fn handle_frame_stream<'a>(
    fake_ep: &'a netemul::TestFakeEndpoint<'_>,
    gateway_v4: net_types::ip::Ipv4Addr,
    gateway_v6: net_types::ip::Ipv6Addr,
    state: Rc<RefCell<State>>,
    echo_notifier: &mpsc::UnboundedSender<()>,
) {
    let mut frame_stream = fake_ep.frame_stream();
    loop {
        if let Some(Ok((frame, dropped))) = frame_stream.next().await {
            echo_notifier.unbounded_send(()).expect("failed to send echo notice");
            assert_eq!(dropped, 0);
            let reply = {
                let state_ref = state.borrow();
                reply_if_echo_request(frame, *state_ref, &gateway_v4, &gateway_v6)
            };
            if let Some(reply) = reply {
                fake_ep
                    .write(reply.as_ref())
                    .await
                    .expect("failed to write echo reply to fake endpoint");
            }
        }
    }
}

struct ReachabilityEnv<'a> {
    sandbox: &'a netemul::TestSandbox,
    realm: netemul::TestRealm<'a>,
    controller: fnet_neighbor::ControllerProxy,
    fake_clock: ftesting::FakeClockControlProxy,
}

async fn setup_reachability_env<'a, N: Netstack>(
    name: &'a str,
    sandbox: &'a netemul::TestSandbox,
    start_reachability_as_eager: bool,
) -> ReachabilityEnv<'a> {
    let realm = sandbox
        .create_netstack_realm_with::<N, _, _>(
            name,
            &[
                KnownServiceProvider::Reachability { eager: start_reachability_as_eager },
                KnownServiceProvider::FakeClock,
                KnownServiceProvider::DnsResolver,
            ],
        )
        .expect("failed to create realm");
    let controller = realm
        .connect_to_protocol::<fnet_neighbor::ControllerMarker>()
        .expect("failed to connect to Controller");
    let fake_clock = realm
        .connect_to_protocol::<ftesting::FakeClockControlMarker>()
        .expect("failed to connect to FakeClockControl");
    let () = initialize_fake_clock(&fake_clock).await;

    ReachabilityEnv { sandbox, realm, controller, fake_clock }
}

async fn create_netemul_interfaces<'a>(
    name: &'a str,
    sandbox: &'a netemul::TestSandbox,
    env: &'a ReachabilityEnv<'_>,
    configs: &[(InterfaceConfig, State)],
) -> Vec<NetemulInterface<'a>> {
    futures::stream::iter(configs.iter())
        .then(|(config, init_state): &(InterfaceConfig, State)| {
            let config = config.clone();
            async {
                let name = format!("{}_{}", name, config.name_suffix);
                let network =
                    sandbox.create_network(name.clone()).await.expect("failed to create network");
                let fake_ep =
                    network.create_fake_endpoint().expect("failed to create fake endpoint");
                let interface = env
                    .realm
                    .join_network(&network, name)
                    .await
                    .expect("failed to join network with netdevice endpoint");
                let state = Rc::new(RefCell::new(*init_state));

                configure_interface(&interface, &env.controller, config).await;
                NetemulInterface { _network: network, interface, fake_ep, state }
            }
        })
        .collect::<_>()
        .await
}

async fn echo_reply_streams(
    interfaces: Vec<NetemulInterface<'_>>,
    configs: Vec<InterfaceConfig>,
    echo_notifier: mpsc::UnboundedSender<()>,
) {
    // TODO(https://fxbug.dev/42071036): Combine configs with the NetemulInterface struct
    let stream = interfaces
        .iter()
        .zip(configs.iter())
        .map(|(NetemulInterface { _network, interface: _, fake_ep, state }, config)| {
            Box::pin(handle_frame_stream(
                &fake_ep,
                config.gateway_v4,
                config.gateway_v6,
                state.clone(),
                &echo_notifier,
            ))
        })
        .collect::<futures::stream::FuturesUnordered<_>>();
    let _: Vec<()> = stream.collect::<Vec<_>>().await;
}

async fn initialize_fake_clock(fake_clock: &ftesting::FakeClockControlProxy) -> () {
    // Pause the fake clock and ignore the FIDL timeout named deadline.
    fake_clock.pause().await.expect("failed to pause the clock");
    fake_clock
        .ignore_named_deadline(&FIDL_TIMEOUT_ID.into())
        .await
        .expect("ignore named deadline failed");

    // Resume clock at normal increments.
    fake_clock
        .resume_with_increments(
            zx::MonotonicDuration::from_millis(1).into_nanos(),
            &ftesting::Increment::Determined(zx::MonotonicDuration::from_millis(1).into_nanos()),
        )
        .await
        .expect("resume with increment failed")
        .expect("resume with increment returned error");
    info!("deadline ignored and time resumed");
}

async fn accelerate_fake_clock(fake_clock: &ftesting::FakeClockControlProxy) -> () {
    // Pause the fake clock.
    fake_clock.pause().await.expect("failed to pause the clock");

    // Fast increment speed after pausing. Acceleration past 5x doesn't reduce test runtime,
    // because it is dominated by setup/teardown.
    fake_clock
        .resume_with_increments(
            zx::MonotonicDuration::from_millis(1).into_nanos(),
            &ftesting::Increment::Determined(zx::MonotonicDuration::from_millis(5).into_nanos()),
        )
        .await
        .expect("resume with increment failed")
        .expect("resume with increment returned error");
    info!("time accelerated");
}

#[netstack_test]
#[variant(N, Netstack)]
#[test_case(
    "gateway",
    &[(InterfaceConfig::new_primary(LOWER_METRIC), LinkState::Gateway.into())];
    "gateway")]
#[test_case(
    "internet",
    &[(InterfaceConfig::new_primary(LOWER_METRIC), LinkState::Internet.into())];
    "internet")]
#[test_case(
    "gateway_gateway",
    &[
        (InterfaceConfig::new_primary(LOWER_METRIC), LinkState::Gateway.into()),
        (InterfaceConfig::new_secondary(HIGHER_METRIC), LinkState::Gateway.into()),
    ];
    "gateway_gateway")]
#[test_case(
    "gateway_internet",
    &[
        (InterfaceConfig::new_primary(LOWER_METRIC), LinkState::Gateway.into()),
        (InterfaceConfig::new_secondary(HIGHER_METRIC), LinkState::Internet.into()),
    ];
    "gateway_internet")]
// This test case guards against the regression where if ICMP echo requests are routed according
// to the default route rather than explicitly through each interface, packets destined for the
// internet intended to be sent out the secondary interface will actually be sent out the primary
// interface due to the lower metric, resulting in the reachability state of the secondary
// interface to be Internet rather than Gateway, causing the test to fail.
#[test_case(
    "internet_gateway",
    &[
        (InterfaceConfig::new_primary(LOWER_METRIC), LinkState::Internet.into()),
        (InterfaceConfig::new_secondary(HIGHER_METRIC), LinkState::Gateway.into()),
    ];
    "internet_gateway")]
#[test_case(
    "internet_internet",
    &[
        (InterfaceConfig::new_primary(LOWER_METRIC), LinkState::Internet.into()),
        (InterfaceConfig::new_secondary(HIGHER_METRIC), LinkState::Internet.into()),
    ];
    "internet_internet")]
async fn test_state<N: Netstack>(
    name: &str,
    sub_test_name: &str,
    configs: &[(InterfaceConfig, State)],
) {
    let name = format!("{}_{}", name, sub_test_name);
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let env = setup_reachability_env::<N>(&name, &sandbox, false).await;

    // Initialize a connection to the Monitor marker to start the reachability component.
    let _monitor = env
        .realm
        .connect_to_protocol::<fnet_reachability::MonitorMarker>()
        .expect("failed to connect to fuchsia.net.reachability.Monitor");

    let interfaces = create_netemul_interfaces(&name, &env.sandbox, &env, &configs).await;
    let interfaces_count = interfaces.len();
    let want_interfaces = interfaces
        .iter()
        .zip(configs.iter())
        .map(|(interface, (_, want_state)): (&NetemulInterface<'_>, &(InterfaceConfig, State))| {
            (interface.id(), IpStates::new(*want_state))
        })
        .collect::<HashMap<_, _>>();

    let interface_configs =
        configs.iter().cloned().map(|(c, _)| c).collect::<Vec<InterfaceConfig>>();
    let (sender, receiver) = mpsc::unbounded();
    let echo_receiver = receiver.fuse();
    let mut echo_receiver = pin!(echo_receiver);
    let echo_reply_streams = echo_reply_streams(interfaces, interface_configs, sender).fuse();
    let mut echo_reply_streams = pin!(echo_reply_streams);
    let () = accelerate_fake_clock(&env.fake_clock).await;

    // TODO(https://fxbug.dev/42144302): Get reachability monitor's reachability state over FIDL rather
    // than the inspect data. Watching for updates to inspect data is currently not supported, so
    // poll instead.
    const INSPECT_COMPONENT: &str = "reachability";
    const INSPECT_TREE_SELECTOR: &str = "root";
    let realm_ref = &env.realm;
    let inspect_data_stream = futures::stream::unfold(None, |duration| async move {
        let () = match duration {
            None => {}
            Some(duration) => fasync::Timer::new(duration).await,
        };
        let duration = std::time::Duration::from_millis(100);
        let yielded = loop {
            match get_inspect_data(realm_ref, INSPECT_COMPONENT, INSPECT_TREE_SELECTOR)
                .map_ok(|data| extract_reachability_states(&data))
                .await
            {
                Ok(yielded) => break yielded,
                // Archivist can sometimes return transient `InconsistentSnapshot` errors. Our
                // wrapper function discards type information so we can't match on the specific
                // error.
                Err(err @ anyhow::Error { .. }) => println!("inspect data stream error: {:?}", err),
            }
            let () = fasync::Timer::new(duration).await;
        };
        Some((yielded, Some(duration)))
    })
    .fuse();
    let mut inspect_data_stream = pin!(inspect_data_stream);

    // Ensure that at least one echo request has been replied to before polling the inspect data
    // stream to guarantee that reachability monitor has initialized its inspect data tree.
    futures::select! {
        _ = echo_reply_streams => panic!("echo reply streams ended unexpectedly"),
        _ = echo_receiver.next() => (),
    }

    let IpStates { ipv4: want_system_ipv4, ipv6: want_system_ipv6 } =
        want_interfaces.values().fold(
            IpStates::new(LinkState::None.into()),
            |IpStates { ipv4: best_ipv4, ipv6: best_ipv6 }, &IpStates { ipv4, ipv6 }| IpStates {
                ipv4: if ipv4 > best_ipv4 { ipv4 } else { best_ipv4 },
                ipv6: if ipv6 > best_ipv6 { ipv6 } else { best_ipv6 },
            },
        );

    let reachability_monitor_wait_fut =
        wait_for_component_stopped(&env.realm, constants::reachability::COMPONENT_NAME, None)
            .fuse();
    let mut reachability_monitor_wait_fut = pin!(reachability_monitor_wait_fut);

    loop {
        futures::select! {
            _ = echo_reply_streams => {
                panic!("interface echo reply stream ended unexpectedly");
            }
            r = inspect_data_stream.next() => {
                let (got_interfaces, IpStates { ipv4: got_system_ipv4, ipv6: got_system_ipv6 }) = r
                    .expect("inspect data stream ended unexpectedly");
                if got_system_ipv4 > want_system_ipv4 {
                    panic!(
                        "system IPv4 state exceeded; got: {:?}, want: {:?}",
                        got_system_ipv4, want_system_ipv4,
                    )
                }
                if got_system_ipv6 > want_system_ipv6 {
                    panic!(
                        "system IPv6 state exceeded; got: {:?}, want: {:?}",
                        got_system_ipv6, want_system_ipv6,
                    )
                }
                let equal_count = got_interfaces
                    .iter()
                    .filter_map(|(id, got)| {
                        let IpStates { ipv4: want_ipv4, ipv6: want_ipv6 } =
                            want_interfaces.get(&id).unwrap_or_else(|| {
                                panic!(
                                    "unknown interface {} with state {:?} found in inspect data",
                                    id, got,
                                )
                            });
                        let IpStates { ipv4: got_ipv4, ipv6: got_ipv6 } = got;
                        if got_ipv4 > want_ipv4 {
                            panic!(
                                "interface {} IPv4 state exceeded; got: {:?}, want: {:?}",
                                id, got_ipv4, want_ipv4,
                            )
                        }
                        if got_ipv6 > want_ipv6 {
                            panic!(
                                "interface {} IPv6 state exceeded; got: {:?}, want: {:?}",
                                id, got_ipv6, want_ipv6,
                            )
                        }
                        (got_ipv4 == want_ipv4 && got_ipv6 == want_ipv6).then(|| ())
                    })
                    .count();
                if got_system_ipv4 == want_system_ipv4
                    && got_system_ipv6 == want_system_ipv6
                    && equal_count == interfaces_count
                {
                    break;
                }
            }
            event = reachability_monitor_wait_fut => {
                panic!("reachability monitor terminated unexpectedly with event: {:?}", event);
            }
        }
    }
}

struct ReachabilityTestHelper<'a> {
    env: &'a ReachabilityEnv<'a>,
    monitor: fnet_reachability::MonitorProxy,
    iface_states: Vec<Rc<RefCell<State>>>,
    echo_reply_streams: std::pin::Pin<Box<dyn FusedFuture<Output = ()> + 'a>>,
    _echo_reply_receiver: mpsc::UnboundedReceiver<()>,
}

impl<'a> ReachabilityTestHelper<'a> {
    async fn new(
        name: &'a str,
        env: &'a ReachabilityEnv<'a>,
        configs: Vec<(InterfaceConfig, State)>,
    ) -> ReachabilityTestHelper<'a> {
        let monitor = env
            .realm
            .connect_to_protocol::<fnet_reachability::MonitorMarker>()
            .expect("failed to connect to fuchsia.net.reachability.Monitor");

        let interface_configs =
            configs.iter().cloned().map(|(c, _)| c).collect::<Vec<InterfaceConfig>>();
        let interfaces = create_netemul_interfaces(name, &env.sandbox, &env, &configs).await;

        let iface_states = interfaces.iter().map(|iface| iface.state.clone()).collect();

        let (sender, receiver) = mpsc::unbounded();

        let echo_reply_streams =
            Box::pin(echo_reply_streams(interfaces, interface_configs, sender).fuse());

        let () = accelerate_fake_clock(&env.fake_clock).await;

        Self { env, monitor, iface_states, echo_reply_streams, _echo_reply_receiver: receiver }
    }

    async fn next_snapshot(&mut self) -> fnet_reachability::Snapshot {
        let reachability_monitor_wait_fut = wait_for_component_stopped(
            &self.env.realm,
            constants::reachability::COMPONENT_NAME,
            None,
        )
        .fuse();
        let mut reachability_monitor_wait_fut = pin!(reachability_monitor_wait_fut);

        let snapshot_fut = self.monitor.watch();
        let mut snapshot_fut = pin!(snapshot_fut);

        futures::select! {
            _ = self.echo_reply_streams.as_mut() => {
                panic!("interface echo reply stream ended unexpectedly");
            }
            r = snapshot_fut => {
                match r {
                   Ok(snapshot) => {
                        return snapshot;
                   },
                   Err(e) => panic!("failed to fetch updated snapshot {}", e),
                }
            }
            event = reachability_monitor_wait_fut => {
                panic!("reachability monitor terminated unexpectedly with event: {:?}", event);
            }
        }
    }

    // Due to the DNS lookup probe running independently from the network check probes,
    // intermediary states may be inconsistently present depending on the test environment. The
    // integration tests specify the network configuration under test, and these helper functions
    // assume that the condition is eventually met.
    async fn next_snapshot_with_internet(
        &mut self,
        is_available: bool,
    ) -> fnet_reachability::Snapshot {
        loop {
            let snapshot = self.next_snapshot().await;
            if snapshot.internet_available == Some(is_available) {
                break snapshot;
            }
        }
    }

    async fn next_snapshot_with_gateway(
        &mut self,
        is_reachable: bool,
    ) -> fnet_reachability::Snapshot {
        loop {
            let snapshot = self.next_snapshot().await;
            if snapshot.gateway_reachable == Some(is_reachable) {
                break snapshot;
            }
        }
    }

    fn set_iface_states(&mut self, states: Vec<State>) {
        assert!(states.len() == self.iface_states.len());
        self.iface_states
            .iter()
            .zip(states)
            .for_each(|(iface_state, new_state)| *iface_state.borrow_mut() = new_state);
    }
}

#[netstack_test]
#[variant(N, Netstack)]
#[test_case(LinkState::Internet.into(), LinkState::Internet.into())]
#[test_case(LinkState::Internet.into(), LinkState::Gateway.into())]
#[test_case(LinkState::Internet.into(), LinkState::Up.into())]
async fn test_internet_available<N: Netstack>(name: &str, state1: State, state2: State) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let env = setup_reachability_env::<N>(name, &sandbox, false).await;
    let configs = vec![
        (InterfaceConfig::new_primary(LOWER_METRIC), state1),
        (InterfaceConfig::new_secondary(HIGHER_METRIC), state2),
    ];
    let mut helper = ReachabilityTestHelper::new(name, &env, configs).await;

    let snapshot = helper.next_snapshot_with_internet(true).await;
    // Gateway reachability is a condition of internet availability,
    // therefore, when internet is available, a gateway is reachable.
    assert_eq!(snapshot.gateway_reachable, Some(true));
    assert_eq!(snapshot.internet_available, Some(true));
}

#[netstack_test]
#[variant(N, Netstack)]
#[test_case(LinkState::Internet.into(), LinkState::Internet.into())]
#[test_case(LinkState::Internet.into(), LinkState::Gateway.into())]
#[test_case(LinkState::Internet.into(), LinkState::Up.into())]
async fn test_internet_comes_up<N: Netstack>(name: &str, state1: State, state2: State) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let env = setup_reachability_env::<N>(name, &sandbox, false).await;
    let configs = vec![
        (InterfaceConfig::new_primary(LOWER_METRIC), LinkState::Up.into()),
        (InterfaceConfig::new_secondary(HIGHER_METRIC), LinkState::Up.into()),
    ];
    let mut helper = ReachabilityTestHelper::new(name, &env, configs).await;

    helper.set_iface_states(vec![state1, state2]);
    let snapshot = helper.next_snapshot_with_internet(true).await;
    assert_eq!(snapshot.gateway_reachable, Some(true));
    assert_eq!(snapshot.internet_available, Some(true));
}

#[netstack_test]
#[variant(N, Netstack)]
#[test_case(LinkState::Gateway.into(), LinkState::Gateway.into())]
#[test_case(LinkState::Up.into(), LinkState::Up.into())]
async fn test_internet_goes_down<N: Netstack>(name: &str, state1: State, state2: State) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let env = setup_reachability_env::<N>(name, &sandbox, false).await;
    let configs = vec![
        (InterfaceConfig::new_primary(LOWER_METRIC), LinkState::Internet.into()),
        (InterfaceConfig::new_secondary(HIGHER_METRIC), LinkState::Internet.into()),
    ];
    let mut helper = ReachabilityTestHelper::new(name, &env, configs).await;

    let snapshot = helper.next_snapshot_with_internet(true).await;
    assert_eq!(snapshot.gateway_reachable, Some(true));
    assert_eq!(snapshot.internet_available, Some(true));

    helper.set_iface_states(vec![state1, state2]);
    let snapshot = helper.next_snapshot_with_internet(false).await;
    assert_eq!(snapshot.internet_available, Some(false));
}

#[netstack_test]
#[variant(N, Netstack)]
#[test_case(LinkState::Gateway.into(), LinkState::Gateway.into())]
#[test_case(LinkState::Gateway.into(), LinkState::Up.into())]
async fn test_gateway_goes_down<N: Netstack>(name: &str, state1: State, state2: State) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let env = setup_reachability_env::<N>(name, &sandbox, false).await;
    let configs = vec![
        (InterfaceConfig::new_primary(LOWER_METRIC), state1),
        (InterfaceConfig::new_secondary(HIGHER_METRIC), state2),
    ];
    let mut helper = ReachabilityTestHelper::new(name, &env, configs).await;

    let snapshot = helper.next_snapshot_with_gateway(true).await;
    assert_eq!(snapshot.gateway_reachable, Some(true));
    assert_eq!(snapshot.internet_available, Some(false));

    helper.set_iface_states(vec![LinkState::Up.into(), LinkState::Up.into()]);
    let snapshot = helper.next_snapshot_with_gateway(false).await;
    assert_eq!(snapshot.gateway_reachable, Some(false));
    assert_eq!(snapshot.internet_available, Some(false));
}

#[netstack_test]
#[variant(N, Netstack)]
async fn test_internet_to_gateway_state<N: Netstack>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let env = setup_reachability_env::<N>(name, &sandbox, false).await;
    let configs = vec![
        (InterfaceConfig::new_primary(LOWER_METRIC), LinkState::Internet.into()),
        (InterfaceConfig::new_secondary(HIGHER_METRIC), LinkState::Gateway.into()),
    ];
    let mut helper = ReachabilityTestHelper::new(name, &env, configs).await;

    let snapshot = helper.next_snapshot_with_internet(true).await;
    assert_eq!(snapshot.gateway_reachable, Some(true));
    assert_eq!(snapshot.internet_available, Some(true));

    helper.set_iface_states(vec![LinkState::Gateway.into(), LinkState::Gateway.into()]);
    let snapshot = helper.next_snapshot_with_gateway(true).await;
    assert_eq!(snapshot.gateway_reachable, Some(true));
    assert_eq!(snapshot.internet_available, Some(false));
}

#[netstack_test]
#[variant(N, Netstack)]
async fn test_hanging_get_multiple_clients<N: Netstack>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm = sandbox
        .create_netstack_realm_with::<N, _, _>(
            name,
            &[
                KnownServiceProvider::Reachability { eager: true },
                KnownServiceProvider::FakeClock,
                KnownServiceProvider::DnsResolver,
            ],
        )
        .expect("failed to create realm");

    let monitor_client1 = realm
        .connect_to_protocol::<fnet_reachability::MonitorMarker>()
        .expect("client 1: failed to connect to fuchsia.net.reachability.Monitor");

    let monitor_client2 = realm
        .connect_to_protocol::<fnet_reachability::MonitorMarker>()
        .expect("client 2: failed to connect to fuchsia.net.reachability.Monitor");

    assert_eq!(
        monitor_client1
            .watch()
            .await
            .expect("client 1: failed to fetch updated snapshot")
            .internet_available,
        Some(false)
    );
    assert_eq!(
        monitor_client2
            .watch()
            .await
            .expect("client 2: failed to fetch updated snapshot")
            .internet_available,
        Some(false)
    );
}

#[netstack_test]
#[variant(N, Netstack)]
async fn test_cannot_call_set_options_after_watch<N: Netstack>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm = sandbox
        .create_netstack_realm_with::<N, _, _>(
            name,
            &[
                KnownServiceProvider::Reachability { eager: true },
                KnownServiceProvider::FakeClock,
                KnownServiceProvider::DnsResolver,
            ],
        )
        .expect("failed to create realm");

    let monitor = realm
        .connect_to_protocol::<fnet_reachability::MonitorMarker>()
        .expect("failed to connect to fuchsia.net.reachability.Monitor");

    assert_matches!(
        monitor.watch().await.expect("failed to fetch updated snapshot").internet_available,
        Some(_)
    );
    assert_matches!(monitor.set_options(&fnet_reachability::MonitorOptions::default()), Ok(()));

    // The request stream should be closed if the client calls SetOptions after calling Watch
    // previously.
    assert_matches!(monitor.on_closed().await, Ok(_));
}

#[netstack_test]
#[variant(N, Netstack)]
async fn test_cannot_call_set_options_twice<N: Netstack>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm = sandbox
        .create_netstack_realm_with::<N, _, _>(
            name,
            &[
                KnownServiceProvider::Reachability { eager: true },
                KnownServiceProvider::FakeClock,
                KnownServiceProvider::DnsResolver,
            ],
        )
        .expect("failed to create realm");

    let monitor = realm
        .connect_to_protocol::<fnet_reachability::MonitorMarker>()
        .expect("failed to connect to fuchsia.net.reachability.Monitor");

    let () = monitor
        .set_options(&fnet_reachability::MonitorOptions::default())
        .expect("failed to set reachability API options");
    let () = monitor
        .set_options(&fnet_reachability::MonitorOptions::default())
        .expect("failed to set reachability API options");

    // The request stream should be closed if the client calls SetOptions after calling it once
    // already.
    assert_matches!(monitor.on_closed().await, Ok(_));
}
