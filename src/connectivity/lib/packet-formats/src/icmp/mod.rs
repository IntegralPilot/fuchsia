// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Parsing and serialization of Internet Control Message Protocol (ICMP)
//! packets.
//!
//! This module supports both ICMPv4 and ICMPv6.
//!
//! The ICMPv4 packet format is defined in [RFC 792], and the ICMPv6
//! packet format is defined in [RFC 4443 Section 2.1].
//!
//! [RFC 792]: https://datatracker.ietf.org/doc/html/rfc792
//! [RFC 4443 Section 2.1]: https://datatracker.ietf.org/doc/html/rfc4443#section-2.1

#[macro_use]
mod macros;
mod common;
mod icmpv4;
mod icmpv6;
pub mod mld;
pub mod ndp;

#[cfg(test)]
mod testdata;

pub use self::common::*;
pub use self::icmpv4::*;
pub use self::icmpv6::*;

use core::fmt::Debug;
use core::marker::PhantomData;
use core::{cmp, mem};

use byteorder::{ByteOrder, NetworkEndian};
use derivative::Derivative;
use internet_checksum::Checksum;
use net_types::ip::{GenericOverIp, Ip, IpAddress, IpVersion, Ipv4, Ipv4Addr, Ipv6, Ipv6Addr};
use packet::records::options::{Options, OptionsImpl};
use packet::{
    AsFragmentedByteSlice, BufferView, FragmentedByteSlice, FragmentedBytesMut, FromRaw,
    NestablePacketBuilder, NoOpSerializationContext, PacketBuilder, PacketConstraints,
    ParsablePacket, ParseMetadata, PartialPacketBuilder, SerializationContext, SerializeTarget,
};
use zerocopy::byteorder::network_endian::U16;
use zerocopy::{
    FromBytes, Immutable, IntoBytes, KnownLayout, Ref, SplitByteSlice, SplitByteSliceMut, Unaligned,
};

use crate::error::{NotZeroError, ParseError, ParseResult};
use crate::ip::{IpProtoExt, Ipv4Proto, Ipv6Proto};
use crate::ipv4::{self, Ipv4PacketRaw};
use crate::ipv6::Ipv6PacketRaw;

#[derive(Copy, Clone, Default, Debug, KnownLayout, FromBytes, IntoBytes, Immutable, Unaligned)]
#[repr(C)]
struct HeaderPrefix {
    msg_type: u8,
    code: u8,
    checksum: [u8; 2],
    /* NOTE: The "Rest of Header" field is stored in message types rather than
     * in the HeaderPrefix. This helps consolidate how callers access data about the
     * packet, and is consistent with ICMPv6, which treats the field as part of
     * messages rather than the header. */
}

impl HeaderPrefix {
    fn set_msg_type<T: Into<u8>>(&mut self, msg_type: T) {
        self.msg_type = msg_type.into();
    }
}

/// Peek at an ICMP header to see what message type is present.
///
/// Since `IcmpPacket` is statically typed with the message type expected, this
/// type must be known ahead of time before calling `parse`. If multiple
/// different types are valid in a given parsing context, and so the caller
/// cannot know ahead of time which type to use, `peek_message_type` can be used
/// to peek at the header first to figure out which static type should be used
/// in a subsequent call to `parse`.
///
/// Note that `peek_message_type` only inspects certain fields in the header,
/// and so `peek_message_type` succeeding does not guarantee that a subsequent
/// call to `parse` will also succeed.
pub fn peek_message_type<MessageType: TryFrom<u8>>(bytes: &[u8]) -> ParseResult<MessageType> {
    let (hdr_pfx, _) = Ref::<_, HeaderPrefix>::from_prefix(bytes).map_err(Into::into).map_err(
        |_: zerocopy::SizeError<_, _>| debug_err!(ParseError::Format, "too few bytes for header"),
    )?;
    MessageType::try_from(hdr_pfx.msg_type).map_err(|_| {
        debug_err!(ParseError::NotSupported, "unrecognized message type: {:x}", hdr_pfx.msg_type,)
    })
}

/// An extension trait adding ICMP-related functionality to `Ipv4` and `Ipv6`.
pub trait IcmpIpExt: IpProtoExt {
    /// The ICMP packet type for this IP version.
    type IcmpPacketTypeRaw<B: SplitByteSliceMut>: IcmpPacketTypeRaw<B, Self>
        + GenericOverIp<Self, Type = Self::IcmpPacketTypeRaw<B>>
        + GenericOverIp<Ipv4, Type = Icmpv4PacketRaw<B>>
        + GenericOverIp<Ipv6, Type = Icmpv6PacketRaw<B>>;

    /// The type of ICMP messages.
    ///
    /// For `Ipv4`, this is `Icmpv4MessageType`, and for `Ipv6`, this is
    /// `Icmpv6MessageType`.
    type IcmpMessageType: IcmpMessageType
        + GenericOverIp<Self, Type = Self::IcmpMessageType>
        + GenericOverIp<Ipv4, Type = Icmpv4MessageType>
        + GenericOverIp<Ipv6, Type = Icmpv6MessageType>;

    /// The type of an ICMP Destination Unreachable Code.
    ///
    /// For `Ipv4`, this is `Icmpv4DestUnreachableCode`, and for `Ipv6` this
    /// is `Icmpv6DestUnreachableCode`.
    type DestUnreachableCode: PartialEq + Send + Sync + Debug;

