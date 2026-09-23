// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Parsing and serialization of TCP segments.
//!
//! The TCP segment format is defined in [RFC 791 Section 3.3].
//!
//! [RFC 793 Section 3.1]: https://datatracker.ietf.org/doc/html/rfc793#section-3.1

use core::convert::TryInto as _;
use core::fmt::Debug;
#[cfg(test)]
use core::fmt::{self, Formatter};
use core::num::NonZeroU16;
use core::ops::{Deref, Range};

use explicit::ResultExt as _;
use net_types::ip::IpAddress;
use packet::{
    BufferView, BufferViewMut, ByteSliceInnerPacketBuilder, EmptyBuf, FragmentedBytesMut, FromRaw,
    InnerPacketBuilder, MaybeParsed, NestablePacketBuilder, NoOpParsingContext,
    NoOpSerializationContext, PacketBuilder, PacketConstraints, ParsablePacket, ParseMetadata,
    PartialPacketBuilder, SerializationContext, SerializeTarget, Serializer, SplitByteSliceBufView,
};
use zerocopy::byteorder::network_endian::{U16, U32};
use zerocopy::{
    ByteSlice, CloneableByteSlice, FromBytes, Immutable, IntoBytes, KnownLayout, Ref,
    SplitByteSlice, SplitByteSliceMut, Unaligned,
};

use crate::error::{ParseError, ParseResult};
use crate::ip::IpProto;
use crate::{
    TransportChecksumAction, compute_transport_checksum_parts,
    compute_transport_checksum_serialize, compute_transport_pseudo_header_partial_checksum,
};

use self::data_offset_reserved_flags::DataOffsetReservedFlags;
use self::options::{TcpOptionsBuilder, TcpOptionsRaw, TcpOptionsRef};

/// The length of the fixed prefix of a TCP header (preceding the options).
pub const HDR_PREFIX_LEN: usize = 20;

/// The maximum length of a TCP header.
pub const MAX_HDR_LEN: usize = 60;

/// The maximum length of the options in a TCP header.
pub const MAX_OPTIONS_LEN: usize = MAX_HDR_LEN - HDR_PREFIX_LEN;

/// The offset of the checksum field, in bytes, from the start of a TCP header.
pub const CHECKSUM_OFFSET: usize = 16;

const CHECKSUM_RANGE: Range<usize> = CHECKSUM_OFFSET..CHECKSUM_OFFSET + 2;

#[derive(Debug, Default, KnownLayout, FromBytes, IntoBytes, Immutable, Unaligned, PartialEq)]
#[repr(C)]
struct HeaderPrefix {
    src_port: U16,
    dst_port: U16,
    seq_num: U32,
    ack: U32,
    data_offset_reserved_flags: DataOffsetReservedFlags,
    window_size: U16,
    checksum: [u8; 2],
    urg_ptr: U16,
}

impl HeaderPrefix {
    #[allow(clippy::too_many_arguments)]
    fn new(
        src_port: u16,
        dst_port: u16,
        seq_num: u32,
        ack: u32,
        data_offset_reserved_flags: DataOffsetReservedFlags,
        window_size: u16,
        checksum: [u8; 2],
        urg_ptr: u16,
    ) -> HeaderPrefix {
        HeaderPrefix {
            src_port: U16::new(src_port),
            dst_port: U16::new(dst_port),
            seq_num: U32::new(seq_num),
            ack: U32::new(ack),
            data_offset_reserved_flags,
            window_size: U16::new(window_size),
            checksum,
            urg_ptr: U16::new(urg_ptr),
        }
    }

    fn data_offset(&self) -> u8 {
        self.data_offset_reserved_flags.data_offset()
    }

    fn ack_num(&self) -> Option<u32> {
        if self.data_offset_reserved_flags.ack() { Some(self.ack.get()) } else { None }
    }

    fn builder<A: IpAddress>(&self, src_ip: A, dst_ip: A) -> TcpSegmentBuilder<A> {
        TcpSegmentBuilder {
            src_ip,
            dst_ip,
            // Might be zero, which is illegal.
            src_port: NonZeroU16::new(self.src_port.get()),
            // Might be zero, which is illegal.
            dst_port: NonZeroU16::new(self.dst_port.get()),
            // All values are valid.
            seq_num: self.seq_num.get(),
            // Might be nonzero even if the ACK flag is not set.
            ack_num: self.ack.get(),
            // Reserved zero bits may be set.
            data_offset_reserved_flags: self.data_offset_reserved_flags,
            // All values are valid.
            window_size: self.window_size.get(),
        }
    }

    pub fn set_src_port(&mut self, new: NonZeroU16) {
        let old = self.src_port;
        let new = U16::from(new.get());
        self.src_port = new;
        self.checksum = internet_checksum::update(self.checksum, old.as_bytes(), new.as_bytes());
    }

    pub fn set_dst_port(&mut self, new: NonZeroU16) {
        let old = self.dst_port;
        let new = U16::from(new.get());
        self.dst_port = new;
        self.checksum = internet_checksum::update(self.checksum, old.as_bytes(), new.as_bytes());
    }

    pub fn update_checksum_pseudo_header_address<A: IpAddress>(&mut self, old: A, new: A) {
        self.checksum = internet_checksum::update(self.checksum, old.bytes(), new.bytes());
    }
}

mod data_offset_reserved_flags {
    use super::*;

    /// The Data Offset field, the reserved zero bits, and the flags.
    ///
    /// When constructed from a packet, `DataOffsetReservedFlags` ensures that
    /// all bits are preserved even if they are reserved as of this writing.
    /// This allows us to be forwards-compatible with future uses of these bits.
    /// This forwards-compatibility doesn't matter when user code is only
    /// parsing a segment because we don't provide getters for any of those
    /// bits. However, it does matter when copying `DataOffsetReservedFlags`
    /// into new segments - in these cases, if we were to unconditionally set
    /// the reserved bits to zero, we could be changing the semantics of a TCP
    /// segment.
    #[derive(
        KnownLayout,
        FromBytes,
        IntoBytes,
        Immutable,
        Unaligned,
        Copy,
        Clone,
        Debug,
        Default,
        Eq,
        PartialEq,
    )]
    #[repr(transparent)]
    pub(super) struct DataOffsetReservedFlags(U16);

    impl DataOffsetReservedFlags {
        pub const EMPTY: DataOffsetReservedFlags = DataOffsetReservedFlags(U16::ZERO);
        pub const ACK_SET: DataOffsetReservedFlags =
            DataOffsetReservedFlags(U16::from_bytes(Self::ACK_FLAG_MASK.to_be_bytes()));

        const DATA_OFFSET_SHIFT: u8 = 12;
        const DATA_OFFSET_MAX: u8 = (1 << (16 - Self::DATA_OFFSET_SHIFT)) - 1;
        const DATA_OFFSET_MASK: u16 = (Self::DATA_OFFSET_MAX as u16) << Self::DATA_OFFSET_SHIFT;

        const ACK_FLAG_MASK: u16 = 0b10000;
        const PSH_FLAG_MASK: u16 = 0b01000;
        const RST_FLAG_MASK: u16 = 0b00100;
        const SYN_FLAG_MASK: u16 = 0b00010;
        const FIN_FLAG_MASK: u16 = 0b00001;

        #[cfg(test)]
        pub fn new(data_offset: u8) -> DataOffsetReservedFlags {
            let mut ret = Self::EMPTY;
            ret.set_data_offset(data_offset);
            ret
        }

        pub fn set_data_offset(&mut self, data_offset: u8) {
            debug_assert!(data_offset <= Self::DATA_OFFSET_MAX);
            let v = self.0.get();
            self.0.set(
                (v & !Self::DATA_OFFSET_MASK) | (u16::from(data_offset)) << Self::DATA_OFFSET_SHIFT,
            );
        }

        pub fn data_offset(&self) -> u8 {
            (self.0.get() >> 12) as u8
        }

        fn get_flag(&self, mask: u16) -> bool {
            self.0.get() & mask > 0
        }

        pub fn ack(&self) -> bool {
            self.get_flag(Self::ACK_FLAG_MASK)
        }

        pub fn psh(&self) -> bool {
            self.get_flag(Self::PSH_FLAG_MASK)
        }

        pub fn rst(&self) -> bool {
            self.get_flag(Self::RST_FLAG_MASK)
        }

        pub fn syn(&self) -> bool {
            self.get_flag(Self::SYN_FLAG_MASK)
        }

        pub fn fin(&self) -> bool {
            self.get_flag(Self::FIN_FLAG_MASK)
        }

        fn set_flag(&mut self, mask: u16, set: bool) {
            let v = self.0.get();
            self.0.set(if set { v | mask } else { v & !mask });
        }

        pub fn set_psh(&mut self, psh: bool) {
            self.set_flag(Self::PSH_FLAG_MASK, psh);
        }

        pub fn set_rst(&mut self, rst: bool) {
            self.set_flag(Self::RST_FLAG_MASK, rst)
        }

        pub fn set_syn(&mut self, syn: bool) {
            self.set_flag(Self::SYN_FLAG_MASK, syn)
        }

        pub fn set_fin(&mut self, fin: bool) {
            self.set_flag(Self::FIN_FLAG_MASK, fin)
        }
    }
}

/// A TCP segment.
///
/// A `TcpSegment` shares its underlying memory with the byte slice it was
/// parsed from or serialized to, meaning that no copying or extra allocation is
/// necessary.
///
/// A `TcpSegment` - whether parsed using `parse` or created using
/// `TcpSegmentBuilder` - maintains the invariant that the checksum is always
/// valid.
pub struct TcpSegment<B> {
    hdr_prefix: Ref<B, HeaderPrefix>,
    options: TcpOptionsRef<B>,
    body: B,
}

/// Context for parsing TCP segments that may be subject to hardware checksum offloading.
pub trait TcpParseContext {
    /// `f` must verify the segment's checksum and return the result. It will be
    /// called if checksum verification is needed.
    fn verify_checksum_if_needed<E>(&mut self, f: impl FnOnce() -> Result<(), E>) -> Result<(), E>;
}

impl TcpParseContext for NoOpParsingContext {
    fn verify_checksum_if_needed<E>(&mut self, f: impl FnOnce() -> Result<(), E>) -> Result<(), E> {
        f()
    }
}

/// Arguments required to parse a TCP segment.
pub struct TcpParseArgs<A: IpAddress, C> {
    src_ip: A,
    dst_ip: A,
    context: C,
}

impl<A: IpAddress> TcpParseArgs<A, NoOpParsingContext> {
    /// Construct a new `TcpParseArgs`.
    pub fn new(src_ip: A, dst_ip: A) -> Self {
        TcpParseArgs { src_ip, dst_ip, context: NoOpParsingContext }
    }
}

