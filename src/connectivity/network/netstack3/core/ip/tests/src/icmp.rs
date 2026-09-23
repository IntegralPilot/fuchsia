// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use alloc::vec;
use alloc::vec::Vec;
use core::fmt::Debug;
use core::num::NonZeroU16;

use assert_matches::assert_matches;
use ip_test_macro::ip_test;
use net_declare::{net_ip_v4, net_ip_v6};
use net_types::ip::{
    GenericOverIp, Ip, IpAddress, IpVersion, Ipv4, Ipv4Addr, Ipv6, Ipv6Addr, Subnet,
};
use net_types::{SpecifiedAddr, Witness};
use packet::{Buf, NestablePacketBuilder, NestableSerializer as _, Serializer};
use packet_formats::ethernet::EthernetFrameLengthCheck;
use packet_formats::icmp::{
    IcmpDestUnreachable, IcmpEchoReply, IcmpEchoRequest, IcmpMessage, IcmpPacket,
    IcmpPacketBuilder, IcmpTimeExceeded, IcmpZeroCode, Icmpv4DestUnreachableCode,
    Icmpv4TimeExceededCode, Icmpv4TimestampRequest, Icmpv6DestUnreachableCode,
    Icmpv6TimeExceededCode, MessageBody, OriginalPacket,
};
use packet_formats::ip::{FragmentOffset, IpPacketBuilder as _, IpProto, Ipv4Proto, Ipv6Proto};
use packet_formats::testutil::parse_icmp_packet_in_ip_packet_in_ethernet_frame;
use packet_formats::udp::UdpPacketBuilder;

use netstack3_base::testutil::{TEST_ADDRS_V4, TEST_ADDRS_V6, TestIpExt, set_logger_for_test};
use netstack3_base::{
    CounterCollection as _, CounterContext, FrameDestination, MarkMatcher, MarkMatchers, Marks,
    NetworkSerializationContext,
};
use netstack3_core::device::DeviceId;
use netstack3_core::ip::MarkDomain;
use netstack3_core::testutil::{Ctx, CtxPairExt as _, FakeBindingsCtx, FakeCtxBuilder};
use netstack3_core::{IpExt, StackStateBuilder};
use netstack3_ip::icmp::{IcmpRxCounters, IcmpTxCounters, Icmpv4StateBuilder};
use netstack3_ip::{
    AddableEntry, AddableMetric, IpCounters, RawMetric, Rule, RuleAction, RuleMatcher,
};
use test_case::test_case;

#[derive(Default, GenericOverIp)]
#[generic_over_ip(I, Ip)]
struct CounterExpectations<I: TestIpExt + IpExt> {
    ip: IpCounters<I, u64>,
    icmp_rx: IcmpRxCounters<I, u64>,
    icmp_tx: IcmpTxCounters<I, u64>,
}

