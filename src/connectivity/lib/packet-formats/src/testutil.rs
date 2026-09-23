// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Testing-related utilities.

// TODO(https://fxbug.dev/326330182): this import seems actually necessary. Is this a bug on the
// lint?
#[allow(unused_imports)]
use alloc::vec::Vec;
use core::num::NonZeroU16;
use core::ops::Range;
use zerocopy::SplitByteSlice;

use log::debug;
use net_types::ethernet::Mac;
use net_types::ip::{Ipv4Addr, Ipv6Addr};
use packet::{ParsablePacket, ParseBuffer, SerializationContext, SliceBufViewMut};

use crate::TransportChecksumAction;
use crate::arp::{ArpOp, ArpPacket};
use crate::error::{IpParseResult, ParseError, ParseResult};
use crate::ethernet::{EtherType, EthernetFrame, EthernetFrameLengthCheck};
use crate::icmp::{IcmpIpExt, IcmpMessage, IcmpPacket, IcmpParseArgs, Icmpv6PacketRaw};
use crate::ip::{DscpAndEcn, IpExt, Ipv4Proto};
use crate::ipv4::{Ipv4FragmentType, Ipv4Header, Ipv4Packet};
use crate::ipv6::{Ipv6Header, Ipv6Packet};
use crate::tcp::options::{TcpOptions, TcpOptionsBuilder};
use crate::tcp::{TcpEnvelope, TcpParseContext, TcpSegment, TcpSerializationContext};
use crate::udp::{UdpEnvelope, UdpPacket, UdpParseContext, UdpSerializationContext};

#[cfg(test)]
pub(crate) use crateonly::*;

/// Metadata of an Ethernet frame.
#[allow(missing_docs)]
pub struct EthernetFrameMetadata {
    pub src_mac: Mac,
    pub dst_mac: Mac,
    pub ethertype: Option<EtherType>,
}

/// Metadata of an IPv4 packet.
#[allow(missing_docs)]
pub struct Ipv4PacketMetadata {
    pub id: u16,
    pub dscp_and_ecn: DscpAndEcn,
    pub dont_fragment: bool,
    pub more_fragments: bool,
    pub fragment_offset: u16,
    pub fragment_type: Ipv4FragmentType,
    pub ttl: u8,
    pub proto: Ipv4Proto,
    pub src_ip: Ipv4Addr,
    pub dst_ip: Ipv4Addr,
}

/// Metadata of an IPv6 packet.
#[allow(missing_docs)]
pub struct Ipv6PacketMetadata {
    pub dscp_and_ecn: DscpAndEcn,
    pub flowlabel: u32,
    pub hop_limit: u8,
    pub src_ip: Ipv6Addr,
    pub dst_ip: Ipv6Addr,
}

/// Metadata of a TCP segment.
#[allow(missing_docs)]
pub struct TcpSegmentMetadata {
    pub src_port: u16,
    pub dst_port: u16,
    pub seq_num: u32,
    pub ack_num: Option<u32>,
    pub flags: u16,
    pub psh: bool,
    pub rst: bool,
    pub syn: bool,
    pub fin: bool,
    pub window_size: u16,
    pub options: TcpOptionsBuilder<'static>,
}

/// Metadata of a UDP packet.
#[allow(missing_docs)]
pub struct UdpPacketMetadata {
    pub src_port: u16,
    pub dst_port: u16,
}

/// Represents a packet (usually from a live capture) used for testing.
///
/// Includes the raw bytes, metadata of the packet (currently just fields from the packet header)
/// and the range which indicates where the body is.
#[allow(missing_docs)]
pub struct TestPacket<M> {
    pub bytes: &'static [u8],
    pub metadata: M,
    pub body_range: Range<usize>,
}

