// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! IP protocol types.

// TODO(https://fxbug.dev/326330182): this import seems actually necessary. Is this a bug on the
// lint?
#[allow(unused_imports)]
use alloc::vec::Vec;
use core::cmp::PartialEq;
use core::convert::Infallible as Never;
use core::fmt::{Debug, Display};
use core::hash::Hash;
use core::marker::PhantomData;

use net_types::ip::{GenericOverIp, Ip, IpAddr, Ipv4, Ipv4Addr, Ipv6, Ipv6Addr};
use packet::{
    BufferViewMut, NoOpSerializationContext, PacketBuilder, ParsablePacket, ParseMetadata,
    PartialPacketBuilder, SerializationContext,
};
use zerocopy::{
    FromBytes, Immutable, IntoBytes, KnownLayout, SplitByteSlice, SplitByteSliceMut, Unaligned,
};

use crate::error::{IpParseResult, Ipv6ParseError, ParseError};
use crate::ethernet::EthernetIpExt;
use crate::icmp::IcmpIpExt;
use crate::ipv4::{IPV4_MIN_HDR_LEN, Ipv4Header, Ipv4Packet, Ipv4PacketBuilder, Ipv4PacketRaw};
use crate::ipv6::{IPV6_FIXED_HDR_LEN, Ipv6Header, Ipv6Packet, Ipv6PacketBuilder, Ipv6PacketRaw};
use crate::private::Sealed;

/// An [`Ip`] extension trait adding an associated type for the IP protocol
/// number.
pub trait IpProtoExt: Ip {
    /// The type representing an IPv4 or IPv6 protocol number.
    ///
    /// For IPv4, this is [`Ipv4Proto`], and for IPv6, this is [`Ipv6Proto`].
    type Proto: IpProtocol
        + GenericOverIp<Self, Type = Self::Proto>
        + GenericOverIp<Ipv4, Type = Ipv4Proto>
        + GenericOverIp<Ipv6, Type = Ipv6Proto>
        + Copy
        + Clone
        + Hash
        + Debug
        + Display
        + PartialEq
        + Eq
        + PartialOrd
        + Ord;
}

impl IpProtoExt for Ipv4 {
    type Proto = Ipv4Proto;
}

impl IpProtoExt for Ipv6 {
    type Proto = Ipv6Proto;
}

/// IP packet context relevant to serialization.
pub struct IpEnvelope<I: IpExt> {
    /// True if the IP packet has options (IPv4) or extension headers (IPv6).
    pub has_options: bool,
    _marker: PhantomData<I>,
}

impl<I: IpExt> IpEnvelope<I> {
    /// Creates a new `IpEnvelope`.
    pub fn new(has_options: bool) -> Self {
        Self { has_options, _marker: PhantomData }
    }
}

/// A trait for IP serialization contexts.
pub trait IpSerializationContext<I: IpExt>: SerializationContext {
    /// Converts an `IpEnvelope` into the serialization context's state.
    fn envelope_to_state(envelope: IpEnvelope<I>) -> Self::ContextState;
}

impl<I: IpExt> IpSerializationContext<I> for NoOpSerializationContext {
    fn envelope_to_state(_envelope: IpEnvelope<I>) -> Self::ContextState {
        ()
    }
}

/// An extension trait to the `Ip` trait adding associated types relevant for
/// packet parsing and serialization.
pub trait IpExt: EthernetIpExt + IcmpIpExt {
    /// The error type returned when parsing a packet of this IP version fails.
    type PacketParseError: From<ParseError> + Debug + PartialEq + Send + Sync;