impl<A: IpAddress, C> TcpParseArgs<A, C> {
    /// Construct a new `TcpParseArgs` with a parsing context.
    pub fn with_context(src_ip: A, dst_ip: A, context: C) -> Self {
        TcpParseArgs { src_ip, dst_ip, context }
    }
}

/// When parsing, this type imposes a `B: CloneableByteSlice` bound. This is
/// so that the type can
///   1) retain the original `B` to return the option bytes exactly as they
///      were, and
///   2) have individual fields reference subsections of the `B` to avoid
///      needless copies.
/// This prevents parsing a `TcpSegment` from a `MutableByteSlice`, but we deem
/// that acceptable because it's not a known requirement.
impl<B: SplitByteSlice + CloneableByteSlice, A: IpAddress, C: TcpParseContext>
    ParsablePacket<B, TcpParseArgs<A, C>> for TcpSegment<B>
{
    type Error = ParseError;

    fn parse_metadata(&self) -> ParseMetadata {
        let header_len = Ref::bytes(&self.hdr_prefix).len() + self.options.len();
        ParseMetadata::from_packet(header_len, self.body.len(), 0)
    }

    fn parse<BV: BufferView<B>>(buffer: BV, args: TcpParseArgs<A, C>) -> ParseResult<Self> {
        TcpSegmentRaw::<B>::parse(buffer, ()).and_then(|u| TcpSegment::try_from_raw_with(u, args))
    }
}

impl<B: SplitByteSlice + CloneableByteSlice, A: IpAddress, C: TcpParseContext>
    FromRaw<TcpSegmentRaw<B>, TcpParseArgs<A, C>> for TcpSegment<B>
{
    type Error = ParseError;

    fn try_from_raw_with(
        raw: TcpSegmentRaw<B>,
        TcpParseArgs { src_ip, dst_ip, mut context }: TcpParseArgs<A, C>,
    ) -> Result<Self, Self::Error> {
        // See for details: https://en.wikipedia.org/wiki/Transmission_Control_Protocol#TCP_segment_structure

        let hdr_prefix = raw
            .hdr_prefix
            .ok_or_else(|_| debug_err!(ParseError::Format, "too few bytes for header"))?;
        let options = raw
            .options
            .ok_or_else(|_| debug_err!(ParseError::Format, "Incomplete options"))
            .and_then(|o| {
                TcpOptionsRef::try_from_raw(o)
                    .map_err(|(_parsed, e)| debug_err!(e, "Options validation failed"))
            })?;
        let body = raw.body;

        let hdr_bytes = (hdr_prefix.data_offset() * 4) as usize;
        if hdr_bytes != Ref::bytes(&hdr_prefix).len() + options.len() {
            return debug_err!(
                Err(ParseError::Format),
                "invalid data offset: {} for header={} + options={}",
                hdr_prefix.data_offset(),
                Ref::bytes(&hdr_prefix).len(),
                options.bytes().len()
            );
        }

        context.verify_checksum_if_needed(|| {
            let parts = [Ref::bytes(&hdr_prefix), options.bytes(), body.deref().as_ref()];
            let checksum =
                compute_transport_checksum_parts(src_ip, dst_ip, IpProto::Tcp.into(), parts.iter())
                    .ok_or_else(debug_err_fn!(ParseError::Format, "segment too large"))?;

            if checksum != [0, 0] {
                return debug_err!(Err(ParseError::Checksum), "invalid checksum");
            }

            Ok(())
        })?;

        if hdr_prefix.src_port == U16::ZERO || hdr_prefix.dst_port == U16::ZERO {
            return debug_err!(Err(ParseError::Format), "zero source or destination port");
        }

        Ok(TcpSegment { hdr_prefix, options, body })
    }
}

impl<B: SplitByteSlice> TcpSegment<B> {
    /// Returns the segment's options.
    pub fn options(&self) -> &TcpOptionsRef<B> {
        &self.options
    }

    /// The segment body.
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    /// Consumes this packet and returns the body.
    ///
    /// Note that the returned `B` has the same lifetime as the buffer from
    /// which this segment was parsed. By contrast, the [`body`] method returns
    /// a slice with the same lifetime as the receiver.
    ///
    /// [`body`]: TcpSegment::body
    pub fn into_body(self) -> B {
        self.body
    }

    /// The source port.
    pub fn src_port(&self) -> NonZeroU16 {
        // Infallible because this was already validated in parse
        NonZeroU16::new(self.hdr_prefix.src_port.get()).unwrap()
    }

    /// The destination port.
    pub fn dst_port(&self) -> NonZeroU16 {
        // Infallible because this was already validated in parse
        NonZeroU16::new(self.hdr_prefix.dst_port.get()).unwrap()
    }

    /// The sequence number.
    pub fn seq_num(&self) -> u32 {
        self.hdr_prefix.seq_num.get()
    }

    /// The acknowledgement number.
    ///
    /// If the ACK flag is not set, `ack_num` returns `None`.
    pub fn ack_num(&self) -> Option<u32> {
        self.hdr_prefix.ack_num()
    }

    /// The PSH flag.
    pub fn psh(&self) -> bool {
        self.hdr_prefix.data_offset_reserved_flags.psh()
    }

    /// The RST flag.
    pub fn rst(&self) -> bool {
        self.hdr_prefix.data_offset_reserved_flags.rst()
    }

    /// The SYN flag.
    pub fn syn(&self) -> bool {
        self.hdr_prefix.data_offset_reserved_flags.syn()
    }

    /// The FIN flag.
    pub fn fin(&self) -> bool {
        self.hdr_prefix.data_offset_reserved_flags.fin()
    }

    /// The sender's window size.
    pub fn window_size(&self) -> u16 {
        self.hdr_prefix.window_size.get()
    }

    /// The length of the header prefix and options.
    pub fn header_len(&self) -> usize {
        Ref::bytes(&self.hdr_prefix).len() + self.options.len()
    }

    // The length of the segment as calculated from the header prefix, options,
    // and body.
    // TODO(rheacock): remove `allow(dead_code)` when this is used.
    #[allow(dead_code)]
    fn total_segment_len(&self) -> usize {
        self.header_len() + self.body.len()
    }

    /// Constructs a builder with the same contents as this packet.
    pub fn builder<A: IpAddress>(
        &self,
        src_ip: A,
        dst_ip: A,
    ) -> TcpSegmentBuilderWithOptions<A, &TcpOptionsRef<B>> {
        TcpSegmentBuilderWithOptions {
            prefix_builder: self.hdr_prefix.deref().builder(src_ip, dst_ip),
            options: &self.options,
        }
    }

    /// Returns packet headers and the body as a list of slices.
    pub fn as_bytes(&self) -> [&[u8]; 3] {
        [self.hdr_prefix.as_bytes(), self.options.bytes(), &self.body]
    }

    /// Consumes this segment and constructs a [`Serializer`] with the same
    /// contents.
    ///
    /// The returned `Serializer` has the [`Buffer`] type [`EmptyBuf`], which
    /// means it is not able to reuse the buffer backing this `TcpSegment` when
    /// serializing, and will always need to allocate a new buffer.
    ///
    /// By consuming `self` instead of taking it by-reference, `into_serializer`
    /// is able to return a `Serializer` whose lifetime is restricted by the
    /// lifetime of the buffer from which this `TcpSegment` was parsed rather
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
        let Self { hdr_prefix, options, body } = self;
        let prefix_builder = hdr_prefix.deref().builder(src_ip, dst_ip);
        TcpSegmentBuilderWithOptions { prefix_builder, options }
            .wrap_body(ByteSliceInnerPacketBuilder(body).into_serializer())
    }
}

impl<B: SplitByteSliceMut> TcpSegment<B> {
    /// Set the source port of the TCP packet.
    pub fn set_src_port(&mut self, new: NonZeroU16) {
        self.hdr_prefix.set_src_port(new)
    }

    /// Set the destination port of the TCP packet.
    pub fn set_dst_port(&mut self, new: NonZeroU16) {
        self.hdr_prefix.set_dst_port(new)
    }

    /// Update the checksum to reflect an updated address in the pseudo header.
    pub fn update_checksum_pseudo_header_address<A: IpAddress>(&mut self, old: A, new: A) {
        self.hdr_prefix.update_checksum_pseudo_header_address(old, new)
    }
}

/// The minimal information required from a TCP segment header.
///
/// A `TcpFlowHeader` may be the result of a partially parsed TCP segment in
/// [`TcpSegmentRaw`].
#[derive(
    Debug, Default, KnownLayout, FromBytes, IntoBytes, Immutable, Unaligned, PartialEq, Copy, Clone,
)]
#[repr(C)]
pub struct TcpFlowHeader {
    /// Source port.
    src_port: U16,
    /// Destination port.
    dst_port: U16,
}

impl TcpFlowHeader {
    /// Gets the (src, dst) port tuple.
    pub fn src_dst(&self) -> (u16, u16) {
        (self.src_port.get(), self.dst_port.get())
    }
}

#[derive(Debug)]
struct PartialHeaderPrefix<B: SplitByteSlice> {
    flow: Ref<B, TcpFlowHeader>,
    rest: B,
}

/// Contains the TCP flow info and its sequence number.
///
/// This is useful for TCP endpoints processing ingress ICMP messages so that it
/// can deliver the ICMP message to the right socket and also perform checks
/// against the sequence number to make sure it corresponds to an in-flight
/// segment.
#[derive(Debug, Default, KnownLayout, FromBytes, IntoBytes, Immutable, Unaligned, PartialEq)]
#[repr(C)]
pub struct TcpFlowAndSeqNum {
    /// The flow header.
    flow: TcpFlowHeader,
    /// The sequence number.
    seqnum: U32,
}

impl TcpFlowAndSeqNum {
    /// Gets the source port.
    pub fn src_port(&self) -> u16 {
        self.flow.src_port.get()
    }

    /// Gets the destination port.
    pub fn dst_port(&self) -> u16 {
        self.flow.dst_port.get()
    }

    /// Gets the sequence number.
    pub fn sequence_num(&self) -> u32 {
        self.seqnum.get()
    }
}

/// A partially-parsed and not yet validated TCP segment.
///
/// A `TcpSegmentRaw` shares its underlying memory with the byte slice it was
/// parsed from or serialized to, meaning that no copying or extra allocation is
/// necessary.
///
/// Parsing a `TcpSegmentRaw` from raw data will succeed as long as at least 4
/// bytes are available, which will be extracted as a [`TcpFlowHeader`] that
/// contains the TCP source and destination ports. A `TcpSegmentRaw` is, then,
/// guaranteed to always have at least that minimal information available.
///
/// [`TcpSegment`] provides a [`FromRaw`] implementation that can be used to
/// validate a `TcpSegmentRaw`.
pub struct TcpSegmentRaw<B: SplitByteSlice> {
    hdr_prefix: MaybeParsed<Ref<B, HeaderPrefix>, PartialHeaderPrefix<B>>,
    options: MaybeParsed<TcpOptionsRaw<B>, B>,
    body: B,
}

