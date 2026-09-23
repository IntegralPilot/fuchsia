// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Parsing and serialization of IPv4 packets.
//!
//! The IPv4 packet format is defined in [RFC 791 Section 3.1].
//!
//! [RFC 791 Section 3.1]: https://datatracker.ietf.org/doc/html/rfc791#section-3.1

use alloc::vec::Vec;
use core::borrow::Borrow;
use core::fmt::{self, Debug, Formatter};
use core::ops::Range;

use internet_checksum::Checksum;
use log::debug;
use net_types::ip::{GenericOverIp, IpAddress, Ipv4, Ipv4Addr, Ipv4SourceAddr, Ipv6Addr};
use packet::records::RecordsIter;
use packet::records::options::{OptionSequenceBuilder, OptionsRaw};
use packet::{
    BufferProvider, BufferView, BufferViewMut, EmptyBuf, FragmentedBytesMut, FromRaw,
    GrowBufferMut, InnerPacketBuilder, LayoutBufferAlloc, MaybeParsed, NestablePacketBuilder,
    NestableSerializer, NoOpSerializationContext, PacketBuilder, PacketConstraints, ParsablePacket,
    ParseMetadata, PartialPacketBuilder, PartialSerializer, SerializeError, SerializeTarget,
    Serializer,
};
use zerocopy::byteorder::network_endian::U16;
use zerocopy::{
    FromBytes, Immutable, IntoBytes, KnownLayout, Ref, SplitByteSlice, SplitByteSliceMut, Unaligned,
};

use crate::TRANSPORT_HEADER_MAX_SIZE;
use crate::error::ParseError;
use crate::ip::{
    DscpAndEcn, FragmentOffset, IpEnvelope, IpExt, IpPacketBuilder, IpProto,
    IpSerializationContext, Ipv4Proto, Ipv6Proto, Nat64Error, Nat64TranslationResult,
};
use crate::ipv6::Ipv6PacketBuilder;
use crate::tcp::{TcpParseArgs, TcpSegment};
use crate::udp::{UdpPacket, UdpParseArgs};

pub(crate) use self::inner::IPV4_MIN_HDR_LEN;
use self::options::{Ipv4Option, Ipv4OptionsImpl};

/// The length of the fixed prefix of an IPv4 header (preceding the options).
pub const HDR_PREFIX_LEN: usize = 20;

/// The maximum length of an IPv4 header.
pub const MAX_HDR_LEN: usize = 60;

/// The maximum length for options in an IPv4 header.
pub const MAX_OPTIONS_LEN: usize = MAX_HDR_LEN - HDR_PREFIX_LEN;

/// The range of bytes within an IPv4 header buffer that the fragment data fields uses.
const IPV4_FRAGMENT_DATA_BYTE_RANGE: Range<usize> = 4..8;

/// The type of an IPv4 packet fragment.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum Ipv4FragmentType {
    InitialFragment,
    NonInitialFragment,
}

/// The prefix of the IPv4 header which precedes any header options and the
/// body.
#[derive(KnownLayout, FromBytes, IntoBytes, Immutable, Unaligned)]
#[repr(C)]
pub struct HeaderPrefix {
    version_ihl: u8,
    dscp_and_ecn: DscpAndEcn,
    total_len: U16,
    id: U16,
    flags_frag_off: [u8; 2],
    ttl: u8,
    proto: u8,
    hdr_checksum: [u8; 2],
    src_ip: Ipv4Addr,
    dst_ip: Ipv4Addr,
}

const IP_VERSION: u8 = 4;
const VERSION_OFFSET: u8 = 4;
const IHL_MASK: u8 = 0xF;
const IHL_MAX: u8 = (1 << VERSION_OFFSET) - 1;
const FLAGS_OFFSET: u8 = 13;
const FLAGS_MAX: u8 = (1 << (16 - FLAGS_OFFSET)) - 1;
const FRAG_OFF_MAX: u16 = (1 << FLAGS_OFFSET) - 1;

impl HeaderPrefix {
    #[allow(clippy::too_many_arguments)]
    fn new(
        ihl: u8,
        dscp_and_ecn: DscpAndEcn,
        total_len: u16,
        id: u16,
        flags: u8,
        frag_off: u16,
        ttl: u8,
        proto: u8,
        hdr_checksum: [u8; 2],
        src_ip: Ipv4Addr,
        dst_ip: Ipv4Addr,
    ) -> HeaderPrefix {
        debug_assert!(ihl <= IHL_MAX);
        debug_assert!(flags <= FLAGS_MAX);
        debug_assert!(frag_off <= FRAG_OFF_MAX);

        HeaderPrefix {
            version_ihl: (IP_VERSION << VERSION_OFFSET) | ihl,
            dscp_and_ecn,
            total_len: U16::new(total_len),
            id: U16::new(id),
            flags_frag_off: ((u16::from(flags) << FLAGS_OFFSET) | frag_off).to_be_bytes(),
            ttl,
            proto,
            src_ip,
            dst_ip,
            hdr_checksum,
        }
    }

    fn version(&self) -> u8 {
        self.version_ihl >> VERSION_OFFSET
    }

    /// Get the Internet Header Length (IHL).
    pub(crate) fn ihl(&self) -> u8 {
        self.version_ihl & IHL_MASK
    }

    /// The More Fragments (MF) flag.
    pub(crate) fn mf_flag(&self) -> bool {
        // `FLAGS_OFFSET` refers to the offset within the 2-byte array
        // containing both the flags and the fragment offset. Since we're
        // accessing the first byte directly, we shift by an extra `FLAGS_OFFSET
        // - 8` bits, not by an extra `FLAGS_OFFSET` bits.
        self.flags_frag_off[0] & (1 << ((FLAGS_OFFSET - 8) + MF_FLAG_OFFSET)) > 0
    }
}

/// Provides common access to IPv4 header fields.
///
/// `Ipv4Header` provides access to IPv4 header fields as a common
/// implementation for both [`Ipv4Packet`] and [`Ipv4PacketRaw`].
pub trait Ipv4Header {
    /// Gets a reference to the IPv4 [`HeaderPrefix`].
    fn get_header_prefix(&self) -> &HeaderPrefix;

    /// The Differentiated Services Code Point (DSCP) and the Explicit Congestion Notification (ECN).
    fn dscp_and_ecn(&self) -> DscpAndEcn {
        self.get_header_prefix().dscp_and_ecn
    }

    /// The identification.
    fn id(&self) -> u16 {
        self.get_header_prefix().id.get()
    }

    /// The Don't Fragment (DF) flag.
    fn df_flag(&self) -> bool {
        // the flags are the top 3 bits, so we need to shift by an extra 5 bits
        self.get_header_prefix().flags_frag_off[0] & (1 << (5 + DF_FLAG_OFFSET)) > 0
    }

    /// The More Fragments (MF) flag.
    fn mf_flag(&self) -> bool {
        self.get_header_prefix().mf_flag()
    }

    /// The fragment offset.
    fn fragment_offset(&self) -> FragmentOffset {
        FragmentOffset::new_with_lsb(U16::from_bytes(self.get_header_prefix().flags_frag_off).get())
    }

    /// The fragment type.
    ///
    /// `p.fragment_type()` returns [`Ipv4FragmentType::InitialFragment`] if
    /// `p.fragment_offset() == 0` and [`Ipv4FragmentType::NonInitialFragment`]
    /// otherwise.
    fn fragment_type(&self) -> Ipv4FragmentType {
        match self.fragment_offset().into_raw() {
            0 => Ipv4FragmentType::InitialFragment,
            _ => Ipv4FragmentType::NonInitialFragment,
        }
    }

    /// The Time To Live (TTL).
    fn ttl(&self) -> u8 {
        self.get_header_prefix().ttl
    }

    /// The IP Protocol.
    ///
    /// `proto` returns the `Ipv4Proto` from the protocol field.
    fn proto(&self) -> Ipv4Proto {
        Ipv4Proto::from(self.get_header_prefix().proto)
    }

    /// The source IP address.
    fn src_ip(&self) -> Ipv4Addr {
        self.get_header_prefix().src_ip
    }

    /// The destination IP address.
    fn dst_ip(&self) -> Ipv4Addr {
        self.get_header_prefix().dst_ip
    }

    /// Construct a builder with the same contents as this header.
    fn builder(&self) -> Ipv4PacketBuilder {
        let mut s = Ipv4PacketBuilder {
            id: self.id(),
            dscp_and_ecn: self.dscp_and_ecn(),
            flags: 0,
            frag_off: self.fragment_offset().into_raw(),
            ttl: self.ttl(),
            proto: self.get_header_prefix().proto.into(),
            src_ip: self.src_ip(),
            dst_ip: self.dst_ip(),
        };
        s.df_flag(self.df_flag());
        s.mf_flag(self.mf_flag());
        s
    }
}

impl Ipv4Header for HeaderPrefix {
    fn get_header_prefix(&self) -> &HeaderPrefix {
        self
    }
}

/// An IPv4 packet.
///
/// An `Ipv4Packet` shares its underlying memory with the byte slice it was
/// parsed from or serialized to, meaning that no copying or extra allocation is
/// necessary.
///
/// An `Ipv4Packet` - whether parsed using `parse` or created using
/// `Ipv4PacketBuilder` - maintains the invariant that the checksum is always
/// valid.
pub struct Ipv4Packet<B> {
    hdr_prefix: Ref<B, HeaderPrefix>,
    options: Options<B>,
    body: B,
}

impl<B: SplitByteSlice, I: IpExt> GenericOverIp<I> for Ipv4Packet<B> {
    type Type = <I as IpExt>::Packet<B>;
}

impl<B: SplitByteSlice> Ipv4Header for Ipv4Packet<B> {
    fn get_header_prefix(&self) -> &HeaderPrefix {
        &self.hdr_prefix
    }
}

impl<B: SplitByteSlice, C: IpSerializationContext<Ipv4>> PartialSerializer<C> for Ipv4Packet<B> {
    // TODO(https://fxbug.dev/473824085): Keep the reference to the whole
    // serialized packet and return it from `partial_serialize()` as
    // `PartialSerializeResult::Slice`.