    /// An IP packet type for this IP version.
    type Packet<B: SplitByteSlice>: IpPacket<B, Self>
        + GenericOverIp<Self, Type = Self::Packet<B>>
        + GenericOverIp<Ipv4, Type = Ipv4Packet<B>>
        + GenericOverIp<Ipv6, Type = Ipv6Packet<B>>;
    /// A raw IP packet type for this IP version.
    type PacketRaw<B: SplitByteSlice>: IpPacketRaw<B, Self>
        + GenericOverIp<Self, Type = Self::PacketRaw<B>>
        + GenericOverIp<Ipv4, Type = Ipv4PacketRaw<B>>
        + GenericOverIp<Ipv6, Type = Ipv6PacketRaw<B>>;
    /// An IP packet builder type for the IP version.
    type PacketBuilder<C: IpSerializationContext<Self>>: IpPacketBuilder<C, Self> + Eq;
    /// Minimal IP header size.
    const MIN_HEADER_LENGTH: usize;
}

impl IpExt for Ipv4 {
    type PacketParseError = ParseError;
    type Packet<B: SplitByteSlice> = Ipv4Packet<B>;
    type PacketRaw<B: SplitByteSlice> = Ipv4PacketRaw<B>;
    type PacketBuilder<C: IpSerializationContext<Self>> = Ipv4PacketBuilder;

    const MIN_HEADER_LENGTH: usize = IPV4_MIN_HDR_LEN;
}

impl IpExt for Ipv6 {
    type PacketParseError = Ipv6ParseError;
    type Packet<B: SplitByteSlice> = Ipv6Packet<B>;
    type PacketRaw<B: SplitByteSlice> = Ipv6PacketRaw<B>;
    type PacketBuilder<C: IpSerializationContext<Self>> = Ipv6PacketBuilder;

    const MIN_HEADER_LENGTH: usize = IPV6_FIXED_HDR_LEN;
}

/// An error encountered during NAT64 translation.
#[derive(Debug)]
pub enum Nat64Error {
    /// Support not yet implemented in the library.
    NotImplemented,
}

/// The result of NAT64 translation.
#[derive(Debug)]
pub enum Nat64TranslationResult<S, E> {
    /// Forward the packet encoded in `S`.
    Forward(S),
    /// Silently drop the packet.
    Drop,
    /// An error was encountered.
    Err(E),
}

/// Combines Differentiated Services Code Point (DSCP) and Explicit Congestion
/// Notification (ECN) values into one. Internally the 2 fields are stored
/// using the same layout as the Traffic Class field in IPv6 and the Type Of
/// Service field in IPv4: 6 higher bits for DSCP and 2 lower bits for ECN.
#[derive(
    Default,
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    KnownLayout,
    FromBytes,
    IntoBytes,
    Immutable,
    Unaligned,
)]
#[repr(C)]
pub struct DscpAndEcn(u8);

const DSCP_OFFSET: u8 = 2;
const DSCP_MAX: u8 = (1 << (8 - DSCP_OFFSET)) - 1;
const ECN_MAX: u8 = (1 << DSCP_OFFSET) - 1;

impl DscpAndEcn {
    /// Returns the default value. Implemented separately from the `Default`
    /// trait to make it `const`.
    pub const fn default() -> Self {
        Self(0)
    }

    /// Creates a new `DscpAndEcn` instance with the specified DSCP and ECN
    /// values.
    pub const fn new(dscp: u8, ecn: u8) -> Self {
        debug_assert!(dscp <= DSCP_MAX);
        debug_assert!(ecn <= ECN_MAX);
        Self((dscp << DSCP_OFFSET) + ecn)
    }

    /// Constructs a new `DspAndEcn` from a raw value, i.e., both fields packet
    /// into one byte.
    pub const fn new_with_raw(value: u8) -> Self {
        Self(value)
    }

    /// Returns the Differentiated Services Code Point value.
    pub fn dscp(self) -> u8 {
        let Self(v) = self;
        v >> 2
    }

    /// Returns the Explicit Congestion Notification value.
    pub fn ecn(self) -> u8 {
        let Self(v) = self;
        v & 0x3
    }