impl<B: SplitByteSliceMut> TcpSegmentRaw<B> {
    /// Set the source port of the TCP packet.
    pub fn set_src_port(&mut self, new: NonZeroU16) {
        match &mut self.hdr_prefix {
            MaybeParsed::Complete(h) => h.set_src_port(new),
            MaybeParsed::Incomplete(h) => {
                h.flow.src_port = U16::from(new.get());

                // We don't have the checksum, so there's nothing to update.
            }
        }
    }

    /// Set the destination port of the TCP packet.
    pub fn set_dst_port(&mut self, new: NonZeroU16) {
        match &mut self.hdr_prefix {
            MaybeParsed::Complete(h) => h.set_dst_port(new),
            MaybeParsed::Incomplete(h) => {
                h.flow.dst_port = U16::from(new.get());

                // We don't have the checksum, so there's nothing to update.
            }
        }
    }

    /// Update the checksum to reflect an updated address in the pseudo header.
    pub fn update_checksum_pseudo_header_address<A: IpAddress>(&mut self, old: A, new: A) {
        match &mut self.hdr_prefix {
            MaybeParsed::Complete(h) => {
                h.update_checksum_pseudo_header_address(old, new);
            }
            MaybeParsed::Incomplete(_) => {
                // We don't have the checksum, so there's nothing to update.
            }
        }
    }
}

impl<B> ParsablePacket<B, ()> for TcpSegmentRaw<B>
where
    B: SplitByteSlice,
{
    type Error = ParseError;

    fn parse_metadata(&self) -> ParseMetadata {
        let header_len = self.options.len()
            + match &self.hdr_prefix {
                MaybeParsed::Complete(h) => Ref::bytes(&h).len(),
                MaybeParsed::Incomplete(h) => Ref::bytes(&h.flow).len() + h.rest.len(),
            };
        ParseMetadata::from_packet(header_len, self.body.len(), 0)
    }

    fn parse<BV: BufferView<B>>(mut buffer: BV, _args: ()) -> ParseResult<Self> {
        // See for details: https://en.wikipedia.org/wiki/Transmission_Control_Protocol#TCP_segment_structure

        let (hdr_prefix, options) = if let Some(pfx) = buffer.take_obj_front::<HeaderPrefix>() {
            // If the subtraction data_offset*4 - HDR_PREFIX_LEN would have been
            // negative, that would imply that data_offset has an invalid value.
            // Even though this will end up being MaybeParsed::Complete, the
            // data_offset value is validated when transforming TcpSegmentRaw to
            // TcpSegment.
            //
            // `options_bytes` upholds the invariant of being no more than
            // `MAX_OPTIONS_LEN` (40) bytes long because the Data Offset field
            // is a 4-bit field with a maximum value of 15. Thus, the maximum
            // value of `pfx.data_offset() * 4` is 15 * 4 = 60, so subtracting
            // `HDR_PREFIX_LEN` (20) leads to a maximum possible value of 40.
            let options_bytes = usize::from(pfx.data_offset() * 4).saturating_sub(HDR_PREFIX_LEN);
            debug_assert!(options_bytes <= MAX_OPTIONS_LEN, "options_bytes: {}", options_bytes);
            let options =
                MaybeParsed::take_from_buffer_with(&mut buffer, options_bytes, TcpOptionsRaw::new);
            let hdr_prefix = MaybeParsed::Complete(pfx);
            (hdr_prefix, options)
        } else {
            let flow = buffer
                .take_obj_front::<TcpFlowHeader>()
                .ok_or_else(debug_err_fn!(ParseError::Format, "too few bytes for flow header"))?;
            let rest = buffer.take_rest_front();
            // if we can't take the entire header, the rest of options will be
            // incomplete:
            let hdr_prefix = MaybeParsed::Incomplete(PartialHeaderPrefix { flow, rest });
            let options = MaybeParsed::Incomplete(buffer.take_rest_front());
            (hdr_prefix, options)
        };

        // A TCP segment's body is always just the rest of the buffer:
        let body = buffer.into_rest();

        Ok(Self { hdr_prefix, options, body })
    }
}

impl<B: SplitByteSlice> TcpSegmentRaw<B> {
    /// Gets the flow header from this packet.
    pub fn flow_header(&self) -> TcpFlowHeader {
        match &self.hdr_prefix {
            MaybeParsed::Complete(c) => {
                let HeaderPrefix { src_port, dst_port, .. } = &**c;
                TcpFlowHeader { src_port: *src_port, dst_port: *dst_port }
            }
            MaybeParsed::Incomplete(i) => *i.flow,
        }
    }
}

impl<B: SplitByteSlice + CloneableByteSlice> TcpSegmentRaw<B> {
    /// Transform this `TcpSegmentRaw` into the equivalent builder, parsed options, and body.
    pub fn into_builder_options<A: IpAddress>(
        self,
        src_ip: A,
        dst_ip: A,
    ) -> Result<
        (TcpSegmentBuilder<A>, Result<TcpOptionsRef<B>, (TcpOptionsRef<B>, ParseError)>, B),
        ParseError,
    > {
        let Self { hdr_prefix, options, body } = self;

        let builder = hdr_prefix
            .complete()
            .ok_checked::<PartialHeaderPrefix<B>>()
            .map(|hdr_prefix| hdr_prefix.builder(src_ip, dst_ip))
            .ok_or(ParseError::Format)?;

        let raw_options = options.complete().ok_checked::<B>().ok_or(ParseError::Format)?;
        let options = TcpOptionsRef::try_from_raw(raw_options);

        Ok((builder, options, body))
    }
}

/// Options provided to [`TcpSegmentBuilderWithOptions::new`] exceed
/// [`MAX_OPTIONS_LEN`] when serialized.
#[derive(Debug)]
pub struct TcpOptionsTooLongError;

/// TCP segment context relevant to serialization.
pub struct TcpEnvelope;

/// A trait for TCP serialization contexts.
pub trait TcpSerializationContext: SerializationContext {
    /// Converts a `TcpEnvelope` into the serialization context's state.
    fn envelope_to_state(envelope: TcpEnvelope) -> Self::ContextState;

    /// Returns the checksum action to take based on the serialization context.
    fn checksum_action(&mut self) -> TransportChecksumAction;
}

impl TcpSerializationContext for NoOpSerializationContext {
    fn envelope_to_state(_envelope: TcpEnvelope) -> Self::ContextState {
        ()
    }

    fn checksum_action(&mut self) -> TransportChecksumAction {
        TransportChecksumAction::ComputeFull
    }
}

/// A builder for TCP segments with options
#[derive(Debug, Clone)]
pub struct TcpSegmentBuilderWithOptions<A: IpAddress, O> {
    prefix_builder: TcpSegmentBuilder<A>,
    options: O,
}

impl<'a, A> TcpSegmentBuilderWithOptions<A, TcpOptionsBuilder<'a>>
where
    A: IpAddress,
{
    /// Creates a `TcpSegmentBuilderWithOptions`.
    ///
    /// Returns `Err` if the segment header would exceed the maximum length of
    /// [`MAX_HDR_LEN`]. This happens if the `options`, when serialized, would
    /// exceed [`MAX_OPTIONS_LEN`].
    pub fn new(
        prefix_builder: TcpSegmentBuilder<A>,
        options: TcpOptionsBuilder<'a>,
    ) -> Result<TcpSegmentBuilderWithOptions<A, TcpOptionsBuilder<'a>>, TcpOptionsTooLongError>
    {
        if options.bytes_len() > MAX_OPTIONS_LEN {
            return Err(TcpOptionsTooLongError);
        }
        Ok(TcpSegmentBuilderWithOptions { prefix_builder, options })
    }
}

impl<A: IpAddress, O> TcpSegmentBuilderWithOptions<A, O> {
    /// Returns the source port for the builder.
    pub fn src_port(&self) -> Option<NonZeroU16> {
        self.prefix_builder.src_port
    }

    /// Returns the destination port for the builder.
    pub fn dst_port(&self) -> Option<NonZeroU16> {
        self.prefix_builder.dst_port
    }

    /// Sets the source IP address for the builder.
    pub fn set_src_ip(&mut self, addr: A) {
        self.prefix_builder.src_ip = addr;
    }

    /// Sets the destination IP address for the builder.
    pub fn set_dst_ip(&mut self, addr: A) {
        self.prefix_builder.dst_ip = addr;
    }

    /// Sets the source port for the builder.
    pub fn set_src_port(&mut self, port: NonZeroU16) {
        self.prefix_builder.src_port = Some(port);
    }

    /// Sets the destination port for the builder.
    pub fn set_dst_port(&mut self, port: NonZeroU16) {
        self.prefix_builder.dst_port = Some(port);
    }

    /// Returns a shared reference to the prefix builder of the segment.
    pub fn prefix_builder(&self) -> &TcpSegmentBuilder<A> {
        &self.prefix_builder
    }

    /// Returns the options in this builder.
    pub fn options(&self) -> &O {
        &self.options
    }
}

impl<A: IpAddress, O: InnerPacketBuilder> NestablePacketBuilder
    for TcpSegmentBuilderWithOptions<A, O>
{
    fn constraints(&self) -> PacketConstraints {
        let header_len = HDR_PREFIX_LEN + self.options.bytes_len();
        assert_eq!(header_len % 4, 0);
        PacketConstraints::new(header_len, 0, 0, (1 << 16) - 1 - header_len)
    }
}

impl<A: IpAddress, O: InnerPacketBuilder, C: TcpSerializationContext> PacketBuilder<C>
    for TcpSegmentBuilderWithOptions<A, O>
{
    fn context_state(&self) -> C::ContextState {
        C::envelope_to_state(TcpEnvelope)
    }

    fn serialize(
        &self,
        context: &mut C,
        target: &mut SerializeTarget<'_>,
        body: FragmentedBytesMut<'_, '_>,
    ) {
        let opt_len = self.options.bytes_len();
        // `take_back_zero` consumes the extent of the receiving slice, but that
        // behavior is undesirable here: `prefix_builder.serialize` also needs
        // to write into the header. To avoid changing the extent of
        // target.header, we re-slice header before calling `take_back_zero`;
        // the re-slice will be consumed, but `target.header` is unaffected.
        let mut header = &mut &mut target.header[..];
        let options = header.take_back_zero(opt_len).expect("too few bytes for TCP options");
        self.options.serialize(options);
        self.prefix_builder.serialize(context, target, body);
    }
}