/// Verify that a parsed Ethernet frame is as expected.
///
/// Ensures the parsed packet's header fields and body are equal to those in the test packet.
pub fn verify_ethernet_frame(
    frame: &EthernetFrame<&[u8]>,
    expected: TestPacket<EthernetFrameMetadata>,
) {
    assert_eq!(frame.src_mac(), expected.metadata.src_mac);
    assert_eq!(frame.dst_mac(), expected.metadata.dst_mac);
    assert_eq!(frame.ethertype(), expected.metadata.ethertype);
    assert_eq!(frame.body(), &expected.bytes[expected.body_range]);
}

/// Verify that a parsed IPv4 packet is as expected.
///
/// Ensures the parsed packet's header fields and body are equal to those in the test packet.
pub fn verify_ipv4_packet(packet: &Ipv4Packet<&[u8]>, expected: TestPacket<Ipv4PacketMetadata>) {
    assert_eq!(packet.dscp_and_ecn(), expected.metadata.dscp_and_ecn);
    assert_eq!(packet.id(), expected.metadata.id);
    assert_eq!(packet.df_flag(), expected.metadata.dont_fragment);
    assert_eq!(packet.mf_flag(), expected.metadata.more_fragments);
    assert_eq!(packet.fragment_offset().into_raw(), expected.metadata.fragment_offset);
    assert_eq!(packet.ttl(), expected.metadata.ttl);
    assert_eq!(packet.proto(), expected.metadata.proto);
    assert_eq!(packet.src_ip(), expected.metadata.src_ip);
    assert_eq!(packet.dst_ip(), expected.metadata.dst_ip);
    assert_eq!(packet.body(), &expected.bytes[expected.body_range]);
}

/// Verify that a parsed IPv6 packet is as expected.
///
/// Ensures the parsed packet's header fields and body are equal to those in the test packet.
pub fn verify_ipv6_packet(packet: &Ipv6Packet<&[u8]>, expected: TestPacket<Ipv6PacketMetadata>) {
    assert_eq!(packet.dscp_and_ecn(), expected.metadata.dscp_and_ecn);
    assert_eq!(packet.flowlabel(), expected.metadata.flowlabel);
    assert_eq!(packet.hop_limit(), expected.metadata.hop_limit);
    assert_eq!(packet.src_ip(), expected.metadata.src_ip);
    assert_eq!(packet.dst_ip(), expected.metadata.dst_ip);
    assert_eq!(packet.body(), &expected.bytes[expected.body_range]);
}

/// Verify that a parsed UDP packet is as expected.
///
/// Ensures the parsed packet's header fields and body are equal to those in the test packet.
pub fn verify_udp_packet(packet: &UdpPacket<&[u8]>, expected: TestPacket<UdpPacketMetadata>) {
    assert_eq!(packet.src_port().map(NonZeroU16::get).unwrap_or(0), expected.metadata.src_port);
    assert_eq!(packet.dst_port().get(), expected.metadata.dst_port);
    assert_eq!(packet.body(), &expected.bytes[expected.body_range]);
}

/// Verify that a parsed TCP segment is as expected.
///
/// Ensures the parsed packet's header fields and body are equal to those in the test packet.
pub fn verify_tcp_segment(segment: &TcpSegment<&[u8]>, expected: TestPacket<TcpSegmentMetadata>) {
    assert_eq!(segment.src_port().get(), expected.metadata.src_port);
    assert_eq!(segment.dst_port().get(), expected.metadata.dst_port);
    assert_eq!(segment.seq_num(), expected.metadata.seq_num);
    assert_eq!(segment.ack_num(), expected.metadata.ack_num);
    assert_eq!(segment.rst(), expected.metadata.rst);
    assert_eq!(segment.syn(), expected.metadata.syn);
    assert_eq!(segment.fin(), expected.metadata.fin);
    assert_eq!(segment.window_size(), expected.metadata.window_size);

    let TcpOptionsBuilder { mss, window_scale, sack_permitted, sack_blocks, timestamp } =
        expected.metadata.options;
    assert_eq!(segment.options().mss(), mss);
    assert_eq!(segment.options().window_scale(), window_scale);
    assert_eq!(segment.options().sack_permitted(), sack_permitted);
    assert_eq!(segment.options().sack_blocks(), sack_blocks);
    assert_eq!(segment.options().timestamp(), timestamp.as_ref());

    assert_eq!(segment.body(), &expected.bytes[expected.body_range]);
}