    /// Returns the raw value, i.e. both fields packed into one byte.
    pub fn raw(self) -> u8 {
        let Self(value) = self;
        value
    }
}

impl From<u8> for DscpAndEcn {
    fn from(value: u8) -> Self {
        Self::new_with_raw(value)
    }
}

/// An IPv4 or IPv6 packet.
///
/// `IpPacket` is implemented by `Ipv4Packet` and `Ipv6Packet`.
pub trait IpPacket<B: SplitByteSlice, I: IpExt>:
    Sized + Debug + ParsablePacket<B, (), Error = I::PacketParseError>
{
    /// A builder for this packet type.
    type Builder<C: IpSerializationContext<I>>: IpPacketBuilder<C, I>;

    /// The source IP address.
    fn src_ip(&self) -> I::Addr;

    /// The destination IP address.
    fn dst_ip(&self) -> I::Addr;

    /// The protocol number.
    fn proto(&self) -> I::Proto;

    /// The Time to Live (TTL) (IPv4) or Hop Limit (IPv6) field.
    fn ttl(&self) -> u8;

    /// The Differentiated Services Code Point (DSCP) and the Explicit
    /// Congestion Notification (ECN).
    fn dscp_and_ecn(&self) -> DscpAndEcn;

    /// Set the Time to Live (TTL) (IPv4) or Hop Limit (IPv6) field.
    ///
    /// `set_ttl` updates the packet's TTL/Hop Limit in place.
    fn set_ttl(&mut self, ttl: u8)
    where
        B: SplitByteSliceMut;

    /// Get the body.
    fn body(&self) -> &[u8];

    /// Consume the packet and return some metadata.
    ///
    /// Consume the packet and return the source address, destination address,
    /// protocol, and `ParseMetadata`.
    fn into_metadata(self) -> (I::Addr, I::Addr, I::Proto, ParseMetadata) {
        let src_ip = self.src_ip();
        let dst_ip = self.dst_ip();
        let proto = self.proto();
        let meta = self.parse_metadata();
        (src_ip, dst_ip, proto, meta)
    }

    /// Converts a packet reference into a dynamically-typed reference.
    fn as_ip_addr_ref(&self) -> IpAddr<&'_ Ipv4Packet<B>, &'_ Ipv6Packet<B>>;

    /// Reassembles a fragmented packet into a parsed IP packet.
    ///
    /// # Panics
    ///
    /// Panics if the provided header is too small to hold a valid header.
    fn reassemble_fragmented_packet<'a, BV: BufferViewMut<B>, IT: Iterator<Item = &'a [u8]>>(
        buffer: BV,
        header: &[u8],
        body_fragments: IT,
    ) -> IpParseResult<I, ()>
    where
        B: SplitByteSliceMut;

    /// Copies the full packet into a `Vec`.
    fn to_vec(&self) -> Vec<u8>;

    /// Constructs a builder with the same contents as this packet's header.
    fn builder<C: IpSerializationContext<I>>(&self) -> Self::Builder<C>;
}

impl<B: SplitByteSlice> IpPacket<B, Ipv4> for Ipv4Packet<B> {
    type Builder<C: IpSerializationContext<Ipv4>> = Ipv4PacketBuilder;

    fn src_ip(&self) -> Ipv4Addr {
        Ipv4Header::src_ip(self)
    }
    fn dst_ip(&self) -> Ipv4Addr {
        Ipv4Header::dst_ip(self)
    }
    fn proto(&self) -> Ipv4Proto {
        Ipv4Header::proto(self)
    }
    fn dscp_and_ecn(&self) -> DscpAndEcn {
        Ipv4Header::dscp_and_ecn(self)
    }
    fn ttl(&self) -> u8 {
        Ipv4Header::ttl(self)
    }
    fn set_ttl(&mut self, ttl: u8)
    where
        B: SplitByteSliceMut,
    {
        Ipv4Packet::set_ttl(self, ttl)
    }
    fn body(&self) -> &[u8] {
        Ipv4Packet::body(self)
    }