impl<A: IpAddress, O: InnerPacketBuilder, C: TcpSerializationContext> PartialPacketBuilder<C>
    for TcpSegmentBuilderWithOptions<A, O>
{
    fn partial_serialize(&self, context: &mut C, body_len: usize, mut buffer: &mut [u8]) {
        let opt_len = self.options.bytes_len();
        let hdr_len = HDR_PREFIX_LEN + opt_len;
        self.prefix_builder.partial_serialize(context, body_len, &mut buffer[..hdr_len]);

        let options = (&mut buffer).take_back_zero(opt_len).expect("too few bytes for TCP options");
        self.options.serialize(options)
    }
}

// NOTE(joshlf): In order to ensure that the checksum is always valid, we don't
// expose any setters for the fields of the TCP segment; the only way to set
// them is via TcpSegmentBuilder. This, combined with checksum validation
// performed in TcpSegment::parse, provides the invariant that a TcpSegment
// always has a valid checksum.

/// A builder for TCP segments.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct TcpSegmentBuilder<A: IpAddress> {
    src_ip: A,
    dst_ip: A,
    src_port: Option<NonZeroU16>,
    dst_port: Option<NonZeroU16>,
    seq_num: u32,
    ack_num: u32,
    data_offset_reserved_flags: DataOffsetReservedFlags,
    window_size: u16,
}

impl<A: IpAddress> TcpSegmentBuilder<A> {
    /// Constructs a new `TcpSegmentBuilder`.
    ///
    /// If `ack_num` is `Some`, then the ACK flag will be set.
    pub fn new(
        src_ip: A,
        dst_ip: A,
        src_port: NonZeroU16,
        dst_port: NonZeroU16,
        seq_num: u32,
        ack_num: Option<u32>,
        window_size: u16,
    ) -> TcpSegmentBuilder<A> {
        let (data_offset_reserved_flags, ack_num) = ack_num
            .map(|a| (DataOffsetReservedFlags::ACK_SET, a))
            .unwrap_or((DataOffsetReservedFlags::EMPTY, 0));
        TcpSegmentBuilder {
            src_ip,
            dst_ip,
            src_port: Some(src_port),
            dst_port: Some(dst_port),
            seq_num,
            ack_num,
            data_offset_reserved_flags,
            window_size,
        }
    }

    /// Sets the PSH flag.
    pub fn psh(&mut self, psh: bool) {
        self.data_offset_reserved_flags.set_psh(psh);
    }

    /// Returns the current value of the PSH flag.
    pub fn psh_set(&self) -> bool {
        self.data_offset_reserved_flags.psh()
    }

    /// Sets the RST flag.
    pub fn rst(&mut self, rst: bool) {
        self.data_offset_reserved_flags.set_rst(rst);
    }

    /// Returns the current value of the RST flag.
    pub fn rst_set(&self) -> bool {
        self.data_offset_reserved_flags.rst()
    }

    /// Sets the SYN flag.
    pub fn syn(&mut self, syn: bool) {
        self.data_offset_reserved_flags.set_syn(syn);
    }

    /// Returns the current value of the SYN flag.
    pub fn syn_set(&self) -> bool {
        self.data_offset_reserved_flags.syn()
    }

    /// Sets the FIN flag.
    pub fn fin(&mut self, fin: bool) {
        self.data_offset_reserved_flags.set_fin(fin);
    }

    /// Returns the current value of the FIN flag.
    pub fn fin_set(&self) -> bool {
        self.data_offset_reserved_flags.fin()
    }

    /// Returns the source port for the builder.
    pub fn src_port(&self) -> Option<NonZeroU16> {
        self.src_port
    }

    /// Returns the destination port for the builder.
    pub fn dst_port(&self) -> Option<NonZeroU16> {
        self.dst_port
    }

    /// Returns the sequence number for the builder
    pub fn seq_num(&self) -> u32 {
        self.seq_num
    }

    /// Returns the ACK number, if present.
    pub fn ack_num(&self) -> Option<u32> {
        self.data_offset_reserved_flags.ack().then_some(self.ack_num)
    }

    /// Returns the unscaled window size
    pub fn window_size(&self) -> u16 {
        self.window_size
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
    pub fn set_src_port(&mut self, port: NonZeroU16) {
        self.src_port = Some(port);
    }

    /// Sets the destination port for the builder.
    pub fn set_dst_port(&mut self, port: NonZeroU16) {
        self.dst_port = Some(port);
    }

    fn serialize_header(&self, header: &mut [u8]) {
        let hdr_len = header.len();

        debug_assert_eq!(hdr_len % 4, 0, "header length isn't a multiple of 4: {}", hdr_len);
        let mut data_offset_reserved_flags = self.data_offset_reserved_flags;
        data_offset_reserved_flags.set_data_offset(
            (hdr_len / 4).try_into().expect("header length too long for TCP segment"),
        );
        // `write_obj_front` consumes the extent of the receiving slice, but
        // that behavior is undesirable here: at the end of this method, we
        // write the checksum back into the header. To avoid this, we re-slice
        // header before calling `write_obj_front`; the re-slice will be
        // consumed, but `target.header` is unaffected.
        (&mut &mut header[..])
            .write_obj_front(&HeaderPrefix::new(
                self.src_port.map_or(0, NonZeroU16::get),
                self.dst_port.map_or(0, NonZeroU16::get),
                self.seq_num,
                self.ack_num,
                data_offset_reserved_flags,
                self.window_size,
                // Initialize the checksum to 0 so that we will get the
                // correct value when we compute it below.
                [0, 0],
                // We don't support setting the Urgent Pointer.
                0,
            ))
            .expect("too few bytes for TCP header prefix");
    }
}

impl<A: IpAddress> NestablePacketBuilder for TcpSegmentBuilder<A> {
    fn constraints(&self) -> PacketConstraints {
        PacketConstraints::new(HDR_PREFIX_LEN, 0, 0, usize::MAX)
    }
}

impl<A: IpAddress, C: TcpSerializationContext> PacketBuilder<C> for TcpSegmentBuilder<A> {
    fn context_state(&self) -> C::ContextState {
        C::envelope_to_state(TcpEnvelope)
    }

    fn serialize(
        &self,
        context: &mut C,
        target: &mut SerializeTarget<'_>,
        body: FragmentedBytesMut<'_, '_>,
    ) {
        self.serialize_header(target.header);

        let body_len = body.len();

        let checksum = match context.checksum_action() {
            TransportChecksumAction::ComputeFull => compute_transport_checksum_serialize(
                self.src_ip,
                self.dst_ip,
                IpProto::Tcp.into(),
                target,
                body,
            ),
            TransportChecksumAction::ComputePartial => {
                compute_transport_pseudo_header_partial_checksum(
                    self.src_ip,
                    self.dst_ip,
                    IpProto::Tcp.into(),
                    target,
                    body,
                )
            }
        }
        .unwrap_or_else(|| {
            panic!(
                "total TCP segment length of {} bytes overflows length field of pseudo-header",
                target.header.len() + body_len + target.footer.len(),
            )
        });

        target.header[CHECKSUM_RANGE].copy_from_slice(&checksum[..]);
    }
}

impl<A: IpAddress, C: TcpSerializationContext> PartialPacketBuilder<C> for TcpSegmentBuilder<A> {
    fn partial_serialize(&self, _context: &mut C, _body_len: usize, buffer: &mut [u8]) {
        self.serialize_header(buffer)
    }
}

/// Parsing and serialization of TCP options.
pub mod options {
    use derivative::Derivative;
    use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

    use super::*;

    const OPTION_KIND_EOL: u8 = 0;
    pub(super) const OPTION_KIND_NOP: u8 = 1;
    const OPTION_KIND_MSS: u8 = 2;
    const OPTION_KIND_WINDOW_SCALE: u8 = 3;
    const OPTION_KIND_SACK_PERMITTED: u8 = 4;
    pub(super) const OPTION_KIND_SACK: u8 = 5;
    pub(super) const OPTION_KIND_TIMESTAMP: u8 = 8;

    // The size of each TCP Option, including the "kind" and "length" fields.
    // Not all options have a fixed size (e.g. SACK blocks).
    const OPTION_LEN_MSS: usize = 4;
    const OPTION_LEN_WINDOW_SCALE: usize = 3;
    const OPTION_LEN_SACK_PERMITTED: usize = 2;
    pub(super) const OPTION_LEN_TIMESTAMP: usize = 10;

    /// Per RFC 7323 Section 3.2, the TCP Timestamp option has a length of
    /// 10 bytes:
    ///   +-------+-------+---------------------+---------------------+
    ///   |Kind=8 |  10   |   TS Value (TSval)  |TS Echo Reply (TSecr)|
    ///   +-------+-------+---------------------+---------------------+
    ///      1       1              4                     4
    ///
    /// However, once aligned, it will occupy 12 bytes.
    pub const ALIGNED_TIMESTAMP_OPTION_LENGTH: usize =
        crate::utils::round_to_next_multiple_of_four(OPTION_LEN_TIMESTAMP);

    /// Per RFC 7323, Appendix A:
    ///   The following layout is recommended for sending options on
    ///   non-<SYN> segments to achieve maximum feasible alignment of 32-bit
    ///   and 64-bit machines.
    ///
    ///       +--------+--------+--------+--------+
    ///       |   NOP  |  NOP   |  TSopt |   10   |
    ///       +--------+--------+--------+--------+
    ///       |          TSval timestamp          |
    ///       +--------+--------+--------+--------+
    ///       |          TSecr timestamp          |
    ///       +--------+--------+--------+--------+
    ///
    /// In the implementation below, we follow this recommendation for segments
    /// whose only option is the timestamp option.
    const TIMESTAMP_HOTPATH_PREFIX: [u8; 4] =
        [OPTION_KIND_NOP, OPTION_KIND_NOP, OPTION_KIND_TIMESTAMP, OPTION_LEN_TIMESTAMP as u8];

    /// An implementation of TCP Options, as defined of RFC 9293 section 3.1
    ///
    /// Provides a consistent API for accessing TCP Options across various
    /// implementations (e.g. those used for parsing vs serializing).
    pub trait TcpOptions {
        /// Access the MSS option, if present.
        fn mss(&self) -> Option<u16>;

        /// Access the Window Scale option, if present.
        fn window_scale(&self) -> Option<u8>;

        /// Access the SACK Permitted option, if present.
        fn sack_permitted(&self) -> bool;

        /// Access the SACK option, if present.
        fn sack_blocks(&self) -> Option<&[TcpSackBlock]>;

        /// Access the timestamp option, if present.
        fn timestamp(&self) -> Option<&TimestampOption>;
    }

