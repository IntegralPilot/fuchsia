// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Parsing and serialization of UDP packets.
//!
//! The UDP packet format is defined in [RFC 768].
//!
//! [RFC 768]: https://datatracker.ietf.org/doc/html/rfc768

use core::fmt::Debug;
#[cfg(test)]
use core::fmt::{self, Formatter};
use core::num::NonZeroU16;
use core::ops::Range;

use net_types::ip::{Ip, IpAddress, IpVersionMarker};
use packet::{
    BufferView, BufferViewMut, ByteSliceInnerPacketBuilder, EmptyBuf, FragmentedBytesMut, FromRaw,
    InnerPacketBuilder, MaybeParsed, NestablePacketBuilder, NoOpParsingContext,
    NoOpSerializationContext, PacketBuilder, PacketConstraints, ParsablePacket, ParseMetadata,
    PartialPacketBuilder, SerializationContext, SerializeTarget, Serializer,
};
use zerocopy::byteorder::network_endian::U16;
use zerocopy::{
    FromBytes, Immutable, IntoBytes, KnownLayout, Ref, SplitByteSlice, SplitByteSliceMut, Unaligned,
};

use crate::error::{ParseError, ParseResult};
use crate::ip::IpProto;
use crate::{
    TransportChecksumAction, compute_transport_checksum_parts,
    compute_transport_checksum_serialize, compute_transport_pseudo_header_partial_checksum,
};

/// The size of a UDP header in bytes.
pub const HEADER_BYTES: usize = 8;

/// The offset of the checksum field, in bytes, from the start of a UDP header.
pub const CHECKSUM_OFFSET: usize = 6;

const CHECKSUM_RANGE: Range<usize> = CHECKSUM_OFFSET..CHECKSUM_OFFSET + 2;

#[derive(Debug, KnownLayout, FromBytes, IntoBytes, Immutable, Unaligned)]
#[repr(C)]
struct Header {
    src_port: U16,
    dst_port: U16,
    length: U16,
    checksum: [u8; 2],
}

impl Header {
    fn checksummed(&self) -> bool {
        self.checksum != U16::ZERO
    }

    pub fn set_src_port(&mut self, new: u16) {
        let old = self.src_port;
        let new = U16::from(new);
        if old == new {
            return; // Short-circuit to skip checksum work.
        }

        self.src_port = new;
        if self.checksummed() {
            self.checksum =
                internet_checksum::update(self.checksum, old.as_bytes(), new.as_bytes());
            sanitize_checksum(&mut self.checksum);
        }
    }

    pub fn set_dst_port(&mut self, new: NonZeroU16) {
        let old = self.dst_port;
        let new = U16::from(new.get());
        if old == new {
            return; // Short-circuit to skip checksum work.
        }

        self.dst_port = new;
        if self.checksummed() {
            self.checksum =
                internet_checksum::update(self.checksum, old.as_bytes(), new.as_bytes());
            sanitize_checksum(&mut self.checksum);
        }
    }

    pub fn update_checksum_pseudo_header_address<A: IpAddress>(&mut self, old: A, new: A) {
        if old == new {
            return; // Short-circuit to skip checksum work.
        }

        if self.checksummed() {
            self.checksum = internet_checksum::update(self.checksum, old.bytes(), new.bytes());
            sanitize_checksum(&mut self.checksum);
        }
    }
}

/// A UDP packet.
///
/// A `UdpPacket` shares its underlying memory with the byte slice it was parsed
/// from or serialized to, meaning that no copying or extra allocation is
/// necessary.
///
/// A `UdpPacket` - whether parsed using `parse` or created using `serialize` -
/// maintains the invariant that the checksum is always valid.
pub struct UdpPacket<B> {
    header: Ref<B, Header>,
    body: B,
}

/// Context for parsing UDP packets that may be subject to hardware checksum offloading.
pub trait UdpParseContext {
    /// `f` must verify the packet's checksum and return the result. It will be
    /// called if checksum verification is needed.
    fn verify_checksum_if_needed<E>(&mut self, f: impl FnOnce() -> Result<(), E>) -> Result<(), E>;
}

impl UdpParseContext for NoOpParsingContext {
    fn verify_checksum_if_needed<E>(&mut self, f: impl FnOnce() -> Result<(), E>) -> Result<(), E> {
        f()
    }
}

/// Arguments required to parse a UDP packet.
pub struct UdpParseArgs<A: IpAddress, C> {
    src_ip: A,
    dst_ip: A,
    context: C,
}

impl<A: IpAddress> UdpParseArgs<A, NoOpParsingContext> {
    /// Construct a new `UdpParseArgs`.
    pub fn new(src_ip: A, dst_ip: A) -> Self {
        UdpParseArgs { src_ip, dst_ip, context: NoOpParsingContext }
    }
}

impl<A: IpAddress, C> UdpParseArgs<A, C> {
    /// Construct a new `UdpParseArgs` with a parsing context.
    pub fn with_context(src_ip: A, dst_ip: A, context: C) -> Self {
        UdpParseArgs { src_ip, dst_ip, context }
    }
}

