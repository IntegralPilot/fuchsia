// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! IP fragmentation support.

use core::borrow::Borrow;
use core::fmt::Debug;

use alloc::vec::Vec;

use net_types::ip::{GenericOverIp, Ip, IpInvariant, Ipv4, Ipv6, Mtu};
use netstack3_base::{Counter, NetworkSerializationContext, NetworkSerializer, RngContext};
use netstack3_filter::ForwardedPacket;
use packet::{
    Buf, BufferMut, EmptyBuf, FragmentedBuffer as _, InnerPacketBuilder as _,
    NestablePacketBuilder, Nested, PacketBuilder, PacketConstraints, ParsablePacket,
    SerializeError,
};
use packet_formats::ip::FragmentOffset;
use packet_formats::ipv4::options::Ipv4Option;
use packet_formats::ipv4::{
    Ipv4Header as _, Ipv4PacketBuilder, Ipv4PacketBuilderWithOptions, Ipv4PacketRaw,
};
use packet_formats::ipv6::ext_hdrs::IPV6_FRAGMENT_EXT_HDR_LEN;
use packet_formats::ipv6::{
    Ipv6Packet, Ipv6PacketBuilderBeforeFragment, Ipv6PacketBuilderWithFragmentHeader,
    Ipv6PerFragmentHeaderBuilder,
};
use rand::RngExt as _;

/// The maximum fragment offset that can be expressed in both IPv4 and IPv6
/// headers. The maximum transmissible body is this value plus the maximum bytes
/// transmitted in the last fragment.
// We have 13 bits to express an 8-byte multiple offset.
const MAX_FRAGMENT_OFFSET: usize = ((1 << 13) - 1) * 8;

pub trait FragmentationIpExt:
    packet_formats::ip::IpExt<
        PacketBuilder<NetworkSerializationContext>: AsFragmentableIpPacketBuilder<Self>,
    >
{
    /// The IP packet builder for a forwarded packet.
    type ForwardedFragmentBuilder: FragmentableIpPacketBuilder<Self>;
    /// An identifier generated at fragmentation time.
    type FragmentationId: Copy + Debug;
}

impl FragmentationIpExt for Ipv4 {
    type ForwardedFragmentBuilder = ForwardedIpv4PacketBuilder;
    type FragmentationId = ();
}

impl FragmentationIpExt for Ipv6 {
    type ForwardedFragmentBuilder = ForwardedIpv6PacketBuilder;
    type FragmentationId = u32;
}

/// Fragmentation errors
#[derive(Debug, Eq, PartialEq, GenericOverIp)]
#[generic_over_ip()]
pub enum FragmentationError {
    /// Fragmentation not allowed.
    NotAllowed,
    /// MTU is too small, headers don't fit.
    MtuTooSmall,
    /// Body is too long to be fragmented.
    BodyTooLong,
    /// Inner serializer reported a size limited exceeded.
    SizeLimitExceeded,
}

/// A [`Serializer`] capable of splitting itself into a packet builder and a
/// pre-serialized body for fragmentation.
// TODO(https://fxbug.dev/42148826): Ideally we'd be able to generate fragments
// without requiring the IP body to be dumped into a Vec first. Update this when
// that support is available in packet and packet_formats.
pub trait FragmentableIpSerializer<I: FragmentationIpExt>: NetworkSerializer {
    /// The builder for each fragment.
    type Builder<'a>: FragmentableIpPacketBuilder<I>
    where
        Self: 'a;
    /// The body to be fragmented.
    ///
    /// Note that this API is not attempting to reuse buffers in any way. There
    /// are improvements that can be made here to perhaps avoid allocations and
    /// yield out reusable bodies, but we're constrained to taking references to
    /// the serializers here to avoid changing the body which could interfere
    /// with the higher layers on errors.
    type Body<'a>: AsRef<[u8]>
    where
        Self: 'a;

    /// Returns the inner packet builder for this IP version and a serialized
    /// body.
    fn builder_and_body(&self) -> Result<(Self::Builder<'_>, Self::Body<'_>), FragmentationError>;
}

impl<I, S, B> FragmentableIpSerializer<I> for Nested<S, B>
where
    I: FragmentationIpExt,
    S: NetworkSerializer,
    B: AsFragmentableIpPacketBuilder<I> + PacketBuilder<NetworkSerializationContext>,
{
    type Builder<'a>
        = B::Builder<'a>
    where
        Self: 'a;

    type Body<'a>
        = Buf<Vec<u8>>
    where
        Self: 'a;

    fn builder_and_body(&self) -> Result<(Self::Builder<'_>, Self::Body<'_>), FragmentationError> {
        let builder = self.outer().try_as_fragmentable()?;
        let body = self
            .inner()
            .serialize_new_buf(
                &mut NetworkSerializationContext::default(),
                PacketConstraints::UNCONSTRAINED,
                packet::new_buf_vec,
            )
            .map_err(|e| match e {
                SerializeError::SizeLimitExceeded => FragmentationError::SizeLimitExceeded,
            })?;
        Ok((builder, body))
    }
}

#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub enum FragmentPosition {
    First,
    Middle,
    Last,
}

