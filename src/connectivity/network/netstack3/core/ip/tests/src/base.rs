// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use alloc::vec;
use alloc::vec::Vec;
use assert_matches::assert_matches;
use core::fmt::Debug;
use core::num::{NonZeroU8, NonZeroU16};
use core::time::Duration;

use ip_test_macro::ip_test;
use net_declare::{net_ip_v4, net_ip_v6};
use net_types::ethernet::Mac;
use net_types::ip::{
    AddrSubnet, GenericOverIp, Ip, IpAddr, IpAddress, IpInvariant, IpVersion, Ipv4, Ipv4Addr, Ipv6,
    Ipv6Addr, Mtu, Subnet,
};
use net_types::{
    MulticastAddr, NonMappedAddr, SpecifiedAddr, UnicastAddr, Witness as _, ZonedAddr,
};
use packet::{
    Buf, InnerPacketBuilder, NestablePacketBuilder, NestableSerializer as _, PacketBuilder,
    ParseBuffer, ParseMetadata, Serializer as _,
};
use packet_formats::ethernet::{
    ETHERNET_MIN_BODY_LEN_NO_TAG, EthernetFrame, EthernetFrameBuilder, EthernetFrameLengthCheck,
    EthernetIpExt as _,
};
use packet_formats::icmp::{
    IcmpDestUnreachable, IcmpEchoRequest, IcmpPacketBuilder, IcmpParseArgs, IcmpZeroCode,
    Icmpv4DestUnreachableCode, Icmpv6Packet, Icmpv6PacketTooBig, Icmpv6ParameterProblemCode,
    MessageBody,
};
use packet_formats::ip::{
    FragmentOffset, IpPacket as _, IpPacketBuilder, IpProto, Ipv4Proto, Ipv6ExtHdrType, Ipv6Proto,
};
use packet_formats::ipv4::options::Ipv4Option;
use packet_formats::ipv4::{Ipv4PacketBuilder, Ipv4PacketBuilderWithOptions};
use packet_formats::ipv6::ext_hdrs::{
    ExtensionHeaderOptionAction, HopByHopOption, HopByHopOptionData,
};
use packet_formats::ipv6::{
    Ipv6PacketBuilder, Ipv6PacketBuilderBeforeFragment, Ipv6PacketBuilderWithFragmentHeader,
    Ipv6PacketBuilderWithHbhOptions,
};
use packet_formats::testutil::parse_icmp_packet_in_ip_packet_in_ethernet_frame;
use packet_formats::udp::UdpPacketBuilder;
use rand::RngExt as _;
use test_case::test_case;

use netstack3_base::testutil::{
    FakeInstant, TEST_ADDRS_V4, TEST_ADDRS_V6, TestAddrs, TestDualStackIpExt, TestIpExt, new_rng,
    set_logger_for_test,
};
use netstack3_base::{
    InstantContext as _, IpDeviceAddr, LocalFrameDestination, Marks, NetworkParsingContext,
    NetworkSerializationContext,
};
use netstack3_core::device::{
    DeviceId, EthernetCreationProperties, EthernetLinkDevice, MaxEthernetFrameSize,
    RecvEthernetFrameMeta,
};
use netstack3_core::filter::{
    Action, Hook, IpRoutines, NatRoutines, PacketMatcher, Routine, Routines, Rule, Tuple,
};
use netstack3_core::socket::{ListenerInfo, SocketInfo};
use netstack3_core::testutil::{
    Ctx, CtxPairExt as _, DEFAULT_INTERFACE_METRIC, FakeBindingsCtx, FakeCtx, FakeCtxBuilder,
    new_simple_fake_network,
};
use netstack3_core::{BindingsContext, CoreCtx, IpExt, StackState};
use netstack3_device::queue::{ReceiveQueueContext as _, ReceiveQueueHandler as _};
use netstack3_device::testutil::IPV6_MIN_IMPLIED_MAX_FRAME_SIZE;
use netstack3_filter::{FilterIpContext, FilterIpExt, TransportProtocol};
use netstack3_ip::device::{
    IpDeviceConfigurationUpdate, Ipv4DeviceConfigurationUpdate, Ipv6DeviceConfigurationUpdate,
    SlaacConfigurationUpdate, StableSlaacAddressConfiguration,
};
use netstack3_ip::multicast_forwarding::{
    MulticastRoute, MulticastRouteKey, MulticastRouteTarget, MulticastRouteTargets,
};
use netstack3_ip::socket::IpSocketContext;
use netstack3_ip::testutil::IpCounterExpectations;
use netstack3_ip::{
    self as ip, AddableEntryEither, AddableMetric, AddressStatus, Destination, DropReason,
    FragmentTimerId, FragmentationCounters, InternalForwarding, IpDeviceIngressStateContext,
    IpLayerTimerId, Ipv4PresentAddressStatus, Ipv6PresentAddressStatus, NextHop, RawMetric,
    ReceivePacketAction, ResolveRouteError, ResolvedRoute, RoutableIpAddr,
};

// Some helper functions

/// Verify that an ICMP Parameter Problem packet was actually sent in
/// response to a packet with an unrecognized IPv6 extension header option.
///
/// `verify_icmp_for_unrecognized_ext_hdr_option` verifies that the next
/// frame in `net` is an ICMP packet with code set to `code`, and pointer
/// set to `pointer`.
fn verify_icmp_for_unrecognized_ext_hdr_option(
    code: Icmpv6ParameterProblemCode,
    pointer: u32,
    packet: &[u8],
) {
    // Check the ICMP that bob attempted to send to alice
    let mut buffer = Buf::new(packet, ..);
    let _frame = buffer.parse_with::<_, EthernetFrame<_>>(EthernetFrameLengthCheck::Check).unwrap();
    let packet = buffer.parse::<<Ipv6 as packet_formats::ip::IpExt>::Packet<_>>().unwrap();
    let (src_ip, dst_ip, proto, _): (_, _, _, ParseMetadata) = packet.into_metadata();
    assert_eq!(dst_ip, TEST_ADDRS_V6.remote_ip.get());
    assert_eq!(src_ip, TEST_ADDRS_V6.local_ip.get());
    assert_eq!(proto, Ipv6Proto::Icmpv6);
    let icmp = buffer.parse_with::<_, Icmpv6Packet<_>>(IcmpParseArgs::new(src_ip, dst_ip)).unwrap();
    if let Icmpv6Packet::ParameterProblem(icmp) = icmp {
        assert_eq!(icmp.code(), code);
        assert_eq!(icmp.message().pointer(), pointer);
    } else {
        panic!("Expected ICMPv6 Parameter Problem: {:?}", icmp);
    }
}

/// Populate a buffer `bytes` with data required to test unrecognized
/// options.
///
/// The unrecognized option type will be located at index 48. `bytes` must
/// be at least 64 bytes long. If `to_multicast` is `true`, the destination
/// address of the packet will be a multicast address.
fn buf_for_unrecognized_ext_hdr_option_test(
    bytes: &mut [u8],
    action: ExtensionHeaderOptionAction,
    to_multicast: bool,
) -> Buf<&mut [u8]> {
    assert!(bytes.len() >= 64);

    let action: u8 = action.into();

    // Unrecognized Option type.
    let oty = 63 | (action << 6);

    #[rustfmt::skip]
    bytes[40..64].copy_from_slice(&[
        // Destination Options Extension Header
        IpProto::Udp.into(),      // Next Header
        1,                        // Hdr Ext Len (In 8-octet units, not including first 8 octets)
        0,                        // Pad1
        1,   0,                   // Pad2
        1,   1, 0,                // Pad3
        oty, 6, 0, 0, 0, 0, 0, 0, // Unrecognized type w/ action = discard

        // Body
        1, 2, 3, 4, 5, 6, 7, 8
    ][..]);
    bytes[..4].copy_from_slice(&[0x60, 0x20, 0x00, 0x77][..]);

    let payload_len = u16::try_from(bytes.len() - 40).unwrap();
    bytes[4..6].copy_from_slice(&payload_len.to_be_bytes());

    bytes[6] = Ipv6ExtHdrType::DestinationOptions.into();
    bytes[7] = 64;
    bytes[8..24].copy_from_slice(TEST_ADDRS_V6.remote_ip.bytes());

    if to_multicast {
        bytes[24..40].copy_from_slice(
            &[255, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32][..],
        );
    } else {
        bytes[24..40].copy_from_slice(TEST_ADDRS_V6.local_ip.bytes());
    }

    Buf::new(bytes, ..)
}

/// Create an IPv4 packet builder.
fn get_ipv4_builder() -> Ipv4PacketBuilder {
    Ipv4PacketBuilder::new(TEST_ADDRS_V4.remote_ip, TEST_ADDRS_V4.local_ip, 10, IpProto::Udp.into())
}

/// Process an IP fragment depending on the `Ip` `process_ip_fragment` is
/// specialized with.
fn process_ip_fragment<I: Ip>(
    ctx: &mut FakeCtx,
    device: &DeviceId<FakeBindingsCtx>,
    fragment_id: u16,
    fragment_offset: u8,
    fragment_count: u8,
    with_header_options: bool,
) {
    match I::VERSION {
        IpVersion::V4 => process_ipv4_fragment(
            ctx,
            device,
            fragment_id,
            fragment_offset,
            fragment_count,
            with_header_options,
        ),
        IpVersion::V6 => process_ipv6_fragment(
            ctx,
            device,
            fragment_id,
            fragment_offset,
            fragment_count,
            with_header_options,
        ),
    }
}

fn wrap_body<B: PacketBuilder<NetworkSerializationContext> + Debug>(
    builder: B,
    body: Vec<u8>,
) -> Buf<Vec<u8>> {
    builder
        .wrap_body(Buf::new(body, ..))
        .serialize_vec_outer(&mut NetworkSerializationContext::default())
        .unwrap()
        .into_inner()
}

/// Generate and 'receive' an IPv4 fragment packet.
///
/// `fragment_offset` is the fragment offset. `fragment_count` is the number
/// of fragments for a packet. `include_ipv4_option` indicates whether an IPv4
/// Router Alert option should be included in the packet. The generated packet
/// will have a body of size 8 bytes.
fn process_ipv4_fragment(
    ctx: &mut FakeCtx,
    device: &DeviceId<FakeBindingsCtx>,
    fragment_id: u16,
    fragment_offset: u8,
    fragment_count: u8,
    include_ipv4_option: bool,
) {
    assert!(fragment_offset < fragment_count);

    let m_flag = fragment_offset < (fragment_count - 1);

    let mut builder = get_ipv4_builder();
    builder.id(fragment_id);
    builder.fragment_offset(FragmentOffset::new(fragment_offset.into()).unwrap());
    builder.mf_flag(m_flag);
    let mut body: Vec<u8> = Vec::new();
    body.extend(fragment_offset * 8..fragment_offset * 8 + 8);

    let buffer = if include_ipv4_option {
        let builder =
            Ipv4PacketBuilderWithOptions::new(builder, vec![Ipv4Option::RouterAlert { data: 55 }])
                .expect("Single Router Alert option will fit");
        wrap_body(builder, body)
    } else {
        wrap_body(builder, body)
    };

    ctx.test_api().receive_ip_packet::<Ipv4, _>(
        device,
        Some(LocalFrameDestination::Individual { local: () }),
        buffer,
    );
}

/// Generate and 'receive' an IPv6 fragment packet.
///
/// `fragment_offset` is the fragment offset. `fragment_count` is the number
/// of fragments for a packet. `include_extension_header` indicates whether an
/// IPv6 HopByHop extension header will be included in the packet. The generated
/// packet will have a body of size 8 bytes.
fn process_ipv6_fragment(
    ctx: &mut FakeCtx,
    device: &DeviceId<FakeBindingsCtx>,
    fragment_id: u16,
    fragment_offset: u8,
    fragment_count: u8,
    include_extension_header: bool,
) {
    let builder = Ipv6PacketBuilder::new(
        TEST_ADDRS_V6.remote_ip,
        TEST_ADDRS_V6.local_ip,
        64,
        Ipv6Proto::Proto(IpProto::Udp),
    );

    let mut body: Vec<u8> = Vec::new();
    body.extend(fragment_offset * 8..fragment_offset * 8 + 8);

    fn wrap_fragment_header<B: Ipv6PacketBuilderBeforeFragment>(
        builder: B,
        fragment_id: u16,
        fragment_offset: u8,
        fragment_count: u8,
    ) -> Ipv6PacketBuilderWithFragmentHeader<B> {
        assert!(fragment_offset < fragment_count);
        let m_flag = fragment_offset < (fragment_count - 1);
        Ipv6PacketBuilderWithFragmentHeader::new(
            builder,
            FragmentOffset::new(fragment_offset.into()).unwrap(),
            m_flag,
            fragment_id.into(),
        )
    }

    let buffer = if include_extension_header {
        let builder = Ipv6PacketBuilderWithHbhOptions::new(
            builder,
            [HopByHopOption {
                action: ExtensionHeaderOptionAction::SkipAndContinue,
                mutable: false,
                data: HopByHopOptionData::RouterAlert { data: 55 },
            }],
        )
        .expect("single Router Alert option will fit");
        wrap_body(wrap_fragment_header(builder, fragment_id, fragment_offset, fragment_count), body)
    } else {
        wrap_body(wrap_fragment_header(builder, fragment_id, fragment_offset, fragment_count), body)
    };

    ctx.test_api().receive_ip_packet::<Ipv6, _>(
        device,
        Some(LocalFrameDestination::Individual { local: () }),
        buffer,
    );
}

#[test]
fn test_ipv6_icmp_parameter_problem_non_must() {
    let (mut ctx, device_ids) = FakeCtxBuilder::with_addrs(TEST_ADDRS_V6).build();
    let device: DeviceId<_> = device_ids[0].clone().into();

    // Test parsing an IPv6 packet with invalid next header value which
    // we SHOULD send an ICMP response for (but we don't since its not a
    // MUST).

    #[rustfmt::skip]
    let bytes: &mut [u8] = &mut [
        // FixedHeader (will be replaced later)
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,

        // Body
        1, 2, 3, 4, 5,
    ][..];
    bytes[..4].copy_from_slice(&[0x60, 0x20, 0x00, 0x77][..]);
    let payload_len = u16::try_from(bytes.len() - 40).unwrap();
    bytes[4..6].copy_from_slice(&payload_len.to_be_bytes());
    bytes[6] = 255; // Invalid Next Header
    bytes[7] = 64;
    bytes[8..24].copy_from_slice(TEST_ADDRS_V6.remote_ip.bytes());
    bytes[24..40].copy_from_slice(TEST_ADDRS_V6.local_ip.bytes());
    let buf = Buf::new(bytes, ..);

    ctx.test_api().receive_ip_packet::<Ipv6, _>(
        &device,
        Some(LocalFrameDestination::Individual { local: () }),
        buf,
    );

    assert_eq!(
        ctx.core_ctx
            .ipv6()
            .icmp
            .inner
            .tx_counters
            .parameter_problem
            .unrecognized_next_header_type
            .get(),
        0
    );
    IpCounterExpectations::<Ipv6> {
        dispatch_receive_ip_packet: 0,
        receive_ip_packet: 1,
        ..Default::default()
    }
    .assert_counters(&ctx.core_ctx(), &device);
}