    /// The type of an ICMP parameter problem code.
    ///
    /// For `Ipv4`, this is `Icmpv4ParameterProblemCode`, and for `Ipv6` this
    /// is `Icmpv6ParameterProblemCode`.
    type ParameterProblemCode: PartialEq + Send + Sync + Debug;

    /// The type of an ICMP parameter problem pointer.
    ///
    /// For `Ipv4`, this is `u8`, and for `Ipv6` this is `u32`.
    type ParameterProblemPointer: PartialEq + Send + Sync + Debug;

    /// The identifier for this ICMP version.
    ///
    /// This value will be found in an IPv4 packet's Protocol field (for ICMPv4
    /// packets) or an IPv6 fixed header's or last extension header's Next
    /// Heeader field (for ICMPv6 packets).
    const ICMP_IP_PROTO: <Self as IpProtoExt>::Proto;

    /// Computes the length of the header of the packet prefix stored in
    /// `bytes`.
    ///
    /// Given the prefix of a packet stored in `bytes`, compute the length of
    /// the header of that packet, or `bytes.len()` if `bytes` does not contain
    /// the entire header. If the version is IPv6, the returned length should
    /// include all extension headers.
    fn header_len(bytes: &[u8]) -> usize;

    /// Icmp{v4,v6}MessageType::EchoReply.
    const ECHO_REPLY: Self::IcmpMessageType;
    /// Icmp{v4,v6}MessageType::EchoRequest.
    const ECHO_REQUEST: Self::IcmpMessageType;
}

impl IcmpIpExt for Ipv4 {
    type IcmpPacketTypeRaw<B: SplitByteSliceMut> = Icmpv4PacketRaw<B>;
    type IcmpMessageType = Icmpv4MessageType;
    type DestUnreachableCode = Icmpv4DestUnreachableCode;
    type ParameterProblemCode = Icmpv4ParameterProblemCode;
    type ParameterProblemPointer = u8;

    const ICMP_IP_PROTO: Ipv4Proto = Ipv4Proto::Icmp;

    fn header_len(bytes: &[u8]) -> usize {
        if bytes.len() < ipv4::IPV4_MIN_HDR_LEN {
            return bytes.len();
        }
        let (header_prefix, _) = Ref::<_, ipv4::HeaderPrefix>::from_prefix(bytes).unwrap();
        cmp::min(header_prefix.ihl() as usize * 4, bytes.len())
    }

    const ECHO_REPLY: Icmpv4MessageType = Icmpv4MessageType::EchoReply;
    const ECHO_REQUEST: Icmpv4MessageType = Icmpv4MessageType::EchoRequest;
}

impl IcmpIpExt for Ipv6 {
    type IcmpPacketTypeRaw<B: SplitByteSliceMut> = Icmpv6PacketRaw<B>;
    type IcmpMessageType = Icmpv6MessageType;
    type DestUnreachableCode = Icmpv6DestUnreachableCode;
    type ParameterProblemCode = Icmpv6ParameterProblemCode;
    type ParameterProblemPointer = u32;

    const ICMP_IP_PROTO: Ipv6Proto = Ipv6Proto::Icmpv6;

    // TODO: Re-implement this in terms of partial parsing, and then get rid of
    // the `header_len` method.
    fn header_len(_bytes: &[u8]) -> usize {
        // NOTE: We panic here rather than doing log_unimplemented! because
        // there's no sane default value for this function. If it's called, it
        // doesn't make sense for the program to continue executing; if we did,
        // it would cause bugs in the caller.
        unimplemented!()
    }

    const ECHO_REPLY: Icmpv6MessageType = Icmpv6MessageType::EchoReply;
    const ECHO_REQUEST: Icmpv6MessageType = Icmpv6MessageType::EchoRequest;
}

/// An ICMP or ICMPv6 packet
///
/// 'IcmpPacketType' is implemented by `Icmpv4Packet` and `Icmpv6Packet`
pub trait IcmpPacketTypeRaw<B: SplitByteSliceMut, I: Ip>:
    Sized + ParsablePacket<B, (), Error = ParseError>
{
    /// Update the checksum to reflect an updated address in the pseudo header.
    fn update_checksum_pseudo_header_address(&mut self, old: I::Addr, new: I::Addr);

    /// Update the checksum to reflect a field change in the header.
    ///
    /// It is the caller's responsibility to ensure the field is actually part
    /// of an ICMP header for checksumming.
    fn update_checksum_header_field<F: IntoBytes + Immutable>(&mut self, old: F, new: F);

    /// Like [`IcmpPacketTypeRaw::update_checksum_header_field`], but takes
    /// native endian u16s.
    fn update_checksum_header_field_u16(&mut self, old: u16, new: u16) {
        self.update_checksum_header_field(U16::new(old), U16::new(new))
    }

    /// Recalculates and attempts to write a checksum for this packet.
    ///
    /// Returns whether the checksum was successfully calculated and written. In
    /// the false case, self is left unmodified.
    fn try_write_checksum(&mut self, src_addr: I::Addr, dst_addr: I::Addr) -> bool;

    /// Returns a mutable reference to the body of this packet.
    fn message_body_mut(&mut self) -> &mut B;
}

impl<B: SplitByteSliceMut> IcmpPacketTypeRaw<B, Ipv4> for Icmpv4PacketRaw<B> {
    fn update_checksum_pseudo_header_address(&mut self, old: Ipv4Addr, new: Ipv4Addr) {
        crate::icmpv4_dispatch!(self: raw, p => p.update_checksum_pseudo_header_address(old, new))
    }