    fn partial_serialize_new_buf<BB: GrowBufferMut, A: LayoutBufferAlloc<BB>>(
        &self,
        _context: &mut C,
        constraints: PacketConstraints,
        alloc: A,
    ) -> Result<(BB, usize), SerializeError<A::Error>> {
        // Copy IP header, extension header and up to 64 bytes of the body,
        // which includes the transport headers.
        let hdr_prefix = Ref::bytes(&self.hdr_prefix);
        let options = self.options.bytes();
        let hdr_prefix_len = hdr_prefix.len();
        let header_len = hdr_prefix_len + options.len();
        let body_to_copy = self.body().len().min(TRANSPORT_HEADER_MAX_SIZE);
        let outer_header_len = constraints.header_len();
        let mut buffer = alloc.layout_alloc(outer_header_len + header_len, body_to_copy, 0)?;
        buffer.with_parts_mut(|prefix, mut body, _suffix| {
            let options_pos = outer_header_len + hdr_prefix_len;
            prefix[outer_header_len..options_pos].copy_from_slice(hdr_prefix);
            prefix[options_pos..].copy_from_slice(options);
            body.copy_from_slice(&self.body()[..body_to_copy]);
        });
        buffer.grow_front(header_len);
        let total_size = header_len + self.body.len();
        Ok((buffer, total_size))
    }
}

impl<B: SplitByteSlice> ParsablePacket<B, ()> for Ipv4Packet<B> {
    type Error = ParseError;

    fn parse_metadata(&self) -> ParseMetadata {
        ParseMetadata::from_packet(self.header_len(), self.body.len(), 0)
    }

    fn parse<BV: BufferView<B>>(buffer: BV, _args: ()) -> Result<Self, ParseError> {
        Ipv4PacketRaw::<B>::parse(buffer, ()).and_then(Ipv4Packet::try_from_raw)
    }
}

impl<B: SplitByteSlice> FromRaw<Ipv4PacketRaw<B>, ()> for Ipv4Packet<B> {
    type Error = ParseError;

    fn try_from_raw_with(raw: Ipv4PacketRaw<B>, _args: ()) -> Result<Self, Self::Error> {
        // TODO(https://fxbug.dev/42157630): Some of the errors below should return an
        // `ParameterProblem` error instead of a `ParseError`.
        let hdr_prefix = raw.hdr_prefix;
        let hdr_bytes = (hdr_prefix.ihl() * 4) as usize;

        if hdr_bytes < HDR_PREFIX_LEN {
            return debug_err!(Err(ParseError::Format), "invalid IHL: {}", hdr_prefix.ihl());
        }

        let options = match raw.options {
            MaybeParsed::Incomplete(_) => {
                return debug_err!(Err(ParseError::Format), "Incomplete options");
            }
            MaybeParsed::Complete(unchecked) => Options::try_from_raw(unchecked)
                .map_err(|e| debug_err!(e, "malformed options: {:?}", e))?,
        };

        if hdr_prefix.version() != 4 {
            return debug_err!(
                Err(ParseError::Format),
                "unexpected IP version: {}",
                hdr_prefix.version()
            );
        }

        let body = match raw.body {
            MaybeParsed::Incomplete(_) => {
                if hdr_prefix.mf_flag() {
                    return debug_err!(
                        Err(ParseError::NotSupported),
                        "fragmentation not supported"
                    );
                } else {
                    return debug_err!(Err(ParseError::Format), "Incomplete body");
                }
            }
            MaybeParsed::Complete(bytes) => bytes,
        };

        let packet = Ipv4Packet { hdr_prefix, options, body };
        if packet.compute_header_checksum() != [0, 0] {
            return debug_err!(Err(ParseError::Checksum), "invalid checksum");
        }
        Ok(packet)
    }
}

fn compute_header_checksum(hdr_prefix: &[u8], options: &[u8]) -> [u8; 2] {
    let mut c = Checksum::new();
    c.add_bytes(hdr_prefix);
    c.add_bytes(options);
    c.checksum()
}

impl<B: SplitByteSlice> Ipv4Packet<B> {
    /// Iterate over the IPv4 header options.
    pub fn iter_options(&self) -> impl Iterator<Item = Ipv4Option<'_>> {
        self.options.iter()
    }

    // Compute the header checksum, skipping the checksum field itself.
    fn compute_header_checksum(&self) -> [u8; 2] {
        compute_header_checksum(Ref::bytes(&self.hdr_prefix), self.options.bytes())
    }

    /// The packet body.
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    /// The size of the header prefix and options.
    pub fn header_len(&self) -> usize {
        Ref::bytes(&self.hdr_prefix).len() + self.options.bytes().len()
    }

    /// The source IP address represented as an [`Ipv4SourceAddr`].
    ///
    /// Unlike [`IpHeader::src_ip`], `src_ipv4` returns an `Ipv4SourceAddr`,
    /// which represents the valid values that a source address can take.
    pub fn src_ipv4(&self) -> Option<Ipv4SourceAddr> {
        Ipv4SourceAddr::new(self.src_ip())
    }

    /// Return a buffer that is a copy of the header bytes in this
    /// packet, but patched to be not fragmented.
    ///
    /// Return a buffer of this packet's header and options with
    /// the fragment data zeroed out.
    pub fn copy_header_bytes_for_fragment(&self) -> Vec<u8> {
        let expected_bytes_len = self.header_len();
        let mut bytes = Vec::with_capacity(expected_bytes_len);

        bytes.extend_from_slice(Ref::bytes(&self.hdr_prefix));
        bytes.extend_from_slice(self.options.bytes());

        // `bytes`'s length should be exactly `expected_bytes_len`.
        assert_eq!(bytes.len(), expected_bytes_len);

        // Zero out the fragment data.
        bytes[IPV4_FRAGMENT_DATA_BYTE_RANGE].copy_from_slice(&[0; 4][..]);

        bytes
    }

    /// Performs the header translation part of NAT64 as described in [RFC
    /// 7915].
    ///
    /// `nat64_translate` follows the rules described in RFC 7915 to construct
    /// the IPv6 equivalent of this IPv4 packet. If the payload is a TCP segment
    /// or a UDP packet, its checksum will be updated. If the payload is an
    /// ICMPv4 packet, it will be converted to the equivalent ICMPv6 packet.
    /// For all other payloads, the payload will be unchanged, and IP header will
    /// be translated. On success, a [`Serializer`] is returned which describes
    /// the new packet to be sent.
    ///
    /// Note that the IPv4 TTL/IPv6 Hop Limit field is not modified. It is the
    /// caller's responsibility to decrement and process this field per RFC
    /// 7915.
    ///
    /// In some cases, the packet has no IPv6 equivalent, in which case the
    /// value [`Nat64TranslationResult::Drop`] will be returned, instructing the
    /// caller to silently drop the packet.
    ///
    /// # Errors
    ///
    /// `nat64_translate` will return an error if support has not yet been
    /// implemented for translation a particular IP protocol.
    ///
    /// [RFC 7915]: https://datatracker.ietf.org/doc/html/rfc7915
    pub fn nat64_translate(
        &self,
        v6_src_addr: Ipv6Addr,
        v6_dst_addr: Ipv6Addr,
    ) -> Nat64TranslationResult<
        impl Serializer<NoOpSerializationContext, Buffer = EmptyBuf> + Debug + '_,
        Nat64Error,
    > {
        // A single `Serializer` type so that all possible return values from
        // this function have the same type.
        #[derive(Debug)]
        enum Nat64Serializer<T, U, O> {
            Tcp(T),
            Udp(U),
            Other(O),
        }
        impl<T, U, O> Serializer<NoOpSerializationContext> for Nat64Serializer<T, U, O>
        where
            T: Serializer<NoOpSerializationContext, Buffer = EmptyBuf>,
            U: Serializer<NoOpSerializationContext, Buffer = EmptyBuf>,
            O: Serializer<NoOpSerializationContext, Buffer = EmptyBuf>,
        {
            type Buffer = EmptyBuf;
            fn serialize<B, P>(
                self,
                context: &mut NoOpSerializationContext,
                outer: PacketConstraints,
                provider: P,
            ) -> Result<B, (SerializeError<P::Error>, Self)>
            where
                B: GrowBufferMut,
                P: BufferProvider<Self::Buffer, B>,
            {
                match self {
                    Nat64Serializer::Tcp(serializer) => serializer
                        .serialize(context, outer, provider)
                        .map_err(|(err, ser)| (err, Nat64Serializer::Tcp(ser))),
                    Nat64Serializer::Udp(serializer) => serializer
                        .serialize(context, outer, provider)
                        .map_err(|(err, ser)| (err, Nat64Serializer::Udp(ser))),
                    Nat64Serializer::Other(serializer) => serializer
                        .serialize(context, outer, provider)
                        .map_err(|(err, ser)| (err, Nat64Serializer::Other(ser))),
                }
            }

            fn serialize_new_buf<B: GrowBufferMut, A: LayoutBufferAlloc<B>>(
                &self,
                context: &mut NoOpSerializationContext,
                outer: PacketConstraints,
                alloc: A,
            ) -> Result<B, SerializeError<A::Error>> {
                match self {
                    Nat64Serializer::Tcp(serializer) => {
                        serializer.serialize_new_buf(context, outer, alloc)
                    }
                    Nat64Serializer::Udp(serializer) => {
                        serializer.serialize_new_buf(context, outer, alloc)
                    }
                    Nat64Serializer::Other(serializer) => {
                        serializer.serialize_new_buf(context, outer, alloc)
                    }
                }
            }
        }

        impl<T, U, O> NestableSerializer for Nat64Serializer<T, U, O>
        where
            T: Serializer<NoOpSerializationContext, Buffer = EmptyBuf>,
            U: Serializer<NoOpSerializationContext, Buffer = EmptyBuf>,
            O: Serializer<NoOpSerializationContext, Buffer = EmptyBuf>,
        {
        }

        let v6_builder = |v6_proto| {
            let mut builder =
                Ipv6PacketBuilder::new(v6_src_addr, v6_dst_addr, self.ttl(), v6_proto);
            builder.dscp_and_ecn(self.dscp_and_ecn());
            builder.flowlabel(0);
            builder
        };

        match self.proto() {
            Ipv4Proto::Igmp => {
                // As per RFC 7915 Section 4.2, silently drop all IGMP packets:
                Nat64TranslationResult::Drop
            }

            Ipv4Proto::Proto(IpProto::Tcp) => {
                let v6_pkt_builder = v6_builder(Ipv6Proto::Proto(IpProto::Tcp));
                let args = TcpParseArgs::new(self.src_ip(), self.dst_ip());
                match TcpSegment::parse(&mut self.body.as_bytes(), args) {
                    Ok(tcp) => {
                        // Creating a new tcp_serializer for IPv6 packet from
                        // the existing one ensures that checksum is
                        // updated due to changed IP addresses.
                        let tcp_serializer =
                            Nat64Serializer::Tcp(tcp.into_serializer(v6_src_addr, v6_dst_addr));
                        Nat64TranslationResult::Forward(v6_pkt_builder.wrap_body(tcp_serializer))
                    }
                    Err(msg) => {
                        debug!("Parsing of TCP segment failed: {:?}", msg);

                        // This means we can't create a TCP segment builder with
                        // updated checksum. Parsing may fail due to a variety of
                        // reasons, including incorrect checksum in incoming packet.
                        // We should still return a packet with IP payload copied
                        // as is from IPv4 to IPv6.
                        let common_serializer =
                            Nat64Serializer::Other(self.body().into_serializer());
                        Nat64TranslationResult::Forward(v6_pkt_builder.wrap_body(common_serializer))
                    }
                }
            }

            Ipv4Proto::Proto(IpProto::Udp) => {
                let v6_pkt_builder = v6_builder(Ipv6Proto::Proto(IpProto::Udp));
                let args = UdpParseArgs::new(self.src_ip(), self.dst_ip());
                match UdpPacket::parse(&mut self.body.as_bytes(), args) {
                    Ok(udp) => {
                        // Creating a new udp_serializer for IPv6 packet from
                        // the existing one ensures that checksum is
                        // updated due to changed IP addresses.
                        let udp_serializer =
                            Nat64Serializer::Udp(udp.into_serializer(v6_src_addr, v6_dst_addr));
                        Nat64TranslationResult::Forward(v6_pkt_builder.wrap_body(udp_serializer))
                    }
                    Err(msg) => {
                        debug!("Parsing of UDP packet failed: {:?}", msg);

                        // This means we can't create a UDP packet builder with
                        // updated checksum. Parsing may fail due to a variety of
                        // reasons, including incorrect checksum in incoming packet.
                        // We should still return a packet with IP payload copied
                        // as is from IPv4 to IPv6.
                        let common_serializer =
                            Nat64Serializer::Other(self.body().into_serializer());
                        Nat64TranslationResult::Forward(v6_pkt_builder.wrap_body(common_serializer))
                    }
                }
            }

            Ipv4Proto::Icmp => Nat64TranslationResult::Err(Nat64Error::NotImplemented),

            // As per the RFC, for all other protocols, an IPv6 must be forwarded, even if the
            // transport-layer checksum update is not implemented. It's expected to fail
            // checksum verification on receiver end, but still packet must be forwarded for
            // 'troubleshooting and ease of debugging'.
            Ipv4Proto::Other(val) => {
                let v6_pkt_builder = v6_builder(Ipv6Proto::Other(val));
                let common_serializer = Nat64Serializer::Other(self.body().into_serializer());
                Nat64TranslationResult::Forward(v6_pkt_builder.wrap_body(common_serializer))
            }

            // Don't forward packets that use IANA's reserved protocol; they're
            // invalid.
            Ipv4Proto::Proto(IpProto::Reserved) => Nat64TranslationResult::Drop,
        }
    }

    /// Copies the packet (Header + Options + Body) into a `Vec`.
    pub fn to_vec(&self) -> Vec<u8> {
        let Ipv4Packet { hdr_prefix, options, body } = self;
        let mut buf = Vec::with_capacity(
            Ref::bytes(&hdr_prefix).len() + options.bytes().len() + body.as_bytes().len(),
        );
        buf.extend(Ref::bytes(&hdr_prefix));
        buf.extend(options.bytes());
        buf.extend(body.as_bytes());
        buf
    }
}