#[test]
fn test_ipv6_icmp_parameter_problem_must() {
    let (mut ctx, device_ids) = FakeCtxBuilder::with_addrs(TEST_ADDRS_V6).build();
    let device: DeviceId<_> = device_ids[0].clone().into();

    // Test parsing an IPv6 packet where we MUST send an ICMP parameter problem
    // response (invalid routing type for a routing extension header).

    #[rustfmt::skip]
    let bytes: &mut [u8] = &mut [
        // FixedHeader (will be replaced later)
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,

        // Routing Extension Header
        IpProto::Udp.into(),  // Next Header
        4,                    // Hdr Ext Len (In 8-octet units, not including first 8 octets)
        255,                  // Routing Type (Invalid)
        1,                    // Segments Left
        0, 0, 0, 0,           // Reserved
        // Addresses for Routing Header w/ Type 0
        0,  1,  2,  3,  4,  5,  6,  7,  8,  9,  10, 11, 12, 13, 14, 15,
        16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,

        // Body
        1, 2, 3, 4, 5,
    ][..];
    bytes[..4].copy_from_slice(&[0x60, 0x20, 0x00, 0x77][..]);
    let payload_len = u16::try_from(bytes.len() - 40).unwrap();
    bytes[4..6].copy_from_slice(&payload_len.to_be_bytes());
    bytes[6] = Ipv6ExtHdrType::Routing.into();
    bytes[7] = 64;
    bytes[8..24].copy_from_slice(TEST_ADDRS_V6.remote_ip.bytes());
    bytes[24..40].copy_from_slice(TEST_ADDRS_V6.local_ip.bytes());
    let buf = Buf::new(bytes, ..);
    ctx.test_api().receive_ip_packet::<Ipv6, _>(
        &device,
        Some(LocalFrameDestination::Individual { local: () }),
        buf,
    );
    let frames = ctx.bindings_ctx.take_ethernet_frames();
    let (_dev, frame) = assert_matches!(&frames[..], [frame] => frame);
    verify_icmp_for_unrecognized_ext_hdr_option(
        Icmpv6ParameterProblemCode::ErroneousHeaderField,
        42,
        &frame[..],
    );
}

// Verify that no ICMP packets are sent in response to an ICMP error packet.
#[test]
fn test_ipv6_no_icmp_error_response() {
    let (mut ctx, device_ids) = FakeCtxBuilder::with_addrs(TEST_ADDRS_V6).build();
    let device: DeviceId<_> = device_ids[0].clone().into();

    #[rustfmt::skip]
    let bytes: &mut [u8] = &mut [
        // FixedHeader (will be replaced later)
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,

        // Routing Extension Header
        Ipv6Proto::Icmpv6.into(),  // Next Header
        4,                         // Hdr Ext Len (In 8-octet units, not including first 8 octets)
        255,                       // Routing Type (Invalid)
        1,                         // Segments Left
        0, 0, 0, 0,                // Reserved
        // Addresses for Routing Header w/ Type 0
        0,  1,  2,  3,  4,  5,  6,  7,  8,  9,  10, 11, 12, 13, 14, 15,
        16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,

        // ICMPv6: Address unreachable.
        1, 3, 0, 0, 0,
    ][..];
    bytes[..4].copy_from_slice(&[0x60, 0x20, 0x00, 0x77][..]);
    let payload_len = u16::try_from(bytes.len() - 40).unwrap();
    bytes[4..6].copy_from_slice(&payload_len.to_be_bytes());
    bytes[6] = Ipv6ExtHdrType::Routing.into();
    bytes[7] = 64;
    bytes[8..24].copy_from_slice(TEST_ADDRS_V6.remote_ip.bytes());
    bytes[24..40].copy_from_slice(TEST_ADDRS_V6.local_ip.bytes());
    let buf = Buf::new(bytes, ..);
    ctx.test_api().receive_ip_packet::<Ipv6, _>(
        &device,
        Some(LocalFrameDestination::Individual { local: () }),
        buf,
    );
    let frames = ctx.bindings_ctx.take_ethernet_frames();
    assert_eq!(frames.len(), 0);
}

#[test]
fn test_ipv6_unrecognized_ext_hdr_option() {
    let (mut ctx, device_ids) = FakeCtxBuilder::with_addrs(TEST_ADDRS_V6).build();
    let device: DeviceId<_> = device_ids[0].clone().into();
    let mut expected_icmps = 0;
    let mut bytes = [0; 64];
    let frame_dst = LocalFrameDestination::Individual { local: () };

    // Test parsing an IPv6 packet where we MUST send an ICMP parameter
    // problem due to an unrecognized extension header option.

    // Test with unrecognized option type set with action = skip & continue.

    let buf = buf_for_unrecognized_ext_hdr_option_test(
        &mut bytes,
        ExtensionHeaderOptionAction::SkipAndContinue,
        false,
    );
    ctx.test_api().receive_ip_packet::<Ipv6, _>(&device, Some(frame_dst), buf);
    assert_eq!(
        ctx.core_ctx
            .common_icmp::<Ipv6>()
            .tx_counters
            .parameter_problem
            .unrecognized_ipv6_option
            .get(),
        expected_icmps
    );
    IpCounterExpectations::<Ipv6>::expect_dispatched(1).assert_counters(&ctx.core_ctx(), &device);
    assert_matches!(ctx.bindings_ctx.take_ethernet_frames()[..], []);

    // Test with unrecognized option type set with
    // action = discard.

    let buf = buf_for_unrecognized_ext_hdr_option_test(
        &mut bytes,
        ExtensionHeaderOptionAction::DiscardPacket,
        false,
    );
    ctx.test_api().receive_ip_packet::<Ipv6, _>(&device, Some(frame_dst), buf);
    assert_eq!(
        ctx.core_ctx
            .common_icmp::<Ipv6>()
            .tx_counters
            .parameter_problem
            .unrecognized_ipv6_option
            .get(),
        expected_icmps
    );
    assert_matches!(ctx.bindings_ctx.take_ethernet_frames()[..], []);

    // Test with unrecognized option type set with
    // action = discard & send icmp
    // where dest addr is a unicast addr.

    let buf = buf_for_unrecognized_ext_hdr_option_test(
        &mut bytes,
        ExtensionHeaderOptionAction::DiscardPacketSendIcmp,
        false,
    );
    ctx.test_api().receive_ip_packet::<Ipv6, _>(&device, Some(frame_dst), buf);
    expected_icmps += 1;
    assert_eq!(
        ctx.core_ctx
            .common_icmp::<Ipv6>()
            .tx_counters
            .parameter_problem
            .unrecognized_ipv6_option
            .get(),
        expected_icmps
    );
    let frames = ctx.bindings_ctx.take_ethernet_frames();
    let (_dev, frame) = assert_matches!(&frames[..], [frame] => frame);
    verify_icmp_for_unrecognized_ext_hdr_option(
        Icmpv6ParameterProblemCode::UnrecognizedIpv6Option,
        48,
        &frame[..],
    );

    // Test with unrecognized option type set with
    // action = discard & send icmp
    // where dest addr is a multicast addr.

    let buf = buf_for_unrecognized_ext_hdr_option_test(
        &mut bytes,
        ExtensionHeaderOptionAction::DiscardPacketSendIcmp,
        true,
    );
    ctx.test_api().receive_ip_packet::<Ipv6, _>(&device, Some(frame_dst), buf);
    expected_icmps += 1;
    assert_eq!(
        ctx.core_ctx
            .common_icmp::<Ipv6>()
            .tx_counters
            .parameter_problem
            .unrecognized_ipv6_option
            .get(),
        expected_icmps
    );

    let frames = ctx.bindings_ctx.take_ethernet_frames();
    let (_dev, frame) = assert_matches!(&frames[..], [frame] => frame);
    verify_icmp_for_unrecognized_ext_hdr_option(
        Icmpv6ParameterProblemCode::UnrecognizedIpv6Option,
        48,
        &frame[..],
    );

    // Test with unrecognized option type set with
    // action = discard & send icmp if not multicast addr
    // where dest addr is a unicast addr.

    let buf = buf_for_unrecognized_ext_hdr_option_test(
        &mut bytes,
        ExtensionHeaderOptionAction::DiscardPacketSendIcmpNoMulticast,
        false,
    );
    ctx.test_api().receive_ip_packet::<Ipv6, _>(&device, Some(frame_dst), buf);
    expected_icmps += 1;
    assert_eq!(
        ctx.core_ctx
            .common_icmp::<Ipv6>()
            .tx_counters
            .parameter_problem
            .unrecognized_ipv6_option
            .get(),
        expected_icmps
    );

    let frames = ctx.bindings_ctx.take_ethernet_frames();
    let (_dev, frame) = assert_matches!(&frames[..], [frame] => frame);
    verify_icmp_for_unrecognized_ext_hdr_option(
        Icmpv6ParameterProblemCode::UnrecognizedIpv6Option,
        48,
        &frame[..],
    );

    // Test with unrecognized option type set with
    // action = discard & send icmp if not multicast addr
    // but dest addr is a multicast addr.

    let buf = buf_for_unrecognized_ext_hdr_option_test(
        &mut bytes,
        ExtensionHeaderOptionAction::DiscardPacketSendIcmpNoMulticast,
        true,
    );
    // Do not expect an ICMP response for this packet
    ctx.test_api().receive_ip_packet::<Ipv6, _>(&device, Some(frame_dst), buf);
    assert_eq!(
        ctx.core_ctx
            .common_icmp::<Ipv6>()
            .tx_counters
            .parameter_problem
            .unrecognized_ipv6_option
            .get(),
        expected_icmps
    );
    assert_matches!(ctx.bindings_ctx.take_ethernet_frames()[..], []);

    // None of our tests should have dispatched an IP packet after the first.
    IpCounterExpectations::<Ipv6> {
        receive_ip_packet: 6,
        dispatch_receive_ip_packet: 1,
        deliver_unicast: 1,
        parameter_problem: expected_icmps,
        send_ip_packet: expected_icmps,
        ..Default::default()
    }
    .assert_counters(&ctx.core_ctx(), &device);
}

#[ip_test(I)]
fn test_ip_packet_reassembly_not_needed<I: TestIpExt + IpExt>() {
    let (mut ctx, device_ids) = FakeCtxBuilder::with_addrs(I::TEST_ADDRS).build();
    let device: DeviceId<_> = device_ids[0].clone().into();
    let fragment_id = 5;

    IpCounterExpectations::<I>::default().assert_counters(&ctx.core_ctx(), &device);

    // Test that a non fragmented packet gets dispatched right away.

    process_ip_fragment::<I>(&mut ctx, &device, fragment_id, 0, 1, false);

    // Make sure the packet got dispatched.
    IpCounterExpectations::<I>::expect_dispatched(1).assert_counters(&ctx.core_ctx(), &device);
}

#[ip_test(I)]
#[test_case(true; "with_header_options")]
#[test_case(false; "without_header_options")]
fn test_ip_packet_reassembly<I: TestIpExt + IpExt>(with_header_options: bool) {
    let (mut ctx, device_ids) = FakeCtxBuilder::with_addrs(I::TEST_ADDRS).build();
    let device: DeviceId<_> = device_ids[0].clone().into();
    let fragment_id = 5;

    // Test that the received packet gets dispatched only after receiving
    // all the fragments.

    // Process fragment #0
    process_ip_fragment::<I>(&mut ctx, &device, fragment_id, 0, 3, with_header_options);

    // Process fragment #1
    process_ip_fragment::<I>(&mut ctx, &device, fragment_id, 1, 3, with_header_options);

    // Make sure no packets got dispatched yet.
    IpCounterExpectations::<I> {
        dispatch_receive_ip_packet: 0,
        receive_ip_packet: 2,
        need_more_fragments: 2,
        ..Default::default()
    }
    .assert_counters(&ctx.core_ctx(), &device);

    // Process fragment #2
    process_ip_fragment::<I>(&mut ctx, &device, fragment_id, 2, 3, with_header_options);

    // Make sure the packet finally got dispatched now that the final
    // fragment has been 'received'.
    IpCounterExpectations::<I> {
        receive_ip_packet: 3,
        need_more_fragments: 2,
        dispatch_receive_ip_packet: 1,
        deliver_unicast: 1,
        ..Default::default()
    }
    .assert_counters(&ctx.core_ctx(), &device);
}

#[ip_test(I)]
#[test_case(true; "with_header_options")]
#[test_case(false; "without_header_options")]
fn test_ip_packet_reassembly_with_packets_arriving_out_of_order<I: TestIpExt + IpExt>(
    with_header_options: bool,
) {
    let (mut ctx, device_ids) = FakeCtxBuilder::with_addrs(I::TEST_ADDRS).build();
    let device: DeviceId<_> = device_ids[0].clone().into();
    let fragment_id_0 = 5;
    let fragment_id_1 = 10;
    let fragment_id_2 = 15;

    // Test that received packets gets dispatched only after receiving all
    // the fragments with out of order arrival of fragments.

    // Process packet #0, fragment #1
    process_ip_fragment::<I>(&mut ctx, &device, fragment_id_0, 1, 3, with_header_options);

    // Process packet #1, fragment #2
    process_ip_fragment::<I>(&mut ctx, &device, fragment_id_1, 2, 3, with_header_options);

    // Process packet #1, fragment #0
    process_ip_fragment::<I>(&mut ctx, &device, fragment_id_1, 0, 3, with_header_options);

    // Make sure no packets got dispatched yet.
    IpCounterExpectations::<I> {
        dispatch_receive_ip_packet: 0,
        receive_ip_packet: 3,
        need_more_fragments: 3,
        ..Default::default()
    }
    .assert_counters(&ctx.core_ctx(), &device);

    // Process a packet that does not require reassembly (packet #2, fragment #0).
    process_ip_fragment::<I>(&mut ctx, &device, fragment_id_2, 0, 1, with_header_options);

    // Make packet #1 got dispatched since it didn't need reassembly.
    IpCounterExpectations::<I> {
        receive_ip_packet: 4,
        need_more_fragments: 3,
        dispatch_receive_ip_packet: 1,
        deliver_unicast: 1,
        ..Default::default()
    }
    .assert_counters(&ctx.core_ctx(), &device);

    // Process packet #0, fragment #2
    process_ip_fragment::<I>(&mut ctx, &device, fragment_id_0, 2, 3, with_header_options);

    // Make sure no other packets got dispatched yet.
    IpCounterExpectations::<I> {
        receive_ip_packet: 5,
        need_more_fragments: 4,
        dispatch_receive_ip_packet: 1,
        deliver_unicast: 1,
        ..Default::default()
    }
    .assert_counters(&ctx.core_ctx(), &device);

    // Process packet #0, fragment #0
    process_ip_fragment::<I>(&mut ctx, &device, fragment_id_0, 0, 3, with_header_options);

    // Make sure that packet #0 finally got dispatched now that the final
    // fragment has been 'received'.
    IpCounterExpectations::<I> {
        receive_ip_packet: 6,
        need_more_fragments: 4,
        dispatch_receive_ip_packet: 2,
        deliver_unicast: 2,
        ..Default::default()
    }
    .assert_counters(&ctx.core_ctx(), &device);

    // Process packet #1, fragment #1
    process_ip_fragment::<I>(&mut ctx, &device, fragment_id_1, 1, 3, with_header_options);

    // Make sure the packet finally got dispatched now that the final
    // fragment has been 'received'.
    IpCounterExpectations::<I> {
        receive_ip_packet: 7,
        need_more_fragments: 4,
        dispatch_receive_ip_packet: 3,
        deliver_unicast: 3,
        ..Default::default()
    }
    .assert_counters(&ctx.core_ctx(), &device);
}