impl<I: TestIpExt + IpExt> CounterExpectations<I> {
    fn default_receive_send() -> Self {
        Self {
            ip: IpCounters { receive_ip_packet: 1, send_ip_packet: 1, ..Default::default() },
            ..Default::default()
        }
    }
    fn default_receive_deliver_send() -> Self {
        Self {
            ip: IpCounters {
                receive_ip_packet: 1,
                dispatch_receive_ip_packet: 1,
                deliver_unicast: 1,
                send_ip_packet: 1,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Assert that the counters tracked by `core_ctx` match expectations.
    #[track_caller]
    fn assert_counters<
        CC: CounterContext<IpCounters<I>>
            + CounterContext<IcmpRxCounters<I>>
            + CounterContext<IcmpTxCounters<I>>,
    >(
        self,
        core_ctx: &CC,
    ) {
        let Self { ip, icmp_rx, icmp_tx } = self;

        assert_eq!(<CC as CounterContext<IpCounters<I>>>::counters(core_ctx).cast::<u64>(), ip);
        assert_eq!(
            <CC as CounterContext<IcmpRxCounters<I>>>::counters(core_ctx).cast::<u64>(),
            icmp_rx
        );
        assert_eq!(
            <CC as CounterContext<IcmpTxCounters<I>>>::counters(core_ctx).cast::<u64>(),
            icmp_tx
        );
    }
}

/// Test that receiving a particular IP packet results in a particular ICMP
/// response.
///
/// Test that receiving an IP packet from remote host
/// `I::TEST_ADDRS.remote_ip` to host `dst_ip` with `ttl` and `proto`
/// results in all of the counters in `assert_counters` being triggered at
/// least once.
///
/// If `expect_message_code` is `Some`, expect that exactly one ICMP packet
/// was sent in response with the given message and code, and invoke the
/// given function `f` on the packet. Otherwise, if it is `None`, expect
/// that no response was sent.
///
/// `modify_packet_builder` is invoked on the `PacketBuilder` before the
/// packet is serialized.
///
/// `modify_stack_state_builder` is invoked on the `StackStateBuilder`
/// before it is used to build the context.
///
/// The state is initialized to `I::TEST_ADDRS` when testing.
#[allow(clippy::too_many_arguments)]
#[netstack3_macros::context_ip_bounds(I, FakeBindingsCtx)]
fn test_receive_ip_packet<
    I: TestIpExt + IpExt,
    C: PartialEq + Debug,
    M: IcmpMessage<I, Code = C> + PartialEq + Debug,
    PBF: FnOnce(&mut <I as packet_formats::ip::IpExt>::PacketBuilder<NetworkSerializationContext>),
    SSBF: FnOnce(&mut StackStateBuilder),
    F: for<'a> FnOnce(&IcmpPacket<I, &'a [u8], M>),
>(
    modify_packet_builder: PBF,
    modify_stack_state_builder: SSBF,
    body: &mut [u8],
    dst_ip: SpecifiedAddr<I::Addr>,
    ttl: u8,
    proto: I::Proto,
    counter_expects: CounterExpectations<I>,
    expect_message_code: Option<(M, C)>,
    f: F,
    test_mark_reflection: bool,
) {
    set_logger_for_test();
    let mut pb = <I as packet_formats::ip::IpExt>::PacketBuilder::new(
        *I::TEST_ADDRS.remote_ip,
        dst_ip.get(),
        ttl,
        proto,
    );
    modify_packet_builder(&mut pb);
    let buffer = pb
        .wrap_body(Buf::new(body, ..))
        .serialize_vec_outer(&mut NetworkSerializationContext::default())
        .unwrap();

    let (mut ctx, device_ids) = FakeCtxBuilder::with_addrs(I::TEST_ADDRS)
        .build_with_modifications(modify_stack_state_builder);

    let device: DeviceId<_> = device_ids[0].clone().into();
    ctx.test_api().set_unicast_forwarding_enabled::<I>(&device, true);
    if test_mark_reflection {
        let marks = vec![(MarkDomain::Mark1, 100), (MarkDomain::Mark2, 200)];
        let main_table = ctx.core_api().routes::<I>().main_table_id();
        // Installs rules to make sure that only packets with the marks can be routed.
        ctx.test_api().set_rules::<I>(alloc::vec![
            Rule {
                matcher: RuleMatcher {
                    mark_matchers: MarkMatchers::new(marks.iter().cloned().map(
                        |(domain, mark)| {
                            (
                                domain,
                                MarkMatcher::Marked {
                                    mask: !0,
                                    start: mark,
                                    end: mark,
                                    invert: false,
                                },
                            )
                        }
                    )),
                    ..RuleMatcher::match_all_packets()
                },
                action: RuleAction::Lookup(main_table),
            },
            Rule { matcher: RuleMatcher::match_all_packets(), action: RuleAction::Unreachable },
        ]);
        ctx.test_api().receive_ip_packet_with_marks::<I, _>(
            &device,
            Some(FrameDestination::Individual { local: () }),
            buffer,
            Marks::new(marks),
        );
    } else {
        ctx.test_api().receive_ip_packet::<I, _>(
            &device,
            Some(FrameDestination::Individual { local: () }),
            buffer,
        );
    }

    counter_expects.assert_counters(&ctx.core_ctx());

    let Ctx { core_ctx: _, bindings_ctx } = &mut ctx;
    if let Some((expect_message, expect_code)) = expect_message_code {
        let frames = bindings_ctx.take_ethernet_frames();
        let (_dev, frame) = assert_matches!(&frames[..], [frame] => frame);
        assert_eq!(frames.len(), 1);
        let (src_mac, dst_mac, src_ip, dst_ip, _, message, code) =
            parse_icmp_packet_in_ip_packet_in_ethernet_frame::<I, _, M, _>(
                &frame,
                EthernetFrameLengthCheck::NoCheck,
                f,
            )
            .unwrap();

        assert_eq!(src_mac, I::TEST_ADDRS.local_mac.get());
        assert_eq!(dst_mac, I::TEST_ADDRS.remote_mac.get());
        assert_eq!(src_ip, I::TEST_ADDRS.local_ip.get());
        assert_eq!(dst_ip, I::TEST_ADDRS.remote_ip.get());
        assert_eq!(message, expect_message);
        assert_eq!(code, expect_code);
    } else {
        assert_matches!(bindings_ctx.take_ethernet_frames()[..], []);
    }
}

#[netstack3_macros::context_ip_bounds(I, FakeBindingsCtx)]
#[ip_test(I)]
#[test_case(true; "reflection")]
#[test_case(false; "no reflection")]
fn test_receive_echo<I: TestIpExt + IpExt>(test_mark_reflection: bool) {
    set_logger_for_test();

    // Test that, when receiving an echo request, we respond with an echo
    // reply with the appropriate parameters.
    let mut counter_expects = CounterExpectations::default_receive_deliver_send();
    counter_expects.icmp_rx.echo_request = 1;
    counter_expects.icmp_tx.reply = 1;

    let req = IcmpEchoRequest::new(0, 0);
    let req_body = &[1, 2, 3, 4];
    let mut buffer = Buf::new(req_body.to_vec(), ..)
        .wrap_in(IcmpPacketBuilder::<I, _>::new(
            I::TEST_ADDRS.remote_ip.get(),
            I::TEST_ADDRS.local_ip.get(),
            IcmpZeroCode,
            req,
        ))
        .serialize_vec_outer(&mut NetworkSerializationContext::default())
        .unwrap();
    test_receive_ip_packet::<I, _, _, _, _, _>(
        |_| {},
        |_| {},
        buffer.as_mut(),
        I::TEST_ADDRS.local_ip,
        64,
        I::ICMP_IP_PROTO,
        counter_expects,
        Some((req.reply(), IcmpZeroCode)),
        |packet| {
            let (inner_header, inner_body) = packet.original_packet().bytes();
            assert!(inner_body.is_none());
            assert_eq!(inner_header, req_body)
        },
        test_mark_reflection,
    );
}

#[test_case(true; "mark_reflection")]
#[test_case(false; "no reflection")]
fn test_receive_timestamp(test_mark_reflection: bool) {
    set_logger_for_test();

    let mut counter_expects = CounterExpectations::default_receive_deliver_send();
    counter_expects.icmp_rx.timestamp_request = 1;
    counter_expects.icmp_tx.reply = 1;

    let req = Icmpv4TimestampRequest::new(1, 2, 3);
    let mut buffer = Buf::new(Vec::new(), ..)
        .wrap_in(IcmpPacketBuilder::<Ipv4, _>::new(
            TEST_ADDRS_V4.remote_ip,
            TEST_ADDRS_V4.local_ip,
            IcmpZeroCode,
            req,
        ))
        .serialize_vec_outer(&mut NetworkSerializationContext::default())
        .unwrap();
    test_receive_ip_packet::<Ipv4, _, _, _, _, _>(
        |_| {},
        |builder| {
            let _: &mut Icmpv4StateBuilder =
                builder.ipv4_builder().icmpv4_builder().send_timestamp_reply(true);
        },
        buffer.as_mut(),
        TEST_ADDRS_V4.local_ip,
        64,
        Ipv4Proto::Icmp,
        counter_expects,
        Some((req.reply(0x80000000, 0x80000000), IcmpZeroCode)),
        |_| {},
        test_mark_reflection,
    );
}

#[test_case(true; "mark_reflection")]
#[test_case(false; "no reflection")]
fn test_protocol_unreachable(test_mark_reflection: bool) {
    // Test receiving an IP packet for an unreachable protocol. Check to
    // make sure that we respond with the appropriate ICMP message.
    //
    // Currently, for IPv4, we test for all unreachable protocols, while for
    // IPv6, we only test for IGMP and TCP. See the comment below for why
    // that limitation exists. Once the limitation is fixed, we should test
    // with all unreachable protocols for both versions.

    for proto in 0u8..=255 {
        let v4proto = Ipv4Proto::from(proto);
        match v4proto {
            Ipv4Proto::Other(_) | Ipv4Proto::Proto(IpProto::Reserved) => {
                let mut counter_expects =
                    CounterExpectations::<Ipv4>::default_receive_deliver_send();
                counter_expects.icmp_tx.dest_unreachable.dest_protocol_unreachable = 1;
                counter_expects.icmp_tx.error = 1;

                test_receive_ip_packet::<Ipv4, _, _, _, _, _>(
                    |_| {},
                    |_| {},
                    &mut [0u8; 128],
                    TEST_ADDRS_V4.local_ip,
                    64,
                    v4proto,
                    counter_expects,
                    Some((
                        IcmpDestUnreachable::default(),
                        Icmpv4DestUnreachableCode::DestProtocolUnreachable,
                    )),
                    // Ensure packet is truncated to the right length.
                    |packet| assert_eq!(packet.original_packet().len(), 84),
                    test_mark_reflection,
                );
            }
            Ipv4Proto::Icmp
            | Ipv4Proto::Igmp
            | Ipv4Proto::Proto(IpProto::Udp)
            | Ipv4Proto::Proto(IpProto::Tcp) => {}
        }

        // TODO(https://fxbug.dev/42124756): We seem to fail to parse an IPv6 packet if
        // its Next Header value is unrecognized (rather than treating this
        // as a valid parsing but then replying with a parameter problem
        // error message). We should a) fix this and, b) expand this test to
        // ensure we don't regress.
        let v6proto = Ipv6Proto::from(proto);
        match v6proto {
            Ipv6Proto::Icmpv6
            | Ipv6Proto::NoNextHeader
            | Ipv6Proto::Proto(IpProto::Udp)
            | Ipv6Proto::Proto(IpProto::Tcp)
            | Ipv6Proto::Other(_)
            | Ipv6Proto::Proto(IpProto::Reserved) => {}
        }
    }
}

#[netstack3_macros::context_ip_bounds(I, FakeBindingsCtx)]
#[ip_test(I)]
#[test_case(true; "reflection")]
#[test_case(false; "no reflection")]
fn test_port_unreachable<I: TestIpExt + IpExt>(test_mark_reflection: bool)
where
    for<'a> IcmpDestUnreachable:
        IcmpMessage<I, Code = I::DestUnreachableCode, Body<&'a [u8]> = OriginalPacket<&'a [u8]>>,
{
    // TODO(joshlf): Test TCP as well.

    // Receive an IP packet for an unreachable UDP port (1234). Check to
    // make sure that we respond with the appropriate ICMP message. Then, do
    // the same for a stack which has the UDP `send_port_unreachable` option
    // disable, and make sure that we DON'T respond with an ICMP message.

    let mut counter_expects = CounterExpectations::default_receive_deliver_send();
    counter_expects.icmp_tx.error = 1;
    I::map_ip::<_, ()>(
        &mut counter_expects,
        |c| {
            c.icmp_tx.dest_unreachable.dest_port_unreachable = 1;
        },
        |c| {
            c.icmp_tx.dest_unreachable.port_unreachable = 1;
        },
    );

    #[derive(GenericOverIp)]
    #[generic_over_ip(I, Ip)]
    struct CodeWrapper<I: IpExt>(I::DestUnreachableCode);
    let CodeWrapper(code) = I::map_ip(
        (),
        |()| CodeWrapper(Icmpv4DestUnreachableCode::DestPortUnreachable),
        |()| CodeWrapper(Icmpv6DestUnreachableCode::PortUnreachable),
    );

    let original_packet_len = match I::VERSION {
        IpVersion::V4 => 84,
        IpVersion::V6 => 176,
    };

    let mut buffer = Buf::new(vec![0; 128], ..)
        .wrap_in(UdpPacketBuilder::new(
            I::TEST_ADDRS.remote_ip.get(),
            I::TEST_ADDRS.local_ip.get(),
            None,
            NonZeroU16::new(1234).unwrap(),
        ))
        .serialize_vec_outer(&mut NetworkSerializationContext::default())
        .unwrap();
    test_receive_ip_packet::<I, _, _, _, _, _>(
        |_| {},
        |_| {},
        buffer.as_mut(),
        I::TEST_ADDRS.local_ip,
        64,
        IpProto::Udp.into(),
        counter_expects,
        Some((IcmpDestUnreachable::default(), code)),
        // Ensure packet is truncated to the right length.
        |packet| assert_eq!(packet.original_packet().len(), original_packet_len),
        test_mark_reflection,
    );
}

#[test_case(true; "reflection")]
#[test_case(false; "no reflection")]
fn test_net_unreachable(test_mark_reflection: bool) {
    // Receive an IP packet for an unreachable destination address. Check to
    // make sure that we respond with the appropriate ICMP message.
    let mut counter_expects = CounterExpectations::<Ipv4>::default_receive_send();
    counter_expects.ip.no_route_to_host = 1;
    counter_expects.icmp_tx.dest_unreachable.dest_network_unreachable = 1;
    counter_expects.icmp_tx.error = 1;
    test_receive_ip_packet::<Ipv4, _, _, _, _, _>(
        |_| {},
        |_: &mut StackStateBuilder| {},
        &mut [0u8; 128],
        SpecifiedAddr::new(Ipv4Addr::new([1, 2, 3, 4])).unwrap(),
        64,
        IpProto::Udp.into(),
        counter_expects,
        Some((IcmpDestUnreachable::default(), Icmpv4DestUnreachableCode::DestNetworkUnreachable)),
        // Ensure packet is truncated to the right length.
        |packet| assert_eq!(packet.original_packet().len(), 84),
        test_mark_reflection,
    );

    let mut counter_expects = CounterExpectations::<Ipv6>::default_receive_send();
    counter_expects.ip.no_route_to_host = 1;
    counter_expects.icmp_tx.dest_unreachable.no_route = 1;
    counter_expects.icmp_tx.error = 1;
    test_receive_ip_packet::<Ipv6, _, _, _, _, _>(
        |_| {},
        |_: &mut StackStateBuilder| {},
        &mut [0u8; 128],
        SpecifiedAddr::new(Ipv6Addr::from_bytes([1, 2, 3, 4, 5, 6, 7, 8, 1, 2, 3, 4, 5, 6, 7, 8]))
            .unwrap(),
        64,
        IpProto::Udp.into(),
        counter_expects,
        Some((IcmpDestUnreachable::default(), Icmpv6DestUnreachableCode::NoRoute)),
        // Ensure packet is truncated to the right length.
        |packet| assert_eq!(packet.original_packet().len(), 168),
        test_mark_reflection,
    );
    // Same test for IPv4 but with a non-initial fragment. No ICMP error
    // should be sent.
    let mut counter_expects = CounterExpectations::<Ipv4>::default();
    counter_expects.ip.receive_ip_packet = 1;
    counter_expects.ip.need_more_fragments = 1;
    test_receive_ip_packet::<Ipv4, _, IcmpDestUnreachable, _, _, _>(
        |pb| pb.fragment_offset(FragmentOffset::new(64).unwrap()),
        |_: &mut StackStateBuilder| {},
        &mut [0u8; 128],
        SpecifiedAddr::new(Ipv4Addr::new([1, 2, 3, 4])).unwrap(),
        64,
        IpProto::Udp.into(),
        counter_expects,
        None,
        |_| {},
        test_mark_reflection,
    );
}

#[test_case(true; "reflection")]
#[test_case(false; "no reflection")]
fn test_ttl_expired(test_mark_reflection: bool) {
    // Receive an IP packet with an expired TTL. Check to make sure that we
    // respond with the appropriate ICMP message.
    let mut counter_expects = CounterExpectations::<Ipv4>::default_receive_send();
    counter_expects.ip.forward = 1;
    counter_expects.ip.ttl_expired = 1;
    counter_expects.icmp_tx.time_exceeded.ttl_expired = 1;
    counter_expects.icmp_tx.error = 1;
    test_receive_ip_packet::<Ipv4, _, _, _, _, _>(
        |_| {},
        |_: &mut StackStateBuilder| {},
        &mut [0u8; 128],
        TEST_ADDRS_V4.remote_ip,
        1,
        IpProto::Udp.into(),
        counter_expects,
        Some((IcmpTimeExceeded::default(), Icmpv4TimeExceededCode::TtlExpired)),
        // Ensure packet is truncated to the right length.
        |packet| assert_eq!(packet.original_packet().len(), 84),
        test_mark_reflection,
    );

    let mut counter_expects = CounterExpectations::<Ipv6>::default_receive_send();
    counter_expects.ip.forward = 1;
    counter_expects.ip.ttl_expired = 1;
    counter_expects.icmp_tx.time_exceeded.hop_limit_exceeded = 1;
    counter_expects.icmp_tx.error = 1;
    test_receive_ip_packet::<Ipv6, _, _, _, _, _>(
        |_| {},
        |_: &mut StackStateBuilder| {},
        &mut [0u8; 128],
        TEST_ADDRS_V6.remote_ip,
        1,
        IpProto::Udp.into(),
        counter_expects,
        Some((IcmpTimeExceeded::default(), Icmpv6TimeExceededCode::HopLimitExceeded)),
        // Ensure packet is truncated to the right length.
        |packet| assert_eq!(packet.original_packet().len(), 168),
        test_mark_reflection,
    );

    // Same test for IPv4 but with a non-initial fragment. No ICMP error
    // should be sent.
    let mut counter_expects = CounterExpectations::<Ipv4>::default();
    counter_expects.ip.receive_ip_packet = 1;
    counter_expects.ip.need_more_fragments = 1;
    test_receive_ip_packet::<Ipv4, _, IcmpTimeExceeded, _, _, _>(
        |pb| pb.fragment_offset(FragmentOffset::new(64).unwrap()),
        |_: &mut StackStateBuilder| {},
        &mut [0u8; 128],
        SpecifiedAddr::new(Ipv4Addr::new([1, 2, 3, 4])).unwrap(),
        64,
        IpProto::Udp.into(),
        counter_expects,
        None,
        |_| {},
        test_mark_reflection,
    );
}

// Regression test for https://fxbug.dev/395320917. Test that, when receiving an
// echo request, we respond with an echo reply coming out the exact same
// interface.
#[netstack3_macros::context_ip_bounds(I, FakeBindingsCtx)]
#[ip_test(I)]
fn icmp_reply_follows_request_interface<I: TestIpExt + IpExt>() {
    set_logger_for_test();

    let req_body = &mut [1, 2, 3, 4];
    const TTL: u8 = 1;

    let multicast_addr =
        I::map_ip_out((), |()| net_ip_v4!("224.0.0.1"), |()| net_ip_v6!("ff02::1"));

    let buffer = Buf::new(req_body, ..)
        .wrap_in(IcmpPacketBuilder::<I, _>::new(
            I::TEST_ADDRS.remote_ip.get(),
            multicast_addr,
            IcmpZeroCode,
            IcmpEchoRequest::new(0, 0),
        ))
        .wrap_in(<I as packet_formats::ip::IpExt>::PacketBuilder::new(
            I::TEST_ADDRS.remote_ip.get(),
            multicast_addr,
            TTL,
            I::ICMP_IP_PROTO,
        ))
        .serialize_vec_outer(&mut NetworkSerializationContext::default())
        .unwrap();

    let mut builder = FakeCtxBuilder::with_addrs(I::TEST_ADDRS);
    let extra_index = builder.add_device_with_ip(
        I::TEST_ADDRS.local_mac,
        I::get_other_ip_address(20).get(),
        I::TEST_ADDRS.subnet,
    );
    // Add a neighbor entry for the extra device to get better errors in case
    // we're not going out the right device.
    builder.add_arp_or_ndp_table_entry(
        extra_index,
        I::TEST_ADDRS.remote_ip,
        I::TEST_ADDRS.remote_mac,
    );
    let (mut ctx, device_ids) = builder.build();

    let configured_device = &device_ids[0];
    let extra_device: DeviceId<_> = device_ids[extra_index].clone().into();

    // Add a route that would make the reply go out the extra device.
    ctx.test_api()
        .add_route(
            AddableEntry {
                subnet: Subnet::new(
                    I::TEST_ADDRS.remote_ip.get(),
                    <I::Addr as IpAddress>::BYTES * 8,
                )
                .unwrap(),
                device: extra_device,
                gateway: None,
                metric: AddableMetric::ExplicitMetric(RawMetric::HIGHEST_PREFERENCE),
                route_preference: Default::default(),
            }
            .into(),
        )
        .expect("add route");

    ctx.test_api().receive_ip_packet::<I, _>(
        &configured_device.clone().into(),
        Some(FrameDestination::Multicast),
        buffer,
    );

    let Ctx { core_ctx: _, bindings_ctx } = &mut ctx;
    let frames = bindings_ctx.take_ethernet_frames();
    let (dev, frame) = assert_matches!(&frames[..], [frame] => frame);
    let (src_mac, dst_mac, src_ip, dst_ip, _ttl, _message, _code) =
        parse_icmp_packet_in_ip_packet_in_ethernet_frame::<I, _, IcmpEchoReply, _>(
            &frame,
            EthernetFrameLengthCheck::NoCheck,
            |_echo| {},
        )
        .unwrap();

    assert_eq!(dev, configured_device);
    assert_eq!(src_mac, I::TEST_ADDRS.local_mac.get());
    assert_eq!(dst_mac, I::TEST_ADDRS.remote_mac.get());
    assert_eq!(src_ip, I::TEST_ADDRS.local_ip.get());
    assert_eq!(dst_ip, I::TEST_ADDRS.remote_ip.get());
}