impl<B: SplitByteSliceMut> Ipv4Packet<B> {
    /// Set the source IP address.
    ///
    /// Set the source IP address and update the header checksum accordingly.
    pub fn set_src_ip_and_update_checksum(&mut self, addr: Ipv4Addr) {
        let old_bytes = self.hdr_prefix.src_ip.bytes();
        self.hdr_prefix.hdr_checksum =
            internet_checksum::update(self.hdr_prefix.hdr_checksum, &old_bytes, addr.bytes());
        self.hdr_prefix.src_ip = addr;
    }

    /// Set the destination IP address.
    ///
    /// Set the destination IP address and update the header checksum accordingly.
    pub fn set_dst_ip_and_update_checksum(&mut self, addr: Ipv4Addr) {
        let old_bytes = self.hdr_prefix.dst_ip.bytes();
        self.hdr_prefix.hdr_checksum =
            internet_checksum::update(self.hdr_prefix.hdr_checksum, &old_bytes, addr.bytes());
        self.hdr_prefix.dst_ip = addr;
    }

    /// Set the Time To Live (TTL).
    ///
    /// Set the TTL and update the header checksum accordingly.
    pub fn set_ttl(&mut self, ttl: u8) {
        // See the internet_checksum::update documentation for why we need to
        // provide two bytes which are at an even byte offset from the beginning
        // of the header.
        let old_bytes = [self.hdr_prefix.ttl, self.hdr_prefix.proto];
        let new_bytes = [ttl, self.hdr_prefix.proto];
        self.hdr_prefix.hdr_checksum =
            internet_checksum::update(self.hdr_prefix.hdr_checksum, &old_bytes, &new_bytes);
        self.hdr_prefix.ttl = ttl;
    }

    /// The packet body.
    pub fn body_mut(&mut self) -> &mut [u8] {
        &mut self.body
    }

    /// Provides simultaneous access to header prefix, options, and mutable
    /// body.
    pub fn parts_with_body_mut(&mut self) -> (&HeaderPrefix, &Options<B>, &mut [u8]) {
        (&self.hdr_prefix, &self.options, &mut self.body)
    }
}

impl<B> Debug for Ipv4Packet<B>
where
    B: SplitByteSlice,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), fmt::Error> {
        f.debug_struct("Ipv4Packet")
            .field("src_ip", &self.src_ip())
            .field("dst_ip", &self.dst_ip())
            .field("id", &self.id())
            .field("ttl", &self.ttl())
            .field("proto", &self.proto())
            .field("frag_off", &self.fragment_offset())
            .field("dscp", &self.dscp_and_ecn().dscp())
            .field("ecn", &self.dscp_and_ecn().ecn())
            .field("mf_flag", &self.mf_flag())
            .field("df_flag", &self.df_flag())
            .field("body", &alloc::format!("<{} bytes>", self.body.len()))
            .finish()
    }
}

/// A partially parsed and not yet validated IPv4 packet.
///
/// `Ipv4PacketRaw` provides minimal parsing of an IPv4 packet, namely
/// it only requires that the fixed header part ([`HeaderPrefix`]) be retrieved,
/// all the other parts of the packet may be missing when attempting to create
/// it.
///
/// [`Ipv4Packet`] provides a [`FromRaw`] implementation that can be used to
/// validate an `Ipv4PacketRaw`.
pub struct Ipv4PacketRaw<B> {
    hdr_prefix: Ref<B, HeaderPrefix>,
    options: MaybeParsed<OptionsRaw<B, Ipv4OptionsImpl>, B>,
    body: MaybeParsed<B, B>,
}

impl<B> Ipv4PacketRaw<B> {
    /// Returns a mutable reference to the body bytes of this [`Ipv4PacketRaw`].
    ///
    /// Might not be complete if a full packet was not received.
    pub fn body_mut(&mut self) -> &mut B {
        match &mut self.body {
            MaybeParsed::Complete(b) => b,
            MaybeParsed::Incomplete(b) => b,
        }
    }
}

impl<B: SplitByteSlice> Ipv4Header for Ipv4PacketRaw<B> {
    fn get_header_prefix(&self) -> &HeaderPrefix {
        &self.hdr_prefix
    }
}

impl<B: SplitByteSlice> ParsablePacket<B, ()> for Ipv4PacketRaw<B> {
    type Error = ParseError;

    fn parse_metadata(&self) -> ParseMetadata {
        let header_len = Ref::bytes(&self.hdr_prefix).len() + self.options.len();
        ParseMetadata::from_packet(header_len, self.body.len(), 0)
    }

    fn parse<BV: BufferView<B>>(mut buffer: BV, _args: ()) -> Result<Self, ParseError> {
        let hdr_prefix = buffer
            .take_obj_front::<HeaderPrefix>()
            .ok_or_else(debug_err_fn!(ParseError::Format, "too few bytes for header"))?;
        let hdr_bytes = (hdr_prefix.ihl() * 4) as usize;

        let options = MaybeParsed::take_from_buffer_with(
            &mut buffer,
            // If the subtraction hdr_bytes - HDR_PREFIX_LEN would have been
            // negative, that would imply that IHL has an invalid value. Even
            // though this will end up being MaybeParsed::Complete, the IHL
            // value is validated when transforming Ipv4PacketRaw to Ipv4Packet.
            hdr_bytes.saturating_sub(HDR_PREFIX_LEN),
            OptionsRaw::new,
        );

        let total_len: usize = hdr_prefix.total_len.get().into();
        let body_len = total_len.saturating_sub(hdr_bytes);
        if buffer.len() > body_len {
            // Discard the padding left by the previous layer. This unwrap is
            // safe because of the check against total_len.
            let _: B = buffer.take_back(buffer.len() - body_len).unwrap();
        }

        let body = MaybeParsed::new_with_min_len(buffer.into_rest(), body_len);

        Ok(Self { hdr_prefix, options, body })
    }
}