#[ip_test(I)]
fn test_ip_packet_reassembly_timer<I: TestIpExt + IpExt>()
where
    IpLayerTimerId: From<FragmentTimerId<I>>,
{
    let (mut ctx, device_ids) = FakeCtxBuilder::with_addrs(I::TEST_ADDRS).build();
    let device: DeviceId<_> = device_ids[0].clone().into();
    let fragment_id = 5;

    // Test to make sure that packets must arrive within the reassembly
    // timer.

    // Process fragment #0
    process_ip_fragment::<I>(&mut ctx, &device, fragment_id, 0, 3, false);

    // Make sure a timer got added.
    ctx.bindings_ctx.timer_ctx().assert_timers_installed_range([(
        IpLayerTimerId::from(FragmentTimerId::<I>::default()).into(),
        ..,
    )]);

    // Process fragment #1
    process_ip_fragment::<I>(&mut ctx, &device, fragment_id, 1, 3, false);

    assert_eq!(
        ctx.trigger_next_timer().unwrap(),
        IpLayerTimerId::from(FragmentTimerId::<I>::default()).into(),
    );

    // Make sure no other timers exist.
    ctx.bindings_ctx.timer_ctx().assert_no_timers_installed();

    // Process fragment #2
    process_ip_fragment::<I>(&mut ctx, &device, fragment_id, 2, 3, false);

    // Make sure no packets got dispatched yet since even though we
    // technically received all the fragments, this fragment (#2) arrived
    // too late and the reassembly timer was triggered, causing the prior
    // fragment data to be discarded.
    IpCounterExpectations::<I> {
        dispatch_receive_ip_packet: 0,
        receive_ip_packet: 3,
        need_more_fragments: 3,
        ..Default::default()
    }
    .assert_counters(&ctx.core_ctx(), &device);
}

// TODO(https://fxbug.dev/345814518): Once we don't reassemble on forwarding,
// this test should ensure that we forward fragments without touching them.
#[netstack3_macros::context_ip_bounds(I, FakeBindingsCtx)]
#[ip_test(I)]
fn test_ip_reassembly_when_forwarding<I: TestIpExt + IpExt>() {
    // Create a new network with two parties (alice & bob) and enable IP
    // packet routing for alice.
    let a = "alice";
    let b = "bob";
    let fake_config = I::TEST_ADDRS;
    let (mut alice, alice_device_ids) = FakeCtxBuilder::with_addrs(fake_config.swap()).build();
    {
        alice
            .test_api()
            .set_unicast_forwarding_enabled::<I>(&alice_device_ids[0].clone().into(), true);
    }
    let (bob, bob_device_ids) = FakeCtxBuilder::with_addrs(fake_config).build();
    let mut net = new_simple_fake_network(
        a,
        alice,
        alice_device_ids[0].downgrade(),
        b,
        bob,
        bob_device_ids[0].downgrade(),
    );
    // Make sure the (strongly referenced) device IDs are dropped before
    // `net`.
    let alice_device_id: DeviceId<_> = alice_device_ids[0].clone().into();
    let bob_device_id: DeviceId<_> = bob_device_ids[0].clone().into();
    core::mem::drop((alice_device_ids, bob_device_ids));

    let fragment_id = 5;

    // Test that forwarded packets are reassembled at the router before being
    // received by the destination. In this test, Alice is receiving packets
    // from some source that is actually destined for Bob.

    // Process fragment #0
    net.with_context("alice", |ctx| {
        process_ip_fragment::<I>(ctx, &alice_device_id, fragment_id, 0, 3, false);
    });
    // Make sure the fragment was not sent from alice to bob.
    IpCounterExpectations::<I> {
        receive_ip_packet: 1,
        need_more_fragments: 1,
        ..Default::default()
    }
    .assert_counters(&net.context("alice").core_ctx(), &alice_device_id);

    // Process fragment #1
    net.with_context("alice", |ctx| {
        process_ip_fragment::<I>(ctx, &alice_device_id, fragment_id, 1, 3, false);
    });
    IpCounterExpectations::<I> {
        receive_ip_packet: 2,
        need_more_fragments: 2,
        ..Default::default()
    }
    .assert_counters(&net.context("alice").core_ctx(), &alice_device_id);

    // Make sure Bob hasn't received the packet.
    IpCounterExpectations::<I>::default()
        .assert_counters(&net.context("bob").core_ctx(), &bob_device_id);

    // Process fragment #2
    net.with_context("alice", |ctx| {
        process_ip_fragment::<I>(ctx, &alice_device_id, fragment_id, 2, 3, false);
    });
    // Now that the last fragment has arrived, Alice should have forwarded the
    // reassembled packet by re-fragmenting it.
    assert!(!net.step().is_idle());
    IpCounterExpectations::<I> {
        receive_ip_packet: 3,
        need_more_fragments: 2,
        forward: 1,
        fragmentation: FragmentationCounters {
            fragmentation_required: 1,
            fragments: 3,
            ..Default::default()
        },
        ..Default::default()
    }
    .assert_counters(&net.context("alice").core_ctx(), &alice_device_id);

    // Make sure the packet finally got dispatched once Bob received all the
    // fragments and reassembled them.
    IpCounterExpectations::<I> {
        receive_ip_packet: 3,
        need_more_fragments: 2,
        ..IpCounterExpectations::expect_dispatched(1)
    }
    .assert_counters(&net.context("bob").core_ctx(), &bob_device_id);

    // Make sure there are no more events.
    assert!(net.step().is_idle());
}

enum WhichLinkLocalAddr {
    Source,
    Destination,
    Both,
}

#[test_case(WhichLinkLocalAddr::Source)]
#[test_case(WhichLinkLocalAddr::Destination)]
#[test_case(WhichLinkLocalAddr::Both)]
fn drop_ipv6_link_local_packets(which_link_local_addr: WhichLinkLocalAddr) {
    // Test that NS3 won't forward an IPv6 packet if it has a link-local source
    // or destination address.
    //
    // There's only one netstack here, but we're pretending it's on a network
    // where the netstack is acting as a router to some wider network:
    //
    // <client> - <netstack> - <server>
    //
    // In terms of the names of the IP addresses, this is:
    //
    // <extra> - <local> - <remote>
    //
    // In particular, this test fakes packets from either the client or server
    // that have link-local addresses as either the source or destination, in
    // which case we expect the netstack to just drop them.

    let fake_config = Ipv6::TEST_ADDRS;
    let mut dispatcher_builder = FakeCtxBuilder::with_addrs(fake_config.clone());
    let extra_ip = UnicastAddr::new(Ipv6::get_other_ip_address(7).get()).unwrap();
    let extra_mac = UnicastAddr::new(Mac::new([12, 13, 14, 15, 16, 17])).unwrap();
    // This is done for the other IPs in FakeCtxBuilder::with_addrs
    dispatcher_builder.add_ndp_table_entry(0, extra_ip, extra_mac);
    dispatcher_builder.add_ndp_table_entry(
        0,
        extra_mac.to_ipv6_link_local().addr().get(),
        extra_mac,
    );
    let (mut ctx, device_ids) = dispatcher_builder.build();

    let device: DeviceId<_> = device_ids[0].clone().into();
    ctx.test_api().set_unicast_forwarding_enabled::<Ipv6>(&device, true);
    let frame_dst = LocalFrameDestination::Individual { local: () };

    let (src_addr, dst_addr): (Ipv6Addr, Ipv6Addr) = match which_link_local_addr {
        WhichLinkLocalAddr::Source => {
            (**extra_mac.to_ipv6_link_local().addr(), *fake_config.remote_ip)
        }
        WhichLinkLocalAddr::Destination => {
            (extra_ip.into_addr(), **fake_config.remote_mac.to_ipv6_link_local().addr())
        }
        WhichLinkLocalAddr::Both => (
            **extra_mac.to_ipv6_link_local().addr(),
            **fake_config.remote_mac.to_ipv6_link_local().addr(),
        ),
    };

    let ipv6_packet_buf = Buf::new(vec![0; 128], ..)
        .wrap_in(Ipv6PacketBuilder::new(src_addr, dst_addr, 64, IpProto::Udp.into()))
        .serialize_vec_outer(&mut NetworkSerializationContext::default())
        .unwrap();
    // Receive the IP packet.
    ctx.test_api().receive_ip_packet::<Ipv6, _>(&device, Some(frame_dst), ipv6_packet_buf);

    IpCounterExpectations::<Ipv6> {
        receive_ip_packet: 1,
        dispatch_receive_ip_packet: 0,
        forward: 0,
        dropped: 1,
        send_ip_packet: 0,
        ..Default::default()
    }
    .assert_counters(&ctx.core_ctx(), &device);
}

#[test]
fn test_ipv6_packet_too_big() {
    // Test sending an IPv6 Packet Too Big Error when receiving a packet
    // that is too big to be forwarded when it isn't destined for the node
    // it arrived at.

    let fake_config = Ipv6::TEST_ADDRS;
    let mut dispatcher_builder = FakeCtxBuilder::with_addrs(fake_config.clone());
    let extra_ip = UnicastAddr::new(Ipv6::get_other_ip_address(7).get()).unwrap();
    let extra_mac = UnicastAddr::new(Mac::new([12, 13, 14, 15, 16, 17])).unwrap();
    dispatcher_builder.add_ndp_table_entry(0, extra_ip, extra_mac);
    dispatcher_builder.add_ndp_table_entry(
        0,
        extra_mac.to_ipv6_link_local().addr().get(),
        extra_mac,
    );
    let (mut ctx, device_ids) = dispatcher_builder.build();

    let device: DeviceId<_> = device_ids[0].clone().into();
    ctx.test_api().set_unicast_forwarding_enabled::<Ipv6>(&device, true);
    let frame_dst = LocalFrameDestination::Individual { local: () };

    // Construct an IPv6 packet that is too big for our MTU (MTU = 1280;
    // body itself is 5000). Note, the final packet will be larger because
    // of IP header data.
    let mut rng = new_rng(70812476915813);
    let body: Vec<u8> = core::iter::repeat_with(|| rng.random()).take(5000).collect();

    // Ip packet from some node destined to a remote on this network,
    // arriving locally.
    let mut ipv6_packet_buf = Buf::new(body, ..)
        .wrap_in(Ipv6PacketBuilder::new(extra_ip, fake_config.remote_ip, 64, IpProto::Udp.into()))
        .serialize_vec_outer(&mut NetworkSerializationContext::default())
        .unwrap();
    // Receive the IP packet.
    ctx.test_api().receive_ip_packet::<Ipv6, _>(&device, Some(frame_dst), ipv6_packet_buf.clone());

    // Should not have dispatched the packet.
    IpCounterExpectations::<Ipv6> {
        dispatch_receive_ip_packet: 0,
        receive_ip_packet: 1,
        forward: 1,
        send_ip_packet: 1,
        mtu_exceeded: 1,
        fragmentation: FragmentationCounters {
            fragmentation_required: 1,
            error_not_allowed: 1,
            ..Default::default()
        },
        ..Default::default()
    }
    .assert_counters(&ctx.core_ctx(), &device);
    let Ctx { core_ctx, bindings_ctx } = &mut ctx;
    assert_eq!(core_ctx.common_icmp::<Ipv6>().tx_counters.packet_too_big.get(), 1);

    // Should have sent out one frame, and the received packet should be a
    // Packet Too Big ICMP error message.
    let frames = bindings_ctx.take_ethernet_frames();
    let (_dev, frame) = assert_matches!(&frames[..], [frame] => frame);
    // The original packet's TTL gets decremented so we decrement here
    // to validate the rest of the icmp message body.
    let ipv6_packet_buf_mut: &mut [u8] = ipv6_packet_buf.as_mut();
    ipv6_packet_buf_mut[7] -= 1;
    let (_, _, _, _, _, message, code) =
        parse_icmp_packet_in_ip_packet_in_ethernet_frame::<Ipv6, _, Icmpv6PacketTooBig, _>(
            &frame[..],
            EthernetFrameLengthCheck::NoCheck,
            move |packet| {
                // Size of the ICMP message body should be size of the
                // MTU without IP and ICMP headers.
                let expected_len = 1280 - 48;
                let actual_body: &[u8] = ipv6_packet_buf.as_ref();
                let actual_body = &actual_body[..expected_len];
                assert_eq!(packet.body().len(), expected_len);
                let (inner_header, inner_body) = packet.body().bytes();
                assert!(inner_body.is_none());
                assert_eq!(inner_header, actual_body);
            },
        )
        .unwrap();
    assert_eq!(code, IcmpZeroCode);
    // MTU should match the MTU for the link.
    assert_eq!(message, Icmpv6PacketTooBig::new(1280));
}

fn create_packet_too_big_buf<A: IpAddress>(
    src_ip: A,
    dst_ip: A,
    mtu: u16,
    body: Option<Buf<Vec<u8>>>,
) -> Buf<Vec<u8>> {
    let body = body.unwrap_or_else(|| Buf::new(Vec::new(), ..));

    match [src_ip, dst_ip].into() {
        IpAddr::V4([src_ip, dst_ip]) => body
            .wrap_in(IcmpPacketBuilder::<Ipv4, IcmpDestUnreachable>::new(
                dst_ip,
                src_ip,
                Icmpv4DestUnreachableCode::FragmentationRequired,
                NonZeroU16::new(mtu)
                    .map(IcmpDestUnreachable::new_for_frag_req)
                    .unwrap_or_else(Default::default),
            ))
            .wrap_in(Ipv4PacketBuilder::new(src_ip, dst_ip, 64, Ipv4Proto::Icmp))
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap(),
        IpAddr::V6([src_ip, dst_ip]) => body
            .wrap_in(IcmpPacketBuilder::<Ipv6, Icmpv6PacketTooBig>::new(
                dst_ip,
                src_ip,
                IcmpZeroCode,
                Icmpv6PacketTooBig::new(u32::from(mtu)),
            ))
            .wrap_in(Ipv6PacketBuilder::new(src_ip, dst_ip, 64, Ipv6Proto::Icmpv6))
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap(),
    }
    .into_inner()
}

trait GetPmtuIpExt: TestIpExt + IpExt {
    fn get_pmtu<BC: BindingsContext>(
        state: &StackState<BC>,
        local_ip: Self::Addr,
        remote_ip: Self::Addr,
    ) -> Option<Mtu>;
}

