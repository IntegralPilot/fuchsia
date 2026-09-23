// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]
#![warn(unreachable_patterns, clippy::useless_conversion, clippy::redundant_clone)]
// TODO(https://fxbug.dev/339502691): Return to the default limit once lock
// ordering no longer causes overflows.
#![recursion_limit = "256"]

use std::num::{NonZeroU16, NonZeroUsize};
use std::sync::Arc;

use assert_matches::assert_matches;
use ip_test_macro::ip_test;
use net_types::ethernet::Mac;
use net_types::ip::{IpAddress, IpVersion, Ipv4Addr, Ipv6Addr, Subnet};
use net_types::{SpecifiedAddr, UnicastAddr, Witness, ZonedAddr};
use packet::{Buf, NestableSerializer as _, ParsablePacket as _, Serializer as _};
use packet_formats::ethernet::{EtherType, EthernetFrameBuilder};
use packet_formats::ip::{IpPacket as _, IpPacketBuilder as _, IpProto};
use packet_formats::tcp::options::TcpOptionsBuilder;
use packet_formats::tcp::{TcpSegmentBuilder, TcpSegmentBuilderWithOptions};
use packet_formats::udp::UdpPacketBuilder;
use test_case::test_case;

use netstack3_base::testutil::{TestDualStackIpExt, TestIpExt, set_logger_for_test};
use netstack3_base::{
    CtxPair, Mark, MarkDomain, MarkMatcher, MarkMatchers, NetworkParsingContext,
    NetworkSerializationContext, PortMatcher,
};
use netstack3_core::IpExt;
use netstack3_core::device::{DeviceId, EthernetLinkDevice, RecvEthernetFrameMeta};
use netstack3_core::filter::{
    Action, Hook, IpRoutines, MarkAction, NatRoutines, PacketMatcher, Routine, Routines, Rule,
    TransportProtocolMatcher,
};
use netstack3_core::routes::{
    AddableEntryEither, AddableMetric, RawMetric, Rule as RouteRule, RuleAction, RuleMatcher,
};
use netstack3_core::testutil::{
    CtxPairExt as _, FakeBindingsCtx, FakeCoreCtx, FakeCtx, FakeCtxBuilder,
};

const LOCAL_PORT: NonZeroU16 = NonZeroU16::new(22222).unwrap();
const REMOTE_PORT: NonZeroU16 = NonZeroU16::new(44444).unwrap();

fn make_udp_reply_packet<I: TestIpExt>() -> Buf<Vec<u8>> {
    Buf::new([1], ..)
        .wrap_in(UdpPacketBuilder::new(
            *I::TEST_ADDRS.remote_ip,
            *I::TEST_ADDRS.local_ip,
            Some(REMOTE_PORT),
            LOCAL_PORT,
        ))
        .wrap_in(I::PacketBuilder::new(
            *I::TEST_ADDRS.remote_ip,
            *I::TEST_ADDRS.local_ip,
            u8::MAX, /* ttl */
            IpProto::Udp.into(),
        ))
        .wrap_in(EthernetFrameBuilder::new(
            *I::TEST_ADDRS.remote_mac,
            *I::TEST_ADDRS.local_mac,
            EtherType::from_ip_version(I::VERSION),
            0, /* min_body_len */
        ))
        .serialize_vec_outer(&mut NetworkSerializationContext::default())
        .unwrap()
        .unwrap_b()
}

fn masquerade<I: TestIpExt>() -> Routines<I, FakeBindingsCtx, ()> {
    Routines {
        nat: NatRoutines {
            egress: Hook {
                routines: vec![Routine {
                    rules: vec![Rule {
                        matcher: PacketMatcher {
                            transport_protocol: Some(TransportProtocolMatcher {
                                proto: IpProto::Udp.into(),
                                src_port: None,
                                dst_port: None,
                            }),
                            ..Default::default()
                        },
                        action: Action::Masquerade { src_port: None },
                        validation_info: (),
                    }],
                }],
            },
            ..Default::default()
        },
        ..Default::default()
    }
}