/// The header size constraints for `FragmentableIpPacketBuilder`
/// implementations.
pub struct HeaderSizes {
    first: usize,
    remaining: usize,
}

/// A type that may be transformed into a fragmentable ip packet builder.
pub trait AsFragmentableIpPacketBuilder<I: FragmentationIpExt> {
    /// The fragmentable packet builder that can be constructed from this type.
    type Builder<'a>: FragmentableIpPacketBuilder<I>
    where
        Self: 'a;

    /// Attempts to extract a `FragmentableIpPacketBuilder` implementation from
    /// this type, returning an error if it can't be fragmented.
    fn try_as_fragmentable(&self) -> Result<Self::Builder<'_>, FragmentationError>;
}

/// An IP packet builder that can create IP fragments.
pub trait FragmentableIpPacketBuilder<I: FragmentationIpExt> {
    /// Returns the portion of the MTU occupied by IP headers.
    fn header_sizes(&self) -> HeaderSizes;

    /// Returns a builder for fragment at offset `offset`.
    ///
    /// `position` carries information if this is the first or last segment, which
    /// require special logic.
    fn builder_at(
        &self,
        offset: FragmentOffset,
        position: FragmentPosition,
        identifier: I::FragmentationId,
    ) -> impl PacketBuilder<NetworkSerializationContext> + '_;
}

/// Blanket impl for everything that has a shape to fit in `Ipv4FragmentBuilder`
/// as a provider for fragmentation.
impl<B> AsFragmentableIpPacketBuilder<Ipv4> for B
where
    B: InnerIpv4FragmentBuilder,
{
    type Builder<'a>
        = Ipv4FragmentBuilder<'a, Self>
    where
        Self: 'a;

    fn try_as_fragmentable(&self) -> Result<Self::Builder<'_>, FragmentationError> {
        can_fragment_ipv4(self.prefix())?;
        Ok(Ipv4FragmentBuilder { builder: self })
    }
}

/// A trait marking all the IPv4 builder types that can be fragmented with
/// [`Ipv4FragmentBuilder`].
trait InnerIpv4FragmentBuilder: PacketBuilder<NetworkSerializationContext> {
    fn prefix(&self) -> &Ipv4PacketBuilder;
    fn prefix_mut(&mut self) -> &mut Ipv4PacketBuilder;
    fn clone_for_fragment(&self, position: FragmentPosition) -> impl InnerIpv4FragmentBuilder;
    fn header_sizes(&self) -> HeaderSizes;
}

impl InnerIpv4FragmentBuilder for Ipv4PacketBuilder {
    fn prefix(&self) -> &Ipv4PacketBuilder {
        self
    }

    fn prefix_mut(&mut self) -> &mut Ipv4PacketBuilder {
        self
    }

    fn clone_for_fragment(&self, _position: FragmentPosition) -> impl InnerIpv4FragmentBuilder {
        self.clone()
    }

    fn header_sizes(&self) -> HeaderSizes {
        let size = self.constraints().header_len();
        HeaderSizes { first: size, remaining: size }
    }
}

impl<'a, I> InnerIpv4FragmentBuilder for Ipv4PacketBuilderWithOptions<'a, I>
where
    I: Iterator<Item: Borrow<Ipv4Option<'a>>> + Clone,
{
    fn prefix(&self) -> &Ipv4PacketBuilder {
        self.prefix_builder()
    }

    fn prefix_mut(&mut self) -> &mut Ipv4PacketBuilder {
        self.prefix_builder_mut()
    }

    fn clone_for_fragment(&self, position: FragmentPosition) -> impl InnerIpv4FragmentBuilder {
        self.clone().with_fragment_options(position == FragmentPosition::First)
    }

    fn header_sizes(&self) -> HeaderSizes {
        let first = self.constraints().header_len();
        let remaining = self.clone().with_fragment_options(false).constraints().header_len();
        HeaderSizes { first, remaining }
    }
}

pub struct Ipv4FragmentBuilder<'a, B> {
    builder: &'a B,
}

impl<'a, B> FragmentableIpPacketBuilder<Ipv4> for Ipv4FragmentBuilder<'a, B>
where
    B: InnerIpv4FragmentBuilder,
{
    fn header_sizes(&self) -> HeaderSizes {
        self.builder.header_sizes()
    }

    fn builder_at(
        &self,
        offset: FragmentOffset,
        position: FragmentPosition,
        (): (),
    ) -> impl PacketBuilder<NetworkSerializationContext> + '_ {
        let mut builder = self.builder.clone_for_fragment(position);
        set_ipv4_fragment(builder.prefix_mut(), offset, position);
        builder
    }
}

impl<B> AsFragmentableIpPacketBuilder<Ipv6> for B
where
    for<'a> &'a B: Ipv6PacketBuilderBeforeFragment,
{
    type Builder<'a>
        = Ipv6FragmentBuilder<'a, Self>
    where
        Self: 'a;

    fn try_as_fragmentable(&self) -> Result<Self::Builder<'_>, FragmentationError> {
        Ok(Ipv6FragmentBuilder { builder: self })
    }
}

pub struct Ipv6FragmentBuilder<'a, B> {
    builder: &'a B,
}