impl GetPmtuIpExt for Ipv4 {
    fn get_pmtu<BC: BindingsContext>(
        state: &StackState<BC>,
        local_ip: Ipv4Addr,
        remote_ip: Ipv4Addr,
    ) -> Option<Mtu> {
        state.ipv4().inner.pmtu_cache().lock().get_pmtu(local_ip, remote_ip)
    }
}

impl GetPmtuIpExt for Ipv6 {
    fn get_pmtu<BC: BindingsContext>(
        state: &StackState<BC>,
        local_ip: Ipv6Addr,
        remote_ip: Ipv6Addr,
    ) -> Option<Mtu> {
        state.ipv6().inner.pmtu_cache().lock().get_pmtu(local_ip, remote_ip)
    }
}

#[ip_test(I)]
fn test_ip_update_pmtu<I: GetPmtuIpExt>() {
    // Test receiving a Packet Too Big (IPv6) or Dest Unreachable
    // Fragmentation Required (IPv4) which should update the PMTU if it is
    // less than the current value.

    let fake_config = I::TEST_ADDRS;
    let (mut ctx, device_ids) = FakeCtxBuilder::with_addrs(fake_config.clone()).build();
    let device: DeviceId<_> = device_ids[0].clone().into();
    let frame_dst = LocalFrameDestination::Individual { local: () };

    // Update PMTU from None.

    let new_mtu1 = Mtu::new(u32::from(I::MINIMUM_LINK_MTU) + 100);

    // Create ICMP IP buf
    let packet_buf = create_packet_too_big_buf(
        fake_config.remote_ip.get(),
        fake_config.local_ip.get(),
        u16::try_from(u32::from(new_mtu1)).unwrap(),
        None,
    );

    // Receive the IP packet.
    ctx.test_api().receive_ip_packet::<I, _>(&device, Some(frame_dst), packet_buf);

    // Should have dispatched the packet.
    IpCounterExpectations::<I>::expect_dispatched(1).assert_counters(&ctx.core_ctx(), &device);

    assert_eq!(
        I::get_pmtu(&ctx.core_ctx, fake_config.local_ip.get(), fake_config.remote_ip.get())
            .unwrap(),
        new_mtu1
    );

    // Don't update PMTU when current PMTU is less than reported MTU.

    let new_mtu2 = Mtu::new(u32::from(I::MINIMUM_LINK_MTU) + 200);

    // Create IPv6 ICMPv6 packet too big packet with MTU larger than current
    // PMTU.
    let packet_buf = create_packet_too_big_buf(
        fake_config.remote_ip.get(),
        fake_config.local_ip.get(),
        u16::try_from(u32::from(new_mtu2)).unwrap(),
        None,
    );

    // Receive the IP packet.
    ctx.test_api().receive_ip_packet::<I, _>(&device, Some(frame_dst), packet_buf);

    // Should have dispatched the packet.
    IpCounterExpectations::<I>::expect_dispatched(2).assert_counters(&ctx.core_ctx(), &device);

    // The PMTU should not have updated to `new_mtu2`
    assert_eq!(
        I::get_pmtu(&ctx.core_ctx, fake_config.local_ip.get(), fake_config.remote_ip.get())
            .unwrap(),
        new_mtu1
    );

    // Update PMTU when current PMTU is greater than the reported MTU.

    let new_mtu3 = Mtu::new(u32::from(I::MINIMUM_LINK_MTU) + 50);

    // Create IPv6 ICMPv6 packet too big packet with MTU smaller than
    // current PMTU.
    let packet_buf = create_packet_too_big_buf(
        fake_config.remote_ip.get(),
        fake_config.local_ip.get(),
        u16::try_from(u32::from(new_mtu3)).unwrap(),
        None,
    );

    // Receive the IP packet.
    ctx.test_api().receive_ip_packet::<I, _>(&device, Some(frame_dst), packet_buf);

    // Should have dispatched the packet.
    IpCounterExpectations::<I>::expect_dispatched(3).assert_counters(&ctx.core_ctx(), &device);

    // The PMTU should have updated to 1900.
    assert_eq!(
        I::get_pmtu(&ctx.core_ctx, fake_config.local_ip.get(), fake_config.remote_ip.get())
            .unwrap(),
        new_mtu3
    );
}

#[ip_test(I)]
fn test_ip_update_pmtu_too_low<I: GetPmtuIpExt>() {
    // Test receiving a Packet Too Big (IPv6) or Dest Unreachable
    // Fragmentation Required (IPv4) which should not update the PMTU if it
    // is less than the min MTU.

    let fake_config = I::TEST_ADDRS;
    let (mut ctx, device_ids) = FakeCtxBuilder::with_addrs(fake_config.clone()).build();
    let device: DeviceId<_> = device_ids[0].clone().into();
    let frame_dst = LocalFrameDestination::Individual { local: () };

    // Update PMTU from None but with an MTU too low.

    let new_mtu1 = u32::from(I::MINIMUM_LINK_MTU) - 1;

    // Create ICMP IP buf
    let packet_buf = create_packet_too_big_buf(
        fake_config.remote_ip.get(),
        fake_config.local_ip.get(),
        u16::try_from(new_mtu1).unwrap(),
        None,
    );

    // Receive the IP packet.
    ctx.test_api().receive_ip_packet::<I, _>(&device, Some(frame_dst), packet_buf);

    // Should have dispatched the packet.
    IpCounterExpectations::<I>::expect_dispatched(1).assert_counters(&ctx.core_ctx(), &device);

    assert_eq!(
        I::get_pmtu(&ctx.core_ctx, fake_config.local_ip.get(), fake_config.remote_ip.get()),
        None
    );
}

/// Create buffer to be used as the ICMPv4 message body
/// where the original packet's body  length is `body_len`.
fn create_orig_packet_buf(src_ip: Ipv4Addr, dst_ip: Ipv4Addr, body_len: usize) -> Buf<Vec<u8>> {
    Buf::new(vec![0; body_len], ..)
        .wrap_in(Ipv4PacketBuilder::new(src_ip, dst_ip, 64, IpProto::Udp.into()))
        .serialize_vec_outer(&mut NetworkSerializationContext::default())
        .unwrap()
        .into_inner()
}

#[test]
fn test_ipv4_remote_no_rfc1191() {
    // Test receiving an IPv4 Dest Unreachable Fragmentation
    // Required from a node that does not implement RFC 1191.

    let fake_config = Ipv4::TEST_ADDRS;
    let (mut ctx, device_ids) = FakeCtxBuilder::with_addrs(fake_config.clone()).build();
    let device: DeviceId<_> = device_ids[0].clone().into();
    let frame_dst = LocalFrameDestination::Individual { local: () };

    // Update from None.

    // Create ICMP IP buf w/ orig packet body len = 500; orig packet len =
    // 520
    let packet_buf = create_packet_too_big_buf(
        fake_config.remote_ip.get(),
        fake_config.local_ip.get(),
        0, // A 0 value indicates that the source of the
        // ICMP message does not implement RFC 1191.
        create_orig_packet_buf(fake_config.local_ip.get(), fake_config.remote_ip.get(), 500).into(),
    );

    // Receive the IP packet.
    ctx.test_api().receive_ip_packet::<Ipv4, _>(&device, Some(frame_dst), packet_buf);

    // Should have dispatched the packet.
    IpCounterExpectations::<Ipv4> {
        receive_icmp_error: 1,
        ..IpCounterExpectations::expect_dispatched(1)
    }
    .assert_counters(&ctx.core_ctx(), &device);

    // Should have decreased PMTU value to the next lower PMTU
    // plateau from `path_mtu::PMTU_PLATEAUS`.
    assert_eq!(
        Ipv4::get_pmtu(&ctx.core_ctx, fake_config.local_ip.get(), fake_config.remote_ip.get())
            .unwrap(),
        Mtu::new(508),
    );

    // Don't Update when packet size is too small.

    // Create ICMP IP buf w/ orig packet body len = 1; orig packet len = 21
    let packet_buf = create_packet_too_big_buf(
        fake_config.remote_ip.get(),
        fake_config.local_ip.get(),
        0,
        create_orig_packet_buf(fake_config.local_ip.get(), fake_config.remote_ip.get(), 1).into(),
    );

    // Receive the IP packet.
    ctx.test_api().receive_ip_packet::<Ipv4, _>(&device, Some(frame_dst), packet_buf);

    // Should have dispatched the packet.
    IpCounterExpectations::<Ipv4> {
        receive_icmp_error: 2,
        ..IpCounterExpectations::expect_dispatched(2)
    }
    .assert_counters(&ctx.core_ctx(), &device);

    // Should not have updated PMTU as there is no other valid
    // lower PMTU value.
    assert_eq!(
        Ipv4::get_pmtu(&ctx.core_ctx, fake_config.local_ip.get(), fake_config.remote_ip.get())
            .unwrap(),
        Mtu::new(508),
    );

    // Update to lower PMTU estimate based on original packet size.

    // Create ICMP IP buf w/ orig packet body len = 60; orig packet len = 80
    let packet_buf = create_packet_too_big_buf(
        fake_config.remote_ip.get(),
        fake_config.local_ip.get(),
        0,
        create_orig_packet_buf(fake_config.local_ip.get(), fake_config.remote_ip.get(), 60).into(),
    );

    // Receive the IP packet.
    ctx.test_api().receive_ip_packet::<Ipv4, _>(&device, Some(frame_dst), packet_buf);

    // Should have dispatched the packet.
    IpCounterExpectations::<Ipv4> {
        receive_icmp_error: 3,
        ..IpCounterExpectations::expect_dispatched(3)
    }
    .assert_counters(&ctx.core_ctx(), &device);

    // Should have decreased PMTU value to the next lower PMTU
    // plateau from `path_mtu::PMTU_PLATEAUS`.
    assert_eq!(
        Ipv4::get_pmtu(&ctx.core_ctx, fake_config.local_ip.get(), fake_config.remote_ip.get())
            .unwrap(),
        Mtu::new(68),
    );

    // Should not update PMTU because the next low PMTU from this original
    // packet size is higher than current PMTU.

    // Create ICMP IP buf w/ orig packet body len = 290; orig packet len =
    // 310
    let packet_buf = create_packet_too_big_buf(
        fake_config.remote_ip.get(),
        fake_config.local_ip.get(),
        0, // A 0 value indicates that the source of the
        // ICMP message does not implement RFC 1191.
        create_orig_packet_buf(fake_config.local_ip.get(), fake_config.remote_ip.get(), 290).into(),
    );

    // Receive the IP packet.
    ctx.test_api().receive_ip_packet::<Ipv4, _>(&device, Some(frame_dst), packet_buf);

    // Should have dispatched the packet.
    IpCounterExpectations::<Ipv4> {
        receive_icmp_error: 4,
        ..IpCounterExpectations::expect_dispatched(4)
    }
    .assert_counters(&ctx.core_ctx(), &device);

    // Should not have updated the PMTU as the current PMTU is lower.
    assert_eq!(
        Ipv4::get_pmtu(&ctx.core_ctx, fake_config.local_ip.get(), fake_config.remote_ip.get())
            .unwrap(),
        Mtu::new(68),
    );
}

#[test]
fn test_invalid_icmpv4_in_ipv6() {
    let ip_config = Ipv6::TEST_ADDRS;
    let (mut ctx, device_ids) = FakeCtxBuilder::with_addrs(ip_config.clone()).build();
    let device: DeviceId<_> = device_ids[0].clone().into();
    let frame_dst = LocalFrameDestination::Individual { local: () };

    let ic_config = Ipv4::TEST_ADDRS;
    let icmp_builder = IcmpPacketBuilder::<Ipv4, _>::new(
        ic_config.remote_ip,
        ic_config.local_ip,
        IcmpZeroCode,
        IcmpEchoRequest::new(0, 0),
    );

    let ip_builder = Ipv6PacketBuilder::new(
        ip_config.remote_ip,
        ip_config.local_ip,
        64,
        Ipv6Proto::Other(Ipv4Proto::Icmp.into()),
    );

    let buf = Buf::new(Vec::new(), ..)
        .wrap_in(icmp_builder)
        .wrap_in(ip_builder)
        .serialize_vec_outer(&mut NetworkSerializationContext::default())
        .unwrap();

    ctx.test_api().enable_device(&device);
    ctx.test_api().receive_ip_packet::<Ipv6, _>(&device, Some(frame_dst), buf);

    // Should have received, but not dispatched the packet.
    IpCounterExpectations::<Ipv6> {
        dispatch_receive_ip_packet: 0,
        receive_ip_packet: 1,
        ..Default::default()
    }
    .assert_counters(&ctx.core_ctx(), &device);

    // In IPv6, the next header value (ICMP(v4)) would have been considered
    // unrecognized so an ICMP parameter problem response SHOULD be sent,
    // but the netstack chooses to just drop the packet since we are not
    // required to send the ICMP response.
    let Ctx { core_ctx: _, bindings_ctx } = &mut ctx;
    assert_matches!(bindings_ctx.take_ethernet_frames()[..], []);
}

#[test]
fn test_invalid_icmpv6_in_ipv4() {
    let ip_config = Ipv4::TEST_ADDRS;
    let (mut ctx, device_ids) = FakeCtxBuilder::with_addrs(ip_config.clone()).build();
    // First possible device id.
    let device: DeviceId<_> = device_ids[0].clone().into();
    let frame_dst = LocalFrameDestination::Individual { local: () };

    let ic_config = Ipv6::TEST_ADDRS;
    let icmp_builder = IcmpPacketBuilder::<Ipv6, _>::new(
        ic_config.remote_ip,
        ic_config.local_ip,
        IcmpZeroCode,
        IcmpEchoRequest::new(0, 0),
    );

    let ip_builder = Ipv4PacketBuilder::new(
        ip_config.remote_ip,
        ip_config.local_ip,
        64,
        Ipv4Proto::Other(Ipv6Proto::Icmpv6.into()),
    );

    let buf = Buf::new(Vec::new(), ..)
        .wrap_in(icmp_builder)
        .wrap_in(ip_builder)
        .serialize_vec_outer(&mut NetworkSerializationContext::default())
        .unwrap();

    ctx.test_api().receive_ip_packet::<Ipv4, _>(&device, Some(frame_dst), buf);

    // Should have dispatched the packet but resulted in an ICMP error.
    IpCounterExpectations::<Ipv4> {
        send_ip_packet: 1,
        ..IpCounterExpectations::expect_dispatched(1)
    }
    .assert_counters(&ctx.core_ctx(), &device);
    assert_eq!(
        ctx.core_ctx
            .common_icmp::<Ipv4>()
            .tx_counters
            .dest_unreachable
            .dest_protocol_unreachable
            .get(),
        1
    );
    let frames = ctx.bindings_ctx.take_ethernet_frames();
    let (_dev, frame) = assert_matches!(&frames[..], [frame] => frame);
    let (_, _, _, _, _, _, code) = parse_icmp_packet_in_ip_packet_in_ethernet_frame::<
        Ipv4,
        _,
        IcmpDestUnreachable,
        _,
    >(&frame[..], EthernetFrameLengthCheck::NoCheck, |_| {})
    .unwrap();
    assert_eq!(code, Icmpv4DestUnreachableCode::DestProtocolUnreachable);
}