#[netstack3_core::context_ip_bounds(I, FakeBindingsCtx)]
#[ip_test(I)]
fn crash<I: TestDualStackIpExt + IpExt>() {
    set_logger_for_test();

    let mut builder = FakeCtxBuilder::default();
    let dev_index = builder.add_device_with_ip(
        I::TEST_ADDRS.local_mac,
        *I::TEST_ADDRS.local_ip,
        I::TEST_ADDRS.subnet,
    );
    let (FakeCtx { core_ctx, bindings_ctx }, indexes_to_device_ids) = builder.build();
    let mut ctx = CtxPair { core_ctx: Arc::new(core_ctx), bindings_ctx };
    let device = indexes_to_device_ids.into_iter().nth(dev_index).unwrap();

    // Send a packet to a neighbor so that this flow is inserted in the connection
    // tracking table. It will not have NAT configured for it because no NAT rules
    // have been installed.
    let mut udp_api = ctx.core_api().udp::<I>();
    let socket = udp_api.create();
    udp_api
        .listen(&socket, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), Some(LOCAL_PORT))
        .unwrap();
    ctx.core_api()
        .udp()
        .send_to(
            &socket,
            Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)),
            REMOTE_PORT.into(),
            Buf::new([1], ..),
            Default::default(),
        )
        .unwrap();

    // Now configure outgoing traffic to be masqueraded. This rule is a no-op (we
    // are already sending from the assigned address of the interface), but it will
    // cause NAT to be performed rather than skipped.
    ctx.core_api().filter().set_filter_state(masquerade(), masquerade()).unwrap();

    // Race two threads each of which receives an identical UDP packet replying to
    // one that was sent on the socket. The flow is already finalized in conntrack,
    // i.e. inserted in the connection tracking table, so both reply packets will
    // obtain a shared reference to the finalized connection.
    //
    // The NAT module will attempt to configure NAT as a no-op for both these
    // packets; if they both expect the state not to be configured when they update
    // it, the one that loses the race will panic.

    let thread_vars = (ctx.clone(), device.clone());
    let reply_packet_one = std::thread::spawn(move || {
        let (mut ctx, device_id) = thread_vars;
        ctx.core_api().device::<EthernetLinkDevice>().receive_frame(
            RecvEthernetFrameMeta { device_id, parsing_context: NetworkParsingContext::default() },
            make_udp_reply_packet::<I>(),
        );
    });

    let thread_vars = (ctx.clone(), device);
    let reply_packet_two = std::thread::spawn(move || {
        let (mut ctx, device_id) = thread_vars;
        ctx.core_api().device::<EthernetLinkDevice>().receive_frame(
            RecvEthernetFrameMeta { device_id, parsing_context: NetworkParsingContext::default() },
            make_udp_reply_packet::<I>(),
        );
    });

    reply_packet_one.join().unwrap();
    reply_packet_two.join().unwrap();

    // Remove the packets from the receive queue in bindings so that references to
    // core resources are cleaned up before the core context is dropped at the end
    // of the test.
    let _ = ctx.bindings_ctx.take_udp_received(&socket);
}

fn mark_on_incoming_packet<I: TestIpExt>(mark: u32) -> Routines<I, FakeBindingsCtx, ()> {
    Routines {
        ip: IpRoutines {
            ingress: Hook {
                routines: vec![Routine {
                    rules: vec![Rule {
                        matcher: Default::default(),
                        action: Action::Mark {
                            domain: MarkDomain::Mark1,
                            action: MarkAction::SetMark { clearing_mask: 0, mark },
                        },
                        validation_info: (),
                    }],
                }],
            },
            ..Default::default()
        },
        ..Default::default()
    }
}