    fn as_ip_addr_ref(&self) -> IpAddr<&'_ Self, &'_ Ipv6Packet<B>> {
        IpAddr::V4(self)
    }

    fn reassemble_fragmented_packet<'a, BV: BufferViewMut<B>, IT: Iterator<Item = &'a [u8]>>(
        buffer: BV,
        header: &[u8],
        body_fragments: IT,
    ) -> IpParseResult<Ipv4, ()>
    where
        B: SplitByteSliceMut,
    {
        crate::ipv4::reassemble_fragmented_packet(buffer, header, body_fragments)
    }

    fn to_vec(&self) -> Vec<u8> {
        self.to_vec()
    }

    fn builder<C: IpSerializationContext<Ipv4>>(&self) -> Self::Builder<C> {
        Ipv4Header::builder(self)
    }
}

impl<B: SplitByteSlice> IpPacket<B, Ipv6> for Ipv6Packet<B> {
    type Builder<C: IpSerializationContext<Ipv6>> = Ipv6PacketBuilder;

    fn src_ip(&self) -> Ipv6Addr {
        Ipv6Header::src_ip(self)
    }
    fn dst_ip(&self) -> Ipv6Addr {
        Ipv6Header::dst_ip(self)
    }
    fn proto(&self) -> Ipv6Proto {
        Ipv6Packet::proto(self)
    }
    fn dscp_and_ecn(&self) -> DscpAndEcn {
        Ipv6Header::dscp_and_ecn(self)
    }
    fn ttl(&self) -> u8 {
        Ipv6Header::hop_limit(self)
    }
    fn set_ttl(&mut self, ttl: u8)
    where
        B: SplitByteSliceMut,
    {
        Ipv6Packet::set_hop_limit(self, ttl)
    }
    fn body(&self) -> &[u8] {
        Ipv6Packet::body(self)
    }

    fn as_ip_addr_ref(&self) -> IpAddr<&'_ Ipv4Packet<B>, &'_ Self> {
        IpAddr::V6(self)
    }
    fn reassemble_fragmented_packet<'a, BV: BufferViewMut<B>, IT: Iterator<Item = &'a [u8]>>(
        buffer: BV,
        header: &[u8],
        body_fragments: IT,
    ) -> IpParseResult<Ipv6, ()>
    where
        B: SplitByteSliceMut,
    {
        crate::ipv6::reassemble_fragmented_packet(buffer, header, body_fragments)
    }

    fn to_vec(&self) -> Vec<u8> {
        self.to_vec()
    }

    fn builder<C: IpSerializationContext<Ipv6>>(&self) -> Self::Builder<C> {
        self.builder()
    }
}

/// A raw IPv4 or IPv6 packet.
///
/// `IpPacketRaw` is implemented by `Ipv4PacketRaw` and `Ipv6PacketRaw`.
pub trait IpPacketRaw<B: SplitByteSlice, I: IpExt>:
    Sized + ParsablePacket<B, (), Error = I::PacketParseError>
{
}

impl<B: SplitByteSlice> IpPacketRaw<B, Ipv4> for Ipv4PacketRaw<B> {}
impl<B: SplitByteSlice, I: IpExt> GenericOverIp<I> for Ipv4PacketRaw<B> {
    type Type = <I as IpExt>::PacketRaw<B>;
}

impl<B: SplitByteSlice> IpPacketRaw<B, Ipv6> for Ipv6PacketRaw<B> {}
impl<B: SplitByteSlice, I: IpExt> GenericOverIp<I> for Ipv6PacketRaw<B> {
    type Type = <I as IpExt>::PacketRaw<B>;
}