#[netstack3_macros::context_ip_bounds(I, FakeBindingsCtx)]
#[ip_test(I)]
fn test_joining_leaving_ip_multicast_group<I: TestIpExt + IpExt>() {
    // Test receiving a packet destined to a multicast IP (and corresponding
    // multicast MAC).

    let config = I::TEST_ADDRS;
    let (mut ctx, device_ids) = FakeCtxBuilder::with_addrs(config.clone()).build();
    let eth_device = &device_ids[0];
    let device: DeviceId<_> = eth_device.clone().into();
    let multi_addr = I::get_multicast_addr(3).get();
    let dst_mac = Mac::from(&MulticastAddr::new(multi_addr).unwrap());
    let buf = Buf::new(vec![0; 10], ..)
        .wrap_in(I::PacketBuilder::new(config.remote_ip.get(), multi_addr, 64, IpProto::Udp.into()))
        .wrap_in(EthernetFrameBuilder::new(
            config.remote_mac.get(),
            dst_mac,
            I::ETHER_TYPE,
            ETHERNET_MIN_BODY_LEN_NO_TAG,
        ))
        .serialize_vec_outer(&mut NetworkSerializationContext::default())
        .ok()
        .unwrap()
        .into_inner();

    let multi_addr = MulticastAddr::new(multi_addr).unwrap();
    // Should not have dispatched the packet since we are not in the
    // multicast group `multi_addr`.

    assert!(!ctx.test_api().is_in_ip_multicast(&device, multi_addr));
    ctx.core_api().device::<EthernetLinkDevice>().receive_frame(
        RecvEthernetFrameMeta {
            device_id: eth_device.clone(),
            parsing_context: NetworkParsingContext::default(),
        },
        buf.clone(),
    );

    IpCounterExpectations::<I> {
        dispatch_receive_ip_packet: 0,
        receive_ip_packet: 1,
        dropped: 1,
        multicast_no_interest: 1,
        ..Default::default()
    }
    .assert_counters(&ctx.core_ctx(), &device);

    // Join the multicast group and receive the packet, we should dispatch
    // it.
    let Ctx { core_ctx, bindings_ctx } = &mut ctx;
    match multi_addr.into() {
        IpAddr::V4(multicast_addr) => ip::device::join_ip_multicast::<Ipv4, _, _>(
            &mut core_ctx.context(),
            bindings_ctx,
            &device,
            multicast_addr,
        ),
        IpAddr::V6(multicast_addr) => ip::device::join_ip_multicast::<Ipv6, _, _>(
            &mut core_ctx.context(),
            bindings_ctx,
            &device,
            multicast_addr,
        ),
    }
    assert!(ctx.test_api().is_in_ip_multicast(&device, multi_addr));
    ctx.core_api().device::<EthernetLinkDevice>().receive_frame(
        RecvEthernetFrameMeta {
            device_id: eth_device.clone(),
            parsing_context: NetworkParsingContext::default(),
        },
        buf.clone(),
    );

    IpCounterExpectations::<I> {
        receive_ip_packet: 2,
        dropped: 1,
        multicast_no_interest: 1,
        deliver_multicast: 1,
        dispatch_receive_ip_packet: 1,
        ..Default::default()
    }
    .assert_counters(&ctx.core_ctx(), &device);

    // Leave the multicast group and receive the packet, we should not
    // dispatch it.
    let Ctx { core_ctx, bindings_ctx } = &mut ctx;
    match multi_addr.into() {
        IpAddr::V4(multicast_addr) => ip::device::leave_ip_multicast::<Ipv4, _, _>(
            &mut core_ctx.context(),
            bindings_ctx,
            &device,
            multicast_addr,
        ),
        IpAddr::V6(multicast_addr) => ip::device::leave_ip_multicast::<Ipv6, _, _>(
            &mut core_ctx.context(),
            bindings_ctx,
            &device,
            multicast_addr,
        ),
    }
    assert!(!ctx.test_api().is_in_ip_multicast(&device, multi_addr));
    ctx.core_api().device::<EthernetLinkDevice>().receive_frame(
        RecvEthernetFrameMeta {
            device_id: eth_device.clone(),
            parsing_context: NetworkParsingContext::default(),
        },
        buf,
    );
    IpCounterExpectations::<I> {
        receive_ip_packet: 3,
        dropped: 2,
        multicast_no_interest: 2,
        deliver_multicast: 1,
        dispatch_receive_ip_packet: 1,
        ..Default::default()
    }
    .assert_counters(&ctx.core_ctx(), &device);
}