impl<B: SplitByteSlice, A: IpAddress, C: UdpParseContext>
    FromRaw<UdpPacketRaw<B>, UdpParseArgs<A, C>> for UdpPacket<B>
{
    type Error = ParseError;

    fn try_from_raw_with(
        raw: UdpPacketRaw<B>,
        UdpParseArgs { src_ip, dst_ip, mut context }: UdpParseArgs<A, C>,
    ) -> Result<Self, Self::Error> {
        // See for details: https://en.wikipedia.org/wiki/User_Datagram_Protocol#Packet_structure
        let header = raw
            .header
            .ok_or_else(|_| debug_err!(ParseError::Format, "too few bytes for header"))?;
        let body = raw.body.ok_or_else(|_| debug_err!(ParseError::Format, "incomplete body"))?;

        context.verify_checksum_if_needed(|| {
            let checksum = header.checksum;
            // A 0 checksum indicates that the checksum wasn't computed. In
            // IPv4, this means that it shouldn't be validated. In IPv6, the
            // checksum is mandatory, so this is an error.
            if checksum != [0, 0] {
                let parts = [Ref::bytes(&header), body.deref().as_ref()];
                let checksum = compute_transport_checksum_parts(
                    src_ip,
                    dst_ip,
                    IpProto::Udp.into(),
                    parts.iter(),
                )
                .ok_or_else(debug_err_fn!(ParseError::Format, "packet too large"))?;

                // Even the checksum is transmitted as 0xFFFF, the checksum of
                // the whole UDP packet should still be 0. This is because in
                // 1's complement, it is not possible to produce +0(0) from
                // adding non-zero 16-bit words. Since our 0xFFFF ensures there
                // is at least one non-zero 16-bit word, the addition can only
                // produce -0(0xFFFF) and after negation, it is still 0. A test
                // `test_udp_checksum_0xffff` is included to make sure this is
                // true.
                if checksum != [0, 0] {
                    return debug_err!(
                        Err(ParseError::Checksum),
                        "invalid checksum {:X?}",
                        header.checksum,
                    );
                }
            } else if A::Version::VERSION.is_v6() {
                return debug_err!(Err(ParseError::Format), "missing checksum");
            }

            Ok(())
        })?;

        if header.dst_port.get() == 0 {
            return debug_err!(Err(ParseError::Format), "zero destination port");
        }

        Ok(UdpPacket { header, body })
    }
}

impl<B: SplitByteSlice, A: IpAddress, C: UdpParseContext> ParsablePacket<B, UdpParseArgs<A, C>>
    for UdpPacket<B>
{
    type Error = ParseError;

    fn parse_metadata(&self) -> ParseMetadata {
        ParseMetadata::from_packet(Ref::bytes(&self.header).len(), self.body.len(), 0)
    }

    fn parse<BV: BufferView<B>>(buffer: BV, args: UdpParseArgs<A, C>) -> ParseResult<Self> {
        UdpPacketRaw::<B>::parse(buffer, IpVersionMarker::<A::Version>::default())
            .and_then(|u| UdpPacket::try_from_raw_with(u, args))
    }
}

impl<B: SplitByteSlice> UdpPacket<B> {
    /// The packet body.
    pub fn body(&self) -> &[u8] {
        self.body.deref()
    }

    /// Returns the contents of the packet as a pair of slices.
    pub fn as_bytes(&self) -> [&[u8]; 2] {
        [&Ref::bytes(&self.header), self.body.deref()]
    }

    /// Consumes this packet and returns the body.
    ///
    /// Note that the returned `B` has the same lifetime as the buffer from
    /// which this packet was parsed. By contrast, the [`body`] method returns a
    /// slice with the same lifetime as the receiver.
    ///
    /// [`body`]: UdpPacket::body
    pub fn into_body(self) -> B {
        self.body
    }

    /// The source UDP port, if any.
    ///
    /// The source port is optional, and may have been omitted by the sender.
    pub fn src_port(&self) -> Option<NonZeroU16> {
        NonZeroU16::new(self.header.src_port.get())
    }

    /// The destination UDP port.
    pub fn dst_port(&self) -> NonZeroU16 {
        // Infallible because it was validated in parse.
        NonZeroU16::new(self.header.dst_port.get()).unwrap()
    }

    /// Did this packet have a checksum?
    ///
    /// On IPv4, the sender may optionally omit the checksum. If this function
    /// returns false, the sender omitted the checksum, and `parse` will not
    /// have validated it.
    ///
    /// On IPv6, it is guaranteed that `checksummed` will return true because
    /// IPv6 requires a checksum, and so any UDP packet missing one will fail
    /// validation in `parse`.
    pub fn checksummed(&self) -> bool {
        self.header.checksummed()
    }

    /// Constructs a builder with the same contents as this packet.
    pub fn builder<A: IpAddress>(&self, src_ip: A, dst_ip: A) -> UdpPacketBuilder<A> {
        UdpPacketBuilder {
            src_ip,
            dst_ip,
            src_port: self.src_port(),
            dst_port: Some(self.dst_port()),
        }
    }

    /// Consumes this packet and constructs a [`Serializer`] with the same
    /// contents.
    ///
    /// The returned `Serializer` has the [`Buffer`] type [`EmptyBuf`], which
    /// means it is not able to reuse the buffer backing this `UdpPacket` when
    /// serializing, and will always need to allocate a new buffer.
    ///
    /// By consuming `self` instead of taking it by-reference, `into_serializer`
    /// is able to return a `Serializer` whose lifetime is restricted by the
    /// lifetime of the buffer from which this `UdpPacket` was parsed rather
    /// than by the lifetime on `&self`, which may be more restricted.
    ///
    /// [`Buffer`]: packet::Serializer::Buffer
    pub fn into_serializer<'a, A: IpAddress>(
        self,
        src_ip: A,
        dst_ip: A,
    ) -> impl Serializer<NoOpSerializationContext, Buffer = EmptyBuf> + Debug + 'a
    where
        B: 'a,
    {
        self.builder(src_ip, dst_ip)
            .wrap_body(ByteSliceInnerPacketBuilder(self.body).into_serializer())
    }
}

impl<B: SplitByteSliceMut> UdpPacket<B> {
    /// Set the source port of the UDP packet.
    pub fn set_src_port(&mut self, new: u16) {
        self.header.set_src_port(new)
    }

    /// Set the destination port of the UDP packet.
    pub fn set_dst_port(&mut self, new: NonZeroU16) {
        self.header.set_dst_port(new);
    }

    /// Update the checksum to reflect an updated address in the pseudo header.
    pub fn update_checksum_pseudo_header_address<A: IpAddress>(&mut self, old: A, new: A) {
        self.header.update_checksum_pseudo_header_address(old, new);
    }
}