impl<B> Ipv4PacketRaw<B> {
    /// Gets the maybe parsed options from the raw packet.
    pub fn options(&self) -> &MaybeParsed<OptionsRaw<B, Ipv4OptionsImpl>, B> {
        &self.options
    }
}

impl<B: SplitByteSlice> Ipv4PacketRaw<B> {
    /// Return the body.
    ///
    /// `body` returns [`MaybeParsed::Complete`] if the entire body is present
    /// (as determined by the header's "total length" and "internet header
    /// length" fields), and [`MaybeParsed::Incomplete`] otherwise.
    pub fn body(&self) -> MaybeParsed<&[u8], &[u8]> {
        self.body.as_ref().map(|b| b.deref()).map_incomplete(|b| b.deref())
    }

    /// Consumes `self` returning the body.
    ///
    /// See [`Ipv4PacketRaw::body`] for details on parsing completeness.
    pub fn into_body(self) -> MaybeParsed<B, B> {
        self.body
    }
}

impl<B: SplitByteSliceMut> Ipv4PacketRaw<B> {
    /// Set the source IP address.
    ///
    /// Set the source IP address and update the header checksum accordingly.
    pub fn set_src_ip_and_update_checksum(&mut self, addr: Ipv4Addr) {
        let old_bytes = self.hdr_prefix.src_ip.bytes();
        self.hdr_prefix.hdr_checksum =
            internet_checksum::update(self.hdr_prefix.hdr_checksum, &old_bytes, addr.bytes());
        self.hdr_prefix.src_ip = addr;
    }

    /// Set the destination IP address.
    ///
    /// Set the destination IP address and update the header checksum accordingly.
    pub fn set_dst_ip_and_update_checksum(&mut self, addr: Ipv4Addr) {
        let old_bytes = self.hdr_prefix.dst_ip.bytes();
        self.hdr_prefix.hdr_checksum =
            internet_checksum::update(self.hdr_prefix.hdr_checksum, &old_bytes, addr.bytes());
        self.hdr_prefix.dst_ip = addr;
    }
}

/// A records parser for IPv4 options.
///
/// See [`Options`] for more details.
///
/// [`Options`]: packet::records::options::Options
pub type Options<B> = packet::records::options::Options<B, Ipv4OptionsImpl>;

/// Options provided to [`Ipv4PacketBuilderWithOptions::new`] exceed
/// [`MAX_OPTIONS_LEN`] when serialized.
#[derive(Debug)]
pub struct Ipv4OptionsTooLongError;

/// A PacketBuilder for Ipv4 Packets but with options.
#[derive(Debug, Clone)]
pub struct Ipv4PacketBuilderWithOptions<'a, I> {
    prefix_builder: Ipv4PacketBuilder,
    options: OptionSequenceBuilder<Ipv4Option<'a>, I>,
}

impl<'a, I> Ipv4PacketBuilderWithOptions<'a, I>
where
    I: Iterator + Clone,
    I::Item: Borrow<Ipv4Option<'a>>,
{
    /// Creates a new IPv4 packet builder without options.
    ///
    /// Returns `Err` if the packet header would exceed the maximum length of
    /// [`MAX_HDR_LEN`]. This happens if the `options`, when serialized, would
    /// exceed [`MAX_OPTIONS_LEN`].
    pub fn new<T: IntoIterator<IntoIter = I>>(
        prefix_builder: Ipv4PacketBuilder,
        options: T,
    ) -> Result<Ipv4PacketBuilderWithOptions<'a, I>, Ipv4OptionsTooLongError> {
        let options = OptionSequenceBuilder::new(options.into_iter());
        if options.serialized_len() > MAX_OPTIONS_LEN {
            return Err(Ipv4OptionsTooLongError);
        }
        Ok(Ipv4PacketBuilderWithOptions { prefix_builder, options })
    }

    fn aligned_options_len(&self) -> usize {
        // Round up to the next 4-byte boundary.
        crate::utils::round_to_next_multiple_of_four(self.options.serialized_len())
    }

    /// Returns a reference to the prefix builder.
    pub fn prefix_builder(&self) -> &Ipv4PacketBuilder {
        &self.prefix_builder
    }

    /// Returns a mutable reference to the prefix builder.
    pub fn prefix_builder_mut(&mut self) -> &mut Ipv4PacketBuilder {
        &mut self.prefix_builder
    }

    /// Returns a reference to the options used to create this builder.
    pub fn options(&self) -> &I {
        self.options.records()
    }

    /// Maps this builder optionally maintaining only the options that are meant
    /// to be copied on all fragments.
    ///
    /// If `first_fragment` is `true`, all options are maintained, otherwise
    /// only the options meant to be copied on all fragments will be yielded.
    pub fn with_fragment_options(
        self,
        first_fragment: bool,
    ) -> Ipv4PacketBuilderWithOptions<'a, impl Iterator<Item: Borrow<Ipv4Option<'a>>> + Clone> {
        let Self { prefix_builder, options } = self;
        Ipv4PacketBuilderWithOptions {
            prefix_builder,
            // We don't need to run the check on the builder options again since
            // we're strictly removing options.
            options: OptionSequenceBuilder::new(
                options
                    .records()
                    .clone()
                    .filter(move |opt| first_fragment || opt.borrow().copied()),
            ),
        }
    }
}

impl<'a, B> Ipv4PacketBuilderWithOptions<'a, RecordsIter<'a, B, Ipv4OptionsImpl>> {
    /// Creates a new `Ipv4PacketBuilderWithOptions` with a known-to-be-valid
    /// iterator of IPv4 options records.
    pub fn new_with_records_iter(
        prefix_builder: Ipv4PacketBuilder,
        iter: RecordsIter<'a, B, Ipv4OptionsImpl>,
    ) -> Self {
        Self { prefix_builder, options: OptionSequenceBuilder::new(iter) }
    }
}

impl<'a, I> NestablePacketBuilder for Ipv4PacketBuilderWithOptions<'a, I>
where
    I: Iterator + Clone,
    I::Item: Borrow<Ipv4Option<'a>>,
{
    fn constraints(&self) -> PacketConstraints {
        let header_len = IPV4_MIN_HDR_LEN + self.aligned_options_len();
        assert_eq!(header_len % 4, 0);
        PacketConstraints::new(header_len, 0, 0, (1 << 16) - 1 - header_len)
    }
}

impl<'a, I, C: IpSerializationContext<Ipv4>> PacketBuilder<C>
    for Ipv4PacketBuilderWithOptions<'a, I>
where
    I: Iterator + Clone,
    I::Item: Borrow<Ipv4Option<'a>>,
{
    fn context_state(&self) -> C::ContextState {
        C::envelope_to_state(IpEnvelope::new(true))
    }

    fn serialize(
        &self,
        context: &mut C,
        target: &mut SerializeTarget<'_>,
        body: FragmentedBytesMut<'_, '_>,
    ) {
        let opt_len = self.aligned_options_len();
        // `take_back_zero` consumes the extent of the receiving slice, but that
        // behavior is undesirable here: `prefix_builder.serialize` also needs
        // to write into the header. To avoid changing the extent of
        // target.header, we re-slice header before calling `take_back_zero`;
        // the re-slice will be consumed, but `target.header` is unaffected.
        let mut header = &mut &mut target.header[..];
        let opts = header.take_back_zero(opt_len).expect("too few bytes for Ipv4 options");
        let Ipv4PacketBuilderWithOptions { prefix_builder, options } = self;
        options.serialize_into(opts);
        prefix_builder.serialize(context, target, body);
    }
}

impl<'a, I, C: IpSerializationContext<Ipv4>> PartialPacketBuilder<C>
    for Ipv4PacketBuilderWithOptions<'a, I>
where
    I: Iterator + Clone,
    I::Item: Borrow<Ipv4Option<'a>>,
{
    fn partial_serialize(&self, context: &mut C, body_len: usize, header: &mut [u8]) {
        let Ipv4PacketBuilderWithOptions { prefix_builder, options } = self;
        prefix_builder.partial_serialize(context, body_len, header);
        let options_slice = &mut header[IPV4_MIN_HDR_LEN..];
        assert_eq!(options_slice.len(), self.aligned_options_len());
        options.serialize_into(options_slice);
    }
}

impl<'a, C: IpSerializationContext<Ipv4>, I> IpPacketBuilder<C, Ipv4>
    for Ipv4PacketBuilderWithOptions<'a, I>
where
    I: Default + Debug + Clone + Iterator<Item: Borrow<Ipv4Option<'a>>>,
{
    fn new(src_ip: Ipv4Addr, dst_ip: Ipv4Addr, ttl: u8, proto: Ipv4Proto) -> Self {
        Ipv4PacketBuilderWithOptions::new(
            Ipv4PacketBuilder::new(src_ip, dst_ip, ttl, proto),
            I::default(),
        )
        .expect("packet builder with no options should be valid")
    }

    fn src_ip(&self) -> Ipv4Addr {
        self.prefix_builder.src_ip
    }

    fn set_src_ip(&mut self, addr: Ipv4Addr) {
        <Ipv4PacketBuilder as IpPacketBuilder<C, Ipv4>>::set_src_ip(&mut self.prefix_builder, addr)
    }

    fn dst_ip(&self) -> Ipv4Addr {
        self.prefix_builder.dst_ip
    }

    fn set_dst_ip(&mut self, addr: Ipv4Addr) {
        <Ipv4PacketBuilder as IpPacketBuilder<C, Ipv4>>::set_dst_ip(&mut self.prefix_builder, addr)
    }

    fn proto(&self) -> Ipv4Proto {
        self.prefix_builder.proto
    }

    fn set_dscp_and_ecn(&mut self, dscp_and_ecn: DscpAndEcn) {
        <Ipv4PacketBuilder as IpPacketBuilder<C, Ipv4>>::set_dscp_and_ecn(
            &mut self.prefix_builder,
            dscp_and_ecn,
        )
    }
}