    /// TCP Options that borrow from a backing buffer.
    ///
    /// Typically used for parsing TCP Options.
    ///
    /// When parsing, this type imposes a `B: CloneableByteSlice` bound. This is
    /// so that the type can
    ///   1) retain the original `B` to return the option bytes exactly as they
    ///      were, and
    ///   2) have individual fields reference subsections of the `B` to avoid
    ///      needless copies.
    ///
    /// Note, for options that are small (< 16 bytes), this type will hold owned
    /// copies, as they're cheaper than storing a `Ref<B, _>`.
    #[derive(Derivative)]
    #[derivative(Debug(bound = "B: ByteSlice"))]
    pub struct TcpOptionsRef<B> {
        #[derivative(Debug = "ignore")]
        bytes: B,
        mss: Option<u16>,
        window_scale: Option<u8>,
        sack_permitted: bool,
        sack_blocks: Option<Ref<B, [TcpSackBlock]>>,
        timestamp: Option<TimestampOption>,
    }

    impl<B: ByteSlice> TcpOptionsRef<B> {
        #[inline(always)]
        pub(super) fn len(&self) -> usize {
            self.bytes().len()
        }

        #[inline(always)]
        pub(super) fn bytes(&self) -> &[u8] {
            self.bytes.deref()
        }
    }

    impl<B: ByteSlice> InnerPacketBuilder for TcpOptionsRef<B> {
        fn bytes_len(&self) -> usize {
            self.len()
        }

        fn serialize(&self, buffer: &mut [u8]) {
            buffer.copy_from_slice(self.bytes())
        }
    }

    impl<B: ByteSlice> TcpOptions for &TcpOptionsRef<B> {
        #[inline(always)]
        fn mss(&self) -> Option<u16> {
            self.mss
        }

        #[inline(always)]
        fn window_scale(&self) -> Option<u8> {
            self.window_scale
        }

        #[inline(always)]
        fn sack_permitted(&self) -> bool {
            self.sack_permitted
        }

        #[inline(always)]
        fn sack_blocks(&self) -> Option<&[TcpSackBlock]> {
            self.sack_blocks.as_deref()
        }

        #[inline(always)]
        fn timestamp(&self) -> Option<&TimestampOption> {
            self.timestamp.as_ref()
        }
    }

    impl<B: SplitByteSlice + CloneableByteSlice> TcpOptionsRef<B> {
        /// Parse TCP Options from the raw byte buffer.
        ///
        /// The layout of TCP Options is defined in RFC 9293, section 3.1
        ///
        /// Each Option is composed of a 1 byte "kind" field, followed by a
        /// 1 byte "len" field, followed by variable length "data" field.
        ///
        /// If parsing fails, return the parsed options so far and the error.
        pub(super) fn try_from_raw(raw: TcpOptionsRaw<B>) -> Result<Self, (Self, ParseError)> {
            let TcpOptionsRaw { bytes } = raw;

            // A mutable result to be filled in as we walk the options list.
            //
            // Note, if the options list contains the same value multiple times,
            // subsequent instances will overwrite the previous instances in
            // this struct. Effectively, all but the final instance will be
            // ignored.
            //
            // The RFC does not specify how to handle repeated options, so we
            // instead follow prior art and mimic Linux's behavior. See
            // https://github.com/torvalds/linux/blob/ecfea98b7d0d56c5bf2df3fc02c5501afa5cef6f/net/ipv4/tcp_input.c#L4284
            let mut result = TcpOptionsRef {
                bytes,
                mss: None,
                window_scale: None,
                sack_permitted: false,
                sack_blocks: None,
                timestamp: None,
            };

            // HOT PATH: No Options.
            if result.bytes.deref().len() == 0 {
                return Ok(result);
            }

            // NB: Clone the byte slice (not the underlying data) so that we can
            // retain a reference to the start, while also creating references
            // to options in the middle.
            let mut bytes = SplitByteSliceBufView::new(result.bytes.clone());

            let parse = |result: &mut Self,
                         bytes: &mut SplitByteSliceBufView<B>|
             -> Result<(), ParseError> {
                // HOT PATH: Only Timestamp Option.
                if bytes.len() == ALIGNED_TIMESTAMP_OPTION_LENGTH
                    && bytes.peek_obj_front::<[u8; 4]>() == Some(&TIMESTAMP_HOTPATH_PREFIX)
                {
                    result.timestamp = bytes.take_owned_obj_back::<TimestampOption>();
                    return Ok(());
                }

                while let Some(kind) = bytes.take_owned_obj_front::<u8>() {
                    if kind == OPTION_KIND_EOL {
                        break;
                    }
                    if kind == OPTION_KIND_NOP {
                        continue;
                    }
                    // Every option besides EOL & NOP must have a length.
                    let len = bytes.take_owned_obj_front::<u8>().ok_or(ParseError::Format)?;
                    let len = usize::from(len);

                    match kind {
                        OPTION_KIND_MSS => {
                            if len != OPTION_LEN_MSS {
                                return Err(ParseError::Format);
                            }
                            result.mss = Some(
                                bytes
                                    .take_owned_obj_front::<U16>()
                                    .ok_or(ParseError::Format)?
                                    .get(),
                            );
                        }
                        OPTION_KIND_WINDOW_SCALE => {
                            if len != OPTION_LEN_WINDOW_SCALE {
                                return Err(ParseError::Format);
                            }
                            result.window_scale =
                                Some(bytes.take_owned_obj_front::<u8>().ok_or(ParseError::Format)?);
                        }

                        OPTION_KIND_SACK_PERMITTED => {
                            if len != OPTION_LEN_SACK_PERMITTED {
                                return Err(ParseError::Format);
                            }
                            result.sack_permitted = true;
                        }
                        OPTION_KIND_SACK => {
                            // NB: Subtract 2 since we've already advanced beyond
                            // the kind and length fields
                            let len = len.checked_sub(2).ok_or(ParseError::Format)?;
                            result.sack_blocks = Some(
                                bytes
                                    .take_front(len)
                                    .map(|b| Ref::from_bytes(b).map_err(|_| ParseError::Format))
                                    .unwrap_or(Err(ParseError::Format))?,
                            );
                        }
                        OPTION_KIND_TIMESTAMP => {
                            if len != OPTION_LEN_TIMESTAMP {
                                return Err(ParseError::Format);
                            }
                            result.timestamp = Some(
                                bytes
                                    .take_owned_obj_front::<TimestampOption>()
                                    .ok_or(ParseError::Format)?,
                            );
                        }
                        _ => {
                            // NB: Subtract 2 since we've already advanced beyond
                            // the kind and length fields
                            let len = len.checked_sub(2).ok_or(ParseError::Format)?;

                            // Ignore unknown options, but move `bytes` ahead to
                            // allow subsequent options to be parsed.
                            let _: B = bytes.take_front(len).ok_or(ParseError::Format)?;
                        }
                    }
                }
                Ok(())
            };

            match parse(&mut result, &mut bytes) {
                Ok(()) => Ok(result),
                Err(err) => Err((result, err)),
            }
        }
    }

    /// Partially parsed and not yet validated TCP Options.
    #[derive(Debug)]
    pub(super) struct TcpOptionsRaw<B> {
        bytes: B,
    }

    impl<B> TcpOptionsRaw<B> {
        pub(super) fn new(bytes: B) -> TcpOptionsRaw<B> {
            Self { bytes }
        }
    }

    impl<B: ByteSlice> Deref for TcpOptionsRaw<B> {
        type Target = [u8];

        fn deref(&self) -> &[u8] {
            let Self { bytes } = self;
            bytes.deref()
        }
    }

    impl<B: ByteSlice> InnerPacketBuilder for TcpOptionsRaw<B> {
        fn bytes_len(&self) -> usize {
            self.deref().len()
        }

        fn serialize(&self, buffer: &mut [u8]) {
            buffer.copy_from_slice(self.deref())
        }
    }

    /// A type capable of serializing TCP Options.
    #[derive(Debug, Default)]
    pub struct TcpOptionsBuilder<'a> {
        /// The MSS Option to serialize, if any.
        pub mss: Option<u16>,
        /// The Window Scale Option to serialize, if any.
        pub window_scale: Option<u8>,
        /// Whether or not to serialize a SACK Permitted option.
        pub sack_permitted: bool,
        /// The SACK Option to serialize, if any.
        pub sack_blocks: Option<&'a [TcpSackBlock]>,
        /// The Timestamp Option to serialize, if any.
        pub timestamp: Option<TimestampOption>,
    }

    #[inline(always)]
    fn sack_blocks_len(sack_blocks: &[TcpSackBlock]) -> usize {
        // NB: Add 2, because the length needs to account for the kind
        // and length fields.
        sack_blocks.len() * TcpSackBlock::SIZE_OF_ONE_BLOCK + 2
    }

    impl<'a> InnerPacketBuilder for TcpOptionsBuilder<'a> {
        fn bytes_len(&self) -> usize {
            let Self { mss, window_scale, sack_permitted, sack_blocks, timestamp } = self;
            let mut sum = 0;
            if mss.is_some() {
                sum += OPTION_LEN_MSS;
            }
            if window_scale.is_some() {
                sum += OPTION_LEN_WINDOW_SCALE;
            }
            if *sack_permitted {
                sum += OPTION_LEN_SACK_PERMITTED;
            }
            if let Some(sb) = sack_blocks {
                sum += sack_blocks_len(sb);
            }
            if timestamp.is_some() {
                sum += OPTION_LEN_TIMESTAMP;
            }

            // TCP Options must be aligned to a 4-byte boundary.
            crate::utils::round_to_next_multiple_of_four(sum)
        }

        fn serialize(&self, mut buffer: &mut [u8]) {
            let Self { mss, window_scale, sack_permitted, sack_blocks, timestamp } = self;
            let mut buffer = &mut buffer;

            // NB: Out of an abundance of caution, serialize options in the same
            // order as Linux. It's possible that there are TCP implementations
            // out in the wild that (incorrectly) have a dependency on a
            // specific order. Linux's order is:
            // [MSS, SACK_PERMITTED, TIMESTAMP, WINDOW_SCALE, SACK]
            //
            // See `tcp_options_write`:
            // https://github.com/torvalds/linux/blob/15f295f55656658e65bdbc9b901d6b2e49d68d72/net/ipv4/tcp_output.c#L631

            if let Some(mss) = mss {
                buffer
                    .write_obj_front(&OptionKindAndLen {
                        kind: OPTION_KIND_MSS,
                        len: OPTION_LEN_MSS as u8,
                    })
                    .expect("buffer too short");
                buffer.write_obj_front(&U16::new(*mss)).expect("buffer too short");
            }
            if *sack_permitted {
                buffer
                    .write_obj_front(&OptionKindAndLen {
                        kind: OPTION_KIND_SACK_PERMITTED,
                        len: OPTION_LEN_SACK_PERMITTED as u8,
                    })
                    .expect("buffer too short");
            }
            if let Some(ts) = timestamp {
                // If there's sufficient space available (e.g. the buffer
                // contains padding), prefer to write the timestamp option in
                // an aligned representation. This has negligible improvements
                // to serialization performance, but can enable substantial
                // improvements to the receiver's parsing performance.
                //
                // If the buffer size is `ALIGNED_TIMESTAMP_OPTION_LENGTH` (12)
                // we'll be "stealing" 2 bytes. The tricky thing is knowing
                // whether those bytes are actually padding and safe to steal,
                // or if they were intended to be used by another option.
                // SACK Permitted is the only TCP Option with a length <= 2.
                // Since we've already attempted to serialize Sack Permitted
                // above, we can be certain these 2 bytes are padding. None of
                // the yet to be serialized options would be able to make use of
                // the space.
                if (*buffer).len() == ALIGNED_TIMESTAMP_OPTION_LENGTH {
                    buffer
                        .write_obj_front::<[u8; 4]>(&TIMESTAMP_HOTPATH_PREFIX)
                        .expect("buffer too short");
                } else {
                    buffer
                        .write_obj_front(&OptionKindAndLen {
                            kind: OPTION_KIND_TIMESTAMP,
                            len: OPTION_LEN_TIMESTAMP as u8,
                        })
                        .expect("buffer too short");
                }
                buffer.write_obj_front(ts).expect("buffer too short");
            }
            if let Some(ws) = window_scale {
                buffer
                    .write_obj_front(&OptionKindAndLen {
                        kind: OPTION_KIND_WINDOW_SCALE,
                        len: OPTION_LEN_WINDOW_SCALE as u8,
                    })
                    .expect("buffer too short");
                buffer.write_obj_front(ws).expect("buffer too short");
            }
            if let Some(sb) = sack_blocks {
                let len = sack_blocks_len(sb);
                buffer
                    .write_obj_front(&OptionKindAndLen { kind: OPTION_KIND_SACK, len: len as u8 })
                    .expect("buffer too short");
                buffer.write_obj_front(*sb).expect("buffer too short");
            }
        }
    }