impl<B: zerocopy::CloneableByteSlice + Clone> Clone for UdpPacket<B> {
    fn clone(&self) -> Self {
        UdpPacket { header: self.header.clone(), body: self.body.clone() }
    }
}

/// The minimal information required from a UDP packet header.
///
/// A `UdpPacketHeader` may be the result of a partially parsed UDP packet in
/// [`UdpPacketRaw`].
#[derive(Debug, Default, KnownLayout, FromBytes, IntoBytes, Immutable, Unaligned, PartialEq)]
#[repr(C)]
struct UdpFlowHeader {
    src_port: U16,
    dst_port: U16,
}

/// A partially parsed UDP packet header.
#[derive(Debug)]
struct PartialHeader<B: SplitByteSlice> {
    flow: Ref<B, UdpFlowHeader>,
    rest: B,
}

/// A partially-parsed and not yet validated UDP packet.
///
/// A `UdpPacketRaw` shares its underlying memory with the byte slice it was
/// parsed from or serialized to, meaning that no copying or extra allocation is
/// necessary.
///
/// Parsing a `UdpPacketRaw` from raw data will succeed as long as at least 4
/// bytes are available, which will be extracted as a [`UdpFlowHeader`] that
/// contains the UDP source and destination ports. A `UdpPacketRaw` is, then,
/// guaranteed to always have at least that minimal information available.
///
/// [`UdpPacket`] provides a [`FromRaw`] implementation that can be used to
/// validate a `UdpPacketRaw`.
pub struct UdpPacketRaw<B: SplitByteSlice> {
    header: MaybeParsed<Ref<B, Header>, PartialHeader<B>>,
    body: MaybeParsed<B, B>,
}

impl<B, I> ParsablePacket<B, IpVersionMarker<I>> for UdpPacketRaw<B>
where
    B: SplitByteSlice,
    I: Ip,
{
    type Error = ParseError;

    fn parse_metadata(&self) -> ParseMetadata {
        let header_len = match &self.header {
            MaybeParsed::Complete(h) => Ref::bytes(&h).len(),
            MaybeParsed::Incomplete(h) => Ref::bytes(&h.flow).len() + h.rest.len(),
        };
        ParseMetadata::from_packet(header_len, self.body.len(), 0)
    }

    fn parse<BV: BufferView<B>>(mut buffer: BV, _args: IpVersionMarker<I>) -> ParseResult<Self> {
        // See for details: https://en.wikipedia.org/wiki/User_Datagram_Protocol#Packet_structure

        let header = if let Some(header) = buffer.take_obj_front::<Header>() {
            header
        } else {
            let flow = buffer
                .take_obj_front::<UdpFlowHeader>()
                .ok_or_else(debug_err_fn!(ParseError::Format, "too few bytes for flow header"))?;
            // if we can't parse an entire header, just return early since
            // there's no way to look into how many body bytes to consume:
            return Ok(UdpPacketRaw {
                header: MaybeParsed::Incomplete(PartialHeader {
                    flow,
                    rest: buffer.take_rest_front(),
                }),
                body: MaybeParsed::Incomplete(buffer.into_rest()),
            });
        };
        let buffer_len = buffer.len();

        fn get_udp_body_length<I: Ip>(header: &Header, remaining_buff_len: usize) -> Option<usize> {
            // IPv6 supports jumbograms, so a UDP packet may be greater than
            // 2^16 bytes in size. In this case, the size doesn't fit in the
            // 16-bit length field in the header, and so the length field is set
            // to zero to indicate this.
            //
            // Per RFC 2675 Section 4, we only do that if the UDP header plus
            // data is actually more than 65535.
            if I::VERSION.is_v6()
                && header.length.get() == 0
                && remaining_buff_len.saturating_add(HEADER_BYTES) >= (u16::MAX as usize)
            {
                return Some(remaining_buff_len);
            }

            usize::from(header.length.get()).checked_sub(HEADER_BYTES)
        }

        let body = if let Some(body_len) = get_udp_body_length::<I>(&header, buffer_len) {
            if body_len <= buffer_len {
                // Discard any padding left by the previous layer. The unwrap is safe
                // and the subtraction is always valid because body_len is guaranteed
                // to not exceed buffer.len()
                let _: B = buffer.take_back(buffer_len - body_len).unwrap();
                MaybeParsed::Complete(buffer.into_rest())
            } else {
                // buffer does not contain all the body bytes
                MaybeParsed::Incomplete(buffer.into_rest())
            }
        } else {
            // body_len can't be calculated because it's less than the header
            // length, consider all the rest of the buffer padding and return
            // an incomplete empty body.
            let _: B = buffer.take_rest_back();
            MaybeParsed::Incomplete(buffer.into_rest())
        };

        Ok(UdpPacketRaw { header: MaybeParsed::Complete(header), body })
    }
}

impl<B: SplitByteSlice> UdpPacketRaw<B> {
    /// The source UDP port, if any.
    ///
    /// The source port is optional, and may have been omitted by the sender.
    pub fn src_port(&self) -> Option<NonZeroU16> {
        NonZeroU16::new(
            self.header
                .as_ref()
                .map(|header| header.src_port)
                .map_incomplete(|partial_header| partial_header.flow.src_port)
                .into_inner()
                .get(),
        )
    }

    /// The destination UDP port.
    ///
    /// UDP packets must not have a destination port of 0; thus, if this
    /// function returns `None`, then the packet is malformed.
    pub fn dst_port(&self) -> Option<NonZeroU16> {
        NonZeroU16::new(
            self.header
                .as_ref()
                .map(|header| header.dst_port)
                .map_incomplete(|partial_header| partial_header.flow.dst_port)
                .into_inner()
                .get(),
        )
    }