/// A builder for IPv4 packets.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Ipv4PacketBuilder {
    id: u16,
    dscp_and_ecn: DscpAndEcn,
    flags: u8,
    frag_off: u16,
    ttl: u8,
    proto: Ipv4Proto,
    src_ip: Ipv4Addr,
    dst_ip: Ipv4Addr,
}

impl Ipv4PacketBuilder {
    /// Construct a new `Ipv4PacketBuilder`.
    pub fn new<S: Into<Ipv4Addr>, D: Into<Ipv4Addr>>(
        src_ip: S,
        dst_ip: D,
        ttl: u8,
        proto: Ipv4Proto,
    ) -> Ipv4PacketBuilder {
        Ipv4PacketBuilder {
            id: 0,
            dscp_and_ecn: DscpAndEcn::default(),
            flags: 0,
            frag_off: 0,
            ttl,
            proto: proto,
            src_ip: src_ip.into(),
            dst_ip: dst_ip.into(),
        }
    }

    /// Sets DSCP and ECN fields.
    pub fn dscp_and_ecn(&mut self, dscp_and_ecn: DscpAndEcn) {
        self.dscp_and_ecn = dscp_and_ecn;
    }

    /// Set the ID field.
    pub fn id(&mut self, id: u16) {
        self.id = id
    }

    /// Set the Don't Fragment (DF) flag.
    pub fn df_flag(&mut self, df: bool) {
        if df {
            self.flags |= 1 << DF_FLAG_OFFSET;
        } else {
            self.flags &= !(1 << DF_FLAG_OFFSET);
        }
    }

    /// Set the More Fragments (MF) flag.
    pub fn mf_flag(&mut self, mf: bool) {
        if mf {
            self.flags |= 1 << MF_FLAG_OFFSET;
        } else {
            self.flags &= !(1 << MF_FLAG_OFFSET);
        }
    }

    /// Set the fragment offset.
    pub fn fragment_offset(&mut self, fragment_offset: FragmentOffset) {
        self.frag_off = fragment_offset.into_raw();
    }

    /// Returns the configured Don't Fragment (DF) flag.
    pub fn read_df_flag(&self) -> bool {
        (self.flags & (1 << DF_FLAG_OFFSET)) != 0
    }

    fn get_header_prefix(&self, header_len: usize, total_len: usize) -> HeaderPrefix {
        assert_eq!(header_len % 4, 0);
        let ihl: u8 = u8::try_from(header_len / 4).expect("Header too large");

        // As Per [RFC 6864 Section 2]:
        //
        //   > The IPv4 ID field is thus meaningful only for non-atomic datagrams --
        //   > either those datagrams that have already been fragmented or those for
        //   > which fragmentation remains permitted...
        //   >
        //   > ...Non-atomic datagrams: (DF==0)||(MF==1)||(frag_offset>0)
        //
        // [RFC 6864 Section 2]: https://tools.ietf.org/html/rfc6864#section-2
        let id = if ((self.flags & (1 << DF_FLAG_OFFSET)) == 0)
            || ((self.flags & (1 << MF_FLAG_OFFSET)) == 1)
            || (self.frag_off > 0)
        {
            self.id
        } else {
            0
        };

        HeaderPrefix::new(
            ihl,
            self.dscp_and_ecn,
            {
                // The caller promises to supply a body whose length does not
                // exceed max_body_len. Doing this as a debug_assert (rather
                // than an assert) is fine because, with debug assertions
                // disabled, we'll just write an incorrect header value, which
                // is acceptable if the caller has violated their contract.
                debug_assert!(total_len <= u16::MAX as usize);
                total_len as u16
            },
            id,
            self.flags,
            self.frag_off,
            self.ttl,
            self.proto.into(),
            [0, 0], // header checksum
            self.src_ip,
            self.dst_ip,
        )
    }
}

impl NestablePacketBuilder for Ipv4PacketBuilder {
    fn constraints(&self) -> PacketConstraints {
        PacketConstraints::new(IPV4_MIN_HDR_LEN, 0, 0, (1 << 16) - 1 - IPV4_MIN_HDR_LEN)
    }
}

impl<C: IpSerializationContext<Ipv4>> PacketBuilder<C> for Ipv4PacketBuilder {
    fn context_state(&self) -> C::ContextState {
        C::envelope_to_state(IpEnvelope::new(false))
    }

    fn serialize(
        &self,
        _context: &mut C,
        target: &mut SerializeTarget<'_>,
        body: FragmentedBytesMut<'_, '_>,
    ) {
        let header_len = target.header.len();
        let total_len = header_len + body.len();
        let mut hdr_prefix = self.get_header_prefix(header_len, total_len);
        let options = &target.header[HDR_PREFIX_LEN..];
        let checksum = compute_header_checksum(hdr_prefix.as_bytes(), options);
        hdr_prefix.hdr_checksum = checksum;
        let mut header = &mut target.header;
        header.write_obj_front(&hdr_prefix).expect("too few bytes for IPv4 header prefix");
    }
}

impl<C: IpSerializationContext<Ipv4>> PartialPacketBuilder<C> for Ipv4PacketBuilder {
    fn partial_serialize(&self, _context: &mut C, body_len: usize, mut header: &mut [u8]) {
        let total_len = header.len() + body_len;
        let hdr_prefix = self.get_header_prefix(header.len(), total_len);
        (&mut header).write_obj_front(&hdr_prefix).expect("too few bytes for IPv4 header prefix");
    }
}

impl<C: IpSerializationContext<Ipv4>> IpPacketBuilder<C, Ipv4> for Ipv4PacketBuilder {
    fn new(src_ip: Ipv4Addr, dst_ip: Ipv4Addr, ttl: u8, proto: Ipv4Proto) -> Self {
        Ipv4PacketBuilder::new(src_ip, dst_ip, ttl, proto)
    }

    fn src_ip(&self) -> Ipv4Addr {
        self.src_ip
    }

    fn set_src_ip(&mut self, addr: Ipv4Addr) {
        self.src_ip = addr;
    }

    fn dst_ip(&self) -> Ipv4Addr {
        self.dst_ip
    }

    fn set_dst_ip(&mut self, addr: Ipv4Addr) {
        self.dst_ip = addr;
    }

    fn proto(&self) -> Ipv4Proto {
        self.proto
    }

    fn set_dscp_and_ecn(&mut self, dscp_and_ecn: DscpAndEcn) {
        self.dscp_and_ecn = dscp_and_ecn;
    }
}

// bit positions into the flags bits
const DF_FLAG_OFFSET: u8 = 1;
const MF_FLAG_OFFSET: u8 = 0;

/// Reassembles a fragmented IPv4 packet into a parsed IPv4 packet.
///
/// # Panics
///
/// Panics if the provided header is too small to hold a valid header.
pub(crate) fn reassemble_fragmented_packet<
    'a,
    B: SplitByteSliceMut,
    BV: BufferViewMut<B>,
    I: Iterator<Item = &'a [u8]>,
>(
    mut buffer: BV,
    header: &[u8],
    body_fragments: I,
) -> Result<(), ParseError> {
    assert!(header.len() >= HDR_PREFIX_LEN);

    let bytes = buffer.as_mut();

    // First, copy over the header data.
    bytes[0..header.len()].copy_from_slice(header);
    let mut byte_count = header.len();

    // Next, copy over the body fragments.
    for p in body_fragments {
        bytes[byte_count..byte_count + p.len()].copy_from_slice(p);
        byte_count += p.len();
    }

    // Fix up the IPv4 header

    // Make sure that the packet length is not more than the maximum
    // possible IPv4 packet length.
    if byte_count > usize::from(u16::MAX) {
        return debug_err!(
            Err(ParseError::Format),
            "fragmented packet length of {} bytes is too large",
            byte_count
        );
    }

    // We know the call to `unwrap` will not fail because we verified the length
    // of `header` and copied it's bytes into `bytes`.
    let mut header_ref = Ref::<_, HeaderPrefix>::from_prefix(bytes).unwrap().0;

    let options_bytes = &header[HDR_PREFIX_LEN..header.len()];

    // Update the total length field.
    header_ref.total_len.set(u16::try_from(byte_count).unwrap());

    // Zero out fragment related data since we will now have a
    // reassembled packet that does not need reassembly.
    header_ref.flags_frag_off = [0; 2];

    // Update header checksum.
    header_ref.hdr_checksum = [0; 2];
    header_ref.hdr_checksum = compute_header_checksum(header_ref.as_bytes(), options_bytes);

    Ok(())
}

/// Parsing and serialization of IPv4 options.
pub mod options {
    use byteorder::{ByteOrder, NetworkEndian};
    use packet::BufferViewMut;
    use packet::records::options::{
        OptionBuilder, OptionLayout, OptionParseErr, OptionParseLayout, OptionsImpl,
    };
    use zerocopy::byteorder::network_endian::U16;

    const OPTION_KIND_EOL: u8 = 0;
    const OPTION_KIND_NOP: u8 = 1;
    const OPTION_KIND_RTRALRT: u8 = 148;

    const OPTION_RTRALRT_LEN: usize = 2;

    /// An IPv4 header option.
    ///
    /// See [Wikipedia] or [RFC 791] for more details.
    ///
    /// [Wikipedia]: https://en.wikipedia.org/wiki/IPv4#Options
    /// [RFC 791]: https://tools.ietf.org/html/rfc791#page-15
    #[derive(PartialEq, Eq, Debug, Clone)]
    #[allow(missing_docs)]
    pub enum Ipv4Option<'a> {
        /// Used to tell routers to inspect the packet.
        ///
        /// Used by IGMP host messages per [RFC 2236 section 2].
        ///
        /// [RFC 2236 section 2]: https://tools.ietf.org/html/rfc2236#section-2
        RouterAlert { data: u16 },