/// Parse an ethernet frame.
///
/// `parse_ethernet_frame` parses an ethernet frame, returning the body along
/// with some important header fields.
pub fn parse_ethernet_frame(
    mut buf: &[u8],
    ethernet_length_check: EthernetFrameLengthCheck,
) -> ParseResult<(&[u8], Mac, Mac, Option<EtherType>)> {
    let frame = (&mut buf).parse_with::<_, EthernetFrame<_>>(ethernet_length_check)?;
    let src_mac = frame.src_mac();
    let dst_mac = frame.dst_mac();
    let ethertype = frame.ethertype();
    Ok((buf, src_mac, dst_mac, ethertype))
}

/// Information about an [`ArpPacket`].
#[allow(missing_docs)]
pub struct ArpPacketInfo {
    pub sender_hardware_address: Mac,
    pub sender_protocol_address: Ipv4Addr,
    pub target_hardware_address: Mac,
    pub target_protocol_address: Ipv4Addr,
    pub operation: ArpOp,
}

impl<B: SplitByteSlice> From<ArpPacket<B, Mac, Ipv4Addr>> for ArpPacketInfo {
    fn from(packet: ArpPacket<B, Mac, Ipv4Addr>) -> ArpPacketInfo {
        ArpPacketInfo {
            sender_hardware_address: packet.sender_hardware_address(),
            sender_protocol_address: packet.sender_protocol_address(),
            target_hardware_address: packet.target_hardware_address(),
            target_protocol_address: packet.target_protocol_address(),
            operation: packet.operation(),
        }
    }
}

/// Parse an ARP packet.
pub fn parse_arp_packet(mut buf: &[u8]) -> ParseResult<ArpPacketInfo> {
    (&mut buf).parse::<ArpPacket<_, Mac, Ipv4Addr>>().map(ArpPacketInfo::from)
}

/// Parse an ARP packet in an Ethernet frame.
pub fn parse_arp_packet_in_ethernet_frame(
    buf: &[u8],
    ethernet_length_check: EthernetFrameLengthCheck,
) -> ParseResult<ArpPacketInfo> {
    let (body, _src_mac, _dst_mac, ethertype) = parse_ethernet_frame(buf, ethernet_length_check)?;
    if ethertype != Some(EtherType::Arp) {
        debug!("unexpected ethertype: {:?}", ethertype);
        return Err(ParseError::NotExpected.into());
    }

    parse_arp_packet(body)
}

/// Parse an IP packet.
///
/// `parse_ip_packet` parses an IP packet, returning the body along with some
/// important header fields.
#[allow(clippy::type_complexity)]
pub fn parse_ip_packet<I: IpExt>(
    mut buf: &[u8],
) -> IpParseResult<I, (&[u8], I::Addr, I::Addr, I::Proto, u8)> {
    use crate::ip::IpPacket;

    let packet = (&mut buf).parse::<I::Packet<_>>().map_err(|e| {
        let e: I::PacketParseError = e;
        e
    })?;
    let src_ip = packet.src_ip();
    let dst_ip = packet.dst_ip();
    let proto = packet.proto();
    let ttl = packet.ttl();
    // Because the packet type here is generic, Rust doesn't know that it
    // doesn't implement Drop, and so it doesn't know that it's safe to drop as
    // soon as it's no longer used and allow buf to no longer be borrowed on the
    // next line. It works fine in parse_ethernet_frame because EthernetFrame is
    // a concrete type which Rust knows doesn't implement Drop.
    core::mem::drop(packet);
    Ok((buf, src_ip, dst_ip, proto, ttl))
}