    fn update_checksum_header_field<F: IntoBytes + Immutable>(&mut self, old: F, new: F) {
        crate::icmpv4_dispatch!(self: raw, p => p.update_checksum_header_field(old, new))
    }

    fn try_write_checksum(&mut self, src_addr: Ipv4Addr, dst_addr: Ipv4Addr) -> bool {
        crate::icmpv4_dispatch!(self: raw, p => p.try_write_checksum(src_addr, dst_addr))
    }

    fn message_body_mut(&mut self) -> &mut B {
        crate::icmpv4_dispatch!(self: raw, p => p.message_body_mut())
    }
}

impl<I: IcmpIpExt, B: SplitByteSliceMut> GenericOverIp<I> for Icmpv4PacketRaw<B> {
    type Type = I::IcmpPacketTypeRaw<B>;
}

impl<B: SplitByteSliceMut> IcmpPacketTypeRaw<B, Ipv6> for Icmpv6PacketRaw<B> {
    fn update_checksum_pseudo_header_address(&mut self, old: Ipv6Addr, new: Ipv6Addr) {
        crate::icmpv6_dispatch!(self: raw, p => p.update_checksum_pseudo_header_address(old, new))
    }

    fn update_checksum_header_field<F: IntoBytes + Immutable>(&mut self, old: F, new: F) {
        crate::icmpv6_dispatch!(self: raw, p => p.update_checksum_header_field(old, new))
    }

    fn try_write_checksum(&mut self, src_addr: Ipv6Addr, dst_addr: Ipv6Addr) -> bool {
        crate::icmpv6_dispatch!(self: raw, p => p.try_write_checksum(src_addr, dst_addr))
    }

    fn message_body_mut(&mut self) -> &mut B {
        crate::icmpv6_dispatch!(self: raw, p => p.message_body_mut())
    }
}

impl<I: IcmpIpExt, B: SplitByteSliceMut, M: IcmpMessage<I>> IcmpPacketTypeRaw<B, I>
    for IcmpPacketRaw<I, B, M>
{
    fn update_checksum_pseudo_header_address(&mut self, old: I::Addr, new: I::Addr) {
        match I::VERSION {
            IpVersion::V4 => {
                // ICMPv4 does not have a pseudo header.
            }
            IpVersion::V6 => {
                let checksum = &mut self.header.prefix.checksum;
                *checksum = internet_checksum::update(*checksum, old.bytes(), new.bytes());
            }
        }
    }

    fn update_checksum_header_field<F: IntoBytes + Immutable>(&mut self, old: F, new: F) {
        let checksum = &mut self.header.prefix.checksum;
        *checksum = internet_checksum::update(*checksum, old.as_bytes(), new.as_bytes());
    }

    fn try_write_checksum(&mut self, src_addr: I::Addr, dst_addr: I::Addr) -> bool {
        self.try_write_checksum(src_addr, dst_addr)
    }

    fn message_body_mut(&mut self) -> &mut B {
        self.message_body_mut()
    }
}

impl<I: IcmpIpExt, B: SplitByteSliceMut> GenericOverIp<I> for Icmpv6PacketRaw<B> {
    type Type = I::IcmpPacketTypeRaw<B>;
}

/// Empty message.
#[derive(Derivative, Debug, Clone, Copy, PartialEq, Eq)]
#[derivative(Default(bound = ""))]
pub struct EmptyMessage<B>(core::marker::PhantomData<B>);

/// `MessageBody` represents the parsed body of the ICMP packet.
///
/// - For messages that expect no body, the `MessageBody` is of type `EmptyMessage`.
/// - For NDP messages, the `MessageBody` is of the type `ndp::Options`.
/// - For all other messages, the `MessageBody` will be of the type
///   `OriginalPacket`, which is a thin wrapper around `B`.
pub trait MessageBody: Sized {
    /// The underlying byteslice.
    type B: SplitByteSlice;

    /// Parse the MessageBody from the provided bytes.
    fn parse(bytes: Self::B) -> ParseResult<Self>;

    /// The length of the underlying buffer.
    fn len(&self) -> usize;

    /// Is the body empty?
    ///
    /// `b.is_empty()` is equivalent to `b.len() == 0`.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Return the underlying bytes.
    ///
    /// Not all ICMP messages have a fixed size, some messages like MLDv2 Query or MLDv2 Report
    /// ([RFC 3810 section 5.1] and [RFC 3810 section 5.2]) contain a fixed amount of information
    /// followed by a variable amount of records.
    /// The first value returned contains the fixed size part, while the second value contains the
    /// records for the messages that support them, more precisely, the second value is [None] if
    /// the message does not have a variable part, otherwise it will contain the serialized list of
    /// records.
    ///
    /// [RFC 3810 section 5.1]: https://datatracker.ietf.org/doc/html/rfc3810#section-5.1
    /// [RFC 3810 section 5.2]: https://datatracker.ietf.org/doc/html/rfc3810#section-5.2
    fn bytes(&self) -> (&[u8], Option<&[u8]>);
}

impl<B: SplitByteSlice> MessageBody for EmptyMessage<B> {
    type B = B;