        /// An unrecognized IPv4 option.
        // The maximum header length is 60 bytes, and the fixed-length header is 20
        // bytes, so there are 40 bytes for the options. That leaves a maximum
        // options size of 1 kind byte + 1 length byte + 38 data bytes. Data for an
        // unrecognized option kind.
        //
        // Any unrecognized option kind will have its data parsed using this
        // variant. This allows code to copy unrecognized options into packets when
        // forwarding.
        //
        // `data`'s length is in the range [0, 38].
        Unrecognized { kind: u8, data: &'a [u8] },
    }

    impl<'a> Ipv4Option<'a> {
        /// Returns whether this option should be copied on all fragments.
        pub fn copied(&self) -> bool {
            match self {
                // The router alert option is copied on all fragments. See
                // https://datatracker.ietf.org/doc/html/rfc2113#section-2.1.
                // It is embedded in our definition of OPTION_KIND_RTRALRT.
                Ipv4Option::RouterAlert { .. } => true,
                Ipv4Option::Unrecognized { kind, .. } => *kind & (1 << 7) != 0,
            }
        }
    }

    /// An implementation of [`OptionsImpl`] for IPv4 options.
    #[derive(Debug, Clone)]
    pub struct Ipv4OptionsImpl;

    impl OptionLayout for Ipv4OptionsImpl {
        type KindLenField = u8;
    }

    impl OptionParseLayout for Ipv4OptionsImpl {
        type Error = OptionParseErr;
        const END_OF_OPTIONS: Option<u8> = Some(0);
        const NOP: Option<u8> = Some(1);
    }

    impl OptionsImpl for Ipv4OptionsImpl {
        type Option<'a> = Ipv4Option<'a>;

        fn parse<'a>(kind: u8, data: &'a [u8]) -> Result<Option<Ipv4Option<'a>>, OptionParseErr> {
            match kind {
                self::OPTION_KIND_EOL | self::OPTION_KIND_NOP => {
                    unreachable!("records::options::Options promises to handle EOL and NOP")
                }
                self::OPTION_KIND_RTRALRT => {
                    if data.len() == OPTION_RTRALRT_LEN {
                        Ok(Some(Ipv4Option::RouterAlert { data: NetworkEndian::read_u16(data) }))
                    } else {
                        Err(OptionParseErr)
                    }
                }
                kind => {
                    if data.len() > 38 {
                        Err(OptionParseErr)
                    } else {
                        Ok(Some(Ipv4Option::Unrecognized { kind, data }))
                    }
                }
            }
        }
    }

    impl<'a> OptionBuilder for Ipv4Option<'a> {
        type Layout = Ipv4OptionsImpl;

        fn serialized_len(&self) -> usize {
            match self {
                Ipv4Option::RouterAlert { .. } => OPTION_RTRALRT_LEN,
                Ipv4Option::Unrecognized { data, .. } => data.len(),
            }
        }

        fn option_kind(&self) -> u8 {
            match self {
                Ipv4Option::RouterAlert { .. } => OPTION_KIND_RTRALRT,
                Ipv4Option::Unrecognized { kind, .. } => *kind,
            }
        }

        fn serialize_into(&self, mut buffer: &mut [u8]) {
            match self {
                Ipv4Option::Unrecognized { data, .. } => buffer.copy_from_slice(data),
                Ipv4Option::RouterAlert { data } => {
                    (&mut buffer).write_obj_front(&U16::new(*data)).unwrap()
                }
            };
        }
    }

    #[cfg(test)]
    mod test {
        use packet::records::RecordBuilder;
        use packet::records::options::Options;

        use super::*;

        #[test]
        fn test_serialize_router_alert() {
            let mut buffer = [0u8; 4];
            let option = Ipv4Option::RouterAlert { data: 0 };
            <Ipv4Option<'_> as RecordBuilder>::serialize_into(&option, &mut buffer);
            assert_eq!(buffer[0], 148);
            assert_eq!(buffer[1], 4);
            assert_eq!(buffer[2], 0);
            assert_eq!(buffer[3], 0);
        }

        #[test]
        fn test_parse_router_alert() {
            let mut buffer: Vec<u8> = vec![148, 4, 0, 0];
            let options = Options::<_, Ipv4OptionsImpl>::parse(buffer.as_mut()).unwrap();
            let rtralt = options.iter().next().unwrap();
            assert_eq!(rtralt, Ipv4Option::RouterAlert { data: 0 });
        }
    }
}

mod inner {
    /// The minimum length of an IPv4 header.
    pub const IPV4_MIN_HDR_LEN: usize = super::HDR_PREFIX_LEN;
}

/// IPv4 packet parsing and serialization test utilities.
pub mod testutil {
    pub use super::inner::IPV4_MIN_HDR_LEN;

    /// The offset to the TTL field within an IPv4 header, in bytes.
    pub const IPV4_TTL_OFFSET: usize = 8;

    /// The offset to the checksum field within an IPv4 header, in bytes.
    pub const IPV4_CHECKSUM_OFFSET: usize = 10;
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use net_types::ethernet::Mac;
    use packet::{Buf, FragmentedBuffer, ParseBuffer, PartialSerializeResult};

    use super::*;
    use crate::ethernet::{
        ETHERNET_MIN_BODY_LEN_NO_TAG, EtherType, EthernetFrame, EthernetFrameBuilder,
        EthernetFrameLengthCheck,
    };
    use crate::testutil::*;

    const DEFAULT_SRC_MAC: Mac = Mac::new([1, 2, 3, 4, 5, 6]);
    const DEFAULT_DST_MAC: Mac = Mac::new([7, 8, 9, 0, 1, 2]);
    const DEFAULT_SRC_IP: Ipv4Addr = Ipv4Addr::new([1, 2, 3, 4]);
    const DEFAULT_DST_IP: Ipv4Addr = Ipv4Addr::new([5, 6, 7, 8]);
    // 2001:DB8::1
    const DEFAULT_V6_SRC_IP: Ipv6Addr = Ipv6Addr::new([0x2001, 0x0db8, 0, 0, 0, 0, 0, 1]);
    // 2001:DB8::2
    const DEFAULT_V6_DST_IP: Ipv6Addr = Ipv6Addr::new([0x2001, 0x0db8, 0, 0, 0, 0, 0, 2]);

    #[test]
    fn test_parse_serialize_full_tcp() {
        use crate::testdata::tls_client_hello_v4::*;

        let mut buf = ETHERNET_FRAME.bytes;
        let frame = buf.parse_with::<_, EthernetFrame<_>>(EthernetFrameLengthCheck::Check).unwrap();
        verify_ethernet_frame(&frame, ETHERNET_FRAME);

        let mut body = frame.body();
        let packet = body.parse::<Ipv4Packet<_>>().unwrap();
        verify_ipv4_packet(&packet, IPV4_PACKET);

        // Verify serialization via builders.
        let buffer = packet
            .body()
            .into_serializer()
            .wrap_in(packet.builder())
            .wrap_in(frame.builder())
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap();
        assert_eq!(buffer.as_ref(), ETHERNET_FRAME.bytes);

        // Verify serialization via `to_vec`.
        assert_eq!(&packet.to_vec()[..], IPV4_PACKET.bytes);
    }

    #[test]
    fn test_parse_serialize_full_udp() {
        use crate::testdata::dns_request_v4::*;

        let mut buf = ETHERNET_FRAME.bytes;
        let frame = buf.parse_with::<_, EthernetFrame<_>>(EthernetFrameLengthCheck::Check).unwrap();
        verify_ethernet_frame(&frame, ETHERNET_FRAME);

        let mut body = frame.body();
        let packet = body.parse::<Ipv4Packet<_>>().unwrap();
        verify_ipv4_packet(&packet, IPV4_PACKET);

        // Verify serialization via builders.
        let buffer = packet
            .body()
            .into_serializer()
            .wrap_in(packet.builder())
            .wrap_in(frame.builder())
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap();
        assert_eq!(buffer.as_ref(), ETHERNET_FRAME.bytes);

        // Verify serialization via `to_vec`.
        assert_eq!(&packet.to_vec()[..], IPV4_PACKET.bytes);
    }

    #[test]
    fn test_parse_serialize_with_options() {
        // NB; Use IGMPv2 as test data arbitrarily, because it includes IP
        // header options.
        use crate::testdata::igmpv2_membership::report::*;

        let mut buf = IP_PACKET_BYTES;
        let packet = buf.parse::<Ipv4Packet<_>>().unwrap();
        assert_eq!(packet.iter_options().count(), 1);

        // NB: Don't verify serialization via builders, as they omit IP header
        // options.

        // Verify serialization via `to_vec`.
        assert_eq!(&packet.to_vec()[..], IP_PACKET_BYTES);
    }

    fn hdr_prefix_to_bytes(hdr_prefix: HeaderPrefix) -> [u8; 20] {
        zerocopy::transmute!(hdr_prefix)
    }

    // Return a new HeaderPrefix with reasonable defaults, including a valid
    // header checksum.
    fn new_hdr_prefix() -> HeaderPrefix {
        HeaderPrefix::new(
            5,
            DscpAndEcn::default(),
            20,
            0x0102,
            0,
            0,
            0x03,
            IpProto::Tcp.into(),
            [0xa6, 0xcf],
            DEFAULT_SRC_IP,
            DEFAULT_DST_IP,
        )
    }

    #[test]
    fn test_parse() {
        let mut bytes = &hdr_prefix_to_bytes(new_hdr_prefix())[..];
        let packet = bytes.parse::<Ipv4Packet<_>>().unwrap();
        assert_eq!(packet.id(), 0x0102);
        assert_eq!(packet.ttl(), 0x03);
        assert_eq!(packet.proto(), IpProto::Tcp.into());
        assert_eq!(packet.src_ip(), DEFAULT_SRC_IP);
        assert_eq!(packet.dst_ip(), DEFAULT_DST_IP);
        assert_eq!(packet.body(), []);
    }