    /// Constructs a builder with the same contents as this packet.
    ///
    /// Note that, since `UdpPacketRaw` does not validate its header fields,
    /// it's possible for `builder` to produce a `UdpPacketBuilder` which
    /// describes an invalid UDP packet. In particular, it's possible that its
    /// destination port will be zero, which is illegal.
    pub fn builder<A: IpAddress>(&self, src_ip: A, dst_ip: A) -> UdpPacketBuilder<A> {
        UdpPacketBuilder { src_ip, dst_ip, src_port: self.src_port(), dst_port: self.dst_port() }
    }

    /// Consumes this packet and constructs a [`Serializer`] with the same
    /// contents.
    ///
    /// Returns `None` if the body was not fully parsed.
    ///
    /// This method has the same validity caveats as [`builder`].
    ///
    /// The returned `Serializer` has the [`Buffer`] type [`EmptyBuf`], which
    /// means it is not able to reuse the buffer backing this `UdpPacket` when
    /// serializing, and will always need to allocate a new buffer.
    ///
    /// By consuming `self` instead of taking it by-reference, `into_serializer`
    /// is able to return a `Serializer` whose lifetime is restricted by the
    /// lifetime of the buffer from which this `UdpPacket` was parsed rather
    /// than by the lifetime on `&self`, which may be more restricted.
    ///
    /// [`builder`]: UdpPacketRaw::builder
    /// [`Buffer`]: packet::Serializer::Buffer
    pub fn into_serializer<'a, A: IpAddress>(
        self,
        src_ip: A,
        dst_ip: A,
    ) -> Option<impl Serializer<NoOpSerializationContext, Buffer = EmptyBuf> + 'a>
    where
        B: 'a,
    {
        let builder = self.builder(src_ip, dst_ip);
        self.body
            .complete()
            .ok()
            .map(|body| builder.wrap_body(ByteSliceInnerPacketBuilder(body).into_serializer()))
    }
}

impl<B: SplitByteSliceMut> UdpPacketRaw<B> {
    /// Set the source port of the UDP packet.
    pub fn set_src_port(&mut self, new: u16) {
        match &mut self.header {
            MaybeParsed::Complete(h) => h.set_src_port(new),
            MaybeParsed::Incomplete(h) => {
                h.flow.src_port = U16::from(new);

                // We don't have the checksum, so there's nothing to update.
            }
        }
    }

    /// Set the destination port of the UDP packet.
    pub fn set_dst_port(&mut self, new: NonZeroU16) {
        match &mut self.header {
            MaybeParsed::Complete(h) => h.set_dst_port(new),
            MaybeParsed::Incomplete(h) => {
                h.flow.dst_port = U16::from(new.get());

                // We don't have the checksum, so there's nothing to update.
            }
        }
    }

    /// Update the checksum to reflect an updated address in the pseudo header.
    pub fn update_checksum_pseudo_header_address<A: IpAddress>(&mut self, old: A, new: A) {
        match &mut self.header {
            MaybeParsed::Complete(h) => h.update_checksum_pseudo_header_address(old, new),
            MaybeParsed::Incomplete(_) => {
                // We don't have the checksum, so there's nothing to update.
            }
        }
    }
}

// NOTE(joshlf): In order to ensure that the checksum is always valid, we don't
// expose any setters for the fields of the UDP packet; the only way to set them
// is via UdpPacketBuilder::serialize. This, combined with checksum validation
// performed in UdpPacket::parse, provides the invariant that a UdpPacket always
// has a valid checksum.

/// UDP packet context relevant to serialization.
pub struct UdpEnvelope;

/// A trait for UDP serialization contexts.
pub trait UdpSerializationContext: SerializationContext {
    /// Converts a `UdpEnvelope` into the serialization context's state.
    fn envelope_to_state(envelope: UdpEnvelope) -> Self::ContextState;

    /// Returns the checksum action to take based on the serialization context.
    fn checksum_action(&mut self) -> TransportChecksumAction;
}

impl UdpSerializationContext for NoOpSerializationContext {
    fn envelope_to_state(_envelope: UdpEnvelope) -> Self::ContextState {
        ()
    }

    fn checksum_action(&mut self) -> TransportChecksumAction {
        TransportChecksumAction::ComputeFull
    }
}

/// A builder for UDP packets.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct UdpPacketBuilder<A: IpAddress> {
    src_ip: A,
    dst_ip: A,
    src_port: Option<NonZeroU16>,
    dst_port: Option<NonZeroU16>,
}

impl<A: IpAddress> UdpPacketBuilder<A> {
    /// Constructs a new `UdpPacketBuilder`.
    pub fn new(
        src_ip: A,
        dst_ip: A,
        src_port: Option<NonZeroU16>,
        dst_port: NonZeroU16,
    ) -> UdpPacketBuilder<A> {
        UdpPacketBuilder { src_ip, dst_ip, src_port, dst_port: Some(dst_port) }
    }

    /// Returns the source port for the builder.
    pub fn src_port(&self) -> Option<NonZeroU16> {
        self.src_port
    }

    /// Returns the destination port for the builder.
    pub fn dst_port(&self) -> Option<NonZeroU16> {
        self.dst_port
    }

    /// Sets the source IP address for the builder.
    pub fn set_src_ip(&mut self, addr: A) {
        self.src_ip = addr;
    }

    /// Sets the destination IP address for the builder.
    pub fn set_dst_ip(&mut self, addr: A) {
        self.dst_ip = addr;
    }

    /// Sets the source port for the builder.
    pub fn set_src_port(&mut self, port: u16) {
        self.src_port = NonZeroU16::new(port);
    }

    /// Sets the destination port for the builder.
    pub fn set_dst_port(&mut self, port: NonZeroU16) {
        self.dst_port = Some(port);
    }