    fn parse(bytes: B) -> ParseResult<Self> {
        if !bytes.is_empty() {
            return debug_err!(Err(ParseError::Format), "unexpected message body");
        }

        Ok(EmptyMessage::default())
    }

    fn len(&self) -> usize {
        0
    }

    fn bytes(&self) -> (&[u8], Option<&[u8]>) {
        (&[], None)
    }
}

/// A thin wrapper around B which implements `MessageBody`.
#[derive(Debug)]
pub struct OriginalPacket<B>(B);

impl<B: SplitByteSlice> OriginalPacket<B> {
    /// Returns the the body of the original packet.
    pub fn body<I: IcmpIpExt>(&self) -> &[u8] {
        // TODO(joshlf): Can these debug_asserts be triggered by external input?
        let header_len = I::header_len(&self.0);
        debug_assert!(header_len <= self.0.len());
        debug_assert!(I::VERSION.is_v6() || self.0.len() - header_len == 8);
        &self.0[header_len..]
    }
}

impl<B: SplitByteSlice> MessageBody for OriginalPacket<B> {
    type B = B;

    fn parse(bytes: B) -> ParseResult<OriginalPacket<B>> {
        Ok(OriginalPacket(bytes))
    }

    fn len(&self) -> usize {
        self.0.len()
    }

    fn bytes(&self) -> (&[u8], Option<&[u8]>) {
        (&self.0, None)
    }
}

impl<B: SplitByteSlice, O: OptionsImpl> MessageBody for Options<B, O> {
    type B = B;
    fn parse(bytes: B) -> ParseResult<Options<B, O>> {
        Self::parse(bytes).map_err(|_e| debug_err!(ParseError::Format, "unable to parse options"))
    }

    fn len(&self) -> usize {
        self.bytes().len()
    }

    fn bytes(&self) -> (&[u8], Option<&[u8]>) {
        (self.bytes(), None)
    }
}

/// An ICMP message.
pub trait IcmpMessage<I: IcmpIpExt>:
    Sized + Copy + FromBytes + IntoBytes + KnownLayout + Immutable + Unaligned
{
    /// Whether or not a message body is expected in an ICMP packet.
    const EXPECTS_BODY: bool = true;

    /// The type of codes used with this message.
    ///
    /// The ICMP header includes an 8-bit "code" field. For a given message
    /// type, different values of this field carry different meanings. Not all
    /// code values are used - some may be invalid. This type represents a
    /// parsed code. For example, for TODO, it is the TODO type.
    type Code: Into<u8> + Copy + Debug;

    /// The type of the body used with this message.
    type Body<B: SplitByteSlice>: MessageBody<B = B>;

    /// The type corresponding to this message type.
    ///
    /// The value of the "type" field in the ICMP header corresponding to
    /// messages of this type.
    const TYPE: I::IcmpMessageType;

    /// Parse a `Code` from an 8-bit number.
    ///
    /// Parse a `Code` from the 8-bit "code" field in the ICMP header. Not all
    /// values for this field are valid. If an invalid value is passed,
    /// `code_from_u8` returns `None`.
    fn code_from_u8(code: u8) -> Option<Self::Code>;
}

/// The type of an ICMP message.
///
/// `IcmpMessageType` is implemented by `Icmpv4MessageType` and
/// `Icmpv6MessageType`.
pub trait IcmpMessageType: TryFrom<u8> + Into<u8> + Copy + Debug {
    /// Is this an error message?
    ///
    /// For ICMP, this is true for the Destination Unreachable, Redirect, Source
    /// Quench, Time Exceeded, and Parameter Problem message types. For ICMPv6,
    /// this is true for the Destination Unreachable, Packet Too Big, Time
    /// Exceeded, and Parameter Problem message types.
    fn is_error_or_redirect(self) -> bool;
}

#[derive(Copy, Clone, Debug, KnownLayout, FromBytes, IntoBytes, Immutable, Unaligned)]
#[repr(C)]
struct Header<M> {
    prefix: HeaderPrefix,
    message: M,
}

/// A partially parsed and not yet validated ICMP packet.
///
/// An `IcmpPacketRaw` provides minimal parsing of an ICMP packet. Namely, it
/// only requires that the header and message (in ICMPv6, these are both
/// considered part of the header) are present, and that the header has the
/// expected message type. The body may be missing (or an unexpected body may be
/// present). Other than the message type, no header, message, or body field
/// values will be validated.
///
/// [`IcmpPacket`] provides a [`FromRaw`] implementation that can be used to
/// validate an [`IcmpPacketRaw`].
#[derive(Debug)]
pub struct IcmpPacketRaw<I: IcmpIpExt, B: SplitByteSlice, M: IcmpMessage<I>> {
    header: Ref<B, Header<M>>,
    message_body: B,
    _marker: PhantomData<I>,
}

impl<I: IcmpIpExt, B: SplitByteSlice, M: IcmpMessage<I>> IcmpPacketRaw<I, B, M> {
    /// Get the ICMP message.
    pub fn message(&self) -> &M {
        &self.header.message
    }

    /// Get the ICMP message body.
    pub fn message_body(&self) -> &B {
        &self.message_body
    }
}

impl<I: IcmpIpExt, B: SplitByteSliceMut, M: IcmpMessage<I>> IcmpPacketRaw<I, B, M> {
    /// Get the mutable ICMP message.
    pub fn message_mut(&mut self) -> &mut M {
        &mut self.header.message
    }