    #[test]
    fn test_parse_padding() {
        // Test that we properly discard post-packet padding.
        let mut buffer = Buf::new(Vec::new(), ..)
            .wrap_in(Ipv4PacketBuilder::new(DEFAULT_DST_IP, DEFAULT_DST_IP, 0, IpProto::Tcp.into()))
            .wrap_in(EthernetFrameBuilder::new(
                DEFAULT_SRC_MAC,
                DEFAULT_DST_MAC,
                EtherType::Ipv4,
                ETHERNET_MIN_BODY_LEN_NO_TAG,
            ))
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap();
        let _: EthernetFrame<_> =
            buffer.parse_with::<_, EthernetFrame<_>>(EthernetFrameLengthCheck::Check).unwrap();
        // Test that the Ethernet body is the minimum length, which far exceeds
        // the IPv4 packet header size of 20 bytes (without options).
        assert_eq!(buffer.len(), 46);
        let packet = buffer.parse::<Ipv4Packet<_>>().unwrap();
        // Test that we've properly discarded the post-packet padding, and have
        // an empty body.
        assert_eq!(packet.body().len(), 0);
        // Test that we not only ignored the padding, but properly consumed it
        // from the underlying buffer as we're required to do by the
        // ParsablePacket contract.
        assert_eq!(buffer.len(), 0);
    }

    #[test]
    fn test_parse_error() {
        // Set the version to 5. The version must be 4.
        let mut hdr_prefix = new_hdr_prefix();
        hdr_prefix.version_ihl = (5 << 4) | 5;
        assert_eq!(
            (&hdr_prefix_to_bytes(hdr_prefix)[..]).parse::<Ipv4Packet<_>>().unwrap_err(),
            ParseError::Format.into()
        );

        // Set the IHL to 4, implying a header length of 16. This is smaller
        // than the minimum of 20.
        let mut hdr_prefix = new_hdr_prefix();
        hdr_prefix.version_ihl = (4 << 4) | 4;
        assert_eq!(
            (&hdr_prefix_to_bytes(hdr_prefix)[..]).parse::<Ipv4Packet<_>>().unwrap_err(),
            ParseError::Format.into()
        );

        // Set the IHL to 6, implying a header length of 24. This is larger than
        // the actual packet length of 20.
        let mut hdr_prefix = new_hdr_prefix();
        hdr_prefix.version_ihl = (4 << 4) | 6;
        assert_eq!(
            (&hdr_prefix_to_bytes(hdr_prefix)[..]).parse::<Ipv4Packet<_>>().unwrap_err(),
            ParseError::Format.into()
        );
    }

    // Return a stock Ipv4PacketBuilder with reasonable default values.
    fn new_builder() -> Ipv4PacketBuilder {
        Ipv4PacketBuilder::new(DEFAULT_SRC_IP, DEFAULT_DST_IP, 64, IpProto::Tcp.into())
    }

    #[test]
    fn test_fragment_type() {
        fn test_fragment_type_helper(fragment_offset: u16, expect_fragment_type: Ipv4FragmentType) {
            let mut builder = new_builder();
            builder.fragment_offset(FragmentOffset::new(fragment_offset).unwrap());

            let mut buf = [0; IPV4_MIN_HDR_LEN]
                .into_serializer()
                .wrap_in(builder)
                .serialize_vec_outer(&mut NoOpSerializationContext)
                .unwrap();

            let packet = buf.parse::<Ipv4Packet<_>>().unwrap();
            assert_eq!(packet.fragment_type(), expect_fragment_type);
        }

        test_fragment_type_helper(0x0000, Ipv4FragmentType::InitialFragment);
        test_fragment_type_helper(0x0008, Ipv4FragmentType::NonInitialFragment);
    }

    #[test]
    fn test_serialize() {
        let mut builder = new_builder();
        builder.dscp_and_ecn(DscpAndEcn::new(0x12, 3));
        builder.id(0x0405);
        builder.df_flag(true);
        builder.mf_flag(true);
        builder.fragment_offset(FragmentOffset::new(0x0607).unwrap());

        let mut buf = (&[0, 1, 2, 3, 3, 4, 5, 7, 8, 9])
            .into_serializer()
            .wrap_in(builder)
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap();
        assert_eq!(
            buf.as_ref(),
            [
                69, 75, 0, 30, 4, 5, 102, 7, 64, 6, 0, 112, 1, 2, 3, 4, 5, 6, 7, 8, 0, 1, 2, 3, 3,
                4, 5, 7, 8, 9
            ]
        );
        let packet = buf.parse::<Ipv4Packet<_>>().unwrap();
        assert_eq!(packet.dscp_and_ecn().dscp(), 0x12);
        assert_eq!(packet.dscp_and_ecn().ecn(), 3);
        assert_eq!(packet.id(), 0x0405);
        assert!(packet.df_flag());
        assert!(packet.mf_flag());
        assert_eq!(packet.fragment_offset().into_raw(), 0x0607);
        assert_eq!(packet.fragment_type(), Ipv4FragmentType::NonInitialFragment);
    }

    #[test]
    fn test_partial_serialize() {
        let mut builder = new_builder();
        builder.dscp_and_ecn(DscpAndEcn::new(0x12, 3));
        builder.id(0x0405);
        builder.df_flag(true);
        builder.mf_flag(true);
        builder.fragment_offset(FragmentOffset::new(0x0607).unwrap());
        const BODY: &[u8] = &[0, 1, 2, 3, 3, 4, 5, 7, 8, 9];
        let packet = BODY.into_serializer().wrap_in(builder);
        const HEADER: &[u8] = &[69, 75, 0, 30, 4, 5, 102, 7, 64, 6, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8];
        const PACKET_LEN: usize = HEADER.len() + BODY.len();

        // Note that this header is different from the one in test_serialize
        // because the checksum is not calculated.

        let buf = assert_matches!(
            packet.partial_serialize(&mut NoOpSerializationContext, packet::new_buf_vec),
            Ok(PartialSerializeResult::NewBuffer { buffer, total_size: PACKET_LEN }) => buffer
        );
        assert_eq!(buf.as_ref(), HEADER);
    }

    #[test]
    fn test_serialize_id_unset() {
        let mut builder = new_builder();
        builder.id(0x0405);
        builder.df_flag(true);

        let mut buf = (&[0, 1, 2, 3, 3, 4, 5, 7, 8, 9])
            .into_serializer()
            .wrap_in(builder)
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap();
        let packet = buf.parse::<Ipv4Packet<_>>().unwrap();
        assert_eq!(packet.id(), 0);
        assert!(packet.df_flag());
        assert_eq!(packet.mf_flag(), false);
        assert_eq!(packet.fragment_offset().into_raw(), 0);
        assert_eq!(packet.fragment_type(), Ipv4FragmentType::InitialFragment);
    }

    #[test]
    fn test_serialize_zeroes() {
        // Test that Ipv4PacketBuilder::serialize properly zeroes memory before
        // serializing the header.
        let mut buf_0 = [0; IPV4_MIN_HDR_LEN];
        let _: Buf<&mut [u8]> = Buf::new(&mut buf_0[..], IPV4_MIN_HDR_LEN..)
            .wrap_in(new_builder())
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap()
            .unwrap_a();
        let mut buf_1 = [0xFF; IPV4_MIN_HDR_LEN];
        let _: Buf<&mut [u8]> = Buf::new(&mut buf_1[..], IPV4_MIN_HDR_LEN..)
            .wrap_in(new_builder())
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap()
            .unwrap_a();
        assert_eq!(buf_0, buf_1);
    }

    #[test]
    #[should_panic(expected = "(SizeLimitExceeded, Nested { inner: Buf { buf:")]
    fn test_serialize_panic_packet_length() {
        // Test that a packet which is longer than 2^16 - 1 bytes is rejected.
        let _: Buf<&mut [u8]> = Buf::new(&mut [0; (1 << 16) - IPV4_MIN_HDR_LEN][..], ..)
            .wrap_in(new_builder())
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap()
            .unwrap_a();
    }

    #[test]
    fn test_copy_header_bytes_for_fragment() {
        let hdr_prefix = new_hdr_prefix();
        let mut bytes = hdr_prefix_to_bytes(hdr_prefix);
        let mut buf = &bytes[..];
        let packet = buf.parse::<Ipv4Packet<_>>().unwrap();
        let copied_bytes = packet.copy_header_bytes_for_fragment();
        bytes[IPV4_FRAGMENT_DATA_BYTE_RANGE].copy_from_slice(&[0; 4][..]);
        assert_eq!(&copied_bytes[..], &bytes[..]);
    }

    #[test]
    fn test_partial_parsing() {
        use core::ops::Deref as _;

        // Try something with only the header, but that would have a larger
        // body:
        let mut hdr_prefix = new_hdr_prefix();
        hdr_prefix.total_len = U16::new(256);
        let mut bytes = hdr_prefix_to_bytes(hdr_prefix)[..].to_owned();
        const PAYLOAD: &[u8] = &[1, 2, 3, 4, 5];
        bytes.extend(PAYLOAD);
        let mut buf = &bytes[..];
        let packet = buf.parse::<Ipv4PacketRaw<_>>().unwrap();
        let Ipv4PacketRaw { hdr_prefix, options, body } = &packet;
        assert_eq!(Ref::bytes(&hdr_prefix), &bytes[0..20]);
        assert_eq!(options.as_ref().complete().unwrap().deref(), []);
        // We must've captured the incomplete bytes in body:
        assert_eq!(body, &MaybeParsed::Incomplete(PAYLOAD));
        // validation should fail:
        assert!(Ipv4Packet::try_from_raw(packet).is_err());

        // Try something with the header plus incomplete options:
        let mut hdr_prefix = new_hdr_prefix();
        hdr_prefix.version_ihl = (4 << 4) | 10;
        let bytes = hdr_prefix_to_bytes(hdr_prefix);
        let mut buf = &bytes[..];
        let packet = buf.parse::<Ipv4PacketRaw<_>>().unwrap();
        let Ipv4PacketRaw { hdr_prefix, options, body } = &packet;
        assert_eq!(Ref::bytes(&hdr_prefix), bytes);
        assert_eq!(options.as_ref().incomplete().unwrap(), &[]);
        assert_eq!(body.complete().unwrap(), []);
        // validation should fail:
        assert!(Ipv4Packet::try_from_raw(packet).is_err());

        // Try an incomplete header:
        let hdr_prefix = new_hdr_prefix();
        let bytes = &hdr_prefix_to_bytes(hdr_prefix);
        let mut buf = &bytes[0..10];
        assert!(buf.parse::<Ipv4PacketRaw<_>>().is_err());
    }