#[test]
fn test_no_dispatch_non_ndp_packets_during_ndp_dad() {
    // Here we make sure we are not dispatching packets destined to a
    // tentative address (that is performing NDP's Duplicate Address
    // Detection (DAD)) -- IPv6 only.

    let config = Ipv6::TEST_ADDRS;
    let mut ctx = FakeCtx::default();
    let device = ctx
        .core_api()
        .device::<EthernetLinkDevice>()
        .add_device_with_default_state(
            EthernetCreationProperties {
                tx_offload_spec: Default::default(),
                mac: config.local_mac,
                max_frame_size: IPV6_MIN_IMPLIED_MAX_FRAME_SIZE,
            },
            DEFAULT_INTERFACE_METRIC,
        )
        .into();

    let _: Ipv6DeviceConfigurationUpdate = ctx
        .core_api()
        .device_ip::<Ipv6>()
        .update_configuration(
            &device,
            Ipv6DeviceConfigurationUpdate {
                // Auto-generate a link-local address.
                slaac_config: SlaacConfigurationUpdate {
                    stable_address_configuration: Some(
                        StableSlaacAddressConfiguration::ENABLED_WITH_EUI64,
                    ),
                    ..Default::default()
                },
                ip_config: IpDeviceConfigurationUpdate {
                    ip_enabled: Some(true),
                    // Doesn't matter as long as DAD is enabled.
                    dad_transmits: Some(NonZeroU16::new(1)),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();

    let frame_dst = LocalFrameDestination::Individual { local: () };

    let ip: Ipv6Addr = config.local_mac.to_ipv6_link_local().addr().get();

    let buf = Buf::new(vec![0; 10], ..)
        .wrap_in(Ipv6PacketBuilder::new(config.remote_ip, ip, 64, IpProto::Udp.into()))
        .serialize_vec_outer(&mut NetworkSerializationContext::default())
        .unwrap()
        .into_inner();

    // Received packet should not have been dispatched.
    ctx.test_api().receive_ip_packet::<Ipv6, _>(&device, Some(frame_dst), buf.clone());

    IpCounterExpectations::<Ipv6> {
        dispatch_receive_ip_packet: 0,
        receive_ip_packet: 1,
        dropped: 1,
        send_ip_packet: 1,
        drop_for_tentative: 1,
        ..Default::default()
    }
    .assert_counters(&ctx.core_ctx(), &device);

    // Wait until DAD is complete. Arbitrarily choose a year in the future
    // as a time after which we're confident DAD will be complete. We can't
    // run until there are no timers because some timers will always exist
    // for background tasks.
    //
    // TODO(https://fxbug.dev/42125450): Once this test is contextified, use a
    // more precise condition to ensure that DAD is complete.
    let now = ctx.bindings_ctx.now();
    let _: Vec<_> = ctx.trigger_timers_until_instant(now + Duration::from_secs(60 * 60 * 24 * 365));

    // Received packet should have been dispatched.
    ctx.test_api().receive_ip_packet::<Ipv6, _>(&device, Some(frame_dst), buf);
    IpCounterExpectations::<Ipv6> {
        receive_ip_packet: 2,
        dropped: 1,
        send_ip_packet: 1,
        drop_for_tentative: 1,
        dispatch_receive_ip_packet: 1,
        deliver_unicast: 1,
        ..Default::default()
    }
    .assert_counters(&ctx.core_ctx(), &device);

    // Set the new IP (this should trigger DAD).
    let ip = config.local_ip.get();
    ctx.core_api()
        .device_ip::<Ipv6>()
        .add_ip_addr_subnet(&device, AddrSubnet::new(ip, 128).unwrap())
        .unwrap();

    let buf = Buf::new(vec![0; 10], ..)
        .wrap_in(Ipv6PacketBuilder::new(config.remote_ip, ip, 64, IpProto::Udp.into()))
        .serialize_vec_outer(&mut NetworkSerializationContext::default())
        .unwrap()
        .into_inner();

    // Received packet should not have been dispatched.
    ctx.test_api().receive_ip_packet::<Ipv6, _>(&device, Some(frame_dst), buf.clone());
    IpCounterExpectations::<Ipv6> {
        receive_ip_packet: 3,
        dropped: 2,
        send_ip_packet: 2,
        drop_for_tentative: 2,
        dispatch_receive_ip_packet: 1,
        deliver_unicast: 1,
        ..Default::default()
    }
    .assert_counters(&ctx.core_ctx(), &device);

    // Make sure all timers are done (DAD to complete on the interface due
    // to new IP).
    //
    // TODO(https://fxbug.dev/42125450): Once this test is contextified, use a
    // more precise condition to ensure that DAD is complete.
    let _: Vec<_> = ctx.trigger_timers_until_instant(FakeInstant::LATEST);

    // Received packet should have been dispatched.
    ctx.test_api().receive_ip_packet::<Ipv6, _>(&device, Some(frame_dst), buf);
    IpCounterExpectations::<Ipv6> {
        receive_ip_packet: 4,
        dropped: 2,
        send_ip_packet: 2,
        drop_for_tentative: 2,
        dispatch_receive_ip_packet: 2,
        deliver_unicast: 2,
        ..Default::default()
    }
    .assert_counters(&ctx.core_ctx(), &device);
}

// Test that an inbound IP packet with a multicast source address is dropped.
#[netstack3_macros::context_ip_bounds(I, FakeBindingsCtx)]
#[ip_test(I)]
fn test_drop_multicast_source<I: IpExt + TestIpExt>() {
    let (mut ctx, _device_ids) = FakeCtxBuilder::with_addrs(I::TEST_ADDRS).build();
    let device = ctx
        .core_api()
        .device::<EthernetLinkDevice>()
        .add_device_with_default_state(
            EthernetCreationProperties {
                tx_offload_spec: Default::default(),
                mac: I::TEST_ADDRS.local_mac,
                max_frame_size: MaxEthernetFrameSize::from_mtu(I::MINIMUM_LINK_MTU).unwrap(),
            },
            DEFAULT_INTERFACE_METRIC,
        )
        .into();
    ctx.test_api().enable_device(&device);

    let buf = Buf::new(vec![0; 10], ..)
        .wrap_in(I::PacketBuilder::new(
            I::MULTICAST_SUBNET.network(),
            I::TEST_ADDRS.remote_ip.get(),
            64,
            IpProto::Udp.into(),
        ))
        .serialize_vec_outer(&mut NetworkSerializationContext::default())
        .unwrap()
        .into_inner();

    ctx.test_api().receive_ip_packet::<I, _>(
        &device,
        Some(LocalFrameDestination::Individual { local: () }),
        buf,
    );
    IpCounterExpectations::<I> { receive_ip_packet: 1, invalid_source: 1, ..Default::default() }
        .assert_counters(&ctx.core_ctx(), &device);
}

/// Constructs a buffer containing an IP packet with sensible defaults.
fn new_ip_packet_buf<I: IpExt>(src_addr: I::Addr, dst_addr: I::Addr) -> impl AsRef<[u8]> {
    const TTL: u8 = 255;
    /// Arbitrary data to put inside of an IP packet.
    const IP_BODY: [u8; 10] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
    IP_BODY
        .into_serializer()
        .wrap_in(I::PacketBuilder::new(src_addr, dst_addr, TTL, IpProto::Udp.into()))
        .serialize_vec_outer(&mut NetworkSerializationContext::default())
        .unwrap()
}

#[test]
fn test_receive_ipv4_packet_broadcast_source() {
    let mut builder = FakeCtxBuilder::default();
    let dev_idx = builder.add_device_with_ip(
        Ipv4::TEST_ADDRS.local_mac,
        Ipv4::TEST_ADDRS.local_ip.get(),
        Ipv4::TEST_ADDRS.subnet,
    );
    let (mut ctx, device_ids) = builder.build();
    let device: DeviceId<_> = device_ids[dev_idx].clone().into();
    ctx.test_api().enable_device(&device);

    let send_and_assert_drop =
        |ctx: &mut Ctx<FakeBindingsCtx>, src_ip: Ipv4Addr, expected_count: u64| {
            let buf = new_ip_packet_buf::<Ipv4>(src_ip, Ipv4::TEST_ADDRS.local_ip.get());
            ctx.test_api().receive_ip_packet::<Ipv4, _>(
                &device,
                Some(LocalFrameDestination::Individual { local: () }),
                Buf::new(buf.as_ref().to_vec(), ..),
            );
            IpCounterExpectations::<Ipv4> {
                receive_ip_packet: expected_count,
                invalid_source: expected_count,
                ..Default::default()
            }
            .assert_counters(&ctx.core_ctx(), &device);
        };

    send_and_assert_drop(&mut ctx, *Ipv4::LIMITED_BROADCAST_ADDRESS, 1);
    send_and_assert_drop(&mut ctx, Ipv4::TEST_ADDRS.subnet.broadcast(), 2);
}

#[test]
fn test_receive_ipv4_packet_broadcast_source_unaddressed_device() {
    let mut builder = FakeCtxBuilder::default();
    let dev_idx = builder.add_device(Ipv4::TEST_ADDRS.local_mac);
    let (mut ctx, device_ids) = builder.build();
    let device: DeviceId<_> = device_ids[dev_idx].clone().into();
    ctx.test_api().enable_device(&device);

    let send_ip_packet = |ctx: &mut Ctx<FakeBindingsCtx>, src_ip: Ipv4Addr| {
        let buf = new_ip_packet_buf::<Ipv4>(src_ip, Ipv4::TEST_ADDRS.local_ip.get());
        ctx.test_api().receive_ip_packet::<Ipv4, _>(
            &device,
            Some(LocalFrameDestination::Individual { local: () }),
            Buf::new(buf.as_ref().to_vec(), ..),
        );
    };

    send_ip_packet(&mut ctx, *Ipv4::LIMITED_BROADCAST_ADDRESS);
    IpCounterExpectations::<Ipv4> { receive_ip_packet: 1, invalid_source: 1, ..Default::default() }
        .assert_counters(&ctx.core_ctx(), &device);

    // Since the device doesn't have an address, there's no way for us
    // to detect that the source address was a subnet broadcast. The
    // attempt to forward is incidental.
    send_ip_packet(&mut ctx, Ipv4::TEST_ADDRS.subnet.broadcast());
    IpCounterExpectations::<Ipv4> {
        receive_ip_packet: 2,
        invalid_source: 1,
        forwarding_disabled: 1,
        dropped: 1,
        ..Default::default()
    }
    .assert_counters(&ctx.core_ctx(), &device);
}

// Helper function to call receive ipv4/ipv6 packet action with an source address.
fn receive_ip_packet_action_with_src_addr<I: IpExt + TestIpExt>(
    ctx: &mut Ctx<FakeBindingsCtx>,
    dev: &DeviceId<FakeBindingsCtx>,
    src_addr: I::Addr,
    dst_addr: I::Addr,
) -> ReceivePacketAction<I, DeviceId<FakeBindingsCtx>> {
    let Ctx { core_ctx, bindings_ctx } = ctx;
    const FRAME_DST: Option<LocalFrameDestination> = None;
    const MAX_FRAGMENT_LEN: Option<usize> = None;
    let buf = new_ip_packet_buf::<I>(src_addr, dst_addr);
    let mut buf_ref = buf.as_ref();
    let packet = buf_ref.parse::<I::Packet<_>>().expect("parse should succeed");

    #[derive(GenericOverIp)]
    #[generic_over_ip(I, Ip)]
    struct Out<I: IpExt>(ReceivePacketAction<I, DeviceId<FakeBindingsCtx>>);

    let Out(action) = I::map_ip(
        (&packet, IpInvariant((&mut core_ctx.context(), bindings_ctx, dev))),
        |(packet, IpInvariant((core_ctx, bindings_ctx, dev)))| {
            Out(ip::receive_ipv4_packet_action(
                core_ctx,
                bindings_ctx,
                dev,
                packet,
                FRAME_DST,
                &Default::default(),
                MAX_FRAGMENT_LEN,
            ))
        },
        |(packet, IpInvariant((core_ctx, bindings_ctx, dev)))| {
            Out(ip::receive_ipv6_packet_action(
                core_ctx,
                bindings_ctx,
                dev,
                packet,
                FRAME_DST,
                &Default::default(),
                MAX_FRAGMENT_LEN,
            ))
        },
    );
    action
}

// Helper function to call receive ipv4/ipv6 packet action.
fn receive_ip_packet_action<I: IpExt + TestIpExt>(
    ctx: &mut Ctx<FakeBindingsCtx>,
    dev: &DeviceId<FakeBindingsCtx>,
    dst_addr: I::Addr,
) -> ReceivePacketAction<I, DeviceId<FakeBindingsCtx>> {
    receive_ip_packet_action_with_src_addr(ctx, dev, I::TEST_ADDRS.remote_ip.get(), dst_addr)
}

#[test]
fn test_receive_ip_packet_action() {
    let v4_config = Ipv4::TEST_ADDRS;
    let v6_config = Ipv6::TEST_ADDRS;

    let mut builder = FakeCtxBuilder::default();
    // Both devices have the same MAC address, which is a bit weird, but not
    // a problem for this test.
    let v4_subnet = AddrSubnet::from_witness(v4_config.local_ip, 16).unwrap().subnet();
    let dev_idx0 =
        builder.add_device_with_ip(v4_config.local_mac, v4_config.local_ip.get(), v4_subnet);
    let dev_idx1 = builder.add_device_with_ip_and_config(
        v6_config.local_mac,
        v6_config.local_ip.get(),
        AddrSubnet::from_witness(v6_config.local_ip, 64).unwrap().subnet(),
        Ipv4DeviceConfigurationUpdate::default(),
        Ipv6DeviceConfigurationUpdate {
            // Auto-generate a link-local address.
            slaac_config: SlaacConfigurationUpdate {
                stable_address_configuration: Some(
                    StableSlaacAddressConfiguration::ENABLED_WITH_EUI64,
                ),
                ..Default::default()
            },
            ..Default::default()
        },
    );
    let (mut ctx, device_ids) = builder.clone().build();
    let v4_dev: DeviceId<_> = device_ids[dev_idx0].clone().into();
    let v6_dev: DeviceId<_> = device_ids[dev_idx1].clone().into();

    // Receive packet destined to the unspecified address.
    assert_eq!(
        receive_ip_packet_action::<Ipv4>(&mut ctx, &v4_dev, Ipv4::UNSPECIFIED_ADDRESS),
        ReceivePacketAction::Drop { reason: DropReason::UnspecifiedDestination }
    );
    assert_eq!(
        receive_ip_packet_action::<Ipv6>(&mut ctx, &v6_dev, Ipv6::UNSPECIFIED_ADDRESS),
        ReceivePacketAction::Drop { reason: DropReason::UnspecifiedDestination }
    );

    // Receive packet destined to a local address but with unspecified source address, we should
    // deliver it locally.
    assert_eq!(
        receive_ip_packet_action_with_src_addr::<Ipv4>(
            &mut ctx,
            &v4_dev,
            Ipv4::UNSPECIFIED_ADDRESS,
            v4_config.local_ip.get()
        ),
        ReceivePacketAction::Deliver {
            address_status: Ipv4PresentAddressStatus::UnicastAssigned,
            internal_forwarding: InternalForwarding::NotUsed
        }
    );
    assert_eq!(
        receive_ip_packet_action_with_src_addr::<Ipv6>(
            &mut ctx,
            &v6_dev,
            Ipv6::UNSPECIFIED_ADDRESS,
            v6_config.local_ip.get()
        ),
        ReceivePacketAction::Deliver {
            address_status: Ipv6PresentAddressStatus::UnicastAssigned,
            internal_forwarding: InternalForwarding::NotUsed
        }
    );

    // Receive packet addressed to us.
    assert_eq!(
        receive_ip_packet_action::<Ipv4>(&mut ctx, &v4_dev, *v4_config.local_ip),
        ReceivePacketAction::Deliver {
            address_status: Ipv4PresentAddressStatus::UnicastAssigned,
            internal_forwarding: InternalForwarding::NotUsed
        }
    );
    assert_eq!(
        receive_ip_packet_action::<Ipv6>(&mut ctx, &v6_dev, *v6_config.local_ip),
        ReceivePacketAction::Deliver {
            address_status: Ipv6PresentAddressStatus::UnicastAssigned,
            internal_forwarding: InternalForwarding::NotUsed
        }
    );

    // Receive packet addressed to the IPv4 subnet broadcast address.
    assert_eq!(
        receive_ip_packet_action::<Ipv4>(&mut ctx, &v4_dev, v4_subnet.broadcast()),
        ReceivePacketAction::Deliver {
            address_status: Ipv4PresentAddressStatus::SubnetBroadcast,
            internal_forwarding: InternalForwarding::NotUsed
        }
    );

    // Receive packet addressed to the IPv4 limited broadcast address.
    assert_eq!(
        receive_ip_packet_action::<Ipv4>(&mut ctx, &v4_dev, *Ipv4::LIMITED_BROADCAST_ADDRESS),
        ReceivePacketAction::Deliver {
            address_status: Ipv4PresentAddressStatus::LimitedBroadcast,
            internal_forwarding: InternalForwarding::NotUsed
        }
    );

    // Receive packet addressed to a multicast address we're subscribed to.
    {
        let Ctx { core_ctx, bindings_ctx } = &mut ctx;
        ip::device::join_ip_multicast::<Ipv4, _, _>(
            &mut core_ctx.context(),
            bindings_ctx,
            &v4_dev,
            Ipv4::ALL_ROUTERS_MULTICAST_ADDRESS,
        );
    }
    assert_eq!(
        receive_ip_packet_action::<Ipv4>(&mut ctx, &v4_dev, *Ipv4::ALL_ROUTERS_MULTICAST_ADDRESS),
        ReceivePacketAction::Deliver {
            address_status: Ipv4PresentAddressStatus::Multicast,
            internal_forwarding: InternalForwarding::NotUsed
        }
    );

    // Receive packet addressed to the all-nodes multicast address.
    assert_eq!(
        receive_ip_packet_action::<Ipv6>(
            &mut ctx,
            &v6_dev,
            *Ipv6::ALL_NODES_LINK_LOCAL_MULTICAST_ADDRESS
        ),
        ReceivePacketAction::Deliver {
            address_status: Ipv6PresentAddressStatus::Multicast,
            internal_forwarding: InternalForwarding::NotUsed
        }
    );

    // Receive packet addressed to a multicast address we're subscribed to.
    assert_eq!(
        receive_ip_packet_action::<Ipv6>(
            &mut ctx,
            &v6_dev,
            *v6_config.local_ip.to_solicited_node_address()
        ),
        ReceivePacketAction::Deliver {
            address_status: Ipv6PresentAddressStatus::Multicast,
            internal_forwarding: InternalForwarding::NotUsed
        }
    );

    // Receive packet addressed to a tentative address.
    {
        // Construct a one-off context that has DAD enabled. The context
        // built above has DAD disabled, and so addresses start off in the
        // assigned state rather than the tentative state.
        let mut ctx = FakeCtx::default();
        let local_mac = v6_config.local_mac;
        let eth_device =
            ctx.core_api().device::<EthernetLinkDevice>().add_device_with_default_state(
                EthernetCreationProperties {
                    tx_offload_spec: Default::default(),
                    mac: local_mac,
                    max_frame_size: IPV6_MIN_IMPLIED_MAX_FRAME_SIZE,
                },
                DEFAULT_INTERFACE_METRIC,
            );
        let device = eth_device.clone().into();
        let _: Ipv6DeviceConfigurationUpdate = ctx
            .core_api()
            .device_ip::<Ipv6>()
            .update_configuration(
                &device,
                Ipv6DeviceConfigurationUpdate {
                    // Auto-generate a link-local address.
                    slaac_config: SlaacConfigurationUpdate {
                        stable_address_configuration: Some(
                            StableSlaacAddressConfiguration::ENABLED_WITH_EUI64,
                        ),
                        ..Default::default()
                    },
                    ip_config: IpDeviceConfigurationUpdate {
                        ip_enabled: Some(true),
                        // Doesn't matter as long as DAD is enabled.
                        dad_transmits: Some(NonZeroU16::new(1)),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .unwrap();
        let tentative: UnicastAddr<Ipv6Addr> = local_mac.to_ipv6_link_local().addr().get();
        assert_eq!(
            receive_ip_packet_action::<Ipv6>(&mut ctx, &device, *tentative),
            ReceivePacketAction::Drop { reason: DropReason::Tentative }
        );
        // Clean up secondary context.
        core::mem::drop(device);
        ctx.core_api().device().remove_device(eth_device).into_removed();
    }

    // Receive packet destined to a remote address when forwarding is
    // disabled on the inbound interface.
    assert_eq!(
        receive_ip_packet_action::<Ipv4>(&mut ctx, &v4_dev, *v4_config.remote_ip),
        ReceivePacketAction::Drop { reason: DropReason::ForwardingDisabledInboundIface }
    );
    assert_eq!(
        receive_ip_packet_action::<Ipv6>(&mut ctx, &v6_dev, *v6_config.remote_ip),
        ReceivePacketAction::Drop { reason: DropReason::ForwardingDisabledInboundIface }
    );

    // Receive packet destined to a remote address when forwarding is
    // enabled both globally and on the inbound device.
    ctx.test_api().set_unicast_forwarding_enabled::<Ipv4>(&v4_dev, true);
    ctx.test_api().set_unicast_forwarding_enabled::<Ipv6>(&v6_dev, true);

    // Receive packet destined to a remote address but with unspecified source address, we should
    // drop it.
    assert_eq!(
        receive_ip_packet_action_with_src_addr::<Ipv4>(
            &mut ctx,
            &v4_dev,
            Ipv4::UNSPECIFIED_ADDRESS,
            v4_config.remote_ip.get()
        ),
        ReceivePacketAction::Drop { reason: DropReason::ForwardUnspecifiedSource }
    );
    assert_eq!(
        receive_ip_packet_action_with_src_addr::<Ipv6>(
            &mut ctx,
            &v6_dev,
            Ipv6::UNSPECIFIED_ADDRESS,
            v6_config.remote_ip.get()
        ),
        ReceivePacketAction::Drop { reason: DropReason::ForwardUnspecifiedSource }
    );

    assert_eq!(
        receive_ip_packet_action::<Ipv4>(&mut ctx, &v4_dev, *v4_config.remote_ip),
        ReceivePacketAction::Forward {
            original_dst: v4_config.remote_ip,
            dst: Destination { next_hop: NextHop::RemoteAsNeighbor, device: v4_dev.clone() }
        }
    );
    assert_eq!(
        receive_ip_packet_action::<Ipv6>(&mut ctx, &v6_dev, *v6_config.remote_ip),
        ReceivePacketAction::Forward {
            original_dst: v6_config.remote_ip,
            dst: Destination { next_hop: NextHop::RemoteAsNeighbor, device: v6_dev.clone() }
        }
    );

    // Receive packet destined to a host with no route when forwarding is
    // enabled both globally and on the inbound device.
    {
        let Ctx { core_ctx, bindings_ctx: _ } = &mut ctx;
        *core_ctx.ipv4().inner.main_table_id().table().write() = Default::default();
        *core_ctx.ipv6().inner.main_table_id().table().write() = Default::default();
    }
    assert_eq!(
        receive_ip_packet_action::<Ipv4>(&mut ctx, &v4_dev, *v4_config.remote_ip),
        ReceivePacketAction::SendNoRouteToDest {
            dst: NonMappedAddr::new(v4_config.remote_ip).expect("non-mapped ip")
        }
    );
    assert_eq!(
        receive_ip_packet_action::<Ipv6>(&mut ctx, &v6_dev, *v6_config.remote_ip),
        ReceivePacketAction::SendNoRouteToDest {
            dst: NonMappedAddr::new(v6_config.remote_ip).expect("non-mapped ip")
        }
    );

    // Cleanup all device references.
    core::mem::drop((v4_dev, v6_dev));
    for device in device_ids {
        ctx.core_api().device().remove_device(device).into_removed();
    }
}

#[netstack3_core::context_ip_bounds(I, FakeBindingsCtx)]
#[ip_test(I)]
fn test_multicast_forwarding_receive_ip_packet_action<I: IpExt + TestIpExt>() {
    let mut builder = FakeCtxBuilder::default();
    // Install two devices.
    for _ in 0..2 {
        let _dev_idx = builder.add_device_with_ip(
            I::TEST_ADDRS.local_mac,
            I::TEST_ADDRS.local_ip.get(),
            I::TEST_ADDRS.subnet,
        );
    }
    let (mut ctx, device_ids) = builder.clone().build();
    let [dev, other_dev] = device_ids.try_into().expect("there should be two devices");
    let dev: DeviceId<_> = dev.into();
    let other_dev: DeviceId<_> = other_dev.into();

    let mcast_addr: I::Addr =
        I::map_ip((), |()| net_ip_v4!("224.0.1.1"), |()| net_ip_v6!("ff0e::1"));

    // Helper function to check if the IP generic address status is multicast.
    fn is_multicast<I: IpExt>(address_status: &I::AddressStatus) -> bool {
        #[derive(GenericOverIp)]
        #[generic_over_ip(I, Ip)]
        struct In<'a, I: IpExt>(&'a I::AddressStatus);
        I::map_ip(
            In(address_status),
            |In(address_status)| matches!(address_status, Ipv4PresentAddressStatus::Multicast),
            |In(address_status)| matches!(address_status, Ipv6PresentAddressStatus::Multicast),
        )
    }

    // Enable multicast forwarding.
    assert!(ctx.core_api().multicast_forwarding::<I>().enable());
    ctx.test_api().set_multicast_forwarding_enabled::<I>(&dev, true);

    // Join the multicast group.
    {
        let Ctx { core_ctx, bindings_ctx } = &mut ctx;
        ip::device::join_ip_multicast::<I, _, _>(
            &mut core_ctx.context(),
            bindings_ctx,
            &dev,
            MulticastAddr::new(mcast_addr).unwrap(),
        )
    }

    // Verify `Delivered` in the absence of a multicast route.
    assert_matches!(
        receive_ip_packet_action::<I>(&mut ctx, &dev, mcast_addr.clone()),
        ReceivePacketAction::Deliver {
            address_status,
            internal_forwarding: InternalForwarding::NotUsed
        }
        if is_multicast::<I>(&address_status)
    );

    // Install a multicast route.
    let key =
        MulticastRouteKey::<I>::new(I::TEST_ADDRS.remote_ip.get(), mcast_addr.clone()).unwrap();
    let targets: MulticastRouteTargets<_> =
        [MulticastRouteTarget { output_interface: other_dev, min_ttl: NonZeroU8::new(1).unwrap() }]
            .into();
    let route = MulticastRoute::new_forward(dev.clone(), targets.clone()).unwrap();
    assert_eq!(
        ctx.core_api().multicast_forwarding::<I>().add_multicast_route(key.clone(), route.clone()),
        Ok(None)
    );

    // Verify `MulticastForward` now that we have a route.
    assert_matches!(
        receive_ip_packet_action::<I>(&mut ctx, &dev, mcast_addr.clone()),
        ReceivePacketAction::MulticastForward {
            targets: actual_targets, address_status, dst_ip: _
        }
        if actual_targets == targets && address_status.as_ref().is_some_and(is_multicast::<I>)
    );

    // Remove the multicast group.
    {
        let Ctx { core_ctx, bindings_ctx } = &mut ctx;
        ip::device::leave_ip_multicast::<I, _, _>(
            &mut core_ctx.context(),
            bindings_ctx,
            &dev,
            MulticastAddr::new(mcast_addr).unwrap(),
        )
    }

    // Verify `address_status` is None, now that we've left the group.
    assert_matches!(
        receive_ip_packet_action::<I>(&mut ctx, &dev, mcast_addr.clone()),
        ReceivePacketAction::MulticastForward {
            targets: actual_targets, address_status: None, dst_ip: _
        } if actual_targets == targets
    );

    // Remove the route.
    assert_eq!(
        ctx.core_api().multicast_forwarding::<I>().remove_multicast_route(&key),
        Ok(Some(route))
    );

    // Verify `Drop` now that the route is removed.
    assert_matches!(
        receive_ip_packet_action::<I>(&mut ctx, &dev, mcast_addr),
        ReceivePacketAction::Drop { reason: DropReason::MulticastNoInterest }
    );
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum Device {
    First,
    Second,
    Loopback,
}

impl Device {
    fn index(self) -> usize {
        match self {
            Self::First => 0,
            Self::Second => 1,
            Self::Loopback => 2,
        }
    }

    fn from_index(index: usize) -> Self {
        match index {
            0 => Self::First,
            1 => Self::Second,
            2 => Self::Loopback,
            x => panic!("index out of bounds: {x}"),
        }
    }

    fn ip_address<A: IpAddress>(self) -> IpDeviceAddr<A>
    where
        A::Version: TestIpExt,
    {
        match self {
            Self::First | Self::Second => <A::Version as TestIpExt>::get_other_ip_address(
                (self.index() + 1).try_into().unwrap(),
            )
            .try_into()
            .unwrap(),
            Self::Loopback => <A::Version as Ip>::LOOPBACK_ADDRESS.try_into().unwrap(),
        }
    }

    fn mac(self) -> UnicastAddr<Mac> {
        UnicastAddr::new(Mac::new([0, 1, 2, 3, 4, self.index().try_into().unwrap()])).unwrap()
    }

    fn link_local_addr(self) -> IpDeviceAddr<Ipv6Addr> {
        match self {
            Self::First | Self::Second => SpecifiedAddr::new(Ipv6Addr::new([
                0xfe80,
                0,
                0,
                0,
                0,
                0,
                0,
                self.index().try_into().unwrap(),
            ]))
            .unwrap()
            .try_into()
            .unwrap(),
            Self::Loopback => panic!("should not generate link local addresses for loopback"),
        }
    }
}

fn remote_ip<I: TestIpExt>() -> SpecifiedAddr<I::Addr> {
    I::get_other_ip_address(27)
}

#[netstack3_macros::context_ip_bounds(I, FakeBindingsCtx)]
fn make_test_ctx<I: TestIpExt + IpExt>() -> (Ctx<FakeBindingsCtx>, Vec<DeviceId<FakeBindingsCtx>>) {
    let mut builder = FakeCtxBuilder::default();
    for device in [Device::First, Device::Second] {
        let ip: SpecifiedAddr<I::Addr> = device.ip_address().into();
        let subnet =
            AddrSubnet::from_witness(ip, <I::Addr as IpAddress>::BYTES * 8).unwrap().subnet();
        let index = builder.add_device_with_ip(device.mac(), ip.get(), subnet);
        assert_eq!(index, device.index());
    }
    let (mut ctx, device_ids) = builder.build();
    let mut device_ids = device_ids.into_iter().map(Into::into).collect::<Vec<_>>();

    if I::VERSION.is_v6() {
        for device in [Device::First, Device::Second] {
            ctx.core_api()
                .device_ip::<Ipv6>()
                .add_ip_addr_subnet(
                    &device_ids[device.index()],
                    AddrSubnet::new(device.link_local_addr().addr(), 64).unwrap(),
                )
                .unwrap();
        }
    }

    let loopback_id = ctx.test_api().add_loopback().into();
    assert_eq!(device_ids.len(), Device::Loopback.index());
    device_ids.push(loopback_id);
    (ctx, device_ids)
}

#[netstack3_macros::context_ip_bounds(I, FakeBindingsCtx)]
fn do_route_lookup<I: IpExt>(
    ctx: &mut FakeCtx,
    device_ids: Vec<DeviceId<FakeBindingsCtx>>,
    egress_device: Option<Device>,
    local_ip: Option<IpDeviceAddr<I::Addr>>,
    dest_ip: RoutableIpAddr<I::Addr>,
    transparent: bool,
) -> Result<ResolvedRoute<I, Device>, ResolveRouteError> {
    let egress_device = egress_device.map(|d| &device_ids[d.index()]);

    let (mut core_ctx, bindings_ctx) = ctx.contexts();
    IpSocketContext::<I, _>::lookup_route(
        &mut core_ctx,
        bindings_ctx,
        egress_device,
        local_ip,
        dest_ip,
        transparent,
        &Marks::default(),
    )
    // Convert device IDs in any route so it's easier to compare.
    .map(
        |ResolvedRoute {
             src_addr,
             device,
             local_delivery_device,
             next_hop,
             internal_forwarding,
         }| {
            let device = Device::from_index(device_ids.iter().position(|d| d == &device).unwrap());
            let internal_forwarding = internal_forwarding.map_device(|ingress_device| {
                Device::from_index(device_ids.iter().position(|d| d == &ingress_device).unwrap())
            });
            let local_delivery_device = local_delivery_device.map(|device| {
                Device::from_index(device_ids.iter().position(|d| d == &device).unwrap())
            });

            ResolvedRoute { src_addr, device, local_delivery_device, next_hop, internal_forwarding }
        },
    )
}

#[netstack3_macros::context_ip_bounds(I, FakeBindingsCtx)]
#[ip_test(I)]
#[test_case(None,
                None,
                Device::First.ip_address().into(),
                false,
                Ok(ResolvedRoute { src_addr: Device::First.ip_address(), device: Device::Loopback,
                    local_delivery_device: Some(Device::First), next_hop: NextHop::RemoteAsNeighbor,
                    internal_forwarding: InternalForwarding::NotUsed
                }); "local delivery")]
#[test_case(Some(Device::First.ip_address()),
                None,
                Device::First.ip_address().into(),
                false,
                Ok(ResolvedRoute { src_addr: Device::First.ip_address(), device: Device::Loopback,
                    local_delivery_device: Some(Device::First), next_hop: NextHop::RemoteAsNeighbor,
                    internal_forwarding: InternalForwarding::NotUsed
                }); "local delivery specified local addr")]
#[test_case(Some(Device::First.ip_address()),
                Some(Device::First),
                Device::First.ip_address().into(),
                false,
                Ok(ResolvedRoute { src_addr: Device::First.ip_address(), device: Device::Loopback,
                    local_delivery_device: Some(Device::First), next_hop: NextHop::RemoteAsNeighbor,
                    internal_forwarding: InternalForwarding::NotUsed
                }); "local delivery specified device and addr")]