#[netstack3_core::context_ip_bounds(I, FakeBindingsCtx)]
#[ip_test(I)]
fn tcp_accepted_mark<I: TestDualStackIpExt + IpExt>() {
    set_logger_for_test();

    let builder = FakeCtxBuilder::with_addrs(I::TEST_ADDRS);
    let (FakeCtx { core_ctx, bindings_ctx }, indexes_to_device_ids) = builder.build();
    let mut ctx = CtxPair { core_ctx: Arc::new(core_ctx), bindings_ctx };
    let device_id = &indexes_to_device_ids[0];

    let mut tcp_api = ctx.core_api().tcp::<I>();
    let socket = tcp_api.create(Default::default());
    tcp_api
        .bind(&socket, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), Some(LOCAL_PORT))
        .unwrap();
    tcp_api.listen(&socket, NonZeroUsize::new(1).unwrap()).unwrap();

    const MARK: u32 = 100;
    // Configure the rules that only marked packets can be routed.
    let main_table = ctx.core_api().routes::<I>().main_table_id();
    ctx.test_api().set_rules::<I>(vec![
        RouteRule {
            matcher: RuleMatcher {
                mark_matchers: MarkMatchers::new([(
                    MarkDomain::Mark1,
                    MarkMatcher::Marked { mask: !0, start: MARK, end: MARK, invert: false },
                )]),
                ..RuleMatcher::match_all_packets()
            },
            action: RuleAction::Lookup(main_table),
        },
        RouteRule { matcher: RuleMatcher::match_all_packets(), action: RuleAction::Unreachable },
    ]);

    const SYN_SEQ: u32 = 0;

    let receive_syn_and_take_replies = |ctx: &mut CtxPair<Arc<FakeCoreCtx>, FakeBindingsCtx>| {
        let syn_frame = {
            let mut tcp_seg = TcpSegmentBuilder::new(
                *I::TEST_ADDRS.remote_ip,
                *I::TEST_ADDRS.local_ip,
                REMOTE_PORT,
                LOCAL_PORT,
                SYN_SEQ,
                None,
                u16::MAX,
            );
            tcp_seg.syn(true);
            Buf::new([], ..)
                .wrap_in(tcp_seg)
                .wrap_in(I::PacketBuilder::new(
                    *I::TEST_ADDRS.remote_ip,
                    *I::TEST_ADDRS.local_ip,
                    u8::MAX, /* ttl */
                    IpProto::Tcp.into(),
                ))
                .wrap_in(EthernetFrameBuilder::new(
                    *I::TEST_ADDRS.remote_mac,
                    *I::TEST_ADDRS.local_mac,
                    EtherType::from_ip_version(I::VERSION),
                    0, /* min_body_len */
                ))
                .serialize_vec_outer(&mut NetworkSerializationContext::default())
                .unwrap()
                .unwrap_b()
        };

        ctx.core_api().device::<EthernetLinkDevice>().receive_frame(
            RecvEthernetFrameMeta {
                device_id: device_id.clone(),
                parsing_context: NetworkParsingContext::default(),
            },
            syn_frame,
        );

        ctx.bindings_ctx.take_ethernet_frames()
    };

    // Without the filter rules, the stack is not able to generate a SYN-ACK
    // because it is not routable, so we are not able to get anything back.
    assert_eq!(receive_syn_and_take_replies(&mut ctx), vec![]);

    // Configure the filtering rules to mark incoming packets only on the right version.
    let (ipv4_filter_rules, ipv6_filter_rules) = match I::VERSION {
        IpVersion::V4 => (mark_on_incoming_packet(MARK), Default::default()),
        IpVersion::V6 => (Default::default(), mark_on_incoming_packet(MARK)),
    };
    ctx.core_api().filter().set_filter_state(ipv4_filter_rules, ipv6_filter_rules).unwrap();

    // With the filter rule, the SYN-ACK should be successfully generated and
    // we should be able to further drive the handshake.
    let synack_frame = assert_matches!(
        &receive_syn_and_take_replies(&mut ctx)[..],
        [(_device, frame)] => frame.clone()
    );
    let mut synack_frame = &synack_frame[..];
    let eth = packet_formats::ethernet::EthernetFrame::parse(
        &mut synack_frame,
        packet_formats::ethernet::EthernetFrameLengthCheck::NoCheck,
    )
    .unwrap();
    let mut body = eth.body();
    let ip = I::Packet::parse(&mut body, ()).unwrap();
    assert_eq!(ip.proto(), IpProto::Tcp.into());
    let mut tcp = ip.body();
    let parsed_synack = packet_formats::tcp::TcpSegment::parse(
        &mut tcp,
        packet_formats::tcp::TcpParseArgs::new(ip.src_ip(), ip.dst_ip()),
    )
    .unwrap();
    assert!(parsed_synack.syn());
    assert_eq!(parsed_synack.ack_num(), Some(SYN_SEQ + 1));

    let ack_frame = {
        Buf::new([], ..)
            .wrap_in(TcpSegmentBuilder::new(
                *I::TEST_ADDRS.remote_ip,
                *I::TEST_ADDRS.local_ip,
                REMOTE_PORT,
                LOCAL_PORT,
                1,
                Some(parsed_synack.seq_num() + 1),
                u16::MAX,
            ))
            .wrap_in(I::PacketBuilder::new(
                *I::TEST_ADDRS.remote_ip,
                *I::TEST_ADDRS.local_ip,
                u8::MAX, /* ttl */
                IpProto::Tcp.into(),
            ))
            .wrap_in(EthernetFrameBuilder::new(
                *I::TEST_ADDRS.remote_mac,
                *I::TEST_ADDRS.local_mac,
                EtherType::from_ip_version(I::VERSION),
                0, /* min_body_len */
            ))
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap()
            .unwrap_b()
    };

    ctx.core_api().device::<EthernetLinkDevice>().receive_frame(
        RecvEthernetFrameMeta {
            device_id: device_id.clone(),
            parsing_context: NetworkParsingContext::default(),
        },
        ack_frame,
    );
    drop(indexes_to_device_ids);

    let (accepted, _, _) = ctx.core_api().tcp::<I>().accept(&socket).unwrap();
    assert_eq!(ctx.core_api().tcp::<I>().get_mark(&accepted, MarkDomain::Mark1), Mark(Some(MARK)));
    assert_eq!(ctx.core_api().tcp::<I>().get_mark(&accepted, MarkDomain::Mark2), Mark(None));
}