    fn serialize_header(&self, body_len: usize, mut buffer: &mut [u8]) {
        // See for details: https://en.wikipedia.org/wiki/User_Datagram_Protocol#Packet_structure

        let total_len = buffer.len() + body_len;

        // `write_obj_front` consumes the extent of the receiving slice, but
        // that behavior is undesirable here: at the end of this method, we
        // write the checksum back into the header. To avoid this, we re-slice
        // header before calling `write_obj_front`; the re-slice will be
        // consumed, but `target.header` is unaffected.
        (&mut buffer)
            .write_obj_front(&Header {
                src_port: U16::new(self.src_port.map_or(0, NonZeroU16::get)),
                dst_port: U16::new(self.dst_port.map_or(0, NonZeroU16::get)),
                length: U16::new(total_len.try_into().unwrap_or_else(|_| {
                    if A::Version::VERSION.is_v6() {
                        // See comment in `constraints()`.
                        0u16
                    } else {
                        panic!(
                            "total UDP packet length of {total_len} bytes \
                            overflows 16-bit length field of UDP header"
                        )
                    }
                })),
                // Initialize the checksum to 0 so that we will get the correct
                // value when we compute it below.
                checksum: [0, 0],
            })
            .expect("too few bytes for UDP header");
    }
}

impl<A: IpAddress> NestablePacketBuilder for UdpPacketBuilder<A> {
    fn constraints(&self) -> PacketConstraints {
        PacketConstraints::new(
            HEADER_BYTES,
            0,
            0,
            if A::Version::VERSION.is_v4() {
                (1 << 16) - 1
            } else {
                // IPv6 supports jumbograms, so a UDP packet may be greater than
                // 2^16 bytes. In this case, the size doesn't fit in the 16-bit
                // length field in the header, and so the length field is set to
                // zero. That means that, from this packet's perspective,
                // there's no effective limit on the body size.
                usize::MAX
            },
        )
    }
}

impl<A: IpAddress, C: UdpSerializationContext> PacketBuilder<C> for UdpPacketBuilder<A> {
    fn context_state(&self) -> C::ContextState {
        C::envelope_to_state(UdpEnvelope)
    }

    fn serialize(
        &self,
        context: &mut C,
        target: &mut SerializeTarget<'_>,
        body: FragmentedBytesMut<'_, '_>,
    ) {
        self.serialize_header(body.len(), target.header);

        let checksum = match context.checksum_action() {
            TransportChecksumAction::ComputeFull => compute_transport_checksum_serialize(
                self.src_ip,
                self.dst_ip,
                IpProto::Udp.into(),
                target,
                body,
            )
            .map(|mut c| {
                sanitize_checksum(&mut c);
                c
            }),
            TransportChecksumAction::ComputePartial => {
                compute_transport_pseudo_header_partial_checksum(
                    self.src_ip,
                    self.dst_ip,
                    IpProto::Udp.into(),
                    target,
                    body,
                )
            }
        }
        .unwrap(); // Not expected to fail since we were able to serialize the packet.

        target.header[CHECKSUM_RANGE].copy_from_slice(&checksum[..]);
    }
}

impl<A: IpAddress, C: UdpSerializationContext> PartialPacketBuilder<C> for UdpPacketBuilder<A> {
    fn partial_serialize(&self, _context: &mut C, body_len: usize, buffer: &mut [u8]) {
        self.serialize_header(body_len, buffer);
    }
}

#[inline]
fn sanitize_checksum(checksum_bytes: &mut [u8; 2]) {
    // As Per RFC 768:
    //   If the computed checksum is zero, it is transmitted as all ones
    //   (the equivalent in one's complement arithmetic).
    if *checksum_bytes == [0, 0] {
        *checksum_bytes = [0xFF, 0xFF];
    }
}

// needed by Result::unwrap_err in the tests below
#[cfg(test)]
impl<B> Debug for UdpPacket<B> {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> fmt::Result {
        write!(fmt, "UdpPacket")
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use byteorder::{ByteOrder, NetworkEndian};
    use net_types::ip::{Ipv4, Ipv4Addr, Ipv6, Ipv6Addr};
    use packet::{Buf, NestableSerializer as _, ParseBuffer, ParseBufferMut};
    use test_case::test_case;

    use super::*;
    use crate::add_transport_pseudo_header_checksum;
    use crate::ethernet::{EthernetFrame, EthernetFrameLengthCheck};
    use crate::ipv4::{Ipv4Header, Ipv4Packet};
    use crate::ipv6::{Ipv6Header, Ipv6Packet};
    use crate::testutil::*;
    use packet::NoOpSerializationContext;