    impl<'a> TcpOptions for &TcpOptionsBuilder<'a> {
        #[inline(always)]
        fn mss(&self) -> Option<u16> {
            self.mss
        }

        #[inline(always)]
        fn window_scale(&self) -> Option<u8> {
            self.window_scale
        }

        #[inline(always)]
        fn sack_permitted(&self) -> bool {
            self.sack_permitted
        }

        #[inline(always)]
        fn sack_blocks(&self) -> Option<&[TcpSackBlock]> {
            self.sack_blocks
        }

        #[inline(always)]
        fn timestamp(&self) -> Option<&TimestampOption> {
            self.timestamp.as_ref()
        }
    }

    #[derive(
        Copy, Clone, Eq, PartialEq, Debug, KnownLayout, FromBytes, IntoBytes, Immutable, Unaligned,
    )]
    #[repr(C)]
    struct OptionKindAndLen {
        kind: u8,
        len: u8,
    }

    /// The TCP Timestamp Option, as defined in RFC 7323, section 3.
    #[derive(
        Copy, Clone, Eq, PartialEq, Debug, KnownLayout, FromBytes, IntoBytes, Immutable, Unaligned,
    )]
    #[repr(C)]
    pub struct TimestampOption {
        /// TS Value (TSval).
        ts_val: U32,
        /// TS Echo Reply (TSecr).
        ts_echo_reply: U32,
    }

    impl TimestampOption {
        /// Returns a `TimestampOption` with the specified TSval and TSecr.
        pub const fn new(ts_val: u32, ts_echo_reply: u32) -> Self {
            TimestampOption { ts_val: U32::new(ts_val), ts_echo_reply: U32::new(ts_echo_reply) }
        }

        /// Returns the option's TSval.
        pub const fn ts_val(&self) -> u32 {
            self.ts_val.get()
        }

        /// Returns the option's TSecr.
        pub const fn ts_echo_reply(&self) -> u32 {
            self.ts_echo_reply.get()
        }
    }

    /// A TCP selective ACK block.
    ///
    /// A selective ACK block indicates that the range of bytes `[left_edge,
    /// right_edge)` have been received.
    ///
    /// See [RFC 2018] for more details.
    ///
    /// [RFC 2018]: https://tools.ietf.org/html/rfc2018
    #[derive(
        Copy, Clone, Eq, PartialEq, Debug, KnownLayout, FromBytes, IntoBytes, Immutable, Unaligned,
    )]
    #[repr(C)]
    pub struct TcpSackBlock {
        left_edge: U32,
        right_edge: U32,
    }

    impl TcpSackBlock {
        // The number of bytes occupied by a single TCP SACK block.
        const SIZE_OF_ONE_BLOCK: usize = 8;

        /// Returns a `TcpSackBlock` with the specified left and right edge values.
        pub const fn new(left_edge: u32, right_edge: u32) -> TcpSackBlock {
            TcpSackBlock { left_edge: U32::new(left_edge), right_edge: U32::new(right_edge) }
        }

        /// Returns the left edge of the SACK block.
        pub const fn left_edge(&self) -> u32 {
            self.left_edge.get()
        }

        /// Returns the right edge of the SACK block.
        pub const fn right_edge(&self) -> u32 {
            self.right_edge.get()
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn test_tcp_sack_block() {
            let sack = TcpSackBlock::new(1, 2);
            assert_eq!(sack.left_edge.get(), 1);
            assert_eq!(sack.right_edge.get(), 2);
            assert_eq!(sack.left_edge(), 1);
            assert_eq!(sack.right_edge(), 2);
        }
    }
}

// needed by Result::unwrap_err in the tests below
#[cfg(test)]
impl<B> Debug for TcpSegment<B> {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> fmt::Result {
        write!(fmt, "TcpSegment")
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use byteorder::{ByteOrder, NetworkEndian};
    use net_types::ip::{Ipv4, Ipv4Addr, Ipv6Addr};
    use packet::{Buf, NestableSerializer as _, ParseBuffer};
    use test_case::test_case;

    use super::*;
    use crate::ethernet::{EthernetFrame, EthernetFrameLengthCheck};
    use crate::ipv4::{Ipv4Header, Ipv4Packet};
    use crate::ipv6::{Ipv6Header, Ipv6Packet};
    use crate::tcp::options::{
        ALIGNED_TIMESTAMP_OPTION_LENGTH, OPTION_KIND_NOP, OPTION_KIND_TIMESTAMP,
        OPTION_LEN_TIMESTAMP, TcpOptions, TcpSackBlock, TimestampOption,
    };
    use crate::testutil::*;
    use crate::{add_transport_pseudo_header_checksum, compute_transport_checksum};