    fn create_ipv4_and_ipv6_builders(
        proto_v4: Ipv4Proto,
        proto_v6: Ipv6Proto,
    ) -> (Ipv4PacketBuilder, Ipv6PacketBuilder) {
        const IP_DSCP_AND_ECN: DscpAndEcn = DscpAndEcn::new(0x12, 3);
        const IP_TTL: u8 = 64;

        let mut ipv4_builder =
            Ipv4PacketBuilder::new(DEFAULT_SRC_IP, DEFAULT_DST_IP, IP_TTL, proto_v4);
        ipv4_builder.dscp_and_ecn(IP_DSCP_AND_ECN);
        ipv4_builder.id(0x0405);
        ipv4_builder.df_flag(true);
        ipv4_builder.mf_flag(false);
        ipv4_builder.fragment_offset(FragmentOffset::ZERO);

        let mut ipv6_builder =
            Ipv6PacketBuilder::new(DEFAULT_V6_SRC_IP, DEFAULT_V6_DST_IP, IP_TTL, proto_v6);
        ipv6_builder.dscp_and_ecn(IP_DSCP_AND_ECN);
        ipv6_builder.flowlabel(0);

        (ipv4_builder, ipv6_builder)
    }

    fn create_tcp_ipv4_and_ipv6_pkt()
    -> (packet::Either<EmptyBuf, Buf<Vec<u8>>>, packet::Either<EmptyBuf, Buf<Vec<u8>>>) {
        use crate::tcp::TcpSegmentBuilder;
        use core::num::NonZeroU16;

        let tcp_src_port: NonZeroU16 = NonZeroU16::new(20).unwrap();
        let tcp_dst_port: NonZeroU16 = NonZeroU16::new(30).unwrap();
        const TCP_SEQ_NUM: u32 = 4321;
        const TCP_ACK_NUM: Option<u32> = Some(1234);
        const TCP_WINDOW_SIZE: u16 = 12345;
        const PAYLOAD: [u8; 10] = [0, 1, 2, 3, 3, 4, 5, 7, 8, 9];

        let (ipv4_builder, ipv6_builder) =
            create_ipv4_and_ipv6_builders(IpProto::Tcp.into(), IpProto::Tcp.into());

        let tcp_builder = TcpSegmentBuilder::new(
            DEFAULT_SRC_IP,
            DEFAULT_DST_IP,
            tcp_src_port,
            tcp_dst_port,
            TCP_SEQ_NUM,
            TCP_ACK_NUM,
            TCP_WINDOW_SIZE,
        );

        let v4_pkt_buf = (&PAYLOAD)
            .into_serializer()
            .wrap_in(tcp_builder)
            .wrap_in(ipv4_builder)
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap();

        let v6_tcp_builder = TcpSegmentBuilder::new(
            DEFAULT_V6_SRC_IP,
            DEFAULT_V6_DST_IP,
            tcp_src_port,
            tcp_dst_port,
            TCP_SEQ_NUM,
            TCP_ACK_NUM,
            TCP_WINDOW_SIZE,
        );

        let v6_pkt_buf = (&PAYLOAD)
            .into_serializer()
            .wrap_in(v6_tcp_builder)
            .wrap_in(ipv6_builder)
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap();

        (v4_pkt_buf, v6_pkt_buf)
    }

    #[test]
    fn test_nat64_translate_tcp() {
        let (mut v4_pkt_buf, expected_v6_pkt_buf) = create_tcp_ipv4_and_ipv6_pkt();

        let parsed_v4_packet = v4_pkt_buf.parse::<Ipv4Packet<_>>().unwrap();
        let nat64_translation_result =
            parsed_v4_packet.nat64_translate(DEFAULT_V6_SRC_IP, DEFAULT_V6_DST_IP);

        let serializable_pkt = match nat64_translation_result {
            Nat64TranslationResult::Forward(s) => s,
            _ => panic!("Nat64TranslationResult not of Forward type!"),
        };

        let translated_v6_pkt_buf =
            serializable_pkt.serialize_vec_outer(&mut NoOpSerializationContext).unwrap();

        assert_eq!(
            expected_v6_pkt_buf.to_flattened_vec(),
            translated_v6_pkt_buf.to_flattened_vec()
        );
    }

    fn create_udp_ipv4_and_ipv6_pkt()
    -> (packet::Either<EmptyBuf, Buf<Vec<u8>>>, packet::Either<EmptyBuf, Buf<Vec<u8>>>) {
        use crate::udp::UdpPacketBuilder;
        use core::num::NonZeroU16;

        let udp_src_port: NonZeroU16 = NonZeroU16::new(35000).unwrap();
        let udp_dst_port: NonZeroU16 = NonZeroU16::new(53).unwrap();
        const PAYLOAD: [u8; 10] = [0, 1, 2, 3, 3, 4, 5, 7, 8, 9];

        let (ipv4_builder, ipv6_builder) =
            create_ipv4_and_ipv6_builders(IpProto::Udp.into(), IpProto::Udp.into());

        let udp_builder =
            UdpPacketBuilder::new(DEFAULT_SRC_IP, DEFAULT_DST_IP, Some(udp_src_port), udp_dst_port);

        let v4_pkt_buf = (&PAYLOAD)
            .into_serializer()
            .wrap_in(udp_builder)
            .wrap_in(ipv4_builder)
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap();

        let v6_udp_builder = UdpPacketBuilder::new(
            DEFAULT_V6_SRC_IP,
            DEFAULT_V6_DST_IP,
            Some(udp_src_port),
            udp_dst_port,
        );

        let v6_pkt_buf = (&PAYLOAD)
            .into_serializer()
            .wrap_in(v6_udp_builder)
            .wrap_in(ipv6_builder)
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap();

        (v4_pkt_buf, v6_pkt_buf)
    }

    #[test]
    fn test_nat64_translate_udp() {
        let (mut v4_pkt_buf, expected_v6_pkt_buf) = create_udp_ipv4_and_ipv6_pkt();

        let parsed_v4_packet = v4_pkt_buf.parse::<Ipv4Packet<_>>().unwrap();
        let nat64_translation_result =
            parsed_v4_packet.nat64_translate(DEFAULT_V6_SRC_IP, DEFAULT_V6_DST_IP);

        let serializable_pkt = match nat64_translation_result {
            Nat64TranslationResult::Forward(s) => s,
            _ => panic!(
                "Nat64TranslationResult not of Forward type: {:?} ",
                nat64_translation_result
            ),
        };

        let translated_v6_pkt_buf =
            serializable_pkt.serialize_vec_outer(&mut NoOpSerializationContext).unwrap();

        assert_eq!(
            expected_v6_pkt_buf.to_flattened_vec(),
            translated_v6_pkt_buf.to_flattened_vec()
        );
    }

    #[test]
    fn test_nat64_translate_non_tcp_udp_icmp() {
        const PAYLOAD: [u8; 10] = [0, 1, 2, 3, 3, 4, 5, 7, 8, 9];

        let (ipv4_builder, ipv6_builder) =
            create_ipv4_and_ipv6_builders(Ipv4Proto::Other(50), Ipv6Proto::Other(50));

        let mut v4_pkt_buf = (&PAYLOAD)
            .into_serializer()
            .wrap_in(ipv4_builder)
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap();

        let expected_v6_pkt_buf = (&PAYLOAD)
            .into_serializer()
            .wrap_in(ipv6_builder)
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap();

        let translated_v6_pkt_buf = {
            let parsed_v4_packet = v4_pkt_buf.parse::<Ipv4Packet<_>>().unwrap();

            let nat64_translation_result =
                parsed_v4_packet.nat64_translate(DEFAULT_V6_SRC_IP, DEFAULT_V6_DST_IP);

            let serializable_pkt = match nat64_translation_result {
                Nat64TranslationResult::Forward(s) => s,
                _ => panic!(
                    "Nat64TranslationResult not of Forward type: {:?} ",
                    nat64_translation_result
                ),
            };

            let translated_buf =
                serializable_pkt.serialize_vec_outer(&mut NoOpSerializationContext).unwrap();

            translated_buf
        };

        assert_eq!(
            expected_v6_pkt_buf.to_flattened_vec(),
            translated_v6_pkt_buf.to_flattened_vec()
        );
    }

    #[test]
    fn test_partial_serialize_parsed() {
        const PACKET_BYTES: &[u8] = &[
            69, 75, 0, 30, 4, 5, 102, 7, 64, 6, 0, 112, 1, 2, 3, 4, 5, 6, 7, 8, 0, 1, 2, 3, 3, 4,
            5, 7, 8, 9,
        ];
        const PACKET_LEN: usize = PACKET_BYTES.len();
        let mut packet_bytes_copy = Vec::from(PACKET_BYTES);
        let mut packet_bytes_ref: &mut [u8] = &mut packet_bytes_copy[..];
        let packet = packet_bytes_ref.parse::<Ipv4Packet<_>>().unwrap();

        let buf = assert_matches!(
            packet.partial_serialize(&mut NoOpSerializationContext, packet::new_buf_vec),
            Ok(PartialSerializeResult::NewBuffer { buffer, total_size: PACKET_LEN }) => buffer
        );
        assert_eq!(buf.as_ref(), PACKET_BYTES);
    }
}