    /// Get the mutable message body of the ICMP message.
    pub fn message_body_mut(&mut self) -> &mut B {
        &mut self.message_body
    }

    /// Attempts to calculate and write a Checksum for this [`IcmpPacketRaw`].
    ///
    /// Returns whether the checksum was successfully calculated & written. In
    /// the false case, self is left unmodified.
    pub(crate) fn try_write_checksum(&mut self, src_ip: I::Addr, dst_ip: I::Addr) -> bool {
        // NB: Zero the checksum to avoid interference when computing it.
        let original_checksum = self.header.prefix.checksum;
        self.header.prefix.checksum = [0, 0];

        if let Some(checksum) = IcmpPacket::<I, B, M>::compute_checksum(
            &self.header,
            &self.message_body,
            src_ip,
            dst_ip,
        ) {
            self.header.prefix.checksum = checksum;
            true
        } else {
            self.header.prefix.checksum = original_checksum;
            false
        }
    }
}

/// An ICMP packet.
///
/// An `IcmpPacket` shares its underlying memory with the byte slice it was
/// parsed from, meaning that no copying or extra allocation is necessary.
#[derive(Debug)]
pub struct IcmpPacket<I: IcmpIpExt, B: SplitByteSlice, M: IcmpMessage<I>> {
    header: Ref<B, Header<M>>,
    message_body: M::Body<B>,
    _marker: PhantomData<I>,
}

/// Arguments required to parse an ICMP packet.
pub struct IcmpParseArgs<A: IpAddress> {
    src_ip: A,
    dst_ip: A,
}

impl<A: IpAddress> IcmpParseArgs<A> {
    /// Construct a new `IcmpParseArgs`.
    pub fn new<S: Into<A>, D: Into<A>>(src_ip: S, dst_ip: D) -> IcmpParseArgs<A> {
        IcmpParseArgs { src_ip: src_ip.into(), dst_ip: dst_ip.into() }
    }
}

impl<B: SplitByteSlice, I: IcmpIpExt, M: IcmpMessage<I>> ParsablePacket<B, ()>
    for IcmpPacketRaw<I, B, M>
{
    type Error = ParseError;

    fn parse_metadata(&self) -> ParseMetadata {
        ParseMetadata::from_packet(Ref::bytes(&self.header).len(), self.message_body.len(), 0)
    }

    fn parse<BV: BufferView<B>>(mut buffer: BV, _args: ()) -> ParseResult<Self> {
        let header = buffer.take_obj_front::<Header<M>>().ok_or(ParseError::Format)?;
        let message_body = buffer.into_rest();
        if header.prefix.msg_type != M::TYPE.into() {
            return Err(ParseError::NotExpected);
        }
        Ok(IcmpPacketRaw { header, message_body, _marker: PhantomData })
    }
}

impl<B: SplitByteSlice, I: IcmpIpExt, M: IcmpMessage<I>>
    FromRaw<IcmpPacketRaw<I, B, M>, IcmpParseArgs<I::Addr>> for IcmpPacket<I, B, M>
{
    type Error = ParseError;

    fn try_from_raw_with(
        raw: IcmpPacketRaw<I, B, M>,
        args: IcmpParseArgs<I::Addr>,
    ) -> ParseResult<Self> {
        let IcmpPacketRaw { header, message_body, _marker } = raw;
        if !M::EXPECTS_BODY && !message_body.is_empty() {
            return Err(ParseError::Format);
        }
        let _: M::Code = M::code_from_u8(header.prefix.code).ok_or(ParseError::Format)?;
        let checksum = Self::compute_checksum(&header, &message_body, args.src_ip, args.dst_ip)
            .ok_or(ParseError::Format)?;
        if checksum != [0, 0] {
            return Err(ParseError::Checksum);
        }
        let message_body = M::Body::parse(message_body)?;
        Ok(IcmpPacket { header, message_body, _marker })
    }
}

impl<B: SplitByteSlice, I: IcmpIpExt, M: IcmpMessage<I>> ParsablePacket<B, IcmpParseArgs<I::Addr>>
    for IcmpPacket<I, B, M>
{
    type Error = ParseError;

    fn parse_metadata(&self) -> ParseMetadata {
        ParseMetadata::from_packet(Ref::bytes(&self.header).len(), self.message_body.len(), 0)
    }

    fn parse<BV: BufferView<B>>(buffer: BV, args: IcmpParseArgs<I::Addr>) -> ParseResult<Self> {
        IcmpPacketRaw::parse(buffer, ()).and_then(|p| IcmpPacket::try_from_raw_with(p, args))
    }
}

impl<I: IcmpIpExt, B: SplitByteSlice, M: IcmpMessage<I>> IcmpPacket<I, B, M> {
    /// Get the ICMP message.
    pub fn message(&self) -> &M {
        &self.header.message
    }

    /// Get the ICMP body.
    pub fn body(&self) -> &M::Body<B> {
        &self.message_body
    }

    /// Get the ICMP message code.
    ///
    /// The code provides extra details about the message. Each message type has
    /// its own set of codes that are allowed.
    pub fn code(&self) -> M::Code {
        // infallible since it was validated in parse
        M::code_from_u8(self.header.prefix.code).unwrap()
    }

    /// Construct a builder with the same contents as this packet.
    pub fn builder(&self, src_ip: I::Addr, dst_ip: I::Addr) -> IcmpPacketBuilder<I, M> {
        IcmpPacketBuilder { src_ip, dst_ip, code: self.code(), msg: *self.message() }
    }
}