impl<'a, B> FragmentableIpPacketBuilder<Ipv6> for Ipv6FragmentBuilder<'a, B>
where
    &'a B: Ipv6PacketBuilderBeforeFragment,
{
    fn header_sizes(&self) -> HeaderSizes {
        // NB: We currently only support headers that need to be in all
        // fragments, so we only need to calculate once. We might need to change
        // the trait shape if that changes.
        let header_len =
            Ipv6PacketBuilderWithFragmentHeader::new(self.builder, FragmentOffset::ZERO, false, 0)
                .constraints()
                .header_len();
        HeaderSizes { first: header_len, remaining: header_len }
    }

    fn builder_at(
        &self,
        offset: FragmentOffset,
        position: FragmentPosition,
        identifier: u32,
    ) -> impl PacketBuilder<NetworkSerializationContext> + '_ {
        Ipv6PacketBuilderWithFragmentHeader::new(
            self.builder,
            offset,
            position != FragmentPosition::Last,
            identifier,
        )
    }
}

impl<I, B> FragmentableIpSerializer<I> for ForwardedPacket<I, B>
where
    I: FragmentationIpExt,
    B: BufferMut + NetworkSerializer,
{
    type Builder<'a>
        = I::ForwardedFragmentBuilder
    where
        Self: 'a;
    type Body<'a>
        = Buf<&'a [u8]>
    where
        Self: 'a;

    fn builder_and_body(&self) -> Result<(Self::Builder<'_>, Self::Body<'_>), FragmentationError> {
        #[derive(GenericOverIp)]
        #[generic_over_ip(I, Ip)]
        struct Out<I: FragmentationIpExt>(I::ForwardedFragmentBuilder);
        I::map_ip::<_, Result<(Out<I>, IpInvariant<Buf<&[u8]>>), FragmentationError>>(
            self,
            |forwarded| {
                // Parse an IPv4 packet from the forwarded packet. We can assert
                // strongly on all of the parsing here because ForwardedPacket
                // is guaranteed to have been parsed by the IP stack already.
                let mut buffer = forwarded.buffer().as_ref();
                let packet = Ipv4PacketRaw::parse(&mut buffer, ())
                    .expect("ForwardedPacket must be parseable");
                let builder = packet.builder();
                can_fragment_ipv4(&builder)?;
                let raw_options_bytes = packet
                    .options()
                    .as_ref()
                    .complete()
                    .expect("unexpected incomplete IP header")
                    .bytes();

                let mut raw_options = Buf::new(
                    [0u8; packet_formats::ipv4::MAX_OPTIONS_LEN],
                    ..raw_options_bytes.len(),
                );
                raw_options.as_mut().copy_from_slice(raw_options_bytes);
                let body = Buf::new(
                    packet.into_body().complete().expect("unexpected incomplete IP body"),
                    ..,
                );
                Ok((Out(ForwardedIpv4PacketBuilder { builder, raw_options }), IpInvariant(body)))
            },
            |forwarded| {
                // Per RFC 8200 Section 4.5:
                //   fragmentation in IPv6 is performed only by source nodes,
                //   not by routers along a packet's delivery path.
                // Therefore, in general, we should not fragment forwarded
                // packets. However, during ingress we reassemble IP fragments
                // prior to making a routing decision. In such cases, we have
                // an obligation to re-fragment the packet back to the original
                // MTU.
                if !forwarded.reassembled() {
                    return Err(FragmentationError::NotAllowed);
                }
                let mut buffer = forwarded.buffer().as_ref();
                // NB: `Ipv6Packet::parse` must succeed, because
                // `ForwardedPacket` has already been parsed by the IP stack.
                let packet =
                    Ipv6Packet::parse(&mut buffer, ()).expect("ForwardedPacket must be parseable");
                let builder = packet.per_fragment_builder();
                let body = Buf::new(&forwarded.buffer().as_ref()[builder.header_len()..], ..);
                Ok((Out(ForwardedIpv6PacketBuilder(builder)), IpInvariant(body)))
            },
        )
        .map(|(Out(builder), IpInvariant(body))| (builder, body))
    }
}

pub struct ForwardedIpv4PacketBuilder {
    builder: Ipv4PacketBuilder,
    raw_options: Buf<[u8; packet_formats::ipv4::MAX_OPTIONS_LEN]>,
}

impl FragmentableIpPacketBuilder<Ipv4> for ForwardedIpv4PacketBuilder {
    fn header_sizes(&self) -> HeaderSizes {
        let Self { builder, raw_options } = self;
        if raw_options.is_empty() {
            builder.header_sizes()
        } else {
            let options = packet_formats::ipv4::Options::parse(raw_options.as_ref())
                .expect("must hold valid options");
            Ipv4PacketBuilderWithOptions::new_with_records_iter(builder.clone(), options.iter())
                .header_sizes()
        }
    }

    fn builder_at(
        &self,
        offset: FragmentOffset,
        position: FragmentPosition,
        (): (),
    ) -> impl PacketBuilder<NetworkSerializationContext> + '_ {
        let Self { builder, raw_options } = self;
        let mut builder = builder.clone();
        set_ipv4_fragment(&mut builder, offset, position);
        let options = packet_formats::ipv4::Options::parse(raw_options.as_ref())
            .expect("must hold valid options");
        Ipv4PacketBuilderWithOptions::new_with_records_iter(builder.clone(), options.into_iter())
            .with_fragment_options(position == FragmentPosition::First)
    }
}