    const TEST_SRC_IPV4: Ipv4Addr = Ipv4Addr::new([1, 2, 3, 4]);
    const TEST_DST_IPV4: Ipv4Addr = Ipv4Addr::new([5, 6, 7, 8]);
    const TEST_SRC_IPV6: Ipv6Addr =
        Ipv6Addr::from_bytes([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
    const TEST_DST_IPV6: Ipv6Addr =
        Ipv6Addr::from_bytes([17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32]);

    #[test]
    fn test_parse_serialize_full_ipv4() {
        use crate::testdata::tls_client_hello_v4::*;

        let mut buf = ETHERNET_FRAME.bytes;
        let frame = buf.parse_with::<_, EthernetFrame<_>>(EthernetFrameLengthCheck::Check).unwrap();
        verify_ethernet_frame(&frame, ETHERNET_FRAME);

        let mut body = frame.body();
        let packet = body.parse::<Ipv4Packet<_>>().unwrap();
        verify_ipv4_packet(&packet, IPV4_PACKET);

        let mut body = packet.body();
        let segment = body
            .parse_with::<_, TcpSegment<_>>(TcpParseArgs::new(packet.src_ip(), packet.dst_ip()))
            .unwrap();
        verify_tcp_segment(&segment, TCP_SEGMENT);

        // Serialize using `segment.builder()` to construct a
        // `TcpSegmentBuilderWithOptions`, which simply copies the bytes of the
        // options without parsing or iterating over them.
        let buffer = Buf::new(segment.body().to_vec(), ..)
            .wrap_in(segment.builder(packet.src_ip(), packet.dst_ip()))
            .wrap_in(packet.builder())
            .wrap_in(frame.builder())
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap();
        assert_eq!(buffer.as_ref(), ETHERNET_FRAME.bytes);
    }

    #[test]
    fn test_parse_serialize_full_ipv6() {
        use crate::testdata::syn_v6::*;

        let mut buf = ETHERNET_FRAME.bytes;
        let frame = buf.parse_with::<_, EthernetFrame<_>>(EthernetFrameLengthCheck::Check).unwrap();
        verify_ethernet_frame(&frame, ETHERNET_FRAME);

        let mut body = frame.body();
        let packet = body.parse::<Ipv6Packet<_>>().unwrap();
        verify_ipv6_packet(&packet, IPV6_PACKET);

        let mut body = packet.body();
        let segment = body
            .parse_with::<_, TcpSegment<_>>(TcpParseArgs::new(packet.src_ip(), packet.dst_ip()))
            .unwrap();
        verify_tcp_segment(&segment, TCP_SEGMENT);

        // Serialize using `segment.builder()` to construct a
        // `TcpSegmentBuilderWithOptions`, which simply copies the bytes of the
        // options without parsing or iterating over them.
        let buffer = Buf::new(segment.body().to_vec(), ..)
            .wrap_in(segment.builder(packet.src_ip(), packet.dst_ip()))
            .wrap_in(packet.builder())
            .wrap_in(frame.builder())
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap();
        assert_eq!(buffer.as_ref(), ETHERNET_FRAME.bytes);
    }

    fn hdr_prefix_to_bytes(hdr_prefix: HeaderPrefix) -> [u8; HDR_PREFIX_LEN] {
        zerocopy::transmute!(hdr_prefix)
    }

    // Return a new HeaderPrefix with reasonable defaults, including a valid
    // checksum (assuming no body and the src/dst IPs TEST_SRC_IPV4 and
    // TEST_DST_IPV4).
    fn new_hdr_prefix() -> HeaderPrefix {
        HeaderPrefix::new(1, 2, 0, 0, DataOffsetReservedFlags::new(5), 0, [0x9f, 0xce], 0)
    }

    #[test]
    fn test_parse() {
        let mut buf = &hdr_prefix_to_bytes(new_hdr_prefix())[..];
        let segment = buf
            .parse_with::<_, TcpSegment<_>>(TcpParseArgs::new(TEST_SRC_IPV4, TEST_DST_IPV4))
            .unwrap();
        assert_eq!(segment.src_port().get(), 1);
        assert_eq!(segment.dst_port().get(), 2);
        assert_eq!(segment.body(), []);
    }

    #[test]
    fn test_parse_error() {
        // Assert that parsing a particular header prefix results in an error.
        // This function is responsible for ensuring that the checksum is
        // correct so that checksum errors won't hide the errors we're trying to
        // test.
        fn assert_header_err(hdr_prefix: HeaderPrefix, err: ParseError) {
            let mut buf = &mut hdr_prefix_to_bytes(hdr_prefix)[..];
            NetworkEndian::write_u16(&mut buf[CHECKSUM_OFFSET..], 0);
            let checksum =
                compute_transport_checksum(TEST_SRC_IPV4, TEST_DST_IPV4, IpProto::Tcp.into(), buf)
                    .unwrap();
            buf[CHECKSUM_RANGE].copy_from_slice(&checksum[..]);
            assert_eq!(
                buf.parse_with::<_, TcpSegment<_>>(TcpParseArgs::new(TEST_SRC_IPV4, TEST_DST_IPV4))
                    .unwrap_err(),
                err
            );
        }

        // Set the source port to 0, which is illegal.
        let mut hdr_prefix = new_hdr_prefix();
        hdr_prefix.src_port = U16::ZERO;
        assert_header_err(hdr_prefix, ParseError::Format);

        // Set the destination port to 0, which is illegal.
        let mut hdr_prefix = new_hdr_prefix();
        hdr_prefix.dst_port = U16::ZERO;
        assert_header_err(hdr_prefix, ParseError::Format);

        // Set the data offset to 4, implying a header length of 16. This is
        // smaller than the minimum of 20.
        let mut hdr_prefix = new_hdr_prefix();
        hdr_prefix.data_offset_reserved_flags = DataOffsetReservedFlags::new(4);
        assert_header_err(hdr_prefix, ParseError::Format);

        // Set the data offset to 6, implying a header length of 24. This is
        // larger than the actual segment length of 20.
        let mut hdr_prefix = new_hdr_prefix();
        hdr_prefix.data_offset_reserved_flags = DataOffsetReservedFlags::new(12);
        assert_header_err(hdr_prefix, ParseError::Format);
    }

    // Return a stock TcpSegmentBuilder with reasonable default values.
    fn new_builder<A: IpAddress>(src_ip: A, dst_ip: A) -> TcpSegmentBuilder<A> {
        TcpSegmentBuilder::new(
            src_ip,
            dst_ip,
            NonZeroU16::new(1).unwrap(),
            NonZeroU16::new(2).unwrap(),
            3,
            Some(4),
            5,
        )
    }

    #[test_case(TEST_SRC_IPV4, TEST_DST_IPV4, true; "ipv4 skip")]
    #[test_case(TEST_SRC_IPV4, TEST_DST_IPV4, false; "ipv4 validate")]
    #[test_case(TEST_SRC_IPV6, TEST_DST_IPV6, true; "ipv6 skip")]
    #[test_case(TEST_SRC_IPV6, TEST_DST_IPV6, false; "ipv6 validate")]
    fn test_parse_invalid_checksum<A: IpAddress>(src: A, dst: A, skip: bool) {
        let mut buf = new_builder(src, dst)
            .wrap_body(EmptyBuf)
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap()
            .as_ref()
            .to_vec();

        // Corrupt the checksum.
        buf[CHECKSUM_OFFSET] ^= 0xFF;
        buf[CHECKSUM_OFFSET + 1] ^= 0xFF;

        let mut bv = &buf[..];
        let res = bv.parse_with::<_, TcpSegment<_>>(TcpParseArgs::with_context(
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
    fn test_serialize() {
        let mut builder = new_builder(TEST_SRC_IPV4, TEST_DST_IPV4);
        builder.fin(true);
        builder.rst(true);
        builder.syn(true);

        let mut buf = builder
            .wrap_body((&[0, 1, 2, 3, 4, 5, 7, 8, 9]).into_serializer())
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap();
        // assert that we get the literal bytes we expected
        assert_eq!(
            buf.as_ref(),
            [
                0, 1, 0, 2, 0, 0, 0, 3, 0, 0, 0, 4, 80, 23, 0, 5, 137, 145, 0, 0, 0, 1, 2, 3, 4, 5,
                7, 8, 9
            ]
        );
        let segment = buf
            .parse_with::<_, TcpSegment<_>>(TcpParseArgs::new(TEST_SRC_IPV4, TEST_DST_IPV4))
            .unwrap();
        // assert that when we parse those bytes, we get the values we set in
        // the builder
        assert_eq!(segment.src_port().get(), 1);
        assert_eq!(segment.dst_port().get(), 2);
        assert_eq!(segment.seq_num(), 3);
        assert_eq!(segment.ack_num(), Some(4));
        assert_eq!(segment.window_size(), 5);
        assert_eq!(segment.body(), [0, 1, 2, 3, 4, 5, 7, 8, 9]);
    }

    #[test]
    fn test_serialize_zeroes() {
        // Test that TcpSegmentBuilder::serialize properly zeroes memory before
        // serializing the header.
        let mut buf_0 = [0; HDR_PREFIX_LEN];
        let _: Buf<&mut [u8]> = new_builder(TEST_SRC_IPV4, TEST_DST_IPV4)
            .wrap_body(Buf::new(&mut buf_0[..], HDR_PREFIX_LEN..))
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap()
            .unwrap_a();
        let mut buf_1 = [0xFF; HDR_PREFIX_LEN];
        let _: Buf<&mut [u8]> = new_builder(TEST_SRC_IPV4, TEST_DST_IPV4)
            .wrap_body(Buf::new(&mut buf_1[..], HDR_PREFIX_LEN..))
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap()
            .unwrap_a();
        assert_eq!(&buf_0[..], &buf_1[..]);
    }

    #[test]
    fn test_serialization_checksum_actions() {
        let body = [0x12, 0x34];
        let serializer =
            new_builder(TEST_SRC_IPV4, TEST_DST_IPV4).wrap_body(body.into_serializer());

        // Create checksum over pseudo-header.
        let mut c = internet_checksum::Checksum::new();
        add_transport_pseudo_header_checksum::<Ipv4>(
            &mut c,
            TEST_SRC_IPV4,
            TEST_DST_IPV4,
            IpProto::Tcp.into(),
            HDR_PREFIX_LEN + body.len(),
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
    fn test_parse_serialize_reserved_bits() {
        // Test that we are forwards-compatible with the reserved zero bits in
        // the header being set - we can parse packets with these bits set and
        // we will not reject them. Test that we serialize these bits when
        // serializing from the `builder` methods.

        let mut buffer = new_builder(TEST_SRC_IPV4, TEST_DST_IPV4)
            .wrap_body(EmptyBuf)
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap()
            .unwrap_b();

        // Set all three reserved bits and update the checksum.
        let mut hdr_prefix = Ref::<_, HeaderPrefix>::from_bytes(buffer.as_mut()).unwrap();
        let old_checksum = hdr_prefix.checksum;
        let old_data_offset_reserved_flags = hdr_prefix.data_offset_reserved_flags;
        hdr_prefix.data_offset_reserved_flags.as_mut_bytes()[0] |= 0b00000111;
        hdr_prefix.checksum = internet_checksum::update(
            old_checksum,
            old_data_offset_reserved_flags.as_bytes(),
            hdr_prefix.data_offset_reserved_flags.as_bytes(),
        );

        let mut buf1 = buffer.clone();

        let segment = buf1
            .parse_with::<_, TcpSegment<_>>(TcpParseArgs::new(TEST_SRC_IPV4, TEST_DST_IPV4))
            .unwrap();

        // Serialize using the results of `TcpSegment::builder`.
        assert_eq!(
            segment
                .builder(TEST_SRC_IPV4, TEST_DST_IPV4)
                .wrap_body(EmptyBuf)
                .serialize_vec_outer(&mut NoOpSerializationContext)
                .unwrap()
                .unwrap_b()
                .as_ref(),
            buffer.as_ref()
        );
    }

    #[test]
    #[should_panic(
        expected = "total TCP segment length of 65536 bytes overflows length field of pseudo-header"
    )]
    fn test_serialize_panic_segment_too_long_ipv4() {
        // Test that a segment length which overflows u16 is rejected because it
        // can't fit in the length field in the IPv4 pseudo-header.
        let _: Buf<&mut [u8]> = new_builder(TEST_SRC_IPV4, TEST_DST_IPV4)
            .wrap_body(Buf::new(&mut [0; (1 << 16) - HDR_PREFIX_LEN][..], ..))
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap()
            .unwrap_a();
    }

    #[test]
    #[ignore] // this test panics with stack overflow; TODO(joshlf): Fix
    #[cfg(target_pointer_width = "64")] // 2^32 overflows on 32-bit platforms
    fn test_serialize_panic_segment_too_long_ipv6() {
        // Test that a segment length which overflows u32 is rejected because it
        // can't fit in the length field in the IPv4 pseudo-header.
        let _: Buf<&mut [u8]> = new_builder(TEST_SRC_IPV6, TEST_DST_IPV6)
            .wrap_body(Buf::new(&mut [0; (1 << 32) - HDR_PREFIX_LEN][..], ..))
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap()
            .unwrap_a();
    }

    #[test]
    fn test_partial_parse() {
        use core::ops::Deref as _;

        // Parse options partially:
        let make_hdr_prefix = || {
            let mut hdr_prefix = new_hdr_prefix();
            hdr_prefix.data_offset_reserved_flags.set_data_offset(8);
            hdr_prefix
        };
        let hdr_prefix = hdr_prefix_to_bytes(make_hdr_prefix());
        let mut bytes = hdr_prefix[..].to_owned();
        const OPTIONS: &[u8] = &[1, 2, 3, 4, 5];
        bytes.extend(OPTIONS);
        let mut buf = &bytes[..];
        let packet = buf.parse::<TcpSegmentRaw<_>>().unwrap();
        let TcpSegmentRaw { hdr_prefix, options, body } = &packet;
        assert_eq!(hdr_prefix.as_ref().complete().unwrap().deref(), &make_hdr_prefix());
        assert_eq!(options.as_ref().incomplete().unwrap(), &OPTIONS);
        assert_eq!(body, &[]);
        // validation should fail:
        assert!(
            TcpSegment::try_from_raw_with(packet, TcpParseArgs::new(TEST_SRC_IPV4, TEST_DST_IPV4))
                .is_err()
        );

        // Parse header partially:
        let hdr_prefix = new_hdr_prefix();
        let HeaderPrefix { src_port, dst_port, .. } = hdr_prefix;
        let bytes = hdr_prefix_to_bytes(hdr_prefix);
        let mut buf = &bytes[0..10];
        // Copy the rest portion since the buffer is mutably borrowed after parsing.
        let bytes_rest = buf[4..].to_owned();
        let packet = buf.parse::<TcpSegmentRaw<_>>().unwrap();
        let TcpSegmentRaw { hdr_prefix, options, body } = &packet;
        let PartialHeaderPrefix { flow, rest } = hdr_prefix.as_ref().incomplete().unwrap();
        assert_eq!(flow.deref(), &TcpFlowHeader { src_port, dst_port });
        assert_eq!(*rest, &bytes_rest[..]);
        assert_eq!(options.as_ref().incomplete().unwrap(), &[]);
        assert_eq!(body, &[]);
        // validation should fail:
        assert!(
            TcpSegment::try_from_raw_with(packet, TcpParseArgs::new(TEST_SRC_IPV4, TEST_DST_IPV4))
                .is_err()
        );

        let hdr_prefix = new_hdr_prefix();
        let bytes = hdr_prefix_to_bytes(hdr_prefix);
        // If we don't even have enough header bytes, we should fail partial
        // parsing:
        let mut buf = &bytes[0..3];
        assert!(buf.parse::<TcpSegmentRaw<_>>().is_err());
        // If we don't even have exactly 4 header bytes, we should succeed
        // partial parsing:
        let mut buf = &bytes[0..4];
        assert!(buf.parse::<TcpSegmentRaw<_>>().is_ok());
    }

    #[test]
    fn serialize_with_4_sack_blocks_and_timestamp_invalid() {
        let builder = new_builder(TEST_SRC_IPV4, TEST_DST_IPV4);

        // NOTE: The TCP options length is limited to 40 bytes. A SACK
        // option with 4 blocks would take 34 bytes, and a timestamp
        // option takes 10 bytes, for a total of 44 bytes.
        let sack_blocks = [
            TcpSackBlock::new(100, 200),
            TcpSackBlock::new(300, 400),
            TcpSackBlock::new(500, 600),
            TcpSackBlock::new(700, 800),
        ];
        let timestamp = TimestampOption::new(12345, 67890);
        let options_builder = TcpOptionsBuilder {
            sack_blocks: Some(&sack_blocks),
            timestamp: Some(timestamp),
            ..Default::default()
        };

        assert_matches!(
            TcpSegmentBuilderWithOptions::new(builder, options_builder),
            Err(TcpOptionsTooLongError)
        );
    }

    const MSS: u16 = 1440;
    const WINDOW_SCALE: u8 = 4;
    const SACK_BLOCKS: [TcpSackBlock; 3] =
        [TcpSackBlock::new(1, 2), TcpSackBlock::new(3, 4), TcpSackBlock::new(5, 6)];
    const TIMESTAMP: TimestampOption = TimestampOption::new(12345, 54321);

    #[test_case(TcpOptionsBuilder::default(); "no_options")]
    #[test_case(TcpOptionsBuilder{mss: Some(MSS), ..Default::default()}; "mss")]
    #[test_case(TcpOptionsBuilder{
        window_scale: Some(WINDOW_SCALE), ..Default::default()
    }; "window_scale")]
    #[test_case(TcpOptionsBuilder{sack_permitted: true, ..Default::default()}; "sack_permitted")]
    #[test_case(TcpOptionsBuilder{sack_blocks: Some(&SACK_BLOCKS), ..Default::default()}; "sack")]
    #[test_case(TcpOptionsBuilder{timestamp: Some(TIMESTAMP), ..Default::default()}; "timestamp")]
    #[test_case(TcpOptionsBuilder{
        mss: Some(MSS),
        window_scale: Some(WINDOW_SCALE),
        sack_permitted: true,
        timestamp: Some(TIMESTAMP),
        ..Default::default()
    }; "full_handshake_segment")]
    #[test_case(TcpOptionsBuilder{
        timestamp: Some(TIMESTAMP),
        sack_blocks: Some(&SACK_BLOCKS),
        ..Default::default()
    }; "full_regular_segment")]
    #[test_case(TcpOptionsBuilder {
        timestamp: Some(TIMESTAMP),
        sack_permitted: true,
        ..Default::default()
    }; "timestamp_hotpath_handles_sack_permitted")]
    fn serialize_parse_tcp_option(options_builder: TcpOptionsBuilder<'_>) {
        let TcpOptionsBuilder { mss, window_scale, sack_permitted, sack_blocks, timestamp } =
            options_builder;

        let builder = new_builder(TEST_SRC_IPV4, TEST_DST_IPV4);
        let builder = TcpSegmentBuilderWithOptions::new(builder, options_builder).unwrap();

        // Serialize and Parse the segment.
        let mut buf = builder
            .wrap_body((&[0, 1, 2, 3, 4, 5, 7, 8, 9]).into_serializer())
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap();
        let segment = buf
            .parse_with::<_, TcpSegment<_>>(TcpParseArgs::new(TEST_SRC_IPV4, TEST_DST_IPV4))
            .unwrap();

        // Verify we got back the exact options we put in.
        assert_eq!(segment.options().mss(), mss);
        assert_eq!(segment.options().window_scale(), window_scale);
        assert_eq!(segment.options().sack_permitted(), sack_permitted);
        assert_eq!(segment.options().sack_blocks(), sack_blocks);
        assert_eq!(segment.options().timestamp(), timestamp.as_ref());
    }

    #[test]
    fn test_serialize_aligned_timestamp_option() {
        let builder = TcpSegmentBuilderWithOptions::new(
            new_builder(TEST_SRC_IPV4, TEST_DST_IPV4),
            TcpOptionsBuilder { timestamp: Some(TIMESTAMP), ..Default::default() },
        )
        .unwrap();

        // Serialize the segment.
        let buf = builder
            .wrap_body((&[0, 1, 2, 3, 4, 5, 7, 8, 9]).into_serializer())
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap();

        // Verify the options were serialized as [NOP, NOP, TIMESTAMP].
        let expected_options: Vec<_> =
            [OPTION_KIND_NOP, OPTION_KIND_NOP, OPTION_KIND_TIMESTAMP, OPTION_LEN_TIMESTAMP as u8]
                .iter()
                .chain(TIMESTAMP.as_bytes())
                .copied()
                .collect();
        assert_eq!(
            &buf.as_ref()[HDR_PREFIX_LEN..HDR_PREFIX_LEN + ALIGNED_TIMESTAMP_OPTION_LENGTH],
            &expected_options[..]
        )
    }

    const OPTION_KIND_UNKNOWN: u8 = 255;

    // A TCP Option with an unknown kind.
    const UNKNOWN_TCP_OPTION: [u8; 4] = [OPTION_KIND_UNKNOWN, 4, 0, 0];

    #[derive(Debug)]
    struct TcpSegmentBuilderWithCustomOption<A: IpAddress, O> {
        prefix_builder: TcpSegmentBuilder<A>,
        option: O,
    }

    impl<A: IpAddress, O: AsRef<[u8]>> NestablePacketBuilder
        for TcpSegmentBuilderWithCustomOption<A, O>
    {
        fn constraints(&self) -> PacketConstraints {
            let opt_len = self.option.as_ref().len();
            let header_len = HDR_PREFIX_LEN + usize::from(opt_len);
            PacketConstraints::new(header_len, 0, 0, usize::MAX)
        }
    }

    impl<A: IpAddress, O: AsRef<[u8]>, C: TcpSerializationContext> PacketBuilder<C>
        for TcpSegmentBuilderWithCustomOption<A, O>
    {
        fn context_state(&self) -> C::ContextState {
            C::envelope_to_state(TcpEnvelope)
        }

        fn serialize(
            &self,
            context: &mut C,
            target: &mut SerializeTarget<'_>,
            body: FragmentedBytesMut<'_, '_>,
        ) {
            let Self { option, prefix_builder } = self;
            let mut header = &mut &mut target.header[..];
            header.write_obj_back(option.as_ref()).unwrap();
            prefix_builder.serialize(context, target, body);
        }
    }

    #[test]
    fn test_parse_unknown_option() {
        let builder = TcpSegmentBuilderWithCustomOption {
            option: UNKNOWN_TCP_OPTION,
            prefix_builder: new_builder(TEST_SRC_IPV4, TEST_DST_IPV4),
        };

        // Serialize and Parse the segment. Parsing should ignore the unknown
        // option.
        let mut buf = builder
            .wrap_body((&[0, 1, 2, 3, 4, 5, 7, 8, 9]).into_serializer())
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap();
        let segment = buf
            .parse_with::<_, TcpSegment<_>>(TcpParseArgs::new(TEST_SRC_IPV4, TEST_DST_IPV4))
            .unwrap();

        // Verify no options are set.
        assert_eq!(segment.options().mss(), None);
        assert_eq!(segment.options().window_scale(), None);
        assert_eq!(segment.options().sack_permitted(), false);
        assert_eq!(segment.options().sack_blocks(), None);
        assert_eq!(segment.options().timestamp(), None);
    }

    // A TCP SACK Option with a length that is too short.
    const SACK_OPTION_TOO_SHORT: [u8; 4] = [options::OPTION_KIND_SACK, 1, 0, 0];
    // An unknown TCP Option with a length that is too short.
    const UNKNOWN_OPTION_TOO_SHORT: [u8; 4] = [OPTION_KIND_UNKNOWN, 1, 0, 0];

    // A regression test for https://fxbug.dev/481057779.
    //
    // Ensure that parsing of variable length TCP Options sanitizes the user
    // provided length.
    #[test_case(SACK_OPTION_TOO_SHORT; "sack")]
    #[test_case(UNKNOWN_OPTION_TOO_SHORT; "unknown")]
    fn test_parse_option_too_short(opt_bytes: [u8; 4]) {
        let builder = TcpSegmentBuilderWithCustomOption {
            option: opt_bytes,
            prefix_builder: new_builder(TEST_SRC_IPV4, TEST_DST_IPV4),
        };

        // Serialize and Parse the segment. Parsing should reject the segment.
        let mut buf = builder
            .wrap_body((&[0, 1, 2, 3, 4, 5, 7, 8, 9]).into_serializer())
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap();
        assert_matches!(
            buf.parse_with::<_, TcpSegment<_>>(TcpParseArgs::new(TEST_SRC_IPV4, TEST_DST_IPV4)),
            Err(ParseError::Format)
        );
    }

    // Regression test for https://fxbug.dev/517244297.
    //
    // Ensure that partial_serialization of a segment with options correctly
    // sets the data_offset field.
    #[test]
    fn test_partial_serialize_data_offset() {
        use packet::PartialPacketBuilder;

        let prefix_builder = new_builder(TEST_SRC_IPV4, TEST_DST_IPV4);
        // MSS option takes 4 bytes.
        let options_builder = TcpOptionsBuilder { mss: Some(1460), ..Default::default() };
        let builder = TcpSegmentBuilderWithOptions::new(prefix_builder, options_builder).unwrap();

        let header_len = HDR_PREFIX_LEN + builder.options().bytes_len();
        assert_eq!(header_len, 24); // 20 (prefix) + 4 (MSS)

        let mut buf = vec![0u8; header_len];
        builder.partial_serialize(&mut NoOpSerializationContext, 0, &mut buf[..]);

        let prefix = Ref::<_, HeaderPrefix>::from_bytes(&buf[..HDR_PREFIX_LEN]).unwrap();
        assert_eq!(prefix.data_offset(), 6); // 24 bytes / 4.
    }
}