fn compute_checksum_fragmented<I: IcmpIpExt, BB: packet::Fragment, M: IcmpMessage<I>>(
    header: &Header<M>,
    message_body: &FragmentedByteSlice<'_, BB>,
    src_ip: I::Addr,
    dst_ip: I::Addr,
) -> Option<[u8; 2]> {
    let mut c = Checksum::new();
    if I::VERSION.is_v6() {
        c.add_bytes(src_ip.bytes());
        c.add_bytes(dst_ip.bytes());
        let icmpv6_len = mem::size_of::<Header<M>>() + message_body.len();
        let mut len_bytes = [0; 4];
        NetworkEndian::write_u32(&mut len_bytes, icmpv6_len.try_into().ok()?);
        c.add_bytes(&len_bytes[..]);
        c.add_bytes(&[0, 0, 0]);
        c.add_bytes(&[Ipv6Proto::Icmpv6.into()]);
    }
    c.add_bytes(&[header.prefix.msg_type, header.prefix.code]);
    c.add_bytes(&header.prefix.checksum);
    c.add_bytes(header.message.as_bytes());
    for p in message_body.iter_fragments() {
        c.add_bytes(p);
    }
    Some(c.checksum())
}

impl<I: IcmpIpExt, B: SplitByteSlice, M: IcmpMessage<I>> IcmpPacket<I, B, M> {
    /// Compute the checksum, including the checksum field itself.
    ///
    /// `compute_checksum` returns `None` if the version is IPv6 and the total
    /// ICMP packet length overflows a u32.
    fn compute_checksum(
        header: &Header<M>,
        message_body: &[u8],
        src_ip: I::Addr,
        dst_ip: I::Addr,
    ) -> Option<[u8; 2]> {
        let mut body = [message_body];
        compute_checksum_fragmented(header, &body.as_fragmented_byte_slice(), src_ip, dst_ip)
    }
}

impl<I: IcmpIpExt, B: SplitByteSlice, M: IcmpMessage<I, Body<B> = OriginalPacket<B>>>
    IcmpPacket<I, B, M>
{
    /// Get the body of the packet that caused this ICMP message.
    ///
    /// This ICMP message contains some of the bytes of the packet that caused
    /// this message to be emitted. `original_packet_body` returns as much of
    /// the body of that packet as is contained in this message. For IPv4, this
    /// is guaranteed to be 8 bytes. For IPv6, there are no guarantees about the
    /// length.
    pub fn original_packet_body(&self) -> &[u8] {
        self.message_body.body::<I>()
    }

    /// Returns the original packt that caused this ICMP message.
    ///
    /// This ICMP message contains some of the bytes of the packet that caused
    /// this message to be emitted. `original_packet` returns as much of the
    /// body of that packet as is contained in this message. For IPv4, this is
    /// guaranteed to be 8 bytes. For IPv6, there are no guarantees about the
    /// length.
    pub fn original_packet(&self) -> &OriginalPacket<B> {
        &self.message_body
    }
}

impl<B: SplitByteSlice, M: IcmpMessage<Ipv4, Body<B> = OriginalPacket<B>>> IcmpPacket<Ipv4, B, M> {
    /// Attempt to partially parse the original packet as an IPv4 packet.
    ///
    /// `f` will be invoked on the result of calling `Ipv4PacketRaw::parse` on
    /// the original packet.
    pub fn with_original_packet<O, F: FnOnce(Result<Ipv4PacketRaw<&[u8]>, &[u8]>) -> O>(
        &self,
        f: F,
    ) -> O {
        let mut bv = self.message_body.0.deref();
        f(Ipv4PacketRaw::parse(&mut bv, ()).map_err(|_| self.message_body.0.deref()))
    }
}

impl<B: SplitByteSlice, M: IcmpMessage<Ipv6, Body<B> = OriginalPacket<B>>> IcmpPacket<Ipv6, B, M> {
    /// Attempt to partially parse the original packet as an IPv6 packet.
    ///
    /// `f` will be invoked on the result of calling `Ipv6PacketRaw::parse` on
    /// the original packet.
    pub fn with_original_packet<O, F: FnOnce(Result<Ipv6PacketRaw<&[u8]>, &[u8]>) -> O>(
        &self,
        f: F,
    ) -> O {
        let mut bv = self.message_body.0.deref();
        f(Ipv6PacketRaw::parse(&mut bv, ()).map_err(|_| self.message_body.0.deref()))
    }
}

impl<I: IcmpIpExt, B: SplitByteSlice, M: IcmpMessage<I, Body<B> = ndp::Options<B>>>
    IcmpPacket<I, B, M>
{
    /// Get the pared list of NDP options from the ICMP message.
    pub fn ndp_options(&self) -> &ndp::Options<B> {
        &self.message_body
    }
}

/// ICMP packet context relevant to serialization.
pub struct IcmpEnvelope;

/// A trait for ICMP serialization contexts.
pub trait IcmpSerializationContext: SerializationContext {
    /// Converts an `IcmpEnvelope` into the serialization context's state.
    fn envelope_to_state(envelope: IcmpEnvelope) -> Self::ContextState;
}

impl IcmpSerializationContext for NoOpSerializationContext {
    fn envelope_to_state(_envelope: IcmpEnvelope) -> Self::ContextState {
        ()
    }
}