#[test_case(None,
                Some(Device::Loopback),
                Device::First.ip_address().into(),
                false,
                Err(ResolveRouteError::Unreachable);
                "local delivery specified loopback device no addr")]
#[test_case(None,
                Some(Device::Loopback),
                Device::Loopback.ip_address().into(),
                false,
                Ok(ResolvedRoute { src_addr: Device::Loopback.ip_address(),
                    device: Device::Loopback, local_delivery_device: Some(Device::Loopback),
                    next_hop: NextHop::RemoteAsNeighbor,
                    internal_forwarding: InternalForwarding::NotUsed
                }); "local delivery to loopback addr via specified loopback device no addr")]
#[test_case(None,
                Some(Device::Second),
                Device::First.ip_address().into(),
                false,
                Err(ResolveRouteError::Unreachable);
                "local delivery specified mismatched device no addr")]
#[test_case(Some(Device::First.ip_address()),
                Some(Device::Loopback),
                Device::First.ip_address().into(),
                false,
                Err(ResolveRouteError::Unreachable); "local delivery specified loopback device")]
#[test_case(Some(Device::First.ip_address()),
                Some(Device::Second),
                Device::First.ip_address().into(),
                false,
                Err(ResolveRouteError::Unreachable); "local delivery specified mismatched device")]
#[test_case(None,
                None,
                remote_ip::<I>().try_into().unwrap(),
                false,
                Ok(ResolvedRoute { src_addr: Device::First.ip_address(), device: Device::First,
                    local_delivery_device: None, next_hop: NextHop::RemoteAsNeighbor,
                    internal_forwarding: InternalForwarding::NotUsed
                });
                "remote delivery")]
#[test_case(Some(Device::First.ip_address()),
                None,
                remote_ip::<I>().try_into().unwrap(),
                false,
                Ok(ResolvedRoute { src_addr: Device::First.ip_address(), device: Device::First,
                    local_delivery_device: None, next_hop: NextHop::RemoteAsNeighbor,
                    internal_forwarding: InternalForwarding::NotUsed
                });
                "remote delivery specified addr")]
#[test_case(Some(Device::Second.ip_address()),
                None,
                remote_ip::<I>().try_into().unwrap(),
                false,
                Err(ResolveRouteError::NoSrcAddr);
                "remote delivery specified addr no route"
)]
#[test_case(Some(remote_ip::<I>().try_into().unwrap()),
                None,
                Device::Loopback.ip_address().into(),
                true /* transparent */,
                Ok(ResolvedRoute { src_addr: remote_ip::<I>().try_into().unwrap(),
                    device: Device::Loopback, local_delivery_device: Some(Device::Loopback),
                    next_hop: NextHop::RemoteAsNeighbor,
                    internal_forwarding: InternalForwarding::NotUsed
                });
                "transparent local delivery from specified remote addr"
)]
#[test_case(Some(remote_ip::<I>().try_into().unwrap()),
                None,
                Device::First.ip_address().into(),
                true /* transparent */,
                Ok(ResolvedRoute { src_addr: remote_ip::<I>().try_into().unwrap(),
                    device: Device::Loopback, local_delivery_device: Some(Device::First),
                    next_hop: NextHop::RemoteAsNeighbor,
                    internal_forwarding: InternalForwarding::NotUsed
                });
                "transparent remote delivery from specified remote addr"
)]
#[test_case(None,
                Some(Device::First),
                remote_ip::<I>().try_into().unwrap(),
                false,
                Ok(ResolvedRoute { src_addr: Device::First.ip_address(), device: Device::First,
                    local_delivery_device: None, next_hop: NextHop::RemoteAsNeighbor,
                    internal_forwarding: InternalForwarding::NotUsed
                });
                "remote delivery specified device")]
#[test_case(None, Some(Device::Second), remote_ip::<I>().try_into().unwrap(), false,
                Err(ResolveRouteError::Unreachable); "remote delivery specified device no route")]
#[test_case(Some(Device::Second.ip_address()),
                None,
                Device::First.ip_address().into(),
                false,
                Ok(ResolvedRoute {src_addr: Device::Second.ip_address(), device: Device::Loopback,
                    local_delivery_device: Some(Device::First),
                    next_hop: NextHop::RemoteAsNeighbor,
                    internal_forwarding: InternalForwarding::NotUsed
                });
                "local delivery cross device")]
fn lookup_route<I: TestIpExt + IpExt>(
    local_ip: Option<IpDeviceAddr<I::Addr>>,
    egress_device: Option<Device>,
    dest_ip: RoutableIpAddr<I::Addr>,
    transparent: bool,
    expected_result: Result<ResolvedRoute<I, Device>, ResolveRouteError>,
) {
    set_logger_for_test();

    let (mut ctx, device_ids) = make_test_ctx::<I>();

    // Add a route to the remote address only for Device::First.
    ctx.test_api()
        .add_route(AddableEntryEither::without_gateway(
            Subnet::new(*remote_ip::<I>(), <I::Addr as IpAddress>::BYTES * 8).unwrap().into(),
            device_ids[Device::First.index()].clone(),
            AddableMetric::ExplicitMetric(RawMetric(0)),
        ))
        .unwrap();

    let result =
        do_route_lookup(&mut ctx, device_ids, egress_device, local_ip, dest_ip, transparent);
    assert_eq!(result, expected_result);
}

#[netstack3_macros::context_ip_bounds(I, FakeBindingsCtx)]
#[ip_test(I)]
#[test_case(None,
                None,
                Ok(ResolvedRoute { src_addr: Device::First.ip_address(), device: Device::First,
                    local_delivery_device: None, next_hop: NextHop::RemoteAsNeighbor,
                    internal_forwarding: InternalForwarding::NotUsed
                });
                "no constraints")]
#[test_case(Some(Device::First.ip_address()),
                None,
                Ok(ResolvedRoute { src_addr: Device::First.ip_address(), device: Device::First,
                    local_delivery_device: None, next_hop: NextHop::RemoteAsNeighbor,
                    internal_forwarding: InternalForwarding::NotUsed
                });
                "constrain local addr")]