pub struct ForwardedIpv6PacketBuilder(Ipv6PerFragmentHeaderBuilder<Vec<u8>>);

impl FragmentableIpPacketBuilder<Ipv6> for ForwardedIpv6PacketBuilder {
    fn header_sizes(&self) -> HeaderSizes {
        let size = self.0.header_len() + IPV6_FRAGMENT_EXT_HDR_LEN;
        HeaderSizes { first: size, remaining: size }
    }

    fn builder_at(
        &self,
        offset: FragmentOffset,
        position: FragmentPosition,
        identifier: u32,
    ) -> impl PacketBuilder<NetworkSerializationContext> + '_ {
        Ipv6PacketBuilderWithFragmentHeader::new(
            self.0.as_ref(),
            offset,
            position != FragmentPosition::Last,
            identifier,
        )
    }
}

/// Abstracts fragment ID generation for [`IpFragmenter`].
///
/// A blanket impl is provided for [`RngContext`] implementers, so the bindings
/// context can be used to generate random IDs for IPv6.
pub(crate) trait FragmentationIdGenContext {
    fn generate_id<I: FragmentationIpExt>(&mut self) -> I::FragmentationId;
}

#[derive(GenericOverIp)]
#[generic_over_ip(I, Ip)]
struct WrapFragmentationId<I: FragmentationIpExt>(I::FragmentationId);

impl<BC> FragmentationIdGenContext for BC
where
    BC: RngContext,
{
    fn generate_id<I: FragmentationIpExt>(&mut self) -> I::FragmentationId {
        let WrapFragmentationId(identifier) = I::map_ip_out(
            self,
            |_| WrapFragmentationId(()),
            |rng| {
                // TODO(https://fxbug.dev/373428005): Perhaps we can do better
                // than a simple RNG. This is currently copying what netstack2
                // does. RFC 7739 calls out different strategies for fragment
                // IDs in IPv6. We currently pick an option that is not doing a
                // best effort to avoid collisions, but it guarantees that
                // fragment IDs can't be tracked as an attack vector.
                // We avoid a zero fragment ID like netstack2 does.
                WrapFragmentationId(rng.rng().random_range(1..=u32::MAX))
            },
        );
        identifier
    }
}

pub(crate) struct IpFragmenter<'a, I: FragmentationIpExt, S: FragmentableIpSerializer<I> + 'a> {
    builder: S::Builder<'a>,
    body: S::Body<'a>,
    consumed: usize,
    max_fragment_body_first: usize,
    max_fragment_body_remaining: usize,
    identifier: I::FragmentationId,
}

/// Returns the biggest fragment body that can fit in `mtu` with a given IP
/// `header` size.
///
/// The returned body size is rounded down to the nearest multiple of 8 to fit
/// the IP header representation of fragment offsets.
fn maximum_fragment_body_with_header_and_mtu(
    mtu: Mtu,
    header: usize,
) -> Result<usize, FragmentationError> {
    let v = usize::from(mtu).checked_sub(header).ok_or(FragmentationError::MtuTooSmall)?;
    // Mask the final 8 bits since fragment offset is expressed in units
    // of 8 octets for both IP versions.
    let v = v & !0x07usize;

    if v == 0 {
        // Can't fragment if we don't have at least a single 8 octet
        // of space.
        return Err(FragmentationError::MtuTooSmall);
    }
    Ok(v)
}

impl<'a, I: FragmentationIpExt, S: FragmentableIpSerializer<I>> IpFragmenter<'a, I, S> {
    /// Creates a new `IpFragmenter` with some `serializer` respecting a maximum
    /// IP layer `mtu`.
    pub(crate) fn new<C: FragmentationIdGenContext>(
        id_ctx: &mut C,
        serializer: &'a S,
        mtu: Mtu,
    ) -> Result<Self, FragmentationError> {
        let (builder, body) = serializer.builder_and_body()?;
        let HeaderSizes { first, remaining } = builder.header_sizes();
        let max_fragment_body_first = maximum_fragment_body_with_header_and_mtu(mtu, first)?;
        let max_fragment_body_remaining =
            maximum_fragment_body_with_header_and_mtu(mtu, remaining)?;

        if body.as_ref().len() > MAX_FRAGMENT_OFFSET + max_fragment_body_remaining {
            return Err(FragmentationError::BodyTooLong);
        }

        let identifier = id_ctx.generate_id::<I>();

        Ok(Self {
            builder,
            body,
            consumed: 0,
            max_fragment_body_first,
            max_fragment_body_remaining,
            identifier,
        })
    }