/// A builder for ICMP packets.
#[derive(Debug, PartialEq, Clone)]
pub struct IcmpPacketBuilder<I: IcmpIpExt, M: IcmpMessage<I>> {
    src_ip: I::Addr,
    dst_ip: I::Addr,
    code: M::Code,
    msg: M,
}

impl<I: IcmpIpExt, M: IcmpMessage<I>> IcmpPacketBuilder<I, M> {
    /// Construct a new `IcmpPacketBuilder`.
    pub fn new<S: Into<I::Addr>, D: Into<I::Addr>>(
        src_ip: S,
        dst_ip: D,
        code: M::Code,
        msg: M,
    ) -> IcmpPacketBuilder<I, M> {
        IcmpPacketBuilder { src_ip: src_ip.into(), dst_ip: dst_ip.into(), code, msg }
    }

    /// Returns the message in the ICMP packet.
    pub fn message(&self) -> &M {
        &self.msg
    }

    /// Returns a mutable reference to the message in the ICMP packet.
    pub fn message_mut(&mut self) -> &mut M {
        &mut self.msg
    }

    /// Sets the source IP address of the ICMP packet.
    pub fn set_src_ip(&mut self, addr: I::Addr) {
        self.src_ip = addr;
    }

    /// Sets the destination IP address of the ICMP packet.
    pub fn set_dst_ip(&mut self, addr: I::Addr) {
        self.dst_ip = addr;
    }

    fn serialize_header(
        &self,
        mut header: &mut [u8],
        message_body: Option<FragmentedBytesMut<'_, '_>>,
    ) {
        use packet::BufferViewMut;

        // Implements BufferViewMut, giving us take_obj_xxx_zero methods.
        let mut prefix = &mut header;

        // SECURITY: Use _zero constructors to ensure we zero memory to prevent
        // leaking information from packets previously stored in this buffer.
        let mut header =
            prefix.take_obj_front_zero::<Header<M>>().expect("too few bytes for ICMP message");
        header.prefix.set_msg_type(M::TYPE);
        header.prefix.code = self.code.into();
        header.message = self.msg;

        if let Some(message_body) = message_body {
            assert!(
                M::EXPECTS_BODY || message_body.is_empty(),
                "body provided for message that doesn't take a body"
            );
            let checksum = compute_checksum_fragmented(
                &header,
                &message_body,
                self.src_ip,
                self.dst_ip,
            )
            .unwrap_or_else(|| {
                panic!(
                    "total ICMP packet length of {} overflows 32-bit length field of pseudo-header",
                    Ref::bytes(&header).len() + message_body.len(),
                )
            });
            header.prefix.checksum = checksum;
        }
    }
}

// TODO(joshlf): Figure out a way to split body and non-body message types by
// trait and implement PacketBuilder for some and InnerPacketBuilder for others.

impl<I: IcmpIpExt, M: IcmpMessage<I>> NestablePacketBuilder for IcmpPacketBuilder<I, M> {
    fn constraints(&self) -> PacketConstraints {
        // The maximum body length constraint to make sure the body length
        // doesn't overflow the 32-bit length field in the pseudo-header used
        // for calculating the checksum.
        //
        // Note that, for messages that don't take bodies, it's important that
        // we don't just set this to 0. Trying to serialize a body in a message
        // type which doesn't take bodies is a programmer error, so we should
        // panic in that case. Setting the max_body_len to 0 would surface the
        // issue as an MTU error, which would hide the underlying problem.
        // Instead, we assert in serialize. Eventually, we will hopefully figure
        // out a way to implement InnerPacketBuilder (rather than PacketBuilder)
        // for these message types, and this won't be an issue anymore.
        PacketConstraints::new(mem::size_of::<Header<M>>(), 0, 0, u32::MAX as usize)
    }
}

impl<I: IcmpIpExt, M: IcmpMessage<I>, C: IcmpSerializationContext> PacketBuilder<C>
    for IcmpPacketBuilder<I, M>
{
    fn context_state(&self) -> C::ContextState {
        C::envelope_to_state(IcmpEnvelope)
    }

    fn serialize(
        &self,
        _context: &mut C,
        target: &mut SerializeTarget<'_>,
        message_body: FragmentedBytesMut<'_, '_>,
    ) {
        self.serialize_header(target.header, Some(message_body));
    }
}

impl<I: IcmpIpExt, M: IcmpMessage<I>, C: IcmpSerializationContext> PartialPacketBuilder<C>
    for IcmpPacketBuilder<I, M>
{
    fn partial_serialize(&self, _context: &mut C, _body_len: usize, buffer: &mut [u8]) {
        self.serialize_header(buffer, None);
    }
}

/// An ICMP code that must be zero.
///
/// Some ICMP messages do not use codes. In Rust, the `IcmpMessage::Code` type
/// associated with these messages is `IcmpZeroCode`. The only valid numerical
/// value for this code is 0.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct IcmpZeroCode;

impl From<IcmpZeroCode> for u8 {
    fn from(_: IcmpZeroCode) -> u8 {
        0
    }
}

impl TryFrom<u8> for IcmpZeroCode {
    type Error = NotZeroError<u8>;

    fn try_from(value: u8) -> Result<Self, NotZeroError<u8>> {
        if value == 0 { Ok(Self) } else { Err(NotZeroError(value)) }
    }
}