    const TEST_SRC_IPV4: Ipv4Addr = Ipv4Addr::new([1, 2, 3, 4]);
    const TEST_DST_IPV4: Ipv4Addr = Ipv4Addr::new([5, 6, 7, 8]);
    const TEST_SRC_IPV6: Ipv6Addr =
        Ipv6Addr::from_bytes([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
    const TEST_DST_IPV6: Ipv6Addr =
        Ipv6Addr::from_bytes([17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32]);

    #[test]
    fn test_parse_serialize_full_ipv4() {
        use crate::testdata::dns_request_v4::*;

        let mut buf = ETHERNET_FRAME.bytes;
        let frame = buf.parse_with::<_, EthernetFrame<_>>(EthernetFrameLengthCheck::Check).unwrap();
        verify_ethernet_frame(&frame, ETHERNET_FRAME);

        let mut body = frame.body();
        let ip_packet = body.parse::<Ipv4Packet<_>>().unwrap();
        verify_ipv4_packet(&ip_packet, IPV4_PACKET);

        let mut body = ip_packet.body();
        let udp_packet = body
            .parse_with::<_, UdpPacket<_>>(UdpParseArgs::new(
                ip_packet.src_ip(),
                ip_packet.dst_ip(),
            ))
            .unwrap();
        verify_udp_packet(&udp_packet, UDP_PACKET);

        let buffer = udp_packet
            .body()
            .into_serializer()
            .wrap_in(udp_packet.builder(ip_packet.src_ip(), ip_packet.dst_ip()))
            .wrap_in(ip_packet.builder())
            .wrap_in(frame.builder())
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap();
        assert_eq!(buffer.as_ref(), ETHERNET_FRAME.bytes);
    }

    #[test]
    fn test_parse_serialize_full_ipv6() {
        use crate::testdata::dns_request_v6::*;

        let mut buf = ETHERNET_FRAME.bytes;
        let frame = buf.parse_with::<_, EthernetFrame<_>>(EthernetFrameLengthCheck::Check).unwrap();
        verify_ethernet_frame(&frame, ETHERNET_FRAME);

        let mut body = frame.body();
        let ip_packet = body.parse::<Ipv6Packet<_>>().unwrap();
        verify_ipv6_packet(&ip_packet, IPV6_PACKET);

        let mut body = ip_packet.body();
        let udp_packet = body
            .parse_with::<_, UdpPacket<_>>(UdpParseArgs::new(
                ip_packet.src_ip(),
                ip_packet.dst_ip(),
            ))
            .unwrap();
        verify_udp_packet(&udp_packet, UDP_PACKET);

        let buffer = udp_packet
            .body()
            .into_serializer()
            .wrap_in(udp_packet.builder(ip_packet.src_ip(), ip_packet.dst_ip()))
            .wrap_in(ip_packet.builder())
            .wrap_in(frame.builder())
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap();
        assert_eq!(buffer.as_ref(), ETHERNET_FRAME.bytes);
    }

    #[test]
    fn test_parse() {
        // source port of 0 (meaning none) is allowed, as is a missing checksum
        let mut buf = &[0, 0, 1, 2, 0, 8, 0, 0][..];
        let packet = buf
            .parse_with::<_, UdpPacket<_>>(UdpParseArgs::new(TEST_SRC_IPV4, TEST_DST_IPV4))
            .unwrap();
        assert!(packet.src_port().is_none());
        assert_eq!(packet.dst_port().get(), NetworkEndian::read_u16(&[1, 2]));
        assert!(!packet.checksummed());
        assert!(packet.body().is_empty());

        // length of 0 is allowed in IPv6 if the body is long enough
        let mut buf = vec![0_u8, 0, 1, 2, 0, 0, 0xBF, 0x12];
        buf.extend((0..u16::MAX).into_iter().map(|p| p as u8));
        let bv = &mut &buf[..];
        let packet = bv
            .parse_with::<_, UdpPacket<_>>(UdpParseArgs::new(TEST_SRC_IPV6, TEST_DST_IPV6))
            .unwrap();
        assert!(packet.src_port().is_none());
        assert_eq!(packet.dst_port().get(), NetworkEndian::read_u16(&[1, 2]));
        assert!(packet.checksummed());
        assert_eq!(packet.body().len(), u16::MAX as usize);
    }

    fn new_test_udp_builder() -> UdpPacketBuilder<Ipv4Addr> {
        UdpPacketBuilder::new(
            TEST_SRC_IPV4,
            TEST_DST_IPV4,
            NonZeroU16::new(1),
            NonZeroU16::new(2).unwrap(),
        )
    }

    #[test]
    fn test_serialize() {
        let mut buf = new_test_udp_builder()
            .wrap_body(EmptyBuf)
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap();
        assert_eq!(buf.as_ref(), [0, 1, 0, 2, 0, 8, 239, 199]);
        let packet = buf
            .parse_with::<_, UdpPacket<_>>(UdpParseArgs::new(TEST_SRC_IPV4, TEST_DST_IPV4))
            .unwrap();
        // assert that when we parse those bytes, we get the values we set in
        // the builder
        assert_eq!(packet.src_port().unwrap().get(), 1);
        assert_eq!(packet.dst_port().get(), 2);
        assert!(packet.checksummed());
    }

    #[test]
    fn test_serialize_zeroes() {
        // Test that UdpPacket::serialize properly zeroes memory before serializing
        // the header.
        let mut buf_0 = [0; HEADER_BYTES];
        let _: Buf<&mut [u8]> = new_test_udp_builder()
            .wrap_body(Buf::new(&mut buf_0[..], HEADER_BYTES..))
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap()
            .unwrap_a();
        let mut buf_1 = [0xFF; HEADER_BYTES];
        let _: Buf<&mut [u8]> = new_test_udp_builder()
            .wrap_body(Buf::new(&mut buf_1[..], HEADER_BYTES..))
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap()
            .unwrap_a();
        assert_eq!(buf_0, buf_1);
    }

    #[test]
    fn test_parse_error() {
        // Test that while a given byte pattern optionally succeeds, zeroing out
        // certain bytes causes failure. `zero` is a list of byte indices to
        // zero out that should cause failure.
        fn test_zero<I: IpAddress>(
            src: I,
            dst: I,
            succeeds: bool,
            zero: &[usize],
            err: ParseError,
        ) {
            // Set checksum to zero so that, in IPV4, it will be ignored. In
            // IPv6, this /is/ the test.
            let mut buf = [1, 2, 3, 4, 0, 8, 0, 0];
            if succeeds {
                let mut buf = &buf[..];
                assert!(buf.parse_with::<_, UdpPacket<_>>(UdpParseArgs::new(src, dst)).is_ok());
            }
            for idx in zero {
                buf[*idx] = 0;
            }
            let mut buf = &buf[..];
            assert_eq!(
                buf.parse_with::<_, UdpPacket<_>>(UdpParseArgs::new(src, dst)).unwrap_err(),
                err
            );
        }

        // destination port of 0 is disallowed
        test_zero(TEST_SRC_IPV4, TEST_DST_IPV4, true, &[2, 3], ParseError::Format);
        // length of 0 is disallowed in IPv4
        test_zero(TEST_SRC_IPV4, TEST_DST_IPV4, true, &[4, 5], ParseError::Format);
        // missing checksum is disallowed in IPv6; this won't succeed ahead of
        // time because the checksum bytes are already zero
        test_zero(TEST_SRC_IPV6, TEST_DST_IPV6, false, &[], ParseError::Format);

        // 2^32 overflows on 32-bit platforms
        #[cfg(target_pointer_width = "64")]
        {
            // total length of 2^32 or greater is disallowed in IPv6
            let mut buf = vec![0u8; 1 << 32];
            (&mut buf[..HEADER_BYTES]).copy_from_slice(&[0, 0, 1, 2, 0, 0, 0xFF, 0xE4]);
            assert_eq!(
                (&buf[..])
                    .parse_with::<_, UdpPacket<_>>(UdpParseArgs::new(TEST_SRC_IPV6, TEST_DST_IPV6))
                    .unwrap_err(),
                ParseError::Format
            );
        }
    }

    #[test_case(TEST_SRC_IPV4, TEST_DST_IPV4, true; "ipv4 skip")]
    #[test_case(TEST_SRC_IPV4, TEST_DST_IPV4, false; "ipv4 validate")]
    #[test_case(TEST_SRC_IPV6, TEST_DST_IPV6, true; "ipv6 skip")]
    #[test_case(TEST_SRC_IPV6, TEST_DST_IPV6, false; "ipv6 validate")]
    fn test_parse_invalid_checksum<A: IpAddress>(src: A, dst: A, skip: bool) {
        let mut buf =
            UdpPacketBuilder::new(src, dst, NonZeroU16::new(1), NonZeroU16::new(2).unwrap())
                .wrap_body(EmptyBuf)
                .serialize_vec_outer(&mut NoOpSerializationContext)
                .unwrap()
                .as_ref()
                .to_vec();

        // Corrupt the checksum.
        buf[CHECKSUM_OFFSET] ^= 0xFF;
        buf[CHECKSUM_OFFSET + 1] ^= 0xFF;

        let mut bv = &buf[..];
        let res = bv.parse_with::<_, UdpPacket<_>>(UdpParseArgs::with_context(
            src,
            dst,
            ForceSkipChecksumValidation(skip),
        ));
        if skip {
            assert_matches!(res, Ok(_));
        } else {
            assert_matches!(res, Err(ParseError::Checksum));
        }
    }

    #[test]
    #[should_panic(expected = "too few bytes for UDP header")]
    fn test_serialize_fail_header_too_short() {
        let mut buf = [0u8; 7];
        let mut buf = [&mut buf[..]];
        let buf = FragmentedBytesMut::new(&mut buf[..]);
        let (header, body, footer) = buf.try_split_contiguous(..).unwrap();
        let builder =
            UdpPacketBuilder::new(TEST_SRC_IPV4, TEST_DST_IPV4, None, NonZeroU16::new(1).unwrap());
        builder.serialize(
            &mut NoOpSerializationContext,
            &mut SerializeTarget { header, footer },
            body,
        );
    }

    #[test]
    #[should_panic(expected = "total UDP packet length of 65536 bytes overflows 16-bit length \
                               field of UDP header")]
    fn test_serialize_fail_packet_too_long_ipv4() {
        let ser =
            UdpPacketBuilder::new(TEST_SRC_IPV4, TEST_DST_IPV4, None, NonZeroU16::new(1).unwrap())
                .wrap_body((&[0; (1 << 16) - HEADER_BYTES][..]).into_serializer());
        let _ = ser.serialize_vec_outer(&mut NoOpSerializationContext);
    }

    #[test]
    fn test_partial_parse() {
        use core::ops::Deref as _;

        // Try to get something with only the flow header:
        let buf = [0, 0, 1, 2, 10, 20];
        let mut bv = &buf[..];
        let packet =
            bv.parse_with::<_, UdpPacketRaw<_>>(IpVersionMarker::<Ipv4>::default()).unwrap();
        let UdpPacketRaw { header, body } = &packet;
        let PartialHeader { flow, rest } = header.as_ref().incomplete().unwrap();
        assert_eq!(
            flow.deref(),
            &UdpFlowHeader { src_port: U16::new(0), dst_port: U16::new(0x0102) }
        );
        assert_eq!(*rest, &buf[4..]);
        assert_eq!(body.incomplete().unwrap(), []);
        assert!(
            UdpPacket::try_from_raw_with(packet, UdpParseArgs::new(TEST_SRC_IPV4, TEST_DST_IPV4))
                .is_err()
        );

        // check that we fail if flow header is not retrievable:
        let mut buf = &[0, 0, 1][..];
        assert!(buf.parse_with::<_, UdpPacketRaw<_>>(IpVersionMarker::<Ipv4>::default()).is_err());

        // Get an incomplete body:
        let buf = [0, 0, 1, 2, 0, 30, 0, 0, 10, 20];
        let mut bv = &buf[..];
        let packet =
            bv.parse_with::<_, UdpPacketRaw<_>>(IpVersionMarker::<Ipv4>::default()).unwrap();
        let UdpPacketRaw { header, body } = &packet;
        assert_eq!(Ref::bytes(&header.as_ref().complete().unwrap()), &buf[..8]);
        assert_eq!(body.incomplete().unwrap(), &buf[8..]);
        assert!(
            UdpPacket::try_from_raw_with(packet, UdpParseArgs::new(TEST_SRC_IPV4, TEST_DST_IPV4))
                .is_err()
        );

        // Incomplete empty body if total length in header is less than 8:
        let buf = [0, 0, 1, 2, 0, 6, 0, 0, 10, 20];
        let mut bv = &buf[..];
        let packet =
            bv.parse_with::<_, UdpPacketRaw<_>>(IpVersionMarker::<Ipv4>::default()).unwrap();
        let UdpPacketRaw { header, body } = &packet;
        assert_eq!(Ref::bytes(&header.as_ref().complete().unwrap()), &buf[..8]);
        assert_eq!(body.incomplete().unwrap(), []);
        assert!(
            UdpPacket::try_from_raw_with(packet, UdpParseArgs::new(TEST_SRC_IPV4, TEST_DST_IPV4))
                .is_err()
        );

        // IPv6 allows zero-length body, which will just be the rest of the
        // buffer, but only as long as it has more than 65535 bytes, otherwise
        // it'll just be interpreted as an invalid length:
        let buf = [0, 0, 1, 2, 0, 0, 0, 0, 10, 20];
        let mut bv = &buf[..];
        let packet =
            bv.parse_with::<_, UdpPacketRaw<_>>(IpVersionMarker::<Ipv6>::default()).unwrap();
        let UdpPacketRaw { header, body } = &packet;
        assert_eq!(Ref::bytes(&header.as_ref().complete().unwrap()), &buf[..8]);
        assert_eq!(body.incomplete().unwrap(), []);
        // Now try same thing but with a body that's actually big enough to
        // justify len being 0.
        let mut buf = vec![0, 0, 1, 2, 0, 0, 0, 0, 10, 20];
        buf.extend((0..u16::MAX).into_iter().map(|x| x as u8));
        let bv = &mut &buf[..];
        let packet =
            bv.parse_with::<_, UdpPacketRaw<_>>(IpVersionMarker::<Ipv6>::default()).unwrap();
        let UdpPacketRaw { header, body } = &packet;
        assert_eq!(Ref::bytes(header.as_ref().complete().unwrap()), &buf[..8]);
        assert_eq!(body.complete().unwrap(), &buf[8..]);
    }

    #[test]
    fn test_serialization_checksum_actions() {
        let body = [0x12, 0x34];
        let serializer = new_test_udp_builder().wrap_body(body.into_serializer());

        // Create checksum over pseudo-header.
        let mut c = internet_checksum::Checksum::new();
        add_transport_pseudo_header_checksum::<Ipv4>(
            &mut c,
            TEST_SRC_IPV4,
            TEST_DST_IPV4,
            IpProto::Udp.into(),
            HEADER_BYTES + body.len(),
        )
        .expect("failed to update checksum");

        // ComputePartial should produce the uncomplemented pseudo-header checksum.
        let buf = serializer
            .serialize_vec_outer(&mut ForceChecksumAction(TransportChecksumAction::ComputePartial))
            .unwrap();
        let [c0, c1] = c.checksum();
        assert_eq!(&buf.as_ref()[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 2], [!c0, !c1]);

        // ComputeFull should produce a checksum that verifies.
        let buf = serializer
            .serialize_vec_outer(&mut ForceChecksumAction(TransportChecksumAction::ComputeFull))
            .unwrap();

        c.add_bytes(buf.as_ref());
        assert_eq!(c.checksum(), [0, 0]);
    }

    #[test]
    fn test_udp_checksum_0xffff() {
        // Test the behavior when a UDP packet has to flip its checksum field.
        let serializer = UdpPacketBuilder::new(
            Ipv4Addr::new([0, 0, 0, 0]),
            Ipv4Addr::new([0, 0, 0, 0]),
            None,
            NonZeroU16::new(1).unwrap(),
        )
        .wrap_body((&[0xFF, 0xD9]).into_serializer());
        let buf = serializer.serialize_vec_outer(&mut NoOpSerializationContext).unwrap();
        // The serializer has flipped the bits for us.
        // Normally, 0xFFFF can't be checksum because -0
        // can not be produced by adding non-negtive 16-bit
        // words
        assert_eq!(&buf.as_ref()[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 2], [0xFF, 0xFF]);

        // When validating the checksum, just add'em up.
        let mut c = internet_checksum::Checksum::new();
        c.add_bytes(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 17, 0, 10]);
        c.add_bytes(buf.as_ref());
        assert!(c.checksum() == [0, 0]);
    }

    #[test]
    fn test_udp_checksum_partial_update_0xffff() {
        const DST_PORT: NonZeroU16 = NonZeroU16::new(1).unwrap();
        const ADDR: Ipv4Addr = Ipv4::UNSPECIFIED_ADDRESS;
        let serializer = UdpPacketBuilder::new(ADDR, ADDR, None, DST_PORT)
            .wrap_body((&[0xff, 0xd9]).into_serializer());
        let mut buf = serializer.serialize_vec_outer(&mut NoOpSerializationContext).unwrap();
        let mut packet = buf
            .parse_with_mut::<_, UdpPacket<_>>(UdpParseArgs::new(ADDR, ADDR))
            .expect("parse should succeed");
        assert_eq!(packet.header.checksum, [0xFF, 0xFF]);

        // Verify 0x0000 is set to 0xFFFF when updating the source port.
        packet.set_src_port(0); // No-Op.
        assert_eq!(packet.header.checksum, [0xFF, 0xFF]);
        packet.set_src_port(1234);
        assert_ne!(packet.header.checksum, [0xFF, 0xFF]);
        packet.set_src_port(0); // Real Change.
        assert_eq!(packet.header.checksum, [0xFF, 0xFF]);

        // Verify 0x0000 is set to 0xFFFF when updating the destination port.
        packet.set_dst_port(DST_PORT); // No-Op.
        assert_eq!(packet.header.checksum, [0xFF, 0xFF]);
        packet.set_dst_port(NonZeroU16::new(1234).unwrap());
        assert_ne!(packet.header.checksum, [0xFF, 0xFF]);
        packet.set_dst_port(DST_PORT); // Real Change.
        assert_eq!(packet.header.checksum, [0xFF, 0xFF]);

        // Verify 0x0000 is set to 0xFFFF when updating the pseudo header addr.
        packet.update_checksum_pseudo_header_address(ADDR, ADDR); // No-Op.
        assert_eq!(packet.header.checksum, [0xFF, 0xFF]);
        const OTHER_ADDR: Ipv4Addr = Ipv4Addr::new([123, 124, 125, 126]);
        packet.update_checksum_pseudo_header_address(ADDR, OTHER_ADDR);
        assert_ne!(packet.header.checksum, [0xFF, 0xFF]);
        packet.update_checksum_pseudo_header_address(OTHER_ADDR, ADDR); // Real Change.
        assert_eq!(packet.header.checksum, [0xFF, 0xFF]);
    }
}