    /// Returns the serializer for the next segment and a boolean indicating
    /// whether more fragments are pending, or `None` if all segments have been
    /// produced.
    ///
    /// # Panics
    ///
    /// Panics if fragmentation is not necessary for the `serializer` that
    /// created this `IpFragmenter`.
    pub(crate) fn next(&mut self) -> Option<(impl NetworkSerializer<Buffer = EmptyBuf>, bool)> {
        let Self {
            builder,
            body,
            consumed,
            max_fragment_body_first,
            max_fragment_body_remaining,
            identifier,
        } = self;
        let body = &AsRef::as_ref(body)[*consumed..];
        if body.is_empty() {
            return None;
        }
        let first = *consumed == 0;
        let max_fragment_body =
            if first { max_fragment_body_first } else { max_fragment_body_remaining };
        let take = body.len().min(*max_fragment_body);
        let last = take == body.len();
        let position = match (first, last) {
            (true, true) => {
                panic!("unnecessary fragmentation");
            }
            (true, false) => FragmentPosition::First,
            (false, false) => FragmentPosition::Middle,
            (false, true) => FragmentPosition::Last,
        };
        // Upon construction IpFragmenter verifies that we won't go over the
        // maximum offset since the body length is known.
        let fragment_offset = u16::try_from(*consumed).expect("fragment offset too large");
        // Care is taken above to always take 8-byte multiples to be added to
        // consumed, so we should always have a good representation for
        // FragmentOffset.
        let fragment_offset =
            FragmentOffset::new_with_bytes(fragment_offset).expect("invalid offset");
        let fragment_builder = builder.builder_at(fragment_offset, position, *identifier);
        let end = *consumed + take;
        let has_more = body.len() > take;
        let fragment_body = &body[..take];
        *consumed = end;
        Some((fragment_builder.wrap_body(fragment_body.into_serializer()), has_more))
    }
}

fn can_fragment_ipv4(builder: &Ipv4PacketBuilder) -> Result<(), FragmentationError> {
    if builder.read_df_flag() {
        return Err(FragmentationError::NotAllowed);
    }
    Ok(())
}

fn set_ipv4_fragment(
    builder: &mut Ipv4PacketBuilder,
    offset: FragmentOffset,
    position: FragmentPosition,
) {
    builder.mf_flag(position != FragmentPosition::Last);
    builder.fragment_offset(offset);
}

/// Counters kept by the IP stack pertaining to fragmentation.
#[derive(Default, Debug)]
#[cfg_attr(
    any(test, feature = "testutils"),
    derive(PartialEq, netstack3_macros::CounterCollection)
)]
pub struct FragmentationCounters<C = Counter> {
    /// The number of IP frames requiring fragmentation on egress.
    pub fragmentation_required: C,
    /// The total number of fragments sent.
    pub fragments: C,
    /// The number of `NotAllowed` errors encountered.
    pub error_not_allowed: C,
    /// The number of `MtuTooSmall` errors encountered.
    pub error_mtu_too_small: C,
    /// The number of `BodyTooLong` errors encountered.
    pub error_body_too_long: C,
    /// The number of `SizeLimitExceeded` errors encountered.
    pub error_inner_size_limit_exceeded: C,
    /// Counts the number of times fragmentation was short-circuited due to a
    /// fragment serialization error.
    pub error_fragmented_serializer: C,
}

impl FragmentationCounters {
    pub(crate) fn error_counter(&self, error: &FragmentationError) -> &Counter {
        match error {
            FragmentationError::NotAllowed => &self.error_not_allowed,
            FragmentationError::MtuTooSmall => &self.error_mtu_too_small,
            FragmentationError::BodyTooLong => &self.error_body_too_long,
            FragmentationError::SizeLimitExceeded => &self.error_inner_size_limit_exceeded,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use assert_matches::assert_matches;
    use net_types::Witness as _;
    use netstack3_base::testutil::{TEST_ADDRS_V4, TEST_ADDRS_V6};
    use netstack3_filter::FilterIpExt;
    use packet::{Buffer, BufferView, GrowBuffer, Serializer};
    use packet_formats::ip::IpProto;
    use packet_formats::ipv4::Ipv4Packet;
    use packet_formats::ipv6::ext_hdrs::{
        ExtensionHeaderOptionAction, HopByHopOption, HopByHopOptionData, Ipv6ExtensionHeader,
    };
    use packet_formats::ipv6::{Ipv6Header, Ipv6Packet, Ipv6PacketBuilder};
    use test_case::test_case;

    const TEST_MTU: Mtu = Ipv6::MINIMUM_LINK_MTU;

    fn gen_body(len: usize) -> Vec<u8> {
        // Cycle bytes until 251 which is the largest prime that can fit in a
        // u8. Unlikely this aligns poorly and hides fragmentation bugs.
        (0u8..=251).cycle().take(len).collect::<Vec<u8>>()
    }

    impl<'a, I: FragmentationIpExt, S: FragmentableIpSerializer<I>> IpFragmenter<'a, I, S> {
        fn next_serialized(&mut self) -> Buf<Vec<u8>> {
            self.next()
                .expect("no more fragments")
                .0
                .serialize_vec_outer(&mut NetworkSerializationContext::default())
                .map_err(|(err, _serializer)| err)
                .unwrap()
                .unwrap_b()
        }
    }

    trait FragmentationTestEnv<I: FragmentationIpExt> {
        fn new_serializer<'a>(
            &self,
            body: &'a [u8],
        ) -> impl FragmentableIpSerializer<I, Buffer: Buffer> + 'a;
        fn check_fragment(
            &self,
            fragment: &mut Buf<Vec<u8>>,
            position: FragmentPosition,
            offset: usize,
        );
    }

    #[derive(Default)]
    struct Ipv4TestEnv {
        dont_frag: bool,
    }

    impl Ipv4TestEnv {
        const fn dont_frag() -> Self {
            Self { dont_frag: true }
        }
    }

    const IPV4_ID: u16 = 0x1234;
    fn new_ipv4_packet_builder(dont_frag: bool) -> Ipv4PacketBuilder {
        let mut builder = Ipv4PacketBuilder::new(
            TEST_ADDRS_V4.local_ip,
            TEST_ADDRS_V4.remote_ip,
            1,
            IpProto::Udp.into(),
        );
        builder.id(IPV4_ID);
        builder.df_flag(dont_frag);
        builder
    }

    fn parse_and_check_ipv4_packet(
        fragment: &mut Buf<Vec<u8>>,
        position: FragmentPosition,
        offset: usize,
    ) -> Ipv4Packet<&[u8]> {
        let packet = Ipv4Packet::parse(fragment.buffer_view(), ()).expect("parse fragment");
        assert_eq!(packet.src_ip(), TEST_ADDRS_V4.local_ip.get());
        assert_eq!(packet.dst_ip(), TEST_ADDRS_V4.remote_ip.get());
        assert_eq!(packet.ttl(), 1);
        assert_eq!(packet.id(), IPV4_ID);
        assert_eq!(packet.proto(), IpProto::Udp.into());
        assert_eq!(packet.mf_flag(), position != FragmentPosition::Last);
        assert_eq!(usize::from(packet.fragment_offset().into_bytes()), offset);
        packet
    }

    impl FragmentationTestEnv<Ipv4> for Ipv4TestEnv {
        fn new_serializer<'a>(
            &self,
            body: &'a [u8],
        ) -> impl FragmentableIpSerializer<Ipv4, Buffer: Buffer> + 'a {
            let Self { dont_frag } = self;
            new_ipv4_packet_builder(*dont_frag).wrap_body(body.into_serializer())
        }