/// An ICMP code that is zero on serialization, but ignored on parsing.
///
/// This is used for ICMP messages whose specification states that senders must
/// set Code to 0 but receivers must ignore it (e.g. MLD/MLDv2).
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct IcmpSenderZeroCode;

impl From<IcmpSenderZeroCode> for u8 {
    fn from(_: IcmpSenderZeroCode) -> u8 {
        0
    }
}

impl From<u8> for IcmpSenderZeroCode {
    fn from(_: u8) -> Self {
        Self
    }
}

// TODO(https://github.com/google/zerocopy/issues/1292),
// TODO(https://github.com/rust-lang/rust/issues/45713): This needs to be public
// in order to work around a Rust compiler bug. Once that bug is resolved, this
// can be made private again.
#[doc(hidden)]
#[derive(
    Copy, Clone, Debug, Eq, PartialEq, KnownLayout, FromBytes, IntoBytes, Immutable, Unaligned,
)]
#[repr(C)]
pub struct IdAndSeq {
    id: U16,
    seq: U16,
}

impl IdAndSeq {
    fn new(id: u16, seq: u16) -> IdAndSeq {
        IdAndSeq { id: U16::new(id), seq: U16::new(seq) }
    }
}

#[cfg(test)]
mod tests {
    use ip_test_macro::ip_test;
    use packet::{EmptyBuf, NoOpSerializationContext, ParseBuffer, Serializer, SliceBufViewMut};
    use test_case::test_case;

    use super::*;

    #[test]
    fn test_partial_parse() {
        // Test various behaviors of parsing the `IcmpPacketRaw` type.

        let reference_header = Header {
            prefix: HeaderPrefix {
                msg_type: <IcmpEchoRequest as IcmpMessage<Ipv4>>::TYPE.into(),
                code: 0,
                checksum: [0, 0],
            },
            message: IcmpEchoRequest::new(1, 1),
        };

        // Test that a too-short header is always rejected even if its contents
        // are otherwise valid (the checksum here is probably invalid, but we
        // explicitly check that it's a `Format` error, not a `Checksum`
        // error).
        let mut buf = &reference_header.as_bytes()[..7];
        assert_eq!(
            buf.parse::<IcmpPacketRaw<Ipv4, _, IcmpEchoRequest>>().unwrap_err(),
            ParseError::Format
        );

        // Test that a properly-sized header is rejected if the message type is wrong.
        let mut header = reference_header;
        header.prefix.msg_type = <IcmpEchoReply as IcmpMessage<Ipv4>>::TYPE.into();
        let mut buf = header.as_bytes();
        assert_eq!(
            buf.parse::<IcmpPacketRaw<Ipv4, _, IcmpEchoRequest>>().unwrap_err(),
            ParseError::NotExpected
        );

        // Test that an invalid code is accepted.
        let mut header = reference_header;
        header.prefix.code = 0xFF;
        let mut buf = header.as_bytes();
        assert!(buf.parse::<IcmpPacketRaw<Ipv4, _, IcmpEchoRequest>>().is_ok());

        // Test that an invalid checksum is accepted. Instead of calculating the
        // correct checksum, we just provide two different checksums. They can't
        // both be valid.
        let mut buf = reference_header.as_bytes();
        assert!(buf.parse::<IcmpPacketRaw<Ipv4, _, IcmpEchoRequest>>().is_ok());
        let mut header = reference_header;
        header.prefix.checksum = [1, 1];
        let mut buf = header.as_bytes();
        assert!(buf.parse::<IcmpPacketRaw<Ipv4, _, IcmpEchoRequest>>().is_ok());
    }

    #[ip_test(I)]
    #[test_case([0,0]; "zeroed_checksum")]
    #[test_case([123, 234]; "garbage_checksum")]
    fn test_try_write_checksum<I: IcmpIpExt>(corrupt_checksum: [u8; 2]) {
        // NB: The process of serializing an `IcmpPacketBuilder` will compute a
        // valid checksum.
        let icmp_message_with_checksum = IcmpPacketBuilder::<I, _>::new(
            *I::LOOPBACK_ADDRESS,
            *I::LOOPBACK_ADDRESS,
            IcmpZeroCode,
            IcmpEchoRequest::new(1, 1),
        )
        .wrap_body(EmptyBuf)
        .serialize_vec_outer(&mut NoOpSerializationContext)
        .unwrap()
        .as_ref()
        .to_vec();

        // Clone the message and corrupt the checksum.
        let mut icmp_message_without_checksum = icmp_message_with_checksum.clone();
        {
            let buf = SliceBufViewMut::new(&mut icmp_message_without_checksum);
            let mut message = IcmpPacketRaw::<I, _, IcmpEchoRequest>::parse_mut(buf, ())
                .expect("parse packet raw should succeed");
            message.header.prefix.checksum = corrupt_checksum;
        }
        assert_ne!(&icmp_message_with_checksum[..], &icmp_message_without_checksum[..]);

        // Write the checksum, and verify the message now matches the original.
        let buf = SliceBufViewMut::new(&mut icmp_message_without_checksum);
        let mut message = IcmpPacketRaw::<I, _, IcmpEchoRequest>::parse_mut(buf, ())
            .expect("parse packet raw should succeed");
        assert!(message.try_write_checksum(*I::LOOPBACK_ADDRESS, *I::LOOPBACK_ADDRESS));
        assert_eq!(&icmp_message_with_checksum[..], &icmp_message_without_checksum[..]);
    }
}