#[test_case(Some(Device::Second.ip_address()), None,
                Ok(ResolvedRoute { src_addr: Device::Second.ip_address(), device: Device::Second,
                    local_delivery_device: None, next_hop: NextHop::RemoteAsNeighbor,
                    internal_forwarding: InternalForwarding::NotUsed
                });
                "constrain local addr to second device")]
#[test_case(None,
                Some(Device::First),
                Ok(ResolvedRoute { src_addr: Device::First.ip_address(), device: Device::First,
                    local_delivery_device: None, next_hop: NextHop::RemoteAsNeighbor,
                    internal_forwarding: InternalForwarding::NotUsed
                });
                "constrain device")]
#[test_case(None, Some(Device::Second),
                Ok(ResolvedRoute { src_addr: Device::Second.ip_address(), device: Device::Second,
                    local_delivery_device: None, next_hop: NextHop::RemoteAsNeighbor,
                    internal_forwarding: InternalForwarding::NotUsed
                });
                "constrain to second device")]
fn lookup_route_multiple_devices_with_route<I: TestIpExt + IpExt>(
    local_ip: Option<IpDeviceAddr<I::Addr>>,
    egress_device: Option<Device>,
    expected_result: Result<ResolvedRoute<I, Device>, ResolveRouteError>,
) {
    set_logger_for_test();

    let (mut ctx, device_ids) = make_test_ctx::<I>();

    // Add a route to the remote address for both devices, with preference
    // for the first.
    for device in [Device::First, Device::Second] {
        ctx.test_api()
            .add_route(AddableEntryEither::without_gateway(
                Subnet::new(*remote_ip::<I>(), <I::Addr as IpAddress>::BYTES * 8).unwrap().into(),
                device_ids[device.index()].clone(),
                AddableMetric::ExplicitMetric(RawMetric(device.index().try_into().unwrap())),
            ))
            .unwrap();
    }

    let result = do_route_lookup(
        &mut ctx,
        device_ids,
        egress_device,
        local_ip,
        remote_ip::<I>().try_into().unwrap(),
        false, /* transparent */
    );
    assert_eq!(result, expected_result);
}

#[test_case(None, None, Device::Second.link_local_addr().into(),
                Ok(ResolvedRoute { src_addr: Device::Second.link_local_addr(),
                    device: Device::Loopback, local_delivery_device: Some(Device::Second),
                    next_hop: NextHop::RemoteAsNeighbor,
                    internal_forwarding: InternalForwarding::NotUsed });
                "local delivery no local address to link-local")]
#[test_case(Some(Device::Second.ip_address()), None, Device::Second.link_local_addr().into(),
                Ok(ResolvedRoute { src_addr: Device::Second.ip_address(), device: Device::Loopback,
                    local_delivery_device: Some(Device::Second),
                    next_hop: NextHop::RemoteAsNeighbor,
                    internal_forwarding: InternalForwarding::NotUsed });
                "local delivery same device to link-local")]
#[test_case(Some(Device::Second.link_local_addr()), None, Device::Second.ip_address().into(),
                Ok(ResolvedRoute { src_addr: Device::Second.link_local_addr(),
                    device: Device::Loopback, local_delivery_device: Some(Device::Second),
                    next_hop: NextHop::RemoteAsNeighbor,
                    internal_forwarding: InternalForwarding::NotUsed });
                "local delivery same device from link-local")]
#[test_case(Some(Device::First.ip_address()), None, Device::Second.link_local_addr().into(),
                Err(ResolveRouteError::NoSrcAddr);
                "local delivery cross device to link-local")]
#[test_case(Some(Device::First.link_local_addr()), None, Device::Second.ip_address().into(),
                Err(ResolveRouteError::NoSrcAddr);
                "local delivery cross device from link-local")]
fn lookup_route_v6only(
    local_ip: Option<IpDeviceAddr<Ipv6Addr>>,
    egress_device: Option<Device>,
    dest_ip: RoutableIpAddr<Ipv6Addr>,
    expected_result: Result<ResolvedRoute<Ipv6, Device>, ResolveRouteError>,
) {
    set_logger_for_test();

    let (mut ctx, device_ids) = make_test_ctx::<Ipv6>();

    let result = do_route_lookup(
        &mut ctx,
        device_ids,
        egress_device,
        local_ip,
        dest_ip,
        false, /* transparent */
    );
    assert_eq!(result, expected_result);
}

#[test_case(net_ip_v4!("127.0.0.1"), Ipv4PresentAddressStatus::UnicastAssigned)]
#[test_case(net_ip_v4!("127.0.0.2"), Ipv4PresentAddressStatus::LoopbackSubnet)]
#[test_case(net_ip_v4!("127.255.255.255"), Ipv4PresentAddressStatus::SubnetBroadcast)]
fn loopback_assignment_state_v4(addr: Ipv4Addr, status: Ipv4PresentAddressStatus) {
    set_logger_for_test();

    // Initialize a fake Ctx with a loopback device.
    let builder = FakeCtxBuilder::default();
    let (mut ctx, _device_ids) = builder.build();
    let loopback_id = ctx.test_api().add_loopback().into();

    let addr = SpecifiedAddr::new(addr).expect("test cases should provide specified addrs");
    assert_eq!(
        IpDeviceIngressStateContext::<Ipv4>::address_status_for_device(
            &mut ctx.core_ctx(),
            addr,
            &loopback_id
        ),
        AddressStatus::Present(status)
    );
}

enum ActionAffectingConntrack {
    RemoveConnectionAfterEgress,
    ModifyPacketAfterEgress,
}

#[netstack3_core::context_ip_bounds(I, FakeBindingsCtx)]
#[ip_test(I)]
#[test_case(None; "reuse existing connection")]
#[test_case(
    Some(ActionAffectingConntrack::RemoveConnectionAfterEgress);
    "new connection because connection removed after egress"
)]
#[test_case(
    Some(ActionAffectingConntrack::ModifyPacketAfterEgress);
    "new connection because packet modified at device layer"
)]
fn conntrack_entry_retained_across_loopback<I: TestDualStackIpExt + IpExt>(
    action: Option<ActionAffectingConntrack>,
) where
    for<'a> CoreCtx<'a, FakeBindingsCtx, lock_order::Unlocked>: FilterIpContext<I, FakeBindingsCtx>,
    for<'a> CoreCtx<'a, FakeBindingsCtx, lock_order::Unlocked>:
        FilterIpContext<I::OtherVersion, FakeBindingsCtx>,
{
    set_logger_for_test();

    let (mut ctx, _device_ids) = FakeCtxBuilder::default().build();
    let loopback = ctx.test_api().add_loopback();

    // Add default routes through loopback for IPv4 and IPv6.
    ctx.test_api()
        .add_route(AddableEntryEither::without_gateway(
            Ipv4::ALL_ADDRS_SUBNET.into(),
            loopback.clone().into(),
            AddableMetric::MetricTracksInterface,
        ))
        .unwrap();
    ctx.test_api()
        .add_route(AddableEntryEither::without_gateway(
            Ipv6::ALL_ADDRS_SUBNET.into(),
            loopback.clone().into(),
            AddableMetric::MetricTracksInterface,
        ))
        .unwrap();

    // Add a Redirect rule in the LOCAL_EGRESS NAT hook.
    fn routines<I: IpExt>() -> Routines<I, FakeBindingsCtx, &'static str> {
        Routines {
            nat: NatRoutines {
                local_egress: Hook {
                    routines: vec![Routine {
                        rules: vec![Rule {
                            matcher: PacketMatcher::default(),
                            action: Action::Redirect { dst_port: None },
                            validation_info: "redirect in local egress",
                        }],
                    }],
                },
                ..Default::default()
            },
            ip: IpRoutines::default(),
        }
    }
    ctx.core_api()
        .filter()
        .set_filter_state(routines(), routines())
        .expect("install redirect rule");

    // Create a dual-stack listening socket.
    const LISTENER_PORT: NonZeroU16 = NonZeroU16::new(33333).unwrap();
    let mut v6_api = ctx.core_api().udp::<Ipv6>();
    let listener = v6_api.create();
    v6_api.set_dual_stack_enabled(&listener, true).unwrap();
    v6_api.listen(&listener, None, Some(LISTENER_PORT)).unwrap();

    // Send a packet to a remote destination. It should be redirected to localhost.
    let TestAddrs::<I::Addr> { remote_ip, .. } = I::TEST_ADDRS;
    let mut udp_api = ctx.core_api().udp();
    let socket = udp_api.create();
    const HELLO: &'static [u8] = b"Hello";
    udp_api
        .send_to(
            &socket,
            Some(ZonedAddr::Unzoned(remote_ip)),
            LISTENER_PORT.into(),
            Buf::new(HELLO.to_vec(), ..),
            Default::default(),
        )
        .unwrap();
    let local_port = match udp_api.get_info(&socket) {
        SocketInfo::Unbound | SocketInfo::Connected(_) => panic!("socket should be listener"),
        SocketInfo::Listener(ListenerInfo { local_identifier, .. }) => local_identifier,
    };

    fn original_tuple<I: TestIpExt>(local_port: NonZeroU16) -> Tuple<I> {
        Tuple {
            protocol: TransportProtocol::Udp,
            src_addr: I::LOOPBACK_ADDRESS.get(),
            dst_addr: I::TEST_ADDRS.remote_ip.get(),
            src_port_or_id: local_port.get(),
            dst_port_or_id: LISTENER_PORT.get(),
        }
    }

    fn assert_conntrack_contains_tuple<I: TestIpExt + FilterIpExt>(
        ctx: &mut FakeCtx,
        tuple: Tuple<I>,
    ) where
        for<'a> CoreCtx<'a, FakeBindingsCtx, lock_order::Unlocked>:
            FilterIpContext<I, FakeBindingsCtx>,
    {
        FilterIpContext::<I, _>::with_filter_state(
            &mut ctx.core_ctx(),
            |netstack3_filter::State { conntrack, .. }| {
                assert_eq!(conntrack.num_entries(), 2);
                assert!(conntrack.contains_tuple(&tuple));
            },
        );
    }

    match action {
        None => {
            // If neither the packet nor the conntrack table is modified after the packet
            // egresses to the device, the packet should retain its conntrack connection.
            assert!(ctx.test_api().handle_queued_rx_packets());
            assert_conntrack_contains_tuple::<I>(&mut ctx, original_tuple(local_port));
        }
        Some(ActionAffectingConntrack::RemoveConnectionAfterEgress) => {
            // If the connection is removed from the conntrack table after the packet
            // egresses to the device...
            assert_conntrack_contains_tuple::<I>(&mut ctx, original_tuple(local_port));
            {
                let conntrack = &mut ctx.core_ctx.common_ip::<I>().filter().write().conntrack;
                assert_matches!(conntrack.remove_connection(&original_tuple(local_port)), Some(_));
                assert_eq!(conntrack.num_entries(), 0);
            }
            // ...then a new connection should be created for the packet (based on its post-
            // NAT state) when it is handled at the IP layer.
            assert!(ctx.test_api().handle_queued_rx_packets());
            assert_conntrack_contains_tuple::<I>(
                &mut ctx,
                Tuple { dst_addr: I::LOOPBACK_ADDRESS.get(), ..original_tuple(local_port) },
            );
        }
        Some(ActionAffectingConntrack::ModifyPacketAfterEgress) => {
            // If the packet is modified after egress such that it no longer matches the
            // type of its cached connection (such as the IP version changing)...
            let (mut core_ctx, bindings_ctx) = ctx.contexts();
            let (meta, _frame) = core_ctx
                .with_receive_queue_mut(&loopback, |queue| queue.take_frames())
                .next()
                .expect("one frame should be available");
            // Swap the IP version of the packet, but queue it with the same metadata (which
            // includes the cached conntrack entry).
            let frame = Buf::new(HELLO.to_vec(), ..)
                .wrap_in(UdpPacketBuilder::new(
                    I::OtherVersion::LOOPBACK_ADDRESS.get(),
                    I::OtherVersion::LOOPBACK_ADDRESS.get(),
                    Some(local_port),
                    LISTENER_PORT,
                ))
                .wrap_in(<I::OtherVersion as packet_formats::ip::IpExt>::PacketBuilder::new(
                    I::OtherVersion::LOOPBACK_ADDRESS.get(),
                    I::OtherVersion::LOOPBACK_ADDRESS.get(),
                    64,
                    IpProto::Udp.into(),
                ))
                .wrap_in(EthernetFrameBuilder::new(
                    Mac::UNSPECIFIED,
                    Mac::UNSPECIFIED,
                    I::OtherVersion::ETHER_TYPE,
                    ETHERNET_MIN_BODY_LEN_NO_TAG,
                ))
                .serialize_vec_outer(&mut NetworkSerializationContext::default())
                .ok()
                .unwrap()
                .into_inner();
            core_ctx
                .queue_rx_frame(bindings_ctx, &loopback, meta, frame)
                .expect("queue modified frame");
            assert!(ctx.test_api().handle_queued_rx_packets());

            // ...then a new connection should be created for the packet (in the conntrack
            // table for the new packet's IP version) when it is handled at the IP layer.
            assert_conntrack_contains_tuple::<I::OtherVersion>(
                &mut ctx,
                Tuple {
                    dst_addr: I::OtherVersion::LOOPBACK_ADDRESS.get(),
                    ..original_tuple(local_port)
                },
            );
            // The original connection should also still be in the conntrack table.
            assert_conntrack_contains_tuple::<I>(&mut ctx, original_tuple(local_port));
            assert_eq!(
                ctx.core_ctx
                    .common_ip::<I::OtherVersion>()
                    .counters()
                    .invalid_cached_conntrack_entry
                    .get(),
                1
            );
        }
    }

    // Regardless of changes to the packet or conntrack table, the packet should be
    // successfully redirected to the listener socket on ingress.
    assert_matches!(
        &ctx.bindings_ctx.take_udp_received(&listener)[..],
        [packet] => assert_eq!(packet, HELLO)
    );
    ctx.test_api().clear_routes_and_remove_device(loopback);
}

#[test]
fn test_receive_ipv6_mapped_dst() {
    let (mut ctx, device_ids) = FakeCtxBuilder::with_addrs(TEST_ADDRS_V6).build();
    let device: DeviceId<_> = device_ids[0].clone().into();

    ctx.test_api().set_unicast_forwarding_enabled::<Ipv6>(&device, true);

    let src = TEST_ADDRS_V6.remote_ip;
    let dst = net_ip_v6!("::ffff:192.0.2.1");

    let builder = Ipv6PacketBuilder::new(src.get(), dst, 1, IpProto::Udp.into());

    let buffer = builder
        .wrap_body(Buf::new(vec![0u8; 10], ..))
        .serialize_vec_outer(&mut NetworkSerializationContext::default())
        .unwrap()
        .unwrap_b();

    ctx.test_api().receive_ip_packet::<Ipv6, _>(
        &device,
        Some(LocalFrameDestination::Individual { local: () }),
        buffer,
    );

    // The packet should be dropped due to invalid destination address.
    assert_matches!(ctx.bindings_ctx.take_ethernet_frames()[..], []);
}