        fn check_fragment(
            &self,
            fragment: &mut Buf<Vec<u8>>,
            position: FragmentPosition,
            offset: usize,
        ) {
            let _ = parse_and_check_ipv4_packet(fragment, position, offset);
        }
    }

    #[derive(Default)]
    struct Ipv4WithOptionsTestEnv(Ipv4TestEnv);

    // The MSB of an option kind determines if it should be copied.
    const FAKE_OPTION_COPIED_KIND: u8 = 255;
    const FAKE_OPTION_COPIED: [u8; 1] = [255];
    const FAKE_OPTION_NOT_COPIED_KIND: u8 = 127;
    const FAKE_OPTION_NOT_COPIED: [u8; 1] = [127];

    impl FragmentationTestEnv<Ipv4> for Ipv4WithOptionsTestEnv {
        fn new_serializer<'a>(
            &self,
            body: &'a [u8],
        ) -> impl FragmentableIpSerializer<Ipv4, Buffer: Buffer> + 'a {
            let Self(Ipv4TestEnv { dont_frag }) = self;

            Ipv4PacketBuilderWithOptions::new(
                new_ipv4_packet_builder(*dont_frag),
                [
                    Ipv4Option::Unrecognized {
                        kind: FAKE_OPTION_COPIED_KIND,
                        data: &FAKE_OPTION_COPIED[..],
                    },
                    Ipv4Option::Unrecognized {
                        kind: FAKE_OPTION_NOT_COPIED_KIND,
                        data: &FAKE_OPTION_NOT_COPIED[..],
                    },
                ],
            )
            .unwrap()
            .wrap_body(body.into_serializer())
        }

        fn check_fragment(
            &self,
            fragment: &mut Buf<Vec<u8>>,
            position: FragmentPosition,
            offset: usize,
        ) {
            let packet = parse_and_check_ipv4_packet(fragment, position, offset);
            let (copied, not_copied) = packet.iter_options().fold(
                (false, false),
                |(mut copied, mut not_copied), option| {
                    let (kind, data) = assert_matches!(option,
                        Ipv4Option::Unrecognized{ kind, data } => (kind, data)
                    );
                    assert_eq!(data.len(), 1);
                    assert_eq!(data[0], kind);
                    let seen = match kind {
                        FAKE_OPTION_COPIED_KIND => &mut copied,
                        FAKE_OPTION_NOT_COPIED_KIND => &mut not_copied,
                        k => panic!("unexpected option {k}"),
                    };
                    assert_eq!(core::mem::replace(seen, true), false);
                    (copied, not_copied)
                },
            );
            assert_eq!(copied, true, "must be copied on all fragments {position:?}");
            assert_eq!(
                not_copied,
                position == FragmentPosition::First,
                "must only be in first fragment {position:?}"
            );
        }
    }

    struct ForwardingTestEnv<E> {
        inner: E,
        reassembled: bool,
    }

    impl<E> ForwardingTestEnv<E> {
        fn new(inner: E) -> Self {
            Self { inner, reassembled: false }
        }

        fn new_reassembled(inner: E) -> Self {
            Self { inner, reassembled: true }
        }
    }

    impl<I: FragmentationIpExt + FilterIpExt, E: FragmentationTestEnv<I>> FragmentationTestEnv<I>
        for ForwardingTestEnv<E>
    {
        fn new_serializer<'a>(
            &self,
            body: &'a [u8],
        ) -> impl FragmentableIpSerializer<I, Buffer: Buffer> + 'a {
            use packet_formats::ip::IpPacket as _;
            let Self { inner, reassembled } = self;
            let mut buffer = inner
                .new_serializer(body)
                .serialize_outer(
                    &mut NetworkSerializationContext::default(),
                    packet::NoReuseBufferProvider(packet::new_buf_vec),
                )
                .map_err(|(err, _)| err)
                .unwrap();
            let packet =
                <I::Packet<_> as ParsablePacket<_, _>>::parse(buffer.buffer_view(), ()).unwrap();
            let src_addr = packet.src_ip();
            let dst_addr = packet.dst_ip();
            let proto = packet.proto();
            let meta = packet.parse_metadata();
            drop(packet);
            ForwardedPacket::new(src_addr, dst_addr, proto, meta, buffer, *reassembled)
        }
        fn check_fragment(
            &self,
            fragment: &mut Buf<Vec<u8>>,
            position: FragmentPosition,
            offset: usize,
        ) {
            let Self { inner, reassembled: _ } = self;
            inner.check_fragment(fragment, position, offset)
        }
    }

    struct Ipv6TestEnv;

    const IPV6_ID: u32 = 0x1234ABCD;

    impl FragmentationTestEnv<Ipv6> for Ipv6TestEnv {
        fn new_serializer<'a>(
            &self,
            body: &'a [u8],
        ) -> impl FragmentableIpSerializer<Ipv6, Buffer: Buffer> + 'a {
            Ipv6PacketBuilder::new(
                TEST_ADDRS_V6.local_ip,
                TEST_ADDRS_V6.remote_ip,
                1,
                IpProto::Udp.into(),
            )
            .wrap_body(body.into_serializer())
        }

        fn check_fragment(
            &self,
            fragment: &mut Buf<Vec<u8>>,
            position: FragmentPosition,
            offset: usize,
        ) {
            let packet = Ipv6Packet::parse(fragment.buffer_view(), ()).unwrap();
            assert_eq!(packet.src_ip(), TEST_ADDRS_V6.local_ip.get());
            assert_eq!(packet.dst_ip(), TEST_ADDRS_V6.remote_ip.get());
            assert_eq!(packet.hop_limit(), 1);
            assert_eq!(packet.proto(), IpProto::Udp.into());
            let fragment = packet
                .iter_extension_hdrs()
                .find_map(|h| match h {
                    Ipv6ExtensionHeader::Fragment { fragment_data } => Some(fragment_data),
                    _ => None,
                })
                .expect("no fragment header");
            assert_eq!(fragment.identification(), IPV6_ID);
            assert_eq!(usize::from(fragment.fragment_offset().into_bytes()), offset);
            assert_eq!(fragment.m_flag(), position != FragmentPosition::Last);
        }
    }

    #[derive(Default)]
    struct Ipv6WithOptionsTestEnv;

    impl FragmentationTestEnv<Ipv6> for Ipv6WithOptionsTestEnv {
        fn new_serializer<'a>(
            &self,
            body: &'a [u8],
        ) -> impl FragmentableIpSerializer<Ipv6, Buffer: Buffer> + 'a {
            packet_formats::ipv6::Ipv6PacketBuilderWithHbhOptions::new(
                Ipv6PacketBuilder::new(
                    TEST_ADDRS_V6.local_ip,
                    TEST_ADDRS_V6.remote_ip,
                    1,
                    IpProto::Udp.into(),
                ),
                // Add an arbitrary extension header to setup so that this
                // environment tests fragmentation with extension headers.
                &[HopByHopOption {
                    action: ExtensionHeaderOptionAction::SkipAndContinue,
                    mutable: false,
                    data: HopByHopOptionData::RouterAlert { data: 0 },
                }],
            )
            .unwrap()
            .wrap_body(body.into_serializer())
        }

        fn check_fragment(
            &self,
            fragment: &mut Buf<Vec<u8>>,
            position: FragmentPosition,
            offset: usize,
        ) {
            let packet = Ipv6Packet::parse(fragment.buffer_view(), ()).unwrap();
            assert_eq!(packet.src_ip(), TEST_ADDRS_V6.local_ip.get());
            assert_eq!(packet.dst_ip(), TEST_ADDRS_V6.remote_ip.get());
            assert_eq!(packet.hop_limit(), 1);
            assert_eq!(packet.proto(), IpProto::Udp.into());
            let ext_hdrs = packet.iter_extension_hdrs().collect::<Vec<_>>();
            let (hbh, frag) = assert_matches!(&ext_hdrs[..], [hbh, frag] => (hbh, frag));
            assert_matches!(hbh, Ipv6ExtensionHeader::HopByHopOptions { .. });
            let fragment = assert_matches!(
                frag, Ipv6ExtensionHeader::Fragment { fragment_data } => fragment_data
            );
            assert_eq!(fragment.identification(), IPV6_ID);
            assert_eq!(usize::from(fragment.fragment_offset().into_bytes()), offset);
            assert_eq!(fragment.m_flag(), position != FragmentPosition::Last);
        }
    }

    struct FixedIdContext;
    impl FragmentationIdGenContext for FixedIdContext {
        fn generate_id<I: FragmentationIpExt>(&mut self) -> I::FragmentationId {
            let WrapFragmentationId(id) =
                I::map_ip_out((), |()| WrapFragmentationId(()), |()| WrapFragmentationId(IPV6_ID));
            id
        }
    }

    #[test_case::test_matrix(
        [
            Ipv4TestEnv::default(),
            Ipv4WithOptionsTestEnv::default(),
            ForwardingTestEnv::new(Ipv4TestEnv::default()),
            ForwardingTestEnv::new(Ipv4WithOptionsTestEnv::default()),
            ForwardingTestEnv::new_reassembled(Ipv6TestEnv),
            ForwardingTestEnv::new_reassembled(Ipv6WithOptionsTestEnv),
            Ipv6TestEnv,
            Ipv6WithOptionsTestEnv,
        ],
        0..=2
    )]
    fn fragment<I: FragmentationIpExt, E: FragmentationTestEnv<I>>(
        env: E,
        middle_fragments: usize,
    ) {
        // NB: We're using the fact that MTU is larger than the header sizes
        // here to end up obtaining the right number of middle fragments as
        // expected. This makes this test sensitive to the relation between the
        // picked MTU and the header sizes for the multiple serializers.
        let full_body = gen_body(usize::from(TEST_MTU) * (1 + middle_fragments));
        let mut body_view = Buf::new(&full_body[..], ..);
        let serializer = env.new_serializer(&full_body[..]);
        let mut fragmenter = IpFragmenter::new(&mut FixedIdContext, &serializer, TEST_MTU)
            .expect("create fragmenter");

        let mut frag = fragmenter.next_serialized();
        env.check_fragment(&mut frag, FragmentPosition::First, body_view.prefix_len());
        assert_eq!(
            frag.as_ref(),
            body_view.buffer_view().take_front(fragmenter.max_fragment_body_first).unwrap()
        );

        for _ in 0..middle_fragments {
            let mut frag = fragmenter.next_serialized();
            env.check_fragment(&mut frag, FragmentPosition::Middle, body_view.prefix_len());
            assert_eq!(
                frag.as_ref(),
                body_view.buffer_view().take_front(fragmenter.max_fragment_body_remaining).unwrap()
            );
        }

        let mut frag = fragmenter.next_serialized();
        env.check_fragment(&mut frag, FragmentPosition::Last, body_view.prefix_len());
        assert_eq!(frag.as_ref(), body_view.buffer_view().into_rest());

        // No more fragments.
        assert!(fragmenter.next().is_none());
    }

    #[test_case(Ipv4TestEnv::dont_frag())]
    #[test_case(Ipv4WithOptionsTestEnv(Ipv4TestEnv::dont_frag()))]
    #[test_case(ForwardingTestEnv::new(Ipv4TestEnv::dont_frag()))]
    #[test_case(ForwardingTestEnv::new(Ipv6TestEnv))]
    #[test_case(ForwardingTestEnv::new(Ipv6WithOptionsTestEnv))]
    fn not_allowed<I: FragmentationIpExt, E: FragmentationTestEnv<I>>(env: E) {
        let body = gen_body(usize::from(TEST_MTU));
        let serializer = env.new_serializer(&body[..]);
        let result = IpFragmenter::new(&mut FixedIdContext, &serializer, TEST_MTU).map(|_| ());
        assert_eq!(result, Err(FragmentationError::NotAllowed))
    }

    #[test_case(Ipv4TestEnv::default())]
    #[test_case(Ipv4WithOptionsTestEnv::default())]
    #[test_case(ForwardingTestEnv::new(Ipv4TestEnv::default()))]
    #[test_case(ForwardingTestEnv::new_reassembled(Ipv6TestEnv))]
    #[test_case(ForwardingTestEnv::new_reassembled(Ipv6WithOptionsTestEnv))]
    #[test_case(Ipv6TestEnv)]
    #[test_case(Ipv6WithOptionsTestEnv)]
    fn mtu_too_small<I: FragmentationIpExt, E: FragmentationTestEnv<I>>(env: E) {
        let body = gen_body(usize::from(TEST_MTU));
        let serializer = env.new_serializer(&body[..]);
        let result = IpFragmenter::new(&mut FixedIdContext, &serializer, Mtu::new(10)).map(|_| ());
        assert_eq!(result, Err(FragmentationError::MtuTooSmall));
    }

    #[test_case(Ipv4TestEnv::default())]
    #[test_case(Ipv4WithOptionsTestEnv::default())]
    #[test_case(Ipv6TestEnv)]
    #[test_case(Ipv6WithOptionsTestEnv)]
    fn body_too_long<I: FragmentationIpExt, E: FragmentationTestEnv<I>>(env: E) {
        let body = gen_body(MAX_FRAGMENT_OFFSET + usize::from(TEST_MTU));
        let serializer = env.new_serializer(&body[..]);
        let result = IpFragmenter::new(&mut FixedIdContext, &serializer, TEST_MTU).map(|_| ());
        assert_eq!(result, Err(FragmentationError::BodyTooLong));
    }
}