/// Parse an ICMP packet.
///
/// `parse_icmp_packet` parses an ICMP packet, returning the body along with
/// some important fields. Before returning, it invokes the callback `f` on the
/// parsed packet.
pub fn parse_icmp_packet<
    I: IcmpIpExt,
    C,
    M: IcmpMessage<I, Code = C>,
    F: for<'a> FnOnce(&IcmpPacket<I, &'a [u8], M>),
>(
    mut buf: &[u8],
    src_ip: I::Addr,
    dst_ip: I::Addr,
    f: F,
) -> ParseResult<(M, C)>
where
    for<'a> IcmpPacket<I, &'a [u8], M>:
        ParsablePacket<&'a [u8], IcmpParseArgs<I::Addr>, Error = ParseError>,
{
    let packet =
        (&mut buf).parse_with::<_, IcmpPacket<I, _, M>>(IcmpParseArgs::new(src_ip, dst_ip))?;
    let message = *packet.message();
    let code = packet.code();
    f(&packet);
    Ok((message, code))
}

/// Parse an IP packet in an Ethernet frame.
///
/// `parse_ip_packet_in_ethernet_frame` parses an IP packet in an Ethernet
/// frame, returning the body of the IP packet along with some important fields
/// from both the IP and Ethernet headers.
#[allow(clippy::type_complexity)]
pub fn parse_ip_packet_in_ethernet_frame<I: IpExt>(
    buf: &[u8],
    ethernet_length_check: EthernetFrameLengthCheck,
) -> IpParseResult<I, (&[u8], Mac, Mac, I::Addr, I::Addr, I::Proto, u8)> {
    let (body, src_mac, dst_mac, ethertype) = parse_ethernet_frame(buf, ethernet_length_check)?;
    if ethertype != Some(I::ETHER_TYPE) {
        debug!("unexpected ethertype: {:?}", ethertype);
        return Err(ParseError::NotExpected.into());
    }

    let (body, src_ip, dst_ip, proto, ttl) = parse_ip_packet::<I>(body)?;
    Ok((body, src_mac, dst_mac, src_ip, dst_ip, proto, ttl))
}

/// Parse an ICMP packet in an IP packet in an Ethernet frame.
///
/// `parse_icmp_packet_in_ip_packet_in_ethernet_frame` parses an ICMP packet in
/// an IP packet in an Ethernet frame, returning the message and code from the
/// ICMP packet along with some important fields from both the IP and Ethernet
/// headers. Before returning, it invokes the callback `f` on the parsed packet.
#[allow(clippy::type_complexity)]
pub fn parse_icmp_packet_in_ip_packet_in_ethernet_frame<
    I: IpExt,
    C,
    M: IcmpMessage<I, Code = C>,
    F: for<'a> FnOnce(&IcmpPacket<I, &'a [u8], M>),
>(
    buf: &[u8],
    ethernet_length_check: EthernetFrameLengthCheck,
    f: F,
) -> IpParseResult<I, (Mac, Mac, I::Addr, I::Addr, u8, M, C)>
where
    for<'a> IcmpPacket<I, &'a [u8], M>:
        ParsablePacket<&'a [u8], IcmpParseArgs<I::Addr>, Error = ParseError>,
{
    let (body, src_mac, dst_mac, src_ip, dst_ip, proto, ttl) =
        parse_ip_packet_in_ethernet_frame::<I>(buf, ethernet_length_check)?;
    if proto != I::ICMP_IP_PROTO {
        debug!("unexpected IP protocol: {} (wanted {})", proto, I::ICMP_IP_PROTO);
        return Err(ParseError::NotExpected.into());
    }
    let (message, code) = parse_icmp_packet(body, src_ip, dst_ip, f)?;
    Ok((src_mac, dst_mac, src_ip, dst_ip, ttl, message, code))
}