/// A builder for IP packets.
pub trait IpPacketBuilder<C: SerializationContext, I: IpExt>:
    PacketBuilder<C> + PartialPacketBuilder<C> + Clone + Debug
{
    /// Returns a new packet builder for an associated IP version with the given
    /// given source and destination IP addresses, TTL (IPv4)/Hop Limit (IPv4)
    /// and Protocol Number.
    fn new(src_ip: I::Addr, dst_ip: I::Addr, ttl: u8, proto: I::Proto) -> Self;

    /// Returns the source IP address for the builder.
    fn src_ip(&self) -> I::Addr;

    /// Sets the source IP address for the builder.
    fn set_src_ip(&mut self, addr: I::Addr);

    /// Returns the destination IP address for the builder.
    fn dst_ip(&self) -> I::Addr;

    /// Sets the destination IP address for the builder.
    fn set_dst_ip(&mut self, addr: I::Addr);

    /// Returns the IP protocol number for the builder.
    fn proto(&self) -> I::Proto;

    /// Set DSCP & ECN fields.
    fn set_dscp_and_ecn(&mut self, dscp_and_ecn: DscpAndEcn);
}

/// An IPv4 or IPv6 protocol number.
pub trait IpProtocol: From<IpProto> + From<u8> + Sealed + Send + Sync + 'static {}

impl Sealed for Never {}

create_protocol_enum!(
    /// An IPv4 or IPv6 protocol number.
    ///
    /// `IpProto` encodes the protocol numbers whose values are the same for
    /// both IPv4 and IPv6.
    ///
    /// The protocol numbers are maintained [by IANA][protocol-numbers].
    ///
    /// [protocol-numbers]: https://www.iana.org/assignments/protocol-numbers/protocol-numbers.xhtml
    #[allow(missing_docs)]
    #[derive(Copy, Clone, Hash, Eq, Ord, PartialEq, PartialOrd)]
    pub enum IpProto: u8 {
        Tcp, 6, "TCP";
        Udp, 17, "UDP";
        Reserved, 255, "IANA-RESERVED";
    }
);

create_protocol_enum!(
    /// An IPv4 protocol number.
    ///
    /// The protocol numbers are maintained [by IANA][protocol-numbers].
    ///
    /// [protocol-numbers]: https://www.iana.org/assignments/protocol-numbers/protocol-numbers.xhtml
    #[allow(missing_docs)]
    #[derive(Copy, Clone, Hash, Eq, Ord, PartialEq, PartialOrd)]
    pub enum Ipv4Proto: u8 {
        Icmp, 1, "ICMP";
        Igmp, 2, "IGMP";
        + Proto(IpProto);
        _, "IPv4 protocol {}";
    }
);

impl IpProtocol for Ipv4Proto {}
impl<I: Ip + IpProtoExt> GenericOverIp<I> for Ipv4Proto {
    type Type = I::Proto;
}
impl Sealed for Ipv4Proto {}

create_protocol_enum!(
    /// An IPv6 protocol number.
    ///
    /// The protocol numbers are maintained [by IANA][protocol-numbers].
    ///
    /// [protocol-numbers]: https://www.iana.org/assignments/protocol-numbers/protocol-numbers.xhtml
    #[allow(missing_docs)]
    #[derive(Copy, Clone, Hash, Eq, Ord, PartialEq, PartialOrd)]
    pub enum Ipv6Proto: u8 {
        Icmpv6, 58, "ICMPv6";
        NoNextHeader, 59, "NO NEXT HEADER";
        + Proto(IpProto);
        _, "IPv6 protocol {}";
    }
);

impl IpProtocol for Ipv6Proto {}
impl<I: Ip + IpProtoExt> GenericOverIp<I> for Ipv6Proto {
    type Type = I::Proto;
}
impl Sealed for Ipv6Proto {}