fn drop_tcp_on_port<I: TestIpExt>(port: NonZeroU16) -> Routines<I, FakeBindingsCtx, ()> {
    Routines {
        ip: IpRoutines {
            ingress: Hook {
                routines: vec![Routine {
                    rules: vec![Rule {
                        matcher: PacketMatcher {
                            transport_protocol: Some(TransportProtocolMatcher {
                                proto: IpProto::Tcp.into(),
                                src_port: None,
                                dst_port: Some(PortMatcher {
                                    range: port.get()..=port.get(),
                                    invert: false,
                                }),
                            }),
                            ..Default::default()
                        },
                        action: Action::Drop,
                        validation_info: (),
                    }],
                }],
            },
            ..Default::default()
        },
        ..Default::default()
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum TcpMalformation {
    MalformedFlags,
    InvalidOptions,
}

#[netstack3_core::context_ip_bounds(I, FakeBindingsCtx)]
#[ip_test(I)]
#[test_case(TcpMalformation::MalformedFlags; "malformed flags")]
#[test_case(TcpMalformation::InvalidOptions; "invalid options")]
fn tcp_dropped_filter<I: TestDualStackIpExt + IpExt>(malformation: TcpMalformation) {
    set_logger_for_test();

    let config = I::TEST_ADDRS;
    let mut builder = FakeCtxBuilder::with_addrs(config);

    // Device 0 is device_in.
    // Let's add Device 1 (device_out).
    let device_out_mac = UnicastAddr::new(Mac::new([2, 3, 4, 5, 6, 7])).unwrap();
    let remote_out_mac = UnicastAddr::new(Mac::new([8, 9, 10, 11, 12, 13])).unwrap();

    let (device_out_ip, device_out_subnet, remote_out_ip): (
        SpecifiedAddr<I::Addr>,
        Subnet<I::Addr>,
        SpecifiedAddr<I::Addr>,
    ) = I::map_ip(
        (),
        |()| {
            (
                SpecifiedAddr::new(Ipv4Addr::new([198, 51, 100, 1])).unwrap(),
                Subnet::new(Ipv4Addr::new([198, 51, 100, 0]), 24).unwrap(),
                SpecifiedAddr::new(Ipv4Addr::new([198, 51, 100, 2])).unwrap(),
            )
        },
        |()| {
            (
                SpecifiedAddr::new(Ipv6Addr::from([
                    0x20, 0x01, 0x0d, 0xb8, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1,
                ]))
                .unwrap(),
                Subnet::new(
                    Ipv6Addr::from([0x20, 0x01, 0x0d, 0xb8, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
                    64,
                )
                .unwrap(),
                SpecifiedAddr::new(Ipv6Addr::from([
                    0x20, 0x01, 0x0d, 0xb8, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2,
                ]))
                .unwrap(),
            )
        },
    );

    let dev_out_idx =
        builder.add_device_with_ip(device_out_mac, device_out_ip.get(), device_out_subnet);
    builder.add_arp_or_ndp_table_entry(dev_out_idx, remote_out_ip, remote_out_mac);

    let (FakeCtx { core_ctx, bindings_ctx }, indexes_to_device_ids) = builder.build();
    let mut ctx = CtxPair { core_ctx: Arc::new(core_ctx), bindings_ctx };
    let eth_device_in = indexes_to_device_ids[0].clone();
    let eth_device_out = indexes_to_device_ids[1].clone();
    let device_in: DeviceId<_> = eth_device_in.clone().into();
    let device_out: DeviceId<_> = eth_device_out.into();

    // Enable forwarding.
    ctx.test_api().set_unicast_forwarding_enabled::<I>(&device_in, true);
    ctx.test_api().set_unicast_forwarding_enabled::<I>(&device_out, true);

    // Add route to remote_out_ip via device_out.
    ctx.test_api()
        .add_route(AddableEntryEither::without_gateway(
            Subnet::new(remote_out_ip.get(), <I::Addr as IpAddress>::BYTES * 8).unwrap().into(),
            device_out,
            AddableMetric::ExplicitMetric(RawMetric(0)),
        ))
        .unwrap();

    // Clear any frames sent during setup.
    let _ = ctx.bindings_ctx.take_ethernet_frames();

    // Helper to construct malformed TCP packet.
    let make_malformed_packet = |malformation| {
        let mut options = TcpOptionsBuilder::default();
        let use_options = match malformation {
            TcpMalformation::MalformedFlags => false,
            TcpMalformation::InvalidOptions => {
                options.mss = Some(1460);
                true
            }
        };

        let mut buf = if use_options {
            Buf::new(vec![0u8; 10], ..)
                .wrap_in(
                    TcpSegmentBuilderWithOptions::new(
                        TcpSegmentBuilder::new(
                            *I::TEST_ADDRS.remote_ip,
                            remote_out_ip.get(),
                            REMOTE_PORT,
                            LOCAL_PORT,
                            0,
                            None,
                            u16::MAX,
                        ),
                        options,
                    )
                    .unwrap(),
                )
                .wrap_in(I::PacketBuilder::new(
                    *I::TEST_ADDRS.remote_ip,
                    remote_out_ip.get(),
                    64,
                    IpProto::Tcp.into(),
                ))
                .wrap_in(EthernetFrameBuilder::new(
                    *I::TEST_ADDRS.remote_mac,
                    *I::TEST_ADDRS.local_mac,
                    EtherType::from_ip_version(I::VERSION),
                    0,
                ))
                .serialize_vec_outer(&mut NetworkSerializationContext::default())
                .unwrap()
                .unwrap_b()
        } else {
            Buf::new(vec![0u8; 10], ..)
                .wrap_in(TcpSegmentBuilder::new(
                    *I::TEST_ADDRS.remote_ip,
                    remote_out_ip.get(),
                    REMOTE_PORT,
                    LOCAL_PORT,
                    0,
                    None,
                    u16::MAX,
                ))
                .wrap_in(I::PacketBuilder::new(
                    *I::TEST_ADDRS.remote_ip,
                    remote_out_ip.get(),
                    64,
                    IpProto::Tcp.into(),
                ))
                .wrap_in(EthernetFrameBuilder::new(
                    *I::TEST_ADDRS.remote_mac,
                    *I::TEST_ADDRS.local_mac,
                    EtherType::from_ip_version(I::VERSION),
                    0,
                ))
                .serialize_vec_outer(&mut NetworkSerializationContext::default())
                .unwrap()
                .unwrap_b()
        };

        let ip_header_len = match I::VERSION {
            IpVersion::V4 => 20,
            IpVersion::V6 => 40,
        };
        let tcp_offset = 14 + ip_header_len;

        match malformation {
            TcpMalformation::MalformedFlags => {
                // Modify TCP header to include mutually exclusive flags: SYN (0x02) and RST (0x04).
                // Flags are at offset 13 in TCP header.
                buf.as_mut()[tcp_offset + 13] |= 0x02 | 0x04;
            }
            TcpMalformation::InvalidOptions => {
                // MSS option is at tcp_offset + 20.
                // Kind is 2, Length is 4.
                // Corrupt Length to 0.
                buf.as_mut()[tcp_offset + 21] = 0;
            }
        }

        buf
    };

    // Case 1: No filter rules. Packet should be forwarded.
    ctx.core_api().device::<EthernetLinkDevice>().receive_frame(
        RecvEthernetFrameMeta {
            device_id: eth_device_in.clone(),
            parsing_context: NetworkParsingContext::default(),
        },
        Buf::new(make_malformed_packet(malformation), ..),
    );

    let frames = ctx.bindings_ctx.take_ethernet_frames();
    // We should have forwarded one frame.
    assert_matches!(&frames[..], [(_dev, _frame)]);

    // Case 2: Install a DROP rule for TCP destination port LOCAL_PORT.
    ctx.core_api()
        .filter()
        .set_filter_state(drop_tcp_on_port(LOCAL_PORT), drop_tcp_on_port(LOCAL_PORT))
        .unwrap();

    // Send the malformed packet again.
    ctx.core_api().device::<EthernetLinkDevice>().receive_frame(
        RecvEthernetFrameMeta {
            device_id: eth_device_in,
            parsing_context: NetworkParsingContext::default(),
        },
        Buf::new(make_malformed_packet(malformation), ..),
    );

    let frames = ctx.bindings_ctx.take_ethernet_frames();
    // Packet should be dropped by the filter, so no frames forwarded.
    assert_matches!(frames[..], []);

    // Clean up strong references before CtxPair is dropped.
    drop(device_in);
    drop(indexes_to_device_ids);
}