/// Overwrite the checksum in an ICMPv6 message, returning the original value.
///
/// On `Err`, the provided buf is unmodified.
pub fn overwrite_icmpv6_checksum(buf: &mut [u8], checksum: [u8; 2]) -> ParseResult<[u8; 2]> {
    let buf = SliceBufViewMut::new(buf);
    let mut message = Icmpv6PacketRaw::parse_mut(buf, ())?;
    Ok(message.overwrite_checksum(checksum))
}

/// A [`TcpParseContext`] and [`UdpParseContext`] that forces a specific
/// checksum validation action to be used (or skipped).
pub struct ForceSkipChecksumValidation(pub bool);
impl UdpParseContext for ForceSkipChecksumValidation {
    fn verify_checksum_if_needed<E>(&mut self, f: impl FnOnce() -> Result<(), E>) -> Result<(), E> {
        if self.0 { Ok(()) } else { f() }
    }
}
impl TcpParseContext for ForceSkipChecksumValidation {
    fn verify_checksum_if_needed<E>(&mut self, f: impl FnOnce() -> Result<(), E>) -> Result<(), E> {
        if self.0 { Ok(()) } else { f() }
    }
}

/// A [`SerializationContext`] that forces a specific transport-layer
/// checksum action to be used.
pub struct ForceChecksumAction(pub TransportChecksumAction);
impl SerializationContext for ForceChecksumAction {
    type ContextState = ();
}
impl UdpSerializationContext for ForceChecksumAction {
    fn envelope_to_state(_envelope: UdpEnvelope) -> Self::ContextState {
        ()
    }
    fn checksum_action(&mut self) -> TransportChecksumAction {
        self.0
    }
}
impl TcpSerializationContext for ForceChecksumAction {
    fn envelope_to_state(_envelope: TcpEnvelope) -> Self::ContextState {
        ()
    }
    fn checksum_action(&mut self) -> TransportChecksumAction {
        self.0
    }
}

#[cfg(test)]
mod crateonly {
    use std::sync::Once;

    /// Install a logger for tests.
    ///
    /// Call this method at the beginning of the test for which logging is desired.
    /// This function sets global program state, so all tests that run after this
    /// function is called will use the logger.
    pub(crate) fn set_logger_for_test() {
        /// log::Log implementation that uses stdout.
        ///
        /// Useful when debugging tests.
        struct Logger;

        impl log::Log for Logger {
            fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
                true
            }

            fn log(&self, record: &log::Record<'_>) {
                println!("{}", record.args())
            }

            fn flush(&self) {}
        }
        static LOGGER_ONCE: Once = Once::new();

        // log::set_logger will panic if called multiple times; using a Once makes
        // set_logger_for_test idempotent
        LOGGER_ONCE.call_once(|| {
            log::set_logger(&Logger).unwrap();
            log::set_max_level(log::LevelFilter::Trace);
        })
    }
}

#[cfg(test)]
mod tests {
    use net_types::ip::{Ipv4, Ipv6};

    use crate::icmp::{IcmpDestUnreachable, IcmpEchoReply, Icmpv4DestUnreachableCode};
    use crate::ip::Ipv6Proto;

    use super::*;

    #[test]
    fn test_parse_ethernet_frame() {
        use crate::testdata::arp_request::*;
        let (body, src_mac, dst_mac, ethertype) =
            parse_ethernet_frame(ETHERNET_FRAME.bytes, EthernetFrameLengthCheck::Check).unwrap();
        assert_eq!(body, &ETHERNET_FRAME.bytes[14..]);
        assert_eq!(src_mac, ETHERNET_FRAME.metadata.src_mac);
        assert_eq!(dst_mac, ETHERNET_FRAME.metadata.dst_mac);
        assert_eq!(ethertype, ETHERNET_FRAME.metadata.ethertype);
    }