create_protocol_enum!(
    /// An IPv6 extension header.
    ///
    /// These are next header values that encode for extension header types.
    /// This enum does not include upper layer protocol numbers even though they
    /// may be valid next header values.
    #[allow(missing_docs)]
    #[derive(Copy, Clone, Hash, Eq, PartialEq)]
    pub enum Ipv6ExtHdrType: u8 {
        HopByHopOptions, 0, "IPv6 HOP-BY-HOP OPTIONS HEADER";
        Routing, 43, "IPv6 ROUTING HEADER";
        Fragment, 44, "IPv6 FRAGMENT HEADER";
        EncapsulatingSecurityPayload, 50, "ENCAPSULATING SECURITY PAYLOAD";
        Authentication, 51, "AUTHENTICATION HEADER";
        DestinationOptions, 60, "IPv6 DESTINATION OPTIONS HEADER";
        _,  "IPv6 EXTENSION HEADER {}";
    }
);

/// An IP fragment offset.
///
/// Represents a fragment offset found in an IP header. The offset is expressed
/// in units of 8 octets and must be smaller than `1 << 13`.
///
/// This is valid for both IPv4 ([RFC 791 Section 3.1]) and IPv6 ([RFC 8200
/// Section 4.5]) headers.
///
/// [RFC 791 Section 3.1]: https://datatracker.ietf.org/doc/html/rfc791#section-3.1
/// [RFC 8200 Section 4.5]: https://datatracker.ietf.org/doc/html/rfc8200#section-4.5
#[derive(Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Copy, Clone)]
pub struct FragmentOffset(u16);

impl FragmentOffset {
    /// The zero fragment offset.
    pub const ZERO: FragmentOffset = FragmentOffset(0);

    /// Creates a new offset from a raw u16 value.
    ///
    /// Returns `None` if `offset` is not smaller than `1 << 13`.
    pub const fn new(offset: u16) -> Option<Self> {
        if offset < 1 << 13 { Some(Self(offset)) } else { None }
    }

    /// Creates a new offset from a raw u16 value masking to only the lowest 13
    /// bits.
    pub(crate) fn new_with_lsb(offset: u16) -> Self {
        Self(offset & 0x1FFF)
    }

    /// Creates a new offset from a raw u16 value masking to only the highest 13
    /// bits.
    pub(crate) fn new_with_msb(offset: u16) -> Self {
        Self(offset >> 3)
    }

    /// Creates a new offset from a raw bytes value.
    ///
    /// Returns `None` if `offset_bytes` is not a multiple of `8`.
    pub const fn new_with_bytes(offset_bytes: u16) -> Option<Self> {
        if offset_bytes & 0x7 == 0 {
            // NOTE: check for length above ensures this fits in a u16.
            Some(Self(offset_bytes >> 3))
        } else {
            None
        }
    }

    /// Consumes `self` returning the raw offset value in 8-octets multiples.
    pub const fn into_raw(self) -> u16 {
        self.0
    }

    /// Consumes `self` returning the total number of bytes represented by this
    /// offset.
    ///
    /// Equal to 8 times the raw offset value.
    pub fn into_bytes(self) -> u16 {
        // NB: Shift can't overflow because `FragmentOffset` is guaranteed to
        // fit in 13 bits.
        self.0 << 3
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragment_offset_raw() {
        assert_eq!(FragmentOffset::new(1), Some(FragmentOffset(1)));
        assert_eq!(FragmentOffset::new(1 << 13), None);
    }

    #[test]
    fn fragment_offset_bytes() {
        assert_eq!(FragmentOffset::new_with_bytes(0), Some(FragmentOffset(0)));
        for i in 1..=7 {
            assert_eq!(FragmentOffset::new_with_bytes(i), None);
        }
        assert_eq!(FragmentOffset::new_with_bytes(8), Some(FragmentOffset(1)));
        assert_eq!(FragmentOffset::new_with_bytes(u16::MAX), None);
        assert_eq!(
            FragmentOffset::new_with_bytes(u16::MAX & !0x7),
            Some(FragmentOffset((1 << 13) - 1)),
        );
    }
}