    #[test]
    fn test_parse_ip_packet() {
        use crate::testdata::icmp_redirect::IP_PACKET_BYTES;
        let (body, src_ip, dst_ip, proto, ttl) = parse_ip_packet::<Ipv4>(IP_PACKET_BYTES).unwrap();
        assert_eq!(body, &IP_PACKET_BYTES[20..]);
        assert_eq!(src_ip, Ipv4Addr::new([10, 123, 0, 2]));
        assert_eq!(dst_ip, Ipv4Addr::new([10, 123, 0, 1]));
        assert_eq!(proto, Ipv4Proto::Icmp);
        assert_eq!(ttl, 255);

        use crate::testdata::icmp_echo_v6::REQUEST_IP_PACKET_BYTES;
        let (body, src_ip, dst_ip, proto, ttl) =
            parse_ip_packet::<Ipv6>(REQUEST_IP_PACKET_BYTES).unwrap();
        assert_eq!(body, &REQUEST_IP_PACKET_BYTES[40..]);
        assert_eq!(src_ip, Ipv6Addr::new([0, 0, 0, 0, 0, 0, 0, 1]));
        assert_eq!(dst_ip, Ipv6Addr::new([0xfec0, 0, 0, 0, 0, 0, 0, 0]));
        assert_eq!(proto, Ipv6Proto::Icmpv6);
        assert_eq!(ttl, 64);
    }

    #[test]
    fn test_parse_ip_packet_in_ethernet_frame() {
        use crate::testdata::tls_client_hello_v4::*;
        let (body, src_mac, dst_mac, src_ip, dst_ip, proto, ttl) =
            parse_ip_packet_in_ethernet_frame::<Ipv4>(
                ETHERNET_FRAME.bytes,
                EthernetFrameLengthCheck::Check,
            )
            .unwrap();
        assert_eq!(body, &IPV4_PACKET.bytes[IPV4_PACKET.body_range]);
        assert_eq!(src_mac, ETHERNET_FRAME.metadata.src_mac);
        assert_eq!(dst_mac, ETHERNET_FRAME.metadata.dst_mac);
        assert_eq!(src_ip, IPV4_PACKET.metadata.src_ip);
        assert_eq!(dst_ip, IPV4_PACKET.metadata.dst_ip);
        assert_eq!(proto, IPV4_PACKET.metadata.proto);
        assert_eq!(ttl, IPV4_PACKET.metadata.ttl);
    }

    #[test]
    fn test_parse_icmp_packet() {
        set_logger_for_test();
        use crate::testdata::icmp_dest_unreachable::*;
        let (body, ..) = parse_ip_packet::<Ipv4>(&IP_PACKET_BYTES).unwrap();
        let (_, code) = parse_icmp_packet::<Ipv4, _, IcmpDestUnreachable, _>(
            body,
            Ipv4Addr::new([172, 217, 6, 46]),
            Ipv4Addr::new([192, 168, 0, 105]),
            |_| {},
        )
        .unwrap();
        assert_eq!(code, Icmpv4DestUnreachableCode::DestHostUnreachable);
    }

    #[test]
    fn test_parse_icmp_packet_in_ip_packet_in_ethernet_frame() {
        set_logger_for_test();
        use crate::testdata::icmp_echo_ethernet::*;
        let (src_mac, dst_mac, src_ip, dst_ip, _, _, _) =
            parse_icmp_packet_in_ip_packet_in_ethernet_frame::<Ipv4, _, IcmpEchoReply, _>(
                &REPLY_ETHERNET_FRAME_BYTES,
                EthernetFrameLengthCheck::Check,
                |_| {},
            )
            .unwrap();
        assert_eq!(src_mac, Mac::new([0x50, 0xc7, 0xbf, 0x1d, 0xf4, 0xd2]));
        assert_eq!(dst_mac, Mac::new([0x8c, 0x85, 0x90, 0xc9, 0xc9, 0x00]));
        assert_eq!(src_ip, Ipv4Addr::new([172, 217, 6, 46]));
        assert_eq!(dst_ip, Ipv4Addr::new([192, 168, 0, 105]));
    }
}
